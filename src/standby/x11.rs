//! A minimal X11 client: which host process owns a top-level window.
//!
//! niri reports `xwayland-satellite` as the pid of every X11 window, so
//! Melibea asks the X server instead. It speaks the few requests it needs by
//! hand, little-endian, over the local Unix socket: `InternAtom`,
//! `QueryExtension`, `QueryTree`, `GetProperty` and X-Resource's
//! `XResQueryClientIds` with `LocalClientPID`, which gives the host pid even
//! for a client inside a Flatpak sandbox. Every read is bounded in time (one
//! deadline across the whole exchange) and in size.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fmt;
use std::fs::File;
use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

/// Bound on connecting plus one whole lookup.
pub const X11_TIMEOUT: Duration = Duration::from_secs(1);
/// Root children examined at most.
pub const MAX_X11_WINDOWS: usize = 4096;

/// A reply declaring more than this many bytes closes the connection.
const MAX_REPLY_BYTES: u64 = 1 << 20;
/// `GetProperty` asks for at most this many 32-bit units (4096 bytes).
const PROPERTY_LONG_LENGTH: u32 = 1024;
/// An Xauthority file larger than this is not read past this size.
const MAX_XAUTHORITY_BYTES: u64 = 1 << 20;
const X11_UNIX_DIR: &str = "/tmp/.X11-unix";
const MIT_MAGIC_COOKIE: &[u8] = b"MIT-MAGIC-COOKIE-1";
const FAMILY_LOCAL: u16 = 256;
const FAMILY_WILD: u16 = 0xffff;

const ATOM_STRING: u32 = 31;
const ATOM_WM_NAME: u32 = 39;

const OP_QUERY_TREE: u8 = 15;
const OP_INTERN_ATOM: u8 = 16;
const OP_GET_PROPERTY: u8 = 20;
const OP_QUERY_EXTENSION: u8 = 98;
const XRES_QUERY_CLIENT_IDS: u8 = 4;
const XRES_LOCAL_CLIENT_PID: u32 = 2;
const ERROR_BAD_WINDOW: u8 = 3;
const GENERIC_EVENT: u8 = 35;

/// Who owns the X11 windows matching an app id and a title.
#[derive(Debug, PartialEq, Eq)]
pub enum OwnerLookup {
    /// Exactly one process owns them.
    Owner(u32),
    /// No window matched, or none had a known owner.
    NoMatch,
    /// Several processes own matching windows (sorted pids).
    Ambiguous(Vec<u32>),
}

/// Why an X11 lookup gave no answer.
#[derive(Debug)]
pub enum X11Error {
    /// `$DISPLAY` is missing or not a local `:N` display.
    NoDisplay,
    /// The socket could not be used.
    Io(io::Error),
    /// The server refused the connection, with its reason.
    Refused(String),
    /// The server's bytes made no sense, or it reported an error.
    Protocol(String),
    /// The server lacks the X-Resource extension.
    NoXRes,
    /// The server did not answer in time.
    Timeout,
}

impl fmt::Display for X11Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoDisplay => write!(f, "no local X11 display"),
            Self::Io(err) => write!(f, "X11 socket error: {err}"),
            Self::Refused(reason) => write!(f, "the X server refused the connection: {reason}"),
            Self::Protocol(text) => write!(f, "X11 protocol error: {text}"),
            Self::NoXRes => write!(f, "the X server lacks the X-Resource extension"),
            Self::Timeout => write!(f, "the X server did not answer in time"),
        }
    }
}

impl std::error::Error for X11Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}

fn protocol(text: impl Into<String>) -> X11Error {
    X11Error::Protocol(text.into())
}

fn from_io(err: io::Error) -> X11Error {
    match err.kind() {
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => X11Error::Timeout,
        _ => X11Error::Io(err),
    }
}

/// The socket path and display number of a local display such as `:0` or
/// `:1.0`; `None` for anything else (TCP, a host name, an empty value).
#[must_use]
pub fn display_socket(display: &str) -> Option<(PathBuf, u16)> {
    let rest = display.strip_prefix(':')?;
    let (number, screen) = match rest.split_once('.') {
        Some((number, screen)) => (number, Some(screen)),
        None => (rest, None),
    };
    if !is_decimal(number) || screen.is_some_and(|screen| !is_decimal(screen)) {
        return None;
    }
    let number: u16 = number.parse().ok()?;
    Some((Path::new(X11_UNIX_DIR).join(format!("X{number}")), number))
}

fn is_decimal(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}

/// The MIT-MAGIC-COOKIE-1 for `display` in an Xauthority file's bytes: the
/// first entry of family Local (whatever its host name) or Wild whose display
/// number matches. A truncated file yields what precedes the damage.
#[must_use]
pub fn find_cookie(xauthority: &[u8], display: u16) -> Option<Vec<u8>> {
    let number = display.to_string();
    let mut rest = xauthority;
    loop {
        let family = take_be16(&mut rest)?;
        let _address = take_field(&mut rest)?;
        let entry_number = take_field(&mut rest)?;
        let name = take_field(&mut rest)?;
        let data = take_field(&mut rest)?;
        if matches!(family, FAMILY_LOCAL | FAMILY_WILD)
            && entry_number == number.as_bytes()
            && name == MIT_MAGIC_COOKIE
        {
            return Some(data.to_vec());
        }
    }
}

fn take_be16(rest: &mut &[u8]) -> Option<u16> {
    let (head, tail) = rest.split_first_chunk::<2>()?;
    *rest = tail;
    Some(u16::from_be_bytes(*head))
}

fn take_field<'a>(rest: &mut &'a [u8]) -> Option<&'a [u8]> {
    let len = usize::from(take_be16(rest)?);
    if rest.len() < len {
        return None;
    }
    let (field, tail) = rest.split_at(len);
    *rest = tail;
    Some(field)
}

/// `$XAUTHORITY` when set and not empty, else `$HOME/.Xauthority`.
#[must_use]
pub fn xauthority_path_from(
    xauthority: Option<OsString>,
    home: Option<OsString>,
) -> Option<PathBuf> {
    if let Some(path) = xauthority.filter(|path| !path.is_empty()) {
        return Some(PathBuf::from(path));
    }
    home.filter(|home| !home.is_empty())
        .map(|home| PathBuf::from(home).join(".Xauthority"))
}

fn read_xauthority(path: &Path) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)
        .ok()?
        .take(MAX_XAUTHORITY_BYTES)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(bytes)
}

