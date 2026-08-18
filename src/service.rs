//! Local Unix-socket service for the versioned Melibea contract.

use std::{
    env,
    error::Error,
    fmt, fs,
    io::{self, BufRead, BufReader, Read, Write},
    os::unix::{
        fs::{FileTypeExt, MetadataExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender, SyncSender},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use crate::{
    minimization::MinimizedWindow,
    protocol::{
        ActionResult, ClientRequest, Message, PROTOCOL_VERSION, ProtocolError, Request,
        SUPPORTED_PROTOCOL_VERSIONS, ServerMessage, Window, diff_windows,
    },
};

pub const MELIBEA_SOCKET_ENV: &str = "MELIBEA_SOCKET";
const XDG_RUNTIME_DIR_ENV: &str = "XDG_RUNTIME_DIR";
const DEFAULT_SOCKET_NAME: &str = "melibea.sock";
const ACCEPT_POLL_INTERVAL: Duration = Duration::from_millis(20);
const CLIENT_IO_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_REQUEST_BYTES: u64 = 64 * 1024;

#[derive(serde::Deserialize)]
struct ClientVersionProbe {
    version: u32,
}

/// Resolves the daemon socket without creating files.
///
/// # Errors
///
/// Returns an error when neither `MELIBEA_SOCKET` nor `XDG_RUNTIME_DIR` names
/// a usable path.
pub fn socket_path() -> Result<PathBuf, SocketPathError> {
    if let Some(path) = env::var_os(MELIBEA_SOCKET_ENV).filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }

    let runtime = env::var_os(XDG_RUNTIME_DIR_ENV)
        .filter(|value| !value.is_empty())
        .ok_or(SocketPathError::MissingRuntimeDirectory)?;
    Ok(PathBuf::from(runtime).join(DEFAULT_SOCKET_NAME))
}

/// Executes compositor-authoritative actions requested through the service.
pub trait ActionExecutor: Send + Sync + 'static {
    /// Runs one action request and returns niri's semantic result.
    ///
    /// # Errors
    ///
    /// Returns a user-facing reason when niri rejects or cannot receive the
    /// action.
    fn execute(&self, request: &Request) -> Result<ActionResult, String>;
}

impl<F> ActionExecutor for F
where
    F: Fn(&Request) -> Result<ActionResult, String> + Send + Sync + 'static,
{
    fn execute(&self, request: &Request) -> Result<ActionResult, String> {
        self(request)
    }
}

/// Running Melibea protocol service. Dropping it stops both worker threads and
/// removes only the socket inode created by this instance.
pub struct Service {
    sender: Sender<BrokerMessage>,
    running: Arc<AtomicBool>,
    accept_thread: Option<JoinHandle<()>>,
    broker_thread: Option<JoinHandle<()>>,
    path: PathBuf,
    socket_identity: SocketIdentity,
}

impl Service {
    /// Binds one local service socket and starts the ordered state broker.
    ///
    /// # Errors
    ///
    /// Returns an error for an occupied path, an active daemon, or filesystem
    /// and socket failures.
    pub fn start(
        path: impl Into<PathBuf>,
        executor: impl ActionExecutor,
    ) -> Result<Self, ServiceError> {
        let path = path.into();
        prepare_socket_path(&path)?;
        let listener = UnixListener::bind(&path).map_err(|source| ServiceError::Bind {
            path: path.clone(),
            source,
        })?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).map_err(|source| {
            ServiceError::Permissions {
                path: path.clone(),
                source,
            }
        })?;
        let socket_identity =
            SocketIdentity::read(&path).map_err(|source| ServiceError::Inspect {
                path: path.clone(),
                source,
            })?;
        listener
            .set_nonblocking(true)
            .map_err(ServiceError::Nonblocking)?;

        let (sender, receiver) = mpsc::channel();
        let running = Arc::new(AtomicBool::new(true));
        let broker_thread = {
            let executor = Arc::new(executor);
            thread::spawn(move || broker_loop(&receiver, &executor))
        };
        let accept_thread = {
            let sender = sender.clone();
            let running = Arc::clone(&running);
            thread::spawn(move || accept_loop(&listener, &sender, &running))
        };

