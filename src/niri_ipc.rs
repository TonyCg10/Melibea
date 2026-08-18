//! Minimal MIT-licensed transport for niri's newline-delimited JSON IPC.

use std::{
    env,
    error::Error,
    fmt,
    io::{self, BufRead, BufReader, Write},
    net::Shutdown,
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    attention::Proportion,
    niri::{NiriEvent, NiriEventError, parse_event_line},
    protocol::WindowTransition,
    transition::WindowId,
};

/// Environment variable exported by niri for its IPC socket.
pub const NIRI_SOCKET_ENV: &str = "NIRI_SOCKET";

/// Blocking reader for niri's event stream.
pub struct NiriEventStream {
    reader: BufReader<UnixStream>,
}

impl NiriEventStream {
    /// Connects using the socket path exported in [`NIRI_SOCKET_ENV`].
    ///
    /// # Errors
    ///
    /// Returns an error when the environment variable is absent, the socket
    /// cannot be opened, or niri rejects the event-stream request.
    pub fn connect() -> Result<Self, NiriIpcError> {
        let path = env::var_os(NIRI_SOCKET_ENV).ok_or(NiriIpcError::MissingSocketEnvironment)?;
        Self::connect_to(path)
    }

    /// Connects to a specific niri IPC socket and requests its event stream.
    ///
    /// # Errors
    ///
    /// Returns an error when the socket cannot be opened or the event-stream
    /// handshake fails.
    pub fn connect_to(path: impl AsRef<Path>) -> Result<Self, NiriIpcError> {
        let path = path.as_ref();
        let stream = UnixStream::connect(path).map_err(|source| NiriIpcError::Connect {
            path: path.to_owned(),
            source,
        })?;
        Self::start(stream)
    }

    fn start(mut stream: UnixStream) -> Result<Self, NiriIpcError> {
        stream
            .write_all(b"\"EventStream\"\n")
            .map_err(NiriIpcError::WriteRequest)?;
        stream.flush().map_err(NiriIpcError::WriteRequest)?;

        let mut reader = BufReader::new(stream);
        let mut reply = String::new();
        let bytes = reader
            .read_line(&mut reply)
            .map_err(NiriIpcError::ReadHandshake)?;
        if bytes == 0 {
            return Err(NiriIpcError::ClosedDuringHandshake);
        }

        validate_handshake(&reply)?;
        reader
            .get_mut()
            .shutdown(Shutdown::Write)
            .map_err(NiriIpcError::ShutdownWrite)?;

        Ok(Self { reader })
    }

    /// Blocks until the next event arrives.
    ///
    /// `Ok(None)` means niri closed the event stream cleanly. A caller that
    /// wants continuous observation should reconnect and rebuild state.
    ///
    /// # Errors
    ///
    /// Returns an error for I/O failures or malformed event JSON.
    pub fn read_event(&mut self) -> Result<Option<NiriEvent>, NiriIpcError> {
        let mut line = String::new();
        let bytes = self
            .reader
            .read_line(&mut line)
            .map_err(NiriIpcError::ReadEvent)?;
        if bytes == 0 {
            return Ok(None);
        }

        parse_event_line(&line)
            .map(Some)
            .map_err(NiriIpcError::InvalidEvent)
    }

    /// Creates a handle that can interrupt a blocked event read.
    ///
    /// # Errors
    ///
    /// Returns an error when the operating system cannot duplicate the socket
    /// descriptor.
    pub fn cancellation_handle(&self) -> io::Result<NiriEventStreamCancellation> {
        self.reader
            .get_ref()
            .try_clone()
            .map(|stream| NiriEventStreamCancellation { stream })
    }

    /// Sets the blocking read timeout for future events.
    ///
    /// # Errors
    ///
    /// Returns an operating-system error when the socket option cannot be set.
    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.reader.get_ref().set_read_timeout(timeout)
    }
}

/// A duplicate socket handle used to stop an event-reader thread cleanly.
pub struct NiriEventStreamCancellation {
    stream: UnixStream,
}

impl NiriEventStreamCancellation {
    /// Interrupts current and future reads on the associated event stream.
    ///
    /// # Errors
    ///
    /// Returns an operating-system error when the socket cannot be shut down.
    pub fn cancel(&self) -> io::Result<()> {
        self.stream.shutdown(Shutdown::Both)
    }
}

/// Blocking client for targeted niri layout actions.
pub struct NiriActionClient {
    reader: BufReader<UnixStream>,
}