#[derive(Debug, Default)]
struct Atoms {
    wm_class: u32,
    net_wm_name: u32,
    utf8_string: u32,
}

/// What came back for one request.
enum Answer {
    Reply(Vec<u8>),
    Error(u8),
}

/// A connection to a local X server that answers window-owner questions.
///
/// Connecting and the first lookup share one deadline of `timeout`; each
/// later lookup gets a fresh one. After any error the connection is closed
/// and every later lookup fails.
#[derive(Debug)]
pub struct X11Client {
    stream: UnixStream,
    timeout: Duration,
    deadline: Instant,
    carry_connect_deadline: bool,
    sequence: u16,
    root: u32,
    atoms: Atoms,
    xres_opcode: u8,
    closed: bool,
}

impl X11Client {
    /// Connects to the X server at `socket`, authorizing with `cookie` as
    /// MIT-MAGIC-COOKIE-1 when given, and looks up the atoms and the
    /// X-Resource extension.
    ///
    /// # Errors
    ///
    /// Fails when the socket cannot be used, the server refuses or answers
    /// nonsense, X-Resource is missing, or `timeout` passes.
    pub fn connect(
        socket: &Path,
        cookie: Option<&[u8]>,
        timeout: Duration,
    ) -> Result<Self, X11Error> {
        let socket = socket.to_path_buf();
        Self::connect_via(move || UnixStream::connect(socket), cookie, timeout)
    }

    /// [`X11Client::connect`] with the socket opened by `open`, which runs on
    /// a helper thread so that a blocking connect (a server whose listen
    /// backlog is full) shares the deadline. On expiry the helper is left to
    /// finish on its own and drops whatever stream it gets.
    fn connect_via<F>(open: F, cookie: Option<&[u8]>, timeout: Duration) -> Result<Self, X11Error>
    where
        F: FnOnce() -> io::Result<UnixStream> + Send + 'static,
    {
        let deadline = deadline_after(timeout);
        let stream = open_within(open, deadline)?;
        let mut client = Self {
            stream,
            timeout,
            deadline,
            carry_connect_deadline: true,
            sequence: 0,
            root: 0,
            atoms: Atoms::default(),
            xres_opcode: 0,
            closed: false,
        };
        if let Err(err) = client.handshake(cookie) {
            client.close();
            return Err(err);
        }
        Ok(client)
    }

    /// Connects to `$DISPLAY` with the cookie from `$XAUTHORITY` (else
    /// `$HOME/.Xauthority`) when an entry exists, else no authorization,
    /// bounded by [`X11_TIMEOUT`].
    ///
    /// # Errors
    ///
    /// [`X11Error::NoDisplay`] when `$DISPLAY` is not a local display; the
    /// errors of [`X11Client::connect`] otherwise.
    pub fn connect_from_env() -> Result<Self, X11Error> {
        let display = std::env::var("DISPLAY").map_err(|_| X11Error::NoDisplay)?;
        let (socket, number) = display_socket(&display).ok_or(X11Error::NoDisplay)?;
        let cookie = xauthority_path_from(std::env::var_os("XAUTHORITY"), std::env::var_os("HOME"))
            .and_then(|path| read_xauthority(&path))
            .and_then(|bytes| find_cookie(&bytes, number));
        Self::connect(&socket, cookie.as_deref(), X11_TIMEOUT)
    }

    /// The process owning the root children whose `WM_CLASS` instance or
    /// class equals `app_id` and whose `_NET_WM_NAME` (else `WM_NAME`)
    /// equals `title`. At most [`MAX_X11_WINDOWS`] children are examined.
    ///
    /// # Errors
    ///
    /// Fails on a protocol error, an X error other than a vanished window,
    /// a closed connection, or when the deadline passes.
    pub fn window_owner(&mut self, app_id: &str, title: &str) -> Result<OwnerLookup, X11Error> {
        if self.closed {
            return Err(protocol("the connection was closed after an earlier error"));
        }
        if !std::mem::take(&mut self.carry_connect_deadline) {
            self.deadline = deadline_after(self.timeout);
        }
        let result = self.lookup(app_id, title);
        if result.is_err() {
            self.close();
        }
        result
    }

    fn close(&mut self) {
        self.closed = true;
        let _ = self.stream.shutdown(Shutdown::Both);
    }

    fn lookup(&mut self, app_id: &str, title: &str) -> Result<OwnerLookup, X11Error> {
        let children = self.query_tree()?;
        let mut owners = BTreeSet::new();
        for window in children {
            if self.window_matches(window, app_id, title)?
                && let Some(pid) = self.client_pid(window)?
            {
                owners.insert(pid);
            }
        }
        let mut owners: Vec<u32> = owners.into_iter().collect();
        Ok(match owners.len() {
            0 => OwnerLookup::NoMatch,
            1 => OwnerLookup::Owner(owners.remove(0)),
            _ => OwnerLookup::Ambiguous(owners),
        })
    }

    fn handshake(&mut self, cookie: Option<&[u8]>) -> Result<(), X11Error> {
        let (name, data): (&[u8], &[u8]) = match cookie {
            Some(cookie) => (MIT_MAGIC_COOKIE, cookie),
            None => (&[], &[]),
        };
        let data_len = u16::try_from(data.len()).map_err(|_| protocol("cookie too long"))?;
        let mut setup = vec![b'l', 0];
        put16(&mut setup, 11);
        put16(&mut setup, 0);
        put16(&mut setup, u16::try_from(name.len()).unwrap_or(u16::MAX));
        put16(&mut setup, data_len);
        put16(&mut setup, 0);
        setup.extend_from_slice(name);
        pad4(&mut setup);
        setup.extend_from_slice(data);
        pad4(&mut setup);
        self.write_all(&setup)?;

        let mut head = [0; 8];
        self.read_exact(&mut head)?;
        let mut extra = vec![0; usize::from(get16(&head, 6)?) * 4];
        self.read_exact(&mut extra)?;
        match head[0] {
            1 => {}
            0 => {
                let len = usize::from(head[1]).min(extra.len());
                return Err(X11Error::Refused(reason_text(&extra[..len])));
            }
            2 => return Err(X11Error::Refused(reason_text(&extra))),
            status => return Err(protocol(format!("unknown setup status {status}"))),
        }
        let vendor_len = usize::from(get16(&extra, 16)?);
        let screens = *extra
            .get(20)
            .ok_or_else(|| protocol("setup reply too short"))?;
        let formats = usize::from(
            *extra
                .get(21)
                .ok_or_else(|| protocol("setup reply too short"))?,
        );
        if screens == 0 {
            return Err(protocol("the X server has no screen"));
        }
        self.root = get32(&extra, 32 + vendor_len.div_ceil(4) * 4 + 8 * formats)?;

        self.atoms = Atoms {
            wm_class: self.intern_atom(b"WM_CLASS")?,
            net_wm_name: self.intern_atom(b"_NET_WM_NAME")?,
            utf8_string: self.intern_atom(b"UTF8_STRING")?,
        };
        self.xres_opcode = self
            .query_extension(b"X-Resource")?
            .ok_or(X11Error::NoXRes)?;
        Ok(())
    }