        Ok(Self {
            sender,
            running,
            accept_thread: Some(accept_thread),
            broker_thread: Some(broker_thread),
            path,
            socket_identity,
        })
    }

    /// Atomically publishes a complete authoritative niri snapshot. The call
    /// returns only after subscribers are registered against the new revision.
    ///
    /// # Errors
    ///
    /// Returns an error if the broker has stopped.
    pub fn publish_snapshot(&self, windows: &[MinimizedWindow]) -> Result<(), ServiceStopped> {
        self.send_synchronized(|acknowledged| BrokerMessage::Publish {
            windows: windows.iter().map(Window::from).collect(),
            acknowledged,
        })
    }

    /// Marks the public projection unavailable until the next snapshot.
    /// Existing subscribers remain connected and receive the state transition.
    ///
    /// # Errors
    ///
    /// Returns an error if the broker has stopped.
    pub fn set_unavailable(&self, reason: impl Into<String>) -> Result<(), ServiceStopped> {
        self.send_synchronized(|acknowledged| BrokerMessage::SetUnavailable {
            reason: reason.into(),
            acknowledged,
        })
    }

    fn send_synchronized(
        &self,
        message: impl FnOnce(SyncSender<()>) -> BrokerMessage,
    ) -> Result<(), ServiceStopped> {
        let (acknowledged, wait) = mpsc::sync_channel(0);
        self.sender
            .send(message(acknowledged))
            .map_err(|_| ServiceStopped)?;
        wait.recv().map_err(|_| ServiceStopped)
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
        if let Some(handle) = self.accept_thread.take() {
            let _ = handle.join();
        }
        let _ = self.sender.send(BrokerMessage::Shutdown);
        if let Some(handle) = self.broker_thread.take() {
            let _ = handle.join();
        }

        if SocketIdentity::read(&self.path).ok().as_ref() == Some(&self.socket_identity) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn prepare_socket_path(path: &Path) -> Result<(), ServiceError> {
    let Some(parent) = path.parent() else {
        return Err(ServiceError::InvalidPath(path.to_owned()));
    };
    if !parent.is_dir() {
        return Err(ServiceError::MissingParent(parent.to_owned()));
    }

    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(ServiceError::Inspect {
                path: path.to_owned(),
                source,
            });
        }
    };
    if !metadata.file_type().is_socket() {
        return Err(ServiceError::OccupiedPath(path.to_owned()));
    }

    if UnixStream::connect(path).is_ok() {
        return Err(ServiceError::AlreadyRunning(path.to_owned()));
    }
    fs::remove_file(path).map_err(|source| ServiceError::RemoveStale {
        path: path.to_owned(),
        source,
    })
}

fn accept_loop(listener: &UnixListener, sender: &Sender<BrokerMessage>, running: &AtomicBool) {
    while running.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _)) => {
                let sender = sender.clone();
                thread::spawn(move || read_client_request(stream, &sender));
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(ACCEPT_POLL_INTERVAL);
            }
            Err(_) => break,
        }
    }
}