impl NiriActionClient {
    /// Connects using the socket path exported in [`NIRI_SOCKET_ENV`].
    ///
    /// # Errors
    ///
    /// Returns an error when the environment variable is absent or the socket
    /// cannot be opened.
    pub fn connect() -> Result<Self, NiriActionError> {
        let path = env::var_os(NIRI_SOCKET_ENV).ok_or(NiriActionError::MissingSocketEnvironment)?;
        Self::connect_to(path)
    }

    /// Connects to a specific niri IPC socket.
    ///
    /// # Errors
    ///
    /// Returns an error when the socket cannot be opened.
    pub fn connect_to(path: impl AsRef<Path>) -> Result<Self, NiriActionError> {
        let path = path.as_ref();
        let stream = UnixStream::connect(path).map_err(|source| NiriActionError::Connect {
            path: path.to_owned(),
            source,
        })?;
        Ok(Self::start(stream))
    }

    fn start(stream: UnixStream) -> Self {
        Self {
            reader: BufReader::new(stream),
        }
    }

    /// Sets one specific window to a proportion of the output working area.
    ///
    /// The wire protocol expresses proportions as percentages, so Melibea's
    /// validated `0.0..=1.0` value is multiplied by 100 before serialization.
    /// Targeting a window id avoids accidentally resizing whichever window
    /// happens to be focused when niri receives the request.
    ///
    /// # Errors
    ///
    /// Returns an error when serialization, socket I/O, or niri rejects the
    /// action.
    pub fn set_window_width(
        &mut self,
        window_id: WindowId,
        width: Proportion,
    ) -> Result<(), NiriActionError> {
        self.send_action(NiriAction::SetWindowWidth {
            id: Some(window_id.0),
            change: SizeChange::SetProportion(width.get() * 100.0),
        })
        .map(|_| ())
    }

    /// Moves a visible window into niri's native minimized state.
    ///
    /// When `window_id` is `None`, niri targets the focused window.
    ///
    /// # Errors
    ///
    /// Returns an error when serialization, socket I/O, or niri rejects the
    /// experimental action.
    pub fn minimize_window(
        &mut self,
        window_id: Option<WindowId>,
    ) -> Result<NiriWindowActionResult, NiriActionError> {
        self.minimize_window_with_transition(window_id, None)
    }

    /// Moves a visible window into niri's native minimized state using an
    /// optional shell-provided transition hint.
    ///
    /// # Errors
    ///
    /// Returns an error when serialization, socket I/O, or niri rejects the
    /// experimental action.
    pub fn minimize_window_with_transition(
        &mut self,
        window_id: Option<WindowId>,
        transition: Option<WindowTransition>,
    ) -> Result<NiriWindowActionResult, NiriActionError> {
        let requested_id = window_id.map(|id| id.0);
        self.send_window_action(
            NiriAction::MinimizeWindow {
                id: requested_id,
                transition,
            },
            requested_id,
        )
    }

    /// Restores one native minimized window by compositor id.
    ///
    /// # Errors
    ///
    /// Returns an error when serialization, socket I/O, or niri rejects the
    /// experimental action.
    pub fn restore_window(
        &mut self,
        window_id: WindowId,
    ) -> Result<NiriWindowActionResult, NiriActionError> {
        self.restore_window_with_transition(window_id, None)
    }

    /// Restores one native minimized window using an optional shell-provided
    /// transition hint.
    ///
    /// # Errors
    ///
    /// Returns an error when serialization, socket I/O, or niri rejects the
    /// experimental action.
    pub fn restore_window_with_transition(
        &mut self,
        window_id: WindowId,
        transition: Option<WindowTransition>,
    ) -> Result<NiriWindowActionResult, NiriActionError> {
        self.send_window_action(
            NiriAction::RestoreWindow {
                id: window_id.0,
                transition,
            },
            Some(window_id.0),
        )
    }

    /// Requests that niri close one window by compositor id.
    ///
    /// This is used by bubble consumers after selecting an authoritative
    /// minimized-window entry.
    ///
    /// # Errors
    ///
    /// Returns an error when serialization, socket I/O, or niri rejects the
    /// action.
    pub fn close_window(
        &mut self,
        window_id: WindowId,
    ) -> Result<NiriWindowActionResult, NiriActionError> {
        self.send_window_action(
            NiriAction::CloseWindow {
                id: Some(window_id.0),
            },
            Some(window_id.0),
        )
    }