    /// The atom named `name`, or 0 when the server has none (never created).
    fn intern_atom(&mut self, name: &[u8]) -> Result<u32, X11Error> {
        let reply = self.reply(OP_INTERN_ATOM, 1, &named_body(name)?)?;
        get32(&reply, 8)
    }

    /// The major opcode of extension `name` when present.
    fn query_extension(&mut self, name: &[u8]) -> Result<Option<u8>, X11Error> {
        let reply = self.reply(OP_QUERY_EXTENSION, 0, &named_body(name)?)?;
        let present = *reply
            .get(8)
            .ok_or_else(|| protocol("short QueryExtension reply"))?;
        let opcode = *reply
            .get(9)
            .ok_or_else(|| protocol("short QueryExtension reply"))?;
        Ok((present != 0).then_some(opcode))
    }

    /// The root's children, at most [`MAX_X11_WINDOWS`] of them.
    fn query_tree(&mut self) -> Result<Vec<u32>, X11Error> {
        let mut body = Vec::new();
        put32(&mut body, self.root);
        let reply = self.reply(OP_QUERY_TREE, 0, &body)?;
        let count = usize::from(get16(&reply, 16)?);
        let children = reply
            .get(32..32 + count * 4)
            .ok_or_else(|| protocol("QueryTree reply shorter than its children"))?;
        let (chunks, _) = children.as_chunks::<4>();
        Ok(chunks
            .iter()
            .take(MAX_X11_WINDOWS)
            .map(|chunk| u32::from_le_bytes(*chunk))
            .collect())
    }

    /// The 8-bit value of `property` on `window` when it has type `kind` and
    /// fits in one read; `None` when absent, of another type or format, too
    /// long, or the window is gone.
    fn get_property(
        &mut self,
        window: u32,
        property: u32,
        kind: u32,
    ) -> Result<Option<Vec<u8>>, X11Error> {
        let mut body = Vec::new();
        for value in [window, property, kind, 0, PROPERTY_LONG_LENGTH] {
            put32(&mut body, value);
        }
        let reply = match self.request(OP_GET_PROPERTY, 0, &body)? {
            Answer::Reply(reply) => reply,
            Answer::Error(ERROR_BAD_WINDOW) => return Ok(None),
            Answer::Error(code) => return Err(x_error(OP_GET_PROPERTY, code)),
        };
        let format = reply[1];
        let actual = get32(&reply, 8)?;
        let bytes_after = get32(&reply, 12)?;
        if actual != kind || format != 8 || bytes_after != 0 {
            return Ok(None);
        }
        let len = usize::try_from(get32(&reply, 16)?).map_err(|_| protocol("property too long"))?;
        let value = reply
            .get(32..)
            .and_then(|rest| rest.get(..len))
            .ok_or_else(|| protocol("GetProperty reply shorter than its value"))?;
        Ok(Some(value.to_vec()))
    }

    fn window_matches(&mut self, window: u32, app_id: &str, title: &str) -> Result<bool, X11Error> {
        let Some(class) = self.get_property(window, self.atoms.wm_class, ATOM_STRING)? else {
            return Ok(false);
        };
        if !class
            .split(|&byte| byte == 0)
            .take(2)
            .any(|part| part == app_id.as_bytes())
        {
            return Ok(false);
        }
        let net_wm_name = if self.atoms.net_wm_name != 0 && self.atoms.utf8_string != 0 {
            self.get_property(window, self.atoms.net_wm_name, self.atoms.utf8_string)?
        } else {
            None
        };
        if let Some(name) = net_wm_name {
            return Ok(name == title.as_bytes());
        }
        Ok(self
            .get_property(window, ATOM_WM_NAME, ATOM_STRING)?
            .is_some_and(|name| latin1(&name) == title))
    }

    /// The host pid of the client owning `window`, from X-Resource.
    fn client_pid(&mut self, window: u32) -> Result<Option<u32>, X11Error> {
        let mut body = Vec::new();
        for value in [1, window, XRES_LOCAL_CLIENT_PID] {
            put32(&mut body, value);
        }
        let reply = self.reply(self.xres_opcode, XRES_QUERY_CLIENT_IDS, &body)?;
        let count = get32(&reply, 8)?;
        let mut at = 32;
        for _ in 0..count {
            let mask = get32(&reply, at + 4)?;
            let len = usize::try_from(get32(&reply, at + 8)?)
                .map_err(|_| protocol("client id too long"))?;
            let value_at = at + 12;
            if reply.len() - value_at.min(reply.len()) < len {
                return Err(protocol("XResQueryClientIds reply shorter than its values"));
            }
            if mask == XRES_LOCAL_CLIENT_PID && len >= 4 {
                let pid = get32(&reply, value_at)?;
                return Ok((pid != 0).then_some(pid));
            }
            at = value_at + len;
        }
        Ok(None)
    }

    /// Sends a request and expects a reply; an X error is a protocol error.
    fn reply(&mut self, opcode: u8, data: u8, body: &[u8]) -> Result<Vec<u8>, X11Error> {
        match self.request(opcode, data, body)? {
            Answer::Reply(reply) => Ok(reply),
            Answer::Error(code) => Err(x_error(opcode, code)),
        }
    }