fn read_client_request(mut stream: UnixStream, sender: &Sender<BrokerMessage>) {
    let _ = stream.set_read_timeout(Some(CLIENT_IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(CLIENT_IO_TIMEOUT));

    let mut line = String::new();
    let limited = stream.take(MAX_REQUEST_BYTES + 1);
    let mut reader = BufReader::new(limited);
    let result = reader.read_line(&mut line);
    stream = reader.into_inner().into_inner();

    let bytes = match result {
        Ok(bytes) => bytes,
        Err(error) => {
            let _ = write_message(
                &mut stream,
                &ServerMessage::new(Message::Error(ProtocolError::invalid_request(format!(
                    "cannot read request: {error}"
                )))),
            );
            return;
        }
    };
    if bytes == 0 || bytes as u64 > MAX_REQUEST_BYTES || !line.ends_with('\n') {
        let _ = write_message(
            &mut stream,
            &ServerMessage::new(Message::Error(ProtocolError::invalid_request(
                "request must be one newline-terminated JSON value within 64 KiB",
            ))),
        );
        return;
    }

    let request = match serde_json::from_str::<ClientRequest>(&line) {
        Ok(request) => request,
        Err(error) => {
            // A future protocol version may legitimately carry request types
            // and fields this daemon cannot decode. Probe only the envelope
            // version before classifying the parse failure so every
            // unsupported client receives the advertised compatibility error
            // instead of a misleading current-schema error.
            if let Ok(probe) = serde_json::from_str::<ClientVersionProbe>(&line)
                && !SUPPORTED_PROTOCOL_VERSIONS.contains(&probe.version)
            {
                let _ = write_message(
                    &mut stream,
                    &ServerMessage::new(Message::Error(ProtocolError::incompatible(probe.version))),
                );
                return;
            }
            let _ = write_message(
                &mut stream,
                &ServerMessage::new(Message::Error(ProtocolError::invalid_request(format!(
                    "invalid request JSON: {error}"
                )))),
            );
            return;
        }
    };
    if let Err(error) = request.validate() {
        let response_version = if error.code == crate::protocol::ErrorCode::IncompatibleVersion {
            PROTOCOL_VERSION
        } else {
            request.version
        };
        let _ = write_message(
            &mut stream,
            &ServerMessage::for_version(response_version, Message::Error(error)),
        );
        return;
    }

    let _ = sender.send(BrokerMessage::Client {
        version: request.version,
        request: request.request,
        stream,
    });
}

enum BrokerMessage {
    Publish {
        windows: Vec<Window>,
        acknowledged: SyncSender<()>,
    },
    SetUnavailable {
        reason: String,
        acknowledged: SyncSender<()>,
    },
    Client {
        version: u32,
        request: Request,
        stream: UnixStream,
    },
    Shutdown,
}

struct BrokerState {
    revision: u64,
    ready: bool,
    unavailable_reason: String,
    windows: Vec<Window>,
    subscribers: Vec<Subscriber>,
}

struct Subscriber {
    version: u32,
    stream: UnixStream,
}

impl Default for BrokerState {
    fn default() -> Self {
        Self {
            revision: 0,
            ready: false,
            unavailable_reason: "awaiting authoritative niri snapshot".to_owned(),
            windows: Vec::new(),
            subscribers: Vec::new(),
        }
    }
}

fn broker_loop(receiver: &Receiver<BrokerMessage>, executor: &Arc<impl ActionExecutor>) {
    let mut state = BrokerState::default();
    while let Ok(message) = receiver.recv() {
        match message {
            BrokerMessage::Publish {
                windows,
                acknowledged,
            } => {
                state.publish(windows);
                let _ = acknowledged.send(());
            }
            BrokerMessage::SetUnavailable {
                reason,
                acknowledged,
            } => {
                state.set_unavailable(reason);
                let _ = acknowledged.send(());
            }
            BrokerMessage::Client {
                version,
                request,
                mut stream,
            } => state.handle_client(version, request, &mut stream, executor.as_ref()),
            BrokerMessage::Shutdown => break,
        }
    }
}

impl BrokerState {
    fn publish(&mut self, windows: Vec<Window>) {
        if self.ready && self.windows == windows {
            return;
        }

        let was_ready = self.ready;
        let changes = was_ready.then(|| diff_windows(&self.windows, &windows));
        self.revision = self.revision.saturating_add(1);
        self.ready = true;
        self.unavailable_reason.clear();
        self.windows = windows;

        let message = match changes {
            Some(changes) => Message::Changes {
                revision: self.revision,
                changes,
            },
            None => self.snapshot_message(),
        };
        self.broadcast(&message);
    }

    fn set_unavailable(&mut self, reason: String) {
        if !self.ready && self.unavailable_reason == reason {
            return;
        }
        self.ready = false;
        self.unavailable_reason.clone_from(&reason);
        self.broadcast(&Message::Unavailable {
            revision: self.revision,
            reason,
        });
    }

    fn handle_client(
        &mut self,
        version: u32,
        request: Request,
        stream: &mut UnixStream,
        executor: &impl ActionExecutor,
    ) {
        match request {
            Request::List => {
                let message = if self.ready {
                    self.snapshot_message()
                } else {
                    Message::Error(ProtocolError::unavailable(&self.unavailable_reason))
                };
                let _ = write_message(stream, &ServerMessage::for_version(version, message));
            }
            Request::Subscribe => {
                let message = if self.ready {
                    self.snapshot_message()
                } else {
                    Message::Unavailable {
                        revision: self.revision,
                        reason: self.unavailable_reason.clone(),
                    }
                };
                if write_message(stream, &ServerMessage::for_version(version, message)).is_ok()
                    && let Ok(subscriber) = stream.try_clone()
                {
                    self.subscribers.push(Subscriber {
                        version,
                        stream: subscriber,
                    });
                }
            }
            action @ (Request::Minimize { .. }
            | Request::Restore { .. }
            | Request::Close { .. }) => {
                let message = match executor.execute(&action) {
                    Ok(result) => Message::ActionResult(result),
                    Err(error) => Message::Error(ProtocolError::action_failed(error)),
                };
                let _ = write_message(stream, &ServerMessage::for_version(version, message));
            }
        }
    }

    fn snapshot_message(&self) -> Message {
        Message::Snapshot {
            revision: self.revision,
            windows: self.windows.clone(),
        }
    }

    fn broadcast(&mut self, message: &Message) {
        self.subscribers.retain_mut(|subscriber| {
            write_message(
                &mut subscriber.stream,
                &ServerMessage::for_version(subscriber.version, message.clone()),
            )
            .is_ok()
        });
    }
}

fn write_message(stream: &mut UnixStream, message: &ServerMessage) -> io::Result<()> {
    serde_json::to_writer(&mut *stream, message)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    stream.write_all(b"\n")?;
    stream.flush()
}

/// Small synchronous client used by the reference CLI and shell consumers.
#[derive(Clone, Debug)]
pub struct ServiceClient {
    path: PathBuf,
}

impl ServiceClient {
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Connects to the default socket path.
    ///
    /// # Errors
    ///
    /// Returns an error when the runtime socket path cannot be resolved.
    pub fn from_environment() -> Result<Self, ClientError> {
        socket_path()
            .map(Self::new)
            .map_err(ClientError::SocketPath)
    }

    /// Sends one request and reads exactly one response.
    ///
    /// # Errors
    ///
    /// Returns an error for connection, encoding, I/O, or invalid response
    /// failures.
    pub fn request(&self, request: Request) -> Result<ServerMessage, ClientError> {
        self.request_versioned(&ClientRequest::new(request))
    }

    /// Sends one protocol-v2 request and reads exactly one v2 response.
    ///
    /// # Errors
    ///
    /// Returns an error for connection, encoding, I/O, or invalid response
    /// failures.
    pub fn request_v2(&self, request: Request) -> Result<ServerMessage, ClientError> {
        self.request_versioned(&ClientRequest::v2(request))
    }

    fn request_versioned(&self, request: &ClientRequest) -> Result<ServerMessage, ClientError> {
        let expected_version = request.version;
        let mut reader = self.start_request(request)?;
        read_server_message(&mut reader, expected_version)
    }

    /// Opens a persistent subscription. The first `read` yields either a full
    /// snapshot or an unavailable marker; changes can only follow a snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error for connection or request-encoding failures.
    pub fn subscribe(&self) -> Result<Subscription, ClientError> {
        self.subscribe_versioned(&ClientRequest::new(Request::Subscribe))
    }

    /// Opens a persistent protocol-v2 subscription.
    ///
    /// # Errors
    ///
    /// Returns an error for connection or request-encoding failures.
    pub fn subscribe_v2(&self) -> Result<Subscription, ClientError> {
        self.subscribe_versioned(&ClientRequest::v2(Request::Subscribe))
    }

    fn subscribe_versioned(&self, request: &ClientRequest) -> Result<Subscription, ClientError> {
        let expected_version = request.version;
        self.start_request(request).map(|reader| Subscription {
            reader,
            expected_version,
        })
    }

    fn start_request(&self, request: &ClientRequest) -> Result<BufReader<UnixStream>, ClientError> {
        let mut stream =
            UnixStream::connect(&self.path).map_err(|source| ClientError::Connect {
                path: self.path.clone(),
                source,
            })?;
        serde_json::to_writer(&mut stream, request).map_err(ClientError::Serialize)?;
        stream.write_all(b"\n").map_err(ClientError::Write)?;
        stream.flush().map_err(ClientError::Write)?;
        Ok(BufReader::new(stream))
    }
}

/// Persistent stream of versioned service messages.
pub struct Subscription {
    reader: BufReader<UnixStream>,
    expected_version: u32,
}

impl Subscription {
    /// Reads the next snapshot, state change, or availability message.
    ///
    /// # Errors
    ///
    /// Returns an error for I/O, EOF, malformed JSON, or an unexpected server
    /// protocol version.
    pub fn read(&mut self) -> Result<ServerMessage, ClientError> {
        read_server_message(&mut self.reader, self.expected_version)
    }
}

fn read_server_message(
    reader: &mut BufReader<UnixStream>,
    expected_version: u32,
) -> Result<ServerMessage, ClientError> {
    let mut line = String::new();
    let bytes = reader.read_line(&mut line).map_err(ClientError::Read)?;
    if bytes == 0 {
        return Err(ClientError::Closed);
    }
    let message: ServerMessage = serde_json::from_str(&line).map_err(ClientError::InvalidReply)?;
    if message.version != expected_version {
        return Err(ClientError::IncompatibleServerVersion(message.version));
    }
    Ok(message)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SocketIdentity {
    device: u64,
    inode: u64,
}

impl SocketIdentity {
    fn read(path: &Path) -> io::Result<Self> {
        let metadata = fs::symlink_metadata(path)?;
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
}

#[derive(Debug)]
pub enum SocketPathError {
    MissingRuntimeDirectory,
}

impl fmt::Display for SocketPathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingRuntimeDirectory => write!(
                formatter,
                "neither {MELIBEA_SOCKET_ENV} nor {XDG_RUNTIME_DIR_ENV} is set"
            ),
        }
    }
}