    fn send_window_action(
        &mut self,
        action: NiriAction,
        requested_id: Option<u64>,
    ) -> Result<NiriWindowActionResult, NiriActionError> {
        let result = match self.send_action(action)? {
            ActionReply::Handled => NiriWindowActionResult {
                requested_id,
                window_id: requested_id,
                status: NiriWindowActionStatus::LegacyHandled,
            },
            ActionReply::Window(result) => result,
        };
        Ok(result)
    }

    fn send_action(&mut self, action: NiriAction) -> Result<ActionReply, NiriActionError> {
        let request = ActionRequest::Action(action);
        let mut line = serde_json::to_string(&request).map_err(NiriActionError::Serialize)?;
        line.push('\n');
        self.reader
            .get_mut()
            .write_all(line.as_bytes())
            .map_err(NiriActionError::WriteRequest)?;
        self.reader
            .get_mut()
            .flush()
            .map_err(NiriActionError::WriteRequest)?;

        line.clear();
        let bytes = self
            .reader
            .read_line(&mut line)
            .map_err(NiriActionError::ReadReply)?;
        if bytes == 0 {
            return Err(NiriActionError::ClosedBeforeReply);
        }

        validate_action_reply(&line)
    }
}

/// Semantic niri result retained for Melibea's public protocol.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct NiriWindowActionResult {
    pub requested_id: Option<u64>,
    pub window_id: Option<u64>,
    pub status: NiriWindowActionStatus,
}

/// Native action status plus a compatibility value for the older request-only
/// experimental reply.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub enum NiriWindowActionStatus {
    Applied,
    AlreadyInRequestedState,
    CloseRequested,
    WindowNotFound,
    Blocked,
    LegacyHandled,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ActionReply {
    Handled,
    Window(NiriWindowActionResult),
}

#[derive(Serialize)]
enum ActionRequest {
    Action(NiriAction),
}

#[derive(Serialize)]
enum NiriAction {
    SetWindowWidth {
        id: Option<u64>,
        change: SizeChange,
    },
    CloseWindow {
        id: Option<u64>,
    },
    MinimizeWindow {
        id: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        transition: Option<WindowTransition>,
    },
    RestoreWindow {
        id: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        transition: Option<WindowTransition>,
    },
}

#[derive(Serialize)]
enum SizeChange {
    SetProportion(f64),
}

fn validate_action_reply(line: &str) -> Result<ActionReply, NiriActionError> {
    let reply: Value = serde_json::from_str(line).map_err(NiriActionError::InvalidReply)?;

    match reply {
        Value::Object(object) if object.len() == 1 => match object.into_iter().next() {
            Some((name, Value::String(response))) if name == "Ok" && response == "Handled" => {
                Ok(ActionReply::Handled)
            }
            Some((name, Value::Object(mut response))) if name == "Ok" && response.len() == 1 => {
                let Some(result) = response.remove("WindowActionResult") else {
                    return Err(NiriActionError::UnexpectedReply);
                };
                serde_json::from_value(result)
                    .map(ActionReply::Window)
                    .map_err(|_| NiriActionError::UnexpectedReply)
            }
            Some((name, Value::String(message))) if name == "Err" => {
                Err(NiriActionError::RequestRejected(message))
            }
            _ => Err(NiriActionError::UnexpectedReply),
        },
        _ => Err(NiriActionError::UnexpectedReply),
    }
}

fn validate_handshake(line: &str) -> Result<(), NiriIpcError> {
    let reply: Value = serde_json::from_str(line).map_err(NiriIpcError::InvalidHandshake)?;

    match reply {
        Value::Object(object) if object.len() == 1 => match object.into_iter().next() {
            Some((name, Value::String(response))) if name == "Ok" && response == "Handled" => {
                Ok(())
            }
            Some((name, Value::String(message))) if name == "Err" => {
                Err(NiriIpcError::RequestRejected(message))
            }
            _ => Err(NiriIpcError::UnexpectedHandshake),
        },
        _ => Err(NiriIpcError::UnexpectedHandshake),
    }
}

/// Failure while establishing or reading niri's event stream.
#[derive(Debug)]
pub enum NiriIpcError {
    MissingSocketEnvironment,
    Connect { path: PathBuf, source: io::Error },
    WriteRequest(io::Error),
    ReadHandshake(io::Error),
    ClosedDuringHandshake,
    InvalidHandshake(serde_json::Error),
    UnexpectedHandshake,
    RequestRejected(String),
    ShutdownWrite(io::Error),
    ReadEvent(io::Error),
    InvalidEvent(NiriEventError),
}