    /// Sends one request and reads until its reply or error, skipping events.
    fn request(&mut self, opcode: u8, data: u8, body: &[u8]) -> Result<Answer, X11Error> {
        let units =
            u16::try_from(body.len().div_ceil(4) + 1).map_err(|_| protocol("request too long"))?;
        let mut request = vec![opcode, data];
        put16(&mut request, units);
        request.extend_from_slice(body);
        pad4(&mut request);
        self.sequence = self.sequence.wrapping_add(1);
        self.write_all(&request)?;
        loop {
            let mut head = [0; 32];
            self.read_exact(&mut head)?;
            match head[0] {
                0 | 1 => {
                    let sequence = get16(&head, 2)?;
                    if sequence != self.sequence {
                        return Err(protocol(format!(
                            "answer for request {sequence} while waiting for {}",
                            self.sequence
                        )));
                    }
                    if head[0] == 0 {
                        return Ok(Answer::Error(head[1]));
                    }
                    let extra = Self::extra_length(&head)?;
                    let mut reply = head.to_vec();
                    reply.resize(32 + extra, 0);
                    self.read_exact(&mut reply[32..])?;
                    return Ok(Answer::Reply(reply));
                }
                code if code & 0x7f == GENERIC_EVENT => {
                    let mut skipped = vec![0; Self::extra_length(&head)?];
                    self.read_exact(&mut skipped)?;
                }
                _ => {}
            }
        }
    }

    /// The bytes following a 32-byte reply or generic event, refused past
    /// [`MAX_REPLY_BYTES`].
    fn extra_length(head: &[u8]) -> Result<usize, X11Error> {
        let bytes = u64::from(get32(head, 4)?) * 4;
        if bytes > MAX_REPLY_BYTES {
            return Err(protocol(format!(
                "reply of {bytes} bytes exceeds the limit"
            )));
        }
        usize::try_from(bytes).map_err(|_| protocol("reply too long"))
    }

    fn remaining(&self) -> Result<Duration, X11Error> {
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|left| !left.is_zero())
            .ok_or(X11Error::Timeout)
    }

    fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), X11Error> {
        let mut filled = 0;
        while filled < buf.len() {
            let left = self.remaining()?;
            self.stream
                .set_read_timeout(Some(left))
                .map_err(X11Error::Io)?;
            match self.stream.read(&mut buf[filled..]) {
                Ok(0) => return Err(protocol("the X server closed the connection")),
                Ok(n) => filled += n,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(err) => return Err(from_io(err)),
            }
        }
        Ok(())
    }

    fn write_all(&mut self, buf: &[u8]) -> Result<(), X11Error> {
        let mut written = 0;
        while written < buf.len() {
            let left = self.remaining()?;
            self.stream
                .set_write_timeout(Some(left))
                .map_err(X11Error::Io)?;
            match self.stream.write(&buf[written..]) {
                Ok(0) => return Err(protocol("the X server closed the connection")),
                Ok(n) => written += n,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(err) => return Err(from_io(err)),
            }
        }
        Ok(())
    }
}

/// `timeout` from now; a timeout too large to represent waits about a century.
fn deadline_after(timeout: Duration) -> Instant {
    let now = Instant::now();
    now.checked_add(timeout)
        .or_else(|| now.checked_add(Duration::from_hours(100 * 365 * 24)))
        .unwrap_or(now)
}

/// Runs `open` on a helper thread and waits for it until `deadline`.
fn open_within<F>(open: F, deadline: Instant) -> Result<UnixStream, X11Error>
where
    F: FnOnce() -> io::Result<UnixStream> + Send + 'static,
{
    let (sender, receiver) = mpsc::channel();
    thread::Builder::new()
        .name("melibea-x11-connect".to_string())
        .spawn(move || {
            // The receiver is gone after a timeout; the stream is then dropped.
            let _ = sender.send(open());
        })
        .map_err(X11Error::Io)?;
    let left = deadline
        .checked_duration_since(Instant::now())
        .unwrap_or_default();
    match receiver.recv_timeout(left) {
        Ok(result) => result.map_err(X11Error::Io),
        Err(RecvTimeoutError::Timeout) => Err(X11Error::Timeout),
        Err(RecvTimeoutError::Disconnected) => {
            Err(protocol("the connecting thread ended without an answer"))
        }
    }
}

fn x_error(opcode: u8, code: u8) -> X11Error {
    protocol(format!("X error {code} for request {opcode}"))
}

fn reason_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .trim_end_matches(['\0', '\n'])
        .to_string()
}

fn latin1(bytes: &[u8]) -> String {
    bytes.iter().copied().map(char::from).collect()
}

/// The body of `InternAtom` and `QueryExtension`: length, two unused bytes,
/// the name.
fn named_body(name: &[u8]) -> Result<Vec<u8>, X11Error> {
    let len = u16::try_from(name.len()).map_err(|_| protocol("name too long"))?;
    let mut body = Vec::new();
    put16(&mut body, len);
    put16(&mut body, 0);
    body.extend_from_slice(name);
    Ok(body)
}