impl Error for SocketPathError {}

#[derive(Debug)]
pub enum ServiceError {
    InvalidPath(PathBuf),
    MissingParent(PathBuf),
    OccupiedPath(PathBuf),
    AlreadyRunning(PathBuf),
    Inspect { path: PathBuf, source: io::Error },
    RemoveStale { path: PathBuf, source: io::Error },
    Bind { path: PathBuf, source: io::Error },
    Permissions { path: PathBuf, source: io::Error },
    Nonblocking(io::Error),
}

impl fmt::Display for ServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPath(path) => {
                write!(formatter, "invalid Melibea socket path: {}", path.display())
            }
            Self::MissingParent(path) => write!(
                formatter,
                "Melibea socket parent directory does not exist: {}",
                path.display()
            ),
            Self::OccupiedPath(path) => write!(
                formatter,
                "refusing to replace non-socket path: {}",
                path.display()
            ),
            Self::AlreadyRunning(path) => write!(
                formatter,
                "another Melibea service is already listening on {}",
                path.display()
            ),
            Self::Inspect { path, source } => {
                write!(formatter, "cannot inspect {}: {source}", path.display())
            }
            Self::RemoveStale { path, source } => write!(
                formatter,
                "cannot remove stale Melibea socket {}: {source}",
                path.display()
            ),
            Self::Bind { path, source } => write!(
                formatter,
                "cannot bind Melibea socket {}: {source}",
                path.display()
            ),
            Self::Permissions { path, source } => write!(
                formatter,
                "cannot restrict Melibea socket {}: {source}",
                path.display()
            ),
            Self::Nonblocking(error) => {
                write!(formatter, "cannot configure Melibea listener: {error}")
            }
        }
    }
}