impl fmt::Display for NiriIpcError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingSocketEnvironment => write!(
                formatter,
                "{NIRI_SOCKET_ENV} is not set; run Melibea inside a niri session"
            ),
            Self::Connect { path, source } => {
                write!(
                    formatter,
                    "cannot connect to niri socket {}: {source}",
                    path.display()
                )
            }
            Self::WriteRequest(error) => write!(formatter, "cannot request niri events: {error}"),
            Self::ReadHandshake(error) => {
                write!(formatter, "cannot read niri event-stream reply: {error}")
            }
            Self::ClosedDuringHandshake => {
                formatter.write_str("niri closed IPC before accepting the event stream")
            }
            Self::InvalidHandshake(error) => {
                write!(formatter, "invalid niri event-stream reply: {error}")
            }
            Self::UnexpectedHandshake => {
                formatter.write_str("niri returned an unexpected event-stream reply")
            }
            Self::RequestRejected(message) => {
                write!(formatter, "niri rejected the event stream: {message}")
            }
            Self::ShutdownWrite(error) => {
                write!(
                    formatter,
                    "cannot finalize niri event-stream request: {error}"
                )
            }
            Self::ReadEvent(error) => write!(formatter, "cannot read niri event: {error}"),
            Self::InvalidEvent(error) => write!(formatter, "cannot decode niri event: {error}"),
        }
    }
}

impl Error for NiriIpcError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Connect { source, .. }
            | Self::WriteRequest(source)
            | Self::ReadHandshake(source)
            | Self::ShutdownWrite(source)
            | Self::ReadEvent(source) => Some(source),
            Self::InvalidHandshake(source) => Some(source),
            Self::InvalidEvent(source) => Some(source),
            Self::MissingSocketEnvironment
            | Self::ClosedDuringHandshake
            | Self::UnexpectedHandshake
            | Self::RequestRejected(_) => None,
        }
    }
}

/// Failure while sending a targeted layout action to niri.
#[derive(Debug)]
pub enum NiriActionError {
    MissingSocketEnvironment,
    Connect { path: PathBuf, source: io::Error },
    Serialize(serde_json::Error),
    WriteRequest(io::Error),
    ReadReply(io::Error),
    ClosedBeforeReply,
    InvalidReply(serde_json::Error),
    UnexpectedReply,
    RequestRejected(String),
}

impl fmt::Display for NiriActionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingSocketEnvironment => write!(
                formatter,
                "{NIRI_SOCKET_ENV} is not set; run Melibea inside a niri session"
            ),
            Self::Connect { path, source } => write!(
                formatter,
                "cannot connect to niri action socket {}: {source}",
                path.display()
            ),
            Self::Serialize(error) => write!(formatter, "cannot encode niri action: {error}"),
            Self::WriteRequest(error) => write!(formatter, "cannot send niri action: {error}"),
            Self::ReadReply(error) => write!(formatter, "cannot read niri action reply: {error}"),
            Self::ClosedBeforeReply => {
                formatter.write_str("niri closed IPC before replying to the action")
            }
            Self::InvalidReply(error) => write!(formatter, "invalid niri action reply: {error}"),
            Self::UnexpectedReply => {
                formatter.write_str("niri returned an unexpected action reply")
            }
            Self::RequestRejected(message) => {
                write!(formatter, "niri rejected the action: {message}")
            }
        }
    }
}