fn put16(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn put32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn pad4(bytes: &mut Vec<u8>) {
    bytes.resize(bytes.len().div_ceil(4) * 4, 0);
}

fn get16(bytes: &[u8], at: usize) -> Result<u16, X11Error> {
    at.checked_add(2)
        .and_then(|end| bytes.get(at..end))
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .ok_or_else(|| protocol("short reply"))
}

fn get32(bytes: &[u8], at: usize) -> Result<u32, X11Error> {
    at.checked_add(4)
        .and_then(|end| bytes.get(at..end))
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or_else(|| protocol("short reply"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::{Read, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant};

    const ROOT: u32 = 0x3a5;
    const ATOM_STRING_ID: u32 = 31;
    const ATOM_WM_NAME_ID: u32 = 39;
    const ATOM_WM_CLASS_ID: u32 = 67;
    const ATOM_INTEGER_ID: u32 = 19;
    const ATOM_NET_WM_NAME_ID: u32 = 300;
    const ATOM_UTF8_STRING_ID: u32 = 301;
    const XRES_OPCODE: u8 = 140;

    #[derive(Clone, Copy, Default)]
    struct FakeWindow {
        id: u32,
        class: Option<(&'static str, &'static str)>,
        class_type: Option<u32>,
        net_wm_name: Option<&'static str>,
        wm_name: Option<&'static str>,
        pid: Option<u32>,
    }

    fn window(
        id: u32,
        class: (&'static str, &'static str),
        title: &'static str,
        pid: u32,
    ) -> FakeWindow {
        FakeWindow {
            id,
            class: Some(class),
            net_wm_name: Some(title),
            pid: Some(pid),
            ..FakeWindow::default()
        }
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Fault {
        None,
        Silent,
        Trickle,
        NoXRes,
        TruncatedQueryTree,
        ErrorOnQueryTree,
        HugeQueryTree,
        ShortQueryTreeBody,
        VanishedWindow(u32),
    }

    struct Fake {
        dir: PathBuf,
        socket: PathBuf,
        handle: Option<JoinHandle<()>>,
    }

    impl Fake {
        fn finish(mut self) {
            if let Some(handle) = self.handle.take() {
                handle.join().expect("fake X server panicked");
            }
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    struct Script {
        windows: Vec<FakeWindow>,
        fault: Fault,
        refuse: Option<&'static str>,
        cookie: Option<Vec<u8>>,
    }

    fn script(windows: Vec<FakeWindow>) -> Script {
        Script {
            windows,
            fault: Fault::None,
            refuse: None,
            cookie: None,
        }
    }

    fn spawn(name: &str, script: Script) -> Fake {
        let dir = std::env::temp_dir().join(format!("melibea-x11-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("X0");
        let listener = UnixListener::bind(&socket).unwrap();
        let handle = thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                serve(stream, &script);
            }
        });
        Fake {
            dir,
            socket,
            handle: Some(handle),
        }
    }

    fn connect(fake: &Fake) -> X11Client {
        X11Client::connect(&fake.socket, None, Duration::from_secs(1))
            .expect("connect to the fake X server")
    }

    // ---- canned little-endian replies ----

    fn le16(bytes: &mut Vec<u8>, value: u16) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn le32(bytes: &mut Vec<u8>, value: u32) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn units(len: usize) -> u32 {
        u32::try_from(len / 4).unwrap()
    }

    fn setup_reply(root: u32) -> Vec<u8> {
        let vendor = b"fake";
        let mut data = Vec::new();
        le32(&mut data, 12_345); // release
        le32(&mut data, 0x0020_0000); // resource id base
        le32(&mut data, 0x001f_ffff); // resource id mask
        le32(&mut data, 0); // motion buffer
        le16(&mut data, u16::try_from(vendor.len()).unwrap());
        le16(&mut data, 0xffff); // maximum request length
        data.push(1); // screens
        data.push(1); // formats
        data.extend_from_slice(&[0, 0, 32, 32, 8, 255]);
        data.extend_from_slice(&[0; 4]);
        data.extend_from_slice(vendor);
        pad4(&mut data);
        data.extend_from_slice(&[24, 32, 32, 0, 0, 0, 0, 0]); // one format
        le32(&mut data, root);
        data.extend_from_slice(&[0; 36]); // rest of the screen, no depths
        let mut reply = vec![1, 0];
        le16(&mut reply, 11);
        le16(&mut reply, 0);
        le16(&mut reply, u16::try_from(data.len() / 4).unwrap());
        reply.extend_from_slice(&data);
        reply
    }

    fn refused_setup(reason: &str) -> Vec<u8> {
        let mut data = reason.as_bytes().to_vec();
        pad4(&mut data);
        let mut reply = vec![0, u8::try_from(reason.len()).unwrap()];
        le16(&mut reply, 11);
        le16(&mut reply, 0);
        le16(&mut reply, u16::try_from(data.len() / 4).unwrap());
        reply.extend_from_slice(&data);
        reply
    }

    fn reply_head(seq: u16, data: u8, extra_units: u32) -> Vec<u8> {
        let mut reply = vec![1, data];
        le16(&mut reply, seq);
        le32(&mut reply, extra_units);
        reply
    }

    fn intern_atom_reply(seq: u16, atom: u32) -> Vec<u8> {
        let mut reply = reply_head(seq, 0, 0);
        le32(&mut reply, atom);
        reply.resize(32, 0);
        reply
    }

    fn query_extension_reply(seq: u16, present: bool, opcode: u8) -> Vec<u8> {
        let mut reply = reply_head(seq, 0, 0);
        reply.extend_from_slice(&[u8::from(present), opcode, 0, 0]);
        reply.resize(32, 0);
        reply
    }

    fn query_tree_reply(seq: u16, count: u16, children: &[u32]) -> Vec<u8> {
        let mut reply = reply_head(seq, 0, u32::try_from(children.len()).unwrap());
        le32(&mut reply, ROOT);
        le32(&mut reply, 0);
        le16(&mut reply, count);
        reply.resize(32, 0);
        for child in children {
            le32(&mut reply, *child);
        }
        reply
    }

    fn get_property_reply(seq: u16, kind: u32, format: u8, value: &[u8]) -> Vec<u8> {
        let mut body = value.to_vec();
        pad4(&mut body);
        let mut reply = reply_head(seq, format, units(body.len()));
        le32(&mut reply, kind);
        le32(&mut reply, 0); // bytes after
        le32(&mut reply, u32::try_from(value.len()).unwrap());
        reply.resize(32, 0);
        reply.extend_from_slice(&body);
        reply
    }

    fn xres_reply(seq: u16, ids: &[(u32, u32, &[u32])]) -> Vec<u8> {
        let mut body = Vec::new();
        for (client, mask, values) in ids {
            le32(&mut body, *client);
            le32(&mut body, *mask);
            le32(&mut body, u32::try_from(values.len() * 4).unwrap());
            for value in *values {
                le32(&mut body, *value);
            }
        }
        let mut reply = reply_head(seq, 0, units(body.len()));
        le32(&mut reply, u32::try_from(ids.len()).unwrap());
        reply.resize(32, 0);
        reply.extend_from_slice(&body);
        reply
    }

    fn error_packet(seq: u16, code: u8) -> Vec<u8> {
        let mut packet = vec![0, code];
        le16(&mut packet, seq);
        packet.resize(32, 0);
        packet
    }

    fn expose_event(seq: u16) -> Vec<u8> {
        let mut event = vec![12, 0];
        le16(&mut event, seq);
        event.resize(32, 0);
        event
    }

    fn get16(bytes: &[u8], at: usize) -> u16 {
        u16::from_le_bytes([bytes[at], bytes[at + 1]])
    }

    fn get32(bytes: &[u8], at: usize) -> u32 {
        u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
    }

    fn padded(len: usize) -> usize {
        len.div_ceil(4) * 4
    }

    fn drain(stream: &mut UnixStream) {
        let mut sink = [0; 64];
        while matches!(stream.read(&mut sink), Ok(n) if n > 0) {}
    }

    // ---- the fake server ----

    fn serve(mut stream: UnixStream, script: &Script) {
        let mut head = [0; 12];
        if stream.read_exact(&mut head).is_err() {
            return;
        }
        assert_eq!(head[0], b'l', "client must speak little-endian");
        assert_eq!(get16(&head, 2), 11);
        let name_len = usize::from(get16(&head, 6));
        let data_len = usize::from(get16(&head, 8));
        let mut auth = vec![0; padded(name_len) + padded(data_len)];
        stream.read_exact(&mut auth).unwrap();
        let name = &auth[..name_len];
        let data = &auth[padded(name_len)..padded(name_len) + data_len];
        let authorized = match &script.cookie {
            Some(cookie) => name == b"MIT-MAGIC-COOKIE-1" && data == cookie.as_slice(),
            None => name.is_empty() && data.is_empty(),
        };
        if let Some(reason) = script
            .refuse
            .or((!authorized).then_some("No protocol specified\n"))
        {
            let _ = stream.write_all(&refused_setup(reason));
            return;
        }
        match script.fault {
            Fault::Silent => return drain(&mut stream),
            Fault::Trickle => {
                for byte in setup_reply(ROOT) {
                    if stream.write_all(&[byte]).is_err() {
                        return;
                    }
                    thread::sleep(Duration::from_millis(50));
                }
                return;
            }
            _ => {}
        }
        stream.write_all(&setup_reply(ROOT)).unwrap();
        let mut seq: u16 = 0;
        loop {
            let mut head = [0; 4];
            if stream.read_exact(&mut head).is_err() {
                return;
            }
            let len = usize::from(get16(&head, 2)) * 4;
            let mut request = head.to_vec();
            request.resize(len, 0);
            if stream.read_exact(&mut request[4..]).is_err() {
                return;
            }
            seq = seq.wrapping_add(1);
            let reply = match request[0] {
                16 => {
                    let n = usize::from(get16(&request, 4));
                    assert_eq!(request[1], 1, "atoms are looked up, never created");
                    let atom = match &request[8..8 + n] {
                        b"WM_CLASS" => ATOM_WM_CLASS_ID,
                        b"_NET_WM_NAME" => ATOM_NET_WM_NAME_ID,
                        b"UTF8_STRING" => ATOM_UTF8_STRING_ID,
                        _ => 0,
                    };
                    intern_atom_reply(seq, atom)
                }
                98 => {
                    let n = usize::from(get16(&request, 4));
                    assert_eq!(&request[8..8 + n], b"X-Resource");
                    query_extension_reply(seq, script.fault != Fault::NoXRes, XRES_OPCODE)
                }
                15 => {
                    assert_eq!(get32(&request, 4), ROOT);
                    let children: Vec<u32> = script.windows.iter().map(|w| w.id).collect();
                    let count = u16::try_from(children.len()).unwrap();
                    match script.fault {
                        Fault::TruncatedQueryTree => {
                            let reply = query_tree_reply(seq, count, &children);
                            let _ = stream.write_all(&reply[..reply.len() - 2]);
                            return;
                        }
                        Fault::ErrorOnQueryTree => error_packet(seq, 1),
                        Fault::HugeQueryTree => {
                            let mut reply = reply_head(seq, 0, (1 << 20) / 4 + 1);
                            reply.resize(32, 0);
                            let _ = stream.write_all(&reply);
                            return drain(&mut stream);
                        }
                        Fault::ShortQueryTreeBody => query_tree_reply(seq, count + 5, &children),
                        _ => {
                            let mut reply = expose_event(seq.wrapping_sub(1));
                            reply.extend(query_tree_reply(seq, count, &children));
                            reply
                        }
                    }
                }
                20 => answer_get_property(script, seq, &request),
                XRES_OPCODE => answer_client_ids(script, seq, &request),
                _ => error_packet(seq, 1),
            };
            if stream.write_all(&reply).is_err() {
                return;
            }
        }
    }

    fn answer_get_property(script: &Script, seq: u16, request: &[u8]) -> Vec<u8> {
        assert_eq!(request[1], 0, "properties are never deleted");
        assert!(get32(request, 20) <= 1024, "property reads are bounded");
        let (id, property, kind) = (get32(request, 4), get32(request, 8), get32(request, 12));
        if script.fault == Fault::VanishedWindow(id) {
            return error_packet(seq, 3);
        }
        let Some(w) = script.windows.iter().find(|w| w.id == id) else {
            return error_packet(seq, 3);
        };
        let value: Option<(u32, Vec<u8>)> = match property {
            ATOM_WM_CLASS_ID => w.class.map(|(instance, class)| {
                (
                    w.class_type.unwrap_or(ATOM_STRING_ID),
                    format!("{instance}\0{class}\0").into_bytes(),
                )
            }),
            ATOM_NET_WM_NAME_ID => w
                .net_wm_name
                .map(|t| (ATOM_UTF8_STRING_ID, t.as_bytes().to_vec())),
            ATOM_WM_NAME_ID => w.wm_name.map(|t| (ATOM_STRING_ID, t.as_bytes().to_vec())),
            _ => None,
        };
        match value {
            Some((actual, bytes)) if actual == kind => get_property_reply(seq, actual, 8, &bytes),
            Some((actual, bytes)) => {
                // Wrong type: the server reports the type and size only.
                let mut reply = reply_head(seq, 8, 0);
                le32(&mut reply, actual);
                le32(&mut reply, u32::try_from(bytes.len()).unwrap());
                reply.resize(32, 0);
                reply
            }
            None => get_property_reply(seq, 0, 0, &[]),
        }
    }

    fn answer_client_ids(script: &Script, seq: u16, request: &[u8]) -> Vec<u8> {
        assert_eq!(request[1], 4, "XResQueryClientIds");
        assert_eq!(get32(request, 4), 1, "one client spec");
        let (client, mask) = (get32(request, 8), get32(request, 12));
        assert_eq!(mask, 2, "LocalClientPID");
        let pid = script
            .windows
            .iter()
            .find(|w| w.id == client)
            .and_then(|w| w.pid);
        let base = client & !0x001f_ffff;
        match pid {
            // A ClientXID value first: the pid is the value whose mask is 2.
            Some(pid) => xres_reply(seq, &[(base, 1, &[base]), (base, 2, &[pid])]),
            None => xres_reply(seq, &[]),
        }
    }

    fn victoria_windows() -> Vec<FakeWindow> {
        vec![
            FakeWindow {
                id: 0x0060_0001,
                ..FakeWindow::default()
            },
            window(0x0280_0001, ("steamwebhelper", "steam"), "Steam", 4242),
            window(
                0x0340_0001,
                ("steam_proton", "steam_proton"),
                "Victoria 3",
                90_994,
            ),
            window(
                0x0340_0002,
                ("steam_proton", "steam_proton"),
                "Default IME",
                90_994,
            ),
            window(
                0x0280_0003,
                ("steam_proton", "steam_proton"),
                "Default IME",
                91_006,
            ),
        ]
    }

    // ---- tests ----

    #[test]
    fn display_socket_parses_local_displays() {
        assert_eq!(
            display_socket(":0"),
            Some((PathBuf::from("/tmp/.X11-unix/X0"), 0))
        );
        assert_eq!(
            display_socket(":1.0"),
            Some((PathBuf::from("/tmp/.X11-unix/X1"), 1))
        );
        for display in [
            "host:0",
            "",
            "wayland-1",
            ":",
            ":+1",
            ":a",
            ":0.",
            ":0.x",
            "unix:0",
            ":99999",
        ] {
            assert_eq!(display_socket(display), None, "{display:?}");
        }
    }

    fn xauth_entry(
        family: u16,
        address: &[u8],
        number: &[u8],
        name: &[u8],
        data: &[u8],
    ) -> Vec<u8> {
        let mut entry = family.to_be_bytes().to_vec();
        for field in [address, number, name, data] {
            entry.extend_from_slice(&u16::try_from(field.len()).unwrap().to_be_bytes());
            entry.extend_from_slice(field);
        }
        entry
    }

    #[test]
    fn finds_the_cookie_for_the_display() {
        let mit = b"MIT-MAGIC-COOKIE-1";
        let mut blob = xauth_entry(0, &[127, 0, 0, 1], b"1", mit, &[9; 16]);
        blob.extend(xauth_entry(256, b"host", b"0", mit, &[1; 16]));
        blob.extend(xauth_entry(256, b"other-host", b"1", mit, &[2; 16]));
        assert_eq!(find_cookie(&blob, 1), Some(vec![2; 16]));
        assert_eq!(find_cookie(&blob, 0), Some(vec![1; 16]));
        assert_eq!(find_cookie(&blob, 2), None);

        let wild = xauth_entry(0xffff, b"", b"3", mit, &[3; 16]);
        assert_eq!(find_cookie(&wild, 3), Some(vec![3; 16]));

        let xdm = xauth_entry(256, b"host", b"0", b"XDM-AUTHORIZATION-1", &[4; 16]);
        assert_eq!(find_cookie(&xdm, 0), None);

        let truncated = &blob[..blob.len() - 3];
        assert_eq!(find_cookie(truncated, 1), None);
        assert_eq!(find_cookie(&[], 0), None);
    }

    #[test]
    fn xauthority_path_prefers_the_variable_then_home() {
        let home = Some(OsString::from("/home/u"));
        assert_eq!(
            xauthority_path_from(Some(OsString::from("/run/auth")), home.clone()),
            Some(PathBuf::from("/run/auth"))
        );
        assert_eq!(
            xauthority_path_from(None, home.clone()),
            Some(PathBuf::from("/home/u/.Xauthority"))
        );
        assert_eq!(
            xauthority_path_from(Some(OsString::new()), home),
            Some(PathBuf::from("/home/u/.Xauthority"))
        );
        assert_eq!(xauthority_path_from(None, None), None);
    }

    #[test]
    fn setup_refusal_is_reported() {
        let mut s = script(Vec::new());
        s.refuse = Some("Authorization required");
        let fake = spawn("refused", s);
        let result = X11Client::connect(&fake.socket, None, Duration::from_secs(1));
        assert!(
            matches!(&result, Err(X11Error::Refused(reason)) if reason == "Authorization required"),
            "{result:?}"
        );
        fake.finish();
    }

    #[test]
    fn the_cookie_is_sent() {
        let mut s = script(victoria_windows());
        s.cookie = Some(vec![7; 16]);
        let fake = spawn("cookie", s);
        let client = X11Client::connect(&fake.socket, Some(&[7; 16]), Duration::from_secs(1));
        assert!(client.is_ok(), "{client:?}");
        drop(client);
        fake.finish();

        let mut s = script(victoria_windows());
        s.cookie = Some(vec![7; 16]);
        let fake = spawn("wrong-cookie", s);
        let client = X11Client::connect(&fake.socket, Some(&[8; 16]), Duration::from_secs(1));
        assert!(matches!(client, Err(X11Error::Refused(_))), "{client:?}");
        fake.finish();
    }

    #[test]
    fn finds_the_owner_of_victoria_3() {
        let fake = spawn("victoria", script(victoria_windows()));
        let mut client = connect(&fake);
        assert_eq!(
            client.window_owner("steam_proton", "Victoria 3").unwrap(),
            OwnerLookup::Owner(90_994)
        );
        // The same connection answers a second lookup.
        assert_eq!(
            client.window_owner("steam", "Steam").unwrap(),
            OwnerLookup::Owner(4242)
        );
        drop(client);
        fake.finish();
    }

    #[test]
    fn two_owners_are_ambiguous() {
        let windows = vec![
            window(
                0x0340_0001,
                ("steam_proton", "steam_proton"),
                "Victoria 3",
                2,
            ),
            window(
                0x0280_0001,
                ("steam_proton", "steam_proton"),
                "Victoria 3",
                1,
            ),
        ];
        let fake = spawn("ambiguous", script(windows));
        let mut client = connect(&fake);
        assert_eq!(
            client.window_owner("steam_proton", "Victoria 3").unwrap(),
            OwnerLookup::Ambiguous(vec![1, 2])
        );
        drop(client);
        fake.finish();
    }

    #[test]
    fn no_match_is_no_match() {
        let fake = spawn("no-match", script(victoria_windows()));
        let mut client = connect(&fake);
        assert_eq!(
            client.window_owner("steam_proton", "Stellaris").unwrap(),
            OwnerLookup::NoMatch
        );
        assert_eq!(
            client.window_owner("steam_app_0", "Victoria 3").unwrap(),
            OwnerLookup::NoMatch
        );
        drop(client);
        fake.finish();
    }

    #[test]
    fn matches_the_instance_or_the_class() {
        let fake = spawn("class", script(victoria_windows()));
        let mut client = connect(&fake);
        assert_eq!(
            client.window_owner("steamwebhelper", "Steam").unwrap(),
            OwnerLookup::Owner(4242)
        );
        assert_eq!(
            client.window_owner("steam", "Steam").unwrap(),
            OwnerLookup::Owner(4242)
        );
        drop(client);
        fake.finish();
    }

    #[test]
    fn falls_back_to_wm_name() {
        let windows = vec![
            FakeWindow {
                id: 0x0340_0001,
                class: Some(("steam_proton", "steam_proton")),
                wm_name: Some("Victoria 3"),
                pid: Some(90_994),
                ..FakeWindow::default()
            },
            // _NET_WM_NAME wins over WM_NAME when both exist.
            FakeWindow {
                id: 0x0280_0001,
                class: Some(("steam_proton", "steam_proton")),
                net_wm_name: Some("Default IME"),
                wm_name: Some("Victoria 3"),
                pid: Some(91_006),
                ..FakeWindow::default()
            },
        ];
        let fake = spawn("wm-name", script(windows));
        let mut client = connect(&fake);
        assert_eq!(
            client.window_owner("steam_proton", "Victoria 3").unwrap(),
            OwnerLookup::Owner(90_994)
        );
        drop(client);
        fake.finish();
    }

    #[test]
    fn a_property_of_the_wrong_type_does_not_match() {
        let mut odd = window(
            0x0340_0001,
            ("steam_proton", "steam_proton"),
            "Victoria 3",
            90_994,
        );
        odd.class_type = Some(ATOM_INTEGER_ID);
        let fake = spawn("wrong-type", script(vec![odd]));
        let mut client = connect(&fake);
        assert_eq!(
            client.window_owner("steam_proton", "Victoria 3").unwrap(),
            OwnerLookup::NoMatch
        );
        drop(client);
        fake.finish();
    }

    #[test]
    fn a_window_that_vanishes_is_skipped() {
        let mut s = script(victoria_windows());
        s.fault = Fault::VanishedWindow(0x0280_0001);
        let fake = spawn("vanished", s);
        let mut client = connect(&fake);
        assert_eq!(
            client.window_owner("steam_proton", "Victoria 3").unwrap(),
            OwnerLookup::Owner(90_994)
        );
        drop(client);
        fake.finish();
    }

    #[test]
    fn missing_xres_is_reported() {
        let mut s = script(victoria_windows());
        s.fault = Fault::NoXRes;
        let fake = spawn("no-xres", s);
        let result = X11Client::connect(&fake.socket, None, Duration::from_secs(1));
        assert!(matches!(result, Err(X11Error::NoXRes)), "{result:?}");
        fake.finish();
    }

    #[test]
    fn malformed_replies_are_errors_not_panics() {
        for (name, fault) in [
            ("truncated", Fault::TruncatedQueryTree),
            ("error", Fault::ErrorOnQueryTree),
            ("huge", Fault::HugeQueryTree),
            ("short", Fault::ShortQueryTreeBody),
        ] {
            let mut s = script(victoria_windows());
            s.fault = fault;
            let fake = spawn(&format!("malformed-{name}"), s);
            let mut client = connect(&fake);
            let result = client.window_owner("steam_proton", "Victoria 3");
            assert!(
                matches!(result, Err(X11Error::Protocol(_))),
                "{name}: {result:?}"
            );
            // The connection is closed: the fake sees end of file while the
            // client is still alive, and later lookups fail at once.
            fake.finish();
            let again = client.window_owner("steam_proton", "Victoria 3");
            assert!(
                matches!(again, Err(X11Error::Protocol(_))),
                "{name}: {again:?}"
            );
        }
    }

    #[test]
    fn a_silent_server_times_out() {
        let mut s = script(Vec::new());
        s.fault = Fault::Silent;
        let fake = spawn("silent", s);
        let start = Instant::now();
        let result = X11Client::connect(&fake.socket, None, Duration::from_millis(200));
        assert!(
            matches!(result, Err(X11Error::Timeout | X11Error::Io(_))),
            "{result:?}"
        );
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "{:?}",
            start.elapsed()
        );
        fake.finish();
    }

    #[test]
    fn a_trickling_server_times_out() {
        let mut s = script(Vec::new());
        s.fault = Fault::Trickle;
        let fake = spawn("trickle", s);
        let start = Instant::now();
        let result = X11Client::connect(&fake.socket, None, Duration::from_millis(200));
        assert!(matches!(result, Err(X11Error::Timeout)), "{result:?}");
        assert!(
            start.elapsed() < Duration::from_millis(500),
            "{:?}",
            start.elapsed()
        );
        fake.finish();
    }

    #[test]
    fn a_missing_socket_is_an_io_error() {
        let dir = std::env::temp_dir().join(format!("melibea-x11-missing-{}", std::process::id()));
        let result = X11Client::connect(&dir.join("X0"), None, Duration::from_millis(200));
        assert!(matches!(result, Err(X11Error::Io(_))), "{result:?}");
    }

    #[test]
    fn a_connect_that_blocks_times_out() {
        // Stands in for a listener whose backlog is full: the connect itself
        // never returns within the deadline.
        let fake = spawn("blocked-connect", script(victoria_windows()));
        let socket = fake.socket.clone();
        let start = Instant::now();
        let result = X11Client::connect_via(
            move || {
                thread::sleep(Duration::from_secs(2));
                UnixStream::connect(socket)
            },
            None,
            Duration::from_millis(200),
        );
        assert!(matches!(result, Err(X11Error::Timeout)), "{result:?}");
        assert!(
            start.elapsed() < Duration::from_millis(500),
            "{:?}",
            start.elapsed()
        );
        drop(fake);
    }

    #[test]
    fn a_slow_connect_shares_the_deadline() {
        // The connect returns in time but uses most of the budget; the
        // trickling setup must still end at the one deadline.
        let mut s = script(Vec::new());
        s.fault = Fault::Trickle;
        let fake = spawn("slow-connect", s);
        let socket = fake.socket.clone();
        let start = Instant::now();
        let result = X11Client::connect_via(
            move || {
                thread::sleep(Duration::from_millis(150));
                UnixStream::connect(socket)
            },
            None,
            Duration::from_millis(200),
        );
        assert!(matches!(result, Err(X11Error::Timeout)), "{result:?}");
        assert!(
            start.elapsed() < Duration::from_millis(350),
            "{:?}",
            start.elapsed()
        );
        fake.finish();
    }

    #[test]
    fn an_enormous_timeout_does_not_panic() {
        let fake = spawn("enormous-timeout", script(victoria_windows()));
        let mut client = X11Client::connect(&fake.socket, None, Duration::MAX).expect("connect");
        assert_eq!(
            client.window_owner("steam_proton", "Victoria 3").unwrap(),
            OwnerLookup::Owner(90_994)
        );
        assert_eq!(
            client.window_owner("steam_proton", "Victoria 3").unwrap(),
            OwnerLookup::Owner(90_994)
        );
        drop(client);
        fake.finish();
    }
}