impl Error for ServiceError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServiceStopped;

impl fmt::Display for ServiceStopped {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Melibea service stopped unexpectedly")
    }
}

impl Error for ServiceStopped {}

#[derive(Debug)]
pub enum ClientError {
    SocketPath(SocketPathError),
    Connect { path: PathBuf, source: io::Error },
    Serialize(serde_json::Error),
    Write(io::Error),
    Read(io::Error),
    Closed,
    InvalidReply(serde_json::Error),
    IncompatibleServerVersion(u32),
}

impl fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SocketPath(error) => error.fmt(formatter),
            Self::Connect { path, source } => write!(
                formatter,
                "cannot connect to Melibea service {}: {source}",
                path.display()
            ),
            Self::Serialize(error) => write!(formatter, "cannot encode Melibea request: {error}"),
            Self::Write(error) => write!(formatter, "cannot send Melibea request: {error}"),
            Self::Read(error) => write!(formatter, "cannot read Melibea response: {error}"),
            Self::Closed => formatter.write_str("Melibea service closed the connection"),
            Self::InvalidReply(error) => {
                write!(formatter, "invalid Melibea service response: {error}")
            }
            Self::IncompatibleServerVersion(version) => write!(
                formatter,
                "Melibea service replied with unsupported protocol version {version}"
            ),
        }
    }
}