impl Error for NiriActionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Connect { source, .. } | Self::WriteRequest(source) | Self::ReadReply(source) => {
                Some(source)
            }
            Self::Serialize(source) | Self::InvalidReply(source) => Some(source),
            Self::MissingSocketEnvironment
            | Self::ClosedBeforeReply
            | Self::UnexpectedReply
            | Self::RequestRejected(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{BufRead, BufReader, Write},
        os::unix::net::UnixStream,
        sync::mpsc,
        thread,
    };

    use super::{
        ActionReply, NiriActionClient, NiriActionError, NiriEventStream, NiriIpcError,
        NiriWindowActionStatus, validate_action_reply,
    };
    use crate::{
        attention::{Proportion, WidthPolicy},
        niri::NiriEvent,
        protocol::{BubbleAnchor, WindowTransition},
        transition::WindowId,
    };

    fn stream_with_server(
        reply: &'static [u8],
        event: Option<&'static [u8]>,
    ) -> Result<NiriEventStream, NiriIpcError> {
        let (client, server) = UnixStream::pair().expect("socket pair");
        let server_thread = thread::spawn(move || {
            let mut server = BufReader::new(server);
            let mut request = String::new();
            server.read_line(&mut request).expect("read request");
            server.get_mut().write_all(reply).expect("write reply");
            if let Some(event) = event {
                server.get_mut().write_all(event).expect("write event");
            }
            request
        });

        let stream = NiriEventStream::start(client);
        let request = server_thread.join().expect("server thread");
        assert_eq!(request, "\"EventStream\"\n");
        stream
    }

    fn width(value: f64) -> Proportion {
        match WidthPolicy::proportion(value).expect("valid width") {
            WidthPolicy::Proportion(width) => width,
            WidthPolicy::Preserve => unreachable!(),
        }
    }

    fn send_action_to_server<T>(
        reply: &'static [u8],
        action: impl FnOnce(&mut NiriActionClient) -> Result<T, NiriActionError>,
    ) -> (Result<T, NiriActionError>, String) {
        let (client, server) = UnixStream::pair().expect("socket pair");
        let server_thread = thread::spawn(move || {
            let mut server = BufReader::new(server);
            let mut request = String::new();
            server.read_line(&mut request).expect("read action");
            server.get_mut().write_all(reply).expect("write reply");
            request
        });

        let mut client = NiriActionClient::start(client);
        let result = action(&mut client);
        let request = server_thread.join().expect("server thread");
        (result, request)
    }

    #[test]
    fn starts_stream_and_decodes_event() {
        let mut stream = stream_with_server(
            b"{\"Ok\":\"Handled\"}\n",
            Some(b"{\"WindowFocusChanged\":{\"id\":42}}\n"),
        )
        .expect("valid stream");

        assert_eq!(
            stream.read_event().expect("valid event"),
            Some(NiriEvent::WindowFocusChanged {
                id: Some(WindowId(42))
            })
        );
        assert_eq!(stream.read_event().expect("clean close"), None);
    }

    #[test]
    fn reports_rejected_event_stream() {
        let error = stream_with_server(b"{\"Err\":\"disabled\"}\n", None)
            .err()
            .expect("rejection");
        assert!(matches!(error, NiriIpcError::RequestRejected(message) if message == "disabled"));
    }

    #[test]
    fn rejects_unexpected_success_response() {
        let error = stream_with_server(b"{\"Ok\":{\"Version\":\"26.4\"}}\n", None)
            .err()
            .expect("unexpected reply");
        assert!(matches!(error, NiriIpcError::UnexpectedHandshake));
    }

    #[test]
    fn reports_malformed_event_after_valid_handshake() {
        let mut stream = stream_with_server(b"{\"Ok\":\"Handled\"}\n", Some(b"not-json\n"))
            .expect("valid stream");
        assert!(matches!(
            stream.read_event(),
            Err(NiriIpcError::InvalidEvent(_))
        ));
    }

    #[test]
    fn sends_targeted_window_width_as_niri_percentage() {
        let (result, request) = send_action_to_server(b"{\"Ok\":\"Handled\"}\n", |client| {
            client.set_window_width(WindowId(42), width(0.5))
        });
        result.expect("accepted action");

        let actual: serde_json::Value = serde_json::from_str(&request).expect("action JSON");
        let expected = serde_json::json!({
            "Action": {
                "SetWindowWidth": {
                    "id": 42,
                    "change": { "SetProportion": 50.0 }
                }
            }
        });
        assert_eq!(actual, expected);
    }

    #[test]
    fn reports_rejected_width_action() {
        let (result, _) = send_action_to_server(b"{\"Err\":\"window not found\"}\n", |client| {
            client.set_window_width(WindowId(42), width(0.5))
        });
        assert!(matches!(
            result,
            Err(NiriActionError::RequestRejected(message)) if message == "window not found"
        ));
    }

    #[test]
    fn sends_native_minimize_restore_and_close_actions() {
        let accepted = b"{\"Ok\":\"Handled\"}\n";
        let cases = [
            (
                send_action_to_server(accepted, |client| {
                    client.minimize_window(Some(WindowId(42)))
                }),
                serde_json::json!({"Action":{"MinimizeWindow":{"id":42}}}),
            ),
            (
                send_action_to_server(accepted, |client| client.minimize_window(None)),
                serde_json::json!({"Action":{"MinimizeWindow":{"id":null}}}),
            ),
            (
                send_action_to_server(accepted, |client| client.restore_window(WindowId(42))),
                serde_json::json!({"Action":{"RestoreWindow":{"id":42}}}),
            ),
            (
                send_action_to_server(accepted, |client| client.close_window(WindowId(42))),
                serde_json::json!({"Action":{"CloseWindow":{"id":42}}}),
            ),
        ];

        for ((result, request), expected) in cases {
            result.expect("accepted action");
            let actual: serde_json::Value = serde_json::from_str(&request).expect("action JSON");
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn forwards_anchored_and_disabled_transitions_to_niri() {
        let accepted = b"{\"Ok\":\"Handled\"}\n";
        let anchor =
            BubbleAnchor::new("DP-1", 10.5, 20.25, 48.0, 48.0).expect("valid bubble anchor");
        let cases = [
            (
                send_action_to_server(accepted, |client| {
                    client.minimize_window_with_transition(
                        Some(WindowId(42)),
                        Some(WindowTransition::anchored(anchor)),
                    )
                }),
                serde_json::json!({
                    "Action": {
                        "MinimizeWindow": {
                            "id": 42,
                            "transition": {
                                "type": "anchored",
                                "anchor": {
                                    "output": "DP-1",
                                    "x": 10.5,
                                    "y": 20.25,
                                    "width": 48.0,
                                    "height": 48.0
                                }
                            }
                        }
                    }
                }),
            ),
            (
                send_action_to_server(accepted, |client| {
                    client.restore_window_with_transition(
                        WindowId(42),
                        Some(WindowTransition::Disabled),
                    )
                }),
                serde_json::json!({
                    "Action": {
                        "RestoreWindow": {
                            "id": 42,
                            "transition": { "type": "disabled" }
                        }
                    }
                }),
            ),
        ];

        for ((result, request), expected) in cases {
            result.expect("accepted action");
            let actual: serde_json::Value = serde_json::from_str(&request).expect("action JSON");
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn accepts_semantic_native_action_results() {
        for status in ["Applied", "AlreadyInRequestedState", "CloseRequested"] {
            let reply = format!(
                "{{\"Ok\":{{\"WindowActionResult\":{{\"requested_id\":42,\"window_id\":42,\"status\":\"{status}\"}}}}}}\n"
            );
            assert!(matches!(
                validate_action_reply(&reply),
                Ok(ActionReply::Window(result))
                    if matches!(
                        result.status,
                        NiriWindowActionStatus::Applied
                            | NiriWindowActionStatus::AlreadyInRequestedState
                            | NiriWindowActionStatus::CloseRequested
                    )
            ));
        }
    }

    #[test]
    fn retains_semantic_native_action_failures_for_callers() {
        for status in ["WindowNotFound", "Blocked"] {
            let reply = format!(
                "{{\"Ok\":{{\"WindowActionResult\":{{\"requested_id\":42,\"window_id\":null,\"status\":\"{status}\"}}}}}}\n"
            );
            assert!(matches!(
                validate_action_reply(&reply),
                Ok(ActionReply::Window(result))
                    if matches!(
                        result.status,
                        NiriWindowActionStatus::WindowNotFound
                            | NiriWindowActionStatus::Blocked
                    )
            ));
        }
    }

    #[test]
    fn public_window_action_retains_semantic_failure() {
        let reply = b"{\"Ok\":{\"WindowActionResult\":{\"requested_id\":42,\"window_id\":null,\"status\":\"WindowNotFound\"}}}\n";
        let (result, _) =
            send_action_to_server(reply, |client| client.restore_window(WindowId(42)));
        assert_eq!(
            result.expect("semantic result").status,
            NiriWindowActionStatus::WindowNotFound
        );
    }

    #[test]
    fn cancellation_handle_interrupts_event_read_while_server_is_alive() {
        let (client, mut server) = UnixStream::pair().expect("socket pair");
        let (release_server, wait_for_release) = mpsc::channel();
        let server_thread = thread::spawn(move || {
            let mut request = String::new();
            BufReader::new(server.try_clone().expect("clone server"))
                .read_line(&mut request)
                .expect("read request");
            server
                .write_all(b"{\"Ok\":\"Handled\"}\n")
                .expect("write reply");
            wait_for_release.recv().expect("release server");
        });
        let mut stream = NiriEventStream::start(client).expect("valid stream");
        let cancellation = stream.cancellation_handle().expect("cancellation handle");

        cancellation.cancel().expect("cancel stream");
        assert_eq!(stream.read_event().expect("interrupted read"), None);

        release_server.send(()).expect("release server");
        server_thread.join().expect("server thread");
    }
}