impl Error for ClientError {}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        io::{BufRead, BufReader, Write},
        os::unix::net::UnixStream,
        path::PathBuf,
        sync::{Arc, Mutex},
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{Service, ServiceClient};
    use crate::{
        minimization::MinimizedWindow,
        protocol::{
            ActionResult, ActionStatus, BubbleAnchor, ClientRequest, ErrorCode, Message, Operation,
            PROTOCOL_VERSION, PROTOCOL_VERSION_V2, Request, SUPPORTED_PROTOCOL_VERSIONS,
            ServerMessage, WindowChange, WindowTransition,
        },
        transition::WindowId,
    };

    fn socket() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "melibea-service-test-{}-{nonce}.sock",
            std::process::id()
        ))
    }

    fn window(id: u64, title: &str) -> MinimizedWindow {
        MinimizedWindow {
            id: WindowId(id),
            app_id: Some("org.example.App".to_owned()),
            title: Some(title.to_owned()),
        }
    }

    fn anchor() -> BubbleAnchor {
        BubbleAnchor::new("DP-1", 10.0, 20.0, 48.0, 48.0).expect("valid anchor")
    }

    fn result(request: &Request) -> Result<ActionResult, String> {
        let (operation, requested_id, status) = match request {
            Request::Minimize { window_id, .. } => {
                (Operation::Minimize, *window_id, ActionStatus::Applied)
            }
            Request::Restore { window_id, .. } => (
                Operation::Restore,
                Some(*window_id),
                ActionStatus::AlreadyInRequestedState,
            ),
            Request::Close { window_id } => (
                Operation::Close,
                Some(*window_id),
                ActionStatus::CloseRequested,
            ),
            Request::List | Request::Subscribe => return Err("not an action".to_owned()),
        };
        Ok(ActionResult {
            operation,
            requested_id,
            window_id: requested_id,
            status,
        })
    }

    #[test]
    fn list_is_unavailable_until_first_authoritative_snapshot() {
        let path = socket();
        let service = Service::start(&path, result).expect("start service");
        let response = ServiceClient::new(&path)
            .request(Request::List)
            .expect("service response");
        assert!(matches!(
            response.message,
            Message::Error(error) if error.code == ErrorCode::Unavailable
        ));
        drop(service);
        assert!(!path.exists());
    }

    #[test]
    fn subscriber_receives_snapshot_before_gapless_incremental_changes() {
        let path = socket();
        let service = Service::start(&path, result).expect("start service");
        service
            .publish_snapshot(&[window(1, "one")])
            .expect("publish initial state");

        let mut subscription = ServiceClient::new(&path).subscribe().expect("subscribe");
        assert!(matches!(
            subscription.read().expect("snapshot").message,
            Message::Snapshot { revision: 1, ref windows } if windows.len() == 1 && windows[0].id == 1
        ));

        service
            .publish_snapshot(&[window(1, "updated"), window(2, "two")])
            .expect("publish changes");
        let message = subscription.read().expect("changes");
        assert_eq!(
            message.message,
            Message::Changes {
                revision: 2,
                changes: vec![
                    WindowChange::Updated {
                        index: 0,
                        window: crate::protocol::Window {
                            id: 1,
                            app_id: Some("org.example.App".to_owned()),
                            title: Some("updated".to_owned()),
                            icon_name: None,
                        },
                    },
                    WindowChange::Added {
                        index: 1,
                        window: crate::protocol::Window {
                            id: 2,
                            app_id: Some("org.example.App".to_owned()),
                            title: Some("two".to_owned()),
                            icon_name: None,
                        },
                    },
                ],
            }
        );
    }

    #[test]
    fn unavailable_reconnect_requires_a_fresh_snapshot_before_changes() {
        let path = socket();
        let service = Service::start(&path, result).expect("start service");
        service
            .publish_snapshot(&[window(1, "one")])
            .expect("initial snapshot");
        let mut subscription = ServiceClient::new(&path).subscribe().expect("subscribe");
        let _ = subscription.read().expect("initial snapshot");

        service
            .set_unavailable("niri disconnected")
            .expect("mark unavailable");
        assert!(matches!(
            subscription.read().expect("unavailable").message,
            Message::Unavailable { revision: 1, .. }
        ));

        service
            .publish_snapshot(&[window(1, "one")])
            .expect("resynchronize");
        assert!(matches!(
            subscription.read().expect("fresh snapshot").message,
            Message::Snapshot { revision: 2, .. }
        ));
    }

    #[test]
    fn actions_are_forwarded_and_return_semantic_results() {
        let path = socket();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&calls);
        let service = Service::start(&path, move |request: &Request| {
            recorded.lock().expect("lock calls").push(request.clone());
            result(request)
        })
        .expect("start service");
        service.publish_snapshot(&[]).expect("ready service");

        let client = ServiceClient::new(&path);
        let response = client
            .request(Request::Close { window_id: 42 })
            .expect("action response");
        assert!(matches!(
            response.message,
            Message::ActionResult(ActionResult {
                operation: Operation::Close,
                requested_id: Some(42),
                window_id: Some(42),
                status: ActionStatus::CloseRequested,
            })
        ));
        assert_eq!(
            calls.lock().expect("lock calls").as_slice(),
            &[Request::Close { window_id: 42 }]
        );
    }

    #[test]
    fn v2_transition_is_validated_forwarded_and_replied_to_as_v2() {
        let path = socket();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&calls);
        let service = Service::start(&path, move |request: &Request| {
            recorded.lock().expect("lock calls").push(request.clone());
            result(request)
        })
        .expect("start service");
        service.publish_snapshot(&[]).expect("ready service");

        let request =
            Request::minimize_with_transition(Some(42), WindowTransition::anchored(anchor()));
        let response = ServiceClient::new(&path)
            .request_v2(request.clone())
            .expect("v2 action response");

        assert_eq!(response.version, PROTOCOL_VERSION_V2);
        assert!(matches!(
            response.message,
            Message::ActionResult(ActionResult {
                operation: Operation::Minimize,
                requested_id: Some(42),
                window_id: Some(42),
                status: ActionStatus::Applied,
            })
        ));
        assert_eq!(calls.lock().expect("lock calls").as_slice(), &[request]);
    }

    #[test]
    fn v1_transition_is_rejected_before_action_forwarding() {
        let path = socket();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&calls);
        let _service = Service::start(&path, move |request: &Request| {
            recorded.lock().expect("lock calls").push(request.clone());
            result(request)
        })
        .expect("start service");

        let response = ServiceClient::new(&path)
            .request(Request::restore_with_transition(
                42,
                WindowTransition::Disabled,
            ))
            .expect("v1 error response");

        assert_eq!(response.version, PROTOCOL_VERSION);
        assert!(matches!(
            response.message,
            Message::Error(error) if error.code == ErrorCode::InvalidRequest
        ));
        assert!(calls.lock().expect("lock calls").is_empty());
    }

    #[test]
    fn invalid_v2_anchor_is_rejected_before_action_forwarding() {
        let path = socket();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&calls);
        let _service = Service::start(&path, move |request: &Request| {
            recorded.lock().expect("lock calls").push(request.clone());
            result(request)
        })
        .expect("start service");
        let transition = WindowTransition::Anchored {
            anchor: BubbleAnchor {
                output: String::new(),
                x: 0.0,
                y: 0.0,
                width: 48.0,
                height: 48.0,
            },
        };

        let response = ServiceClient::new(&path)
            .request_v2(Request::minimize_with_transition(None, transition))
            .expect("v2 error response");

        assert_eq!(response.version, PROTOCOL_VERSION_V2);
        assert!(matches!(
            response.message,
            Message::Error(error) if error.code == ErrorCode::InvalidRequest
        ));
        assert!(calls.lock().expect("lock calls").is_empty());
    }

    #[test]
    fn subscriptions_keep_their_requested_protocol_version() {
        let path = socket();
        let service = Service::start(&path, result).expect("start service");
        service
            .publish_snapshot(&[window(1, "one")])
            .expect("publish initial state");

        let client = ServiceClient::new(&path);
        let mut v1 = client.subscribe().expect("v1 subscription");
        let mut v2 = client.subscribe_v2().expect("v2 subscription");
        assert_eq!(
            v1.read().expect("v1 initial snapshot").version,
            PROTOCOL_VERSION
        );
        assert_eq!(
            v2.read().expect("v2 initial snapshot").version,
            PROTOCOL_VERSION_V2
        );

        service
            .publish_snapshot(&[window(1, "updated")])
            .expect("publish update");
        assert_eq!(v1.read().expect("v1 update").version, PROTOCOL_VERSION);
        assert_eq!(v2.read().expect("v2 update").version, PROTOCOL_VERSION_V2);
    }

    #[test]
    fn incompatible_client_gets_explicit_supported_version() {
        let path = socket();
        let _service = Service::start(&path, result).expect("start service");
        let mut stream = UnixStream::connect(&path).expect("connect");
        serde_json::to_writer(
            &mut stream,
            &ClientRequest {
                version: 3,
                request: Request::List,
            },
        )
        .expect("encode request");
        stream.write_all(b"\n").expect("terminate request");

        let mut line = String::new();
        BufReader::new(stream)
            .read_line(&mut line)
            .expect("read response");
        let response: ServerMessage = serde_json::from_str(&line).expect("valid response");
        assert_eq!(response.version, PROTOCOL_VERSION);
        assert!(matches!(
            response.message,
            Message::Error(error)
                if error.code == ErrorCode::IncompatibleVersion
                    && error.supported_versions == vec![PROTOCOL_VERSION, 2]
        ));
    }

    #[test]
    fn incompatible_future_request_is_classified_before_current_request_schema() {
        let path = socket();
        let _service = Service::start(&path, result).expect("start service");
        let mut stream = UnixStream::connect(&path).expect("connect");
        stream
            .write_all(
                b"{\"version\":3,\"request\":{\"type\":\"future_window_portal\",\"opaque\":{\"new\":true}}}\n",
            )
            .expect("send future request");

        let mut line = String::new();
        BufReader::new(stream)
            .read_line(&mut line)
            .expect("read response");
        let response: ServerMessage = serde_json::from_str(&line).expect("valid response");
        assert_eq!(response.version, PROTOCOL_VERSION);
        assert!(matches!(
            response.message,
            Message::Error(error)
                if error.code == ErrorCode::IncompatibleVersion
                    && error.supported_versions
                        == SUPPORTED_PROTOCOL_VERSIONS.to_vec()
        ));
    }

    #[test]
    fn service_refuses_to_replace_regular_file() {
        let path = socket();
        fs::write(&path, "keep").expect("create occupied path");
        assert!(Service::start(&path, result).is_err());
        assert_eq!(
            fs::read_to_string(&path).expect("read occupied path"),
            "keep"
        );
        fs::remove_file(path).expect("remove occupied path");
    }
}
