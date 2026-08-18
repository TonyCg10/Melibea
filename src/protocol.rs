//! Versioned, transport-neutral contract exposed by the Melibea daemon.

use serde::{Deserialize, Deserializer, Serialize};

use crate::minimization::MinimizedWindow;

/// First public Melibea protocol version and the default used by the CLI.
pub const PROTOCOL_VERSION: u32 = 1;
/// Protocol version that adds action-scoped window transition hints.
pub const PROTOCOL_VERSION_V2: u32 = 2;
/// Every protocol version accepted by this daemon, in ascending order.
pub const SUPPORTED_PROTOCOL_VERSIONS: [u32; 2] = [PROTOCOL_VERSION, PROTOCOL_VERSION_V2];

/// Maximum UTF-8 byte length accepted for a compositor output name.
pub const MAX_OUTPUT_NAME_BYTES: usize = 256;

/// Client request envelope. One newline-delimited JSON value is sent per
/// connection; `subscribe` keeps the connection open for server messages.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ClientRequest {
    pub version: u32,
    pub request: Request,
}

impl ClientRequest {
    /// Creates a protocol-v1 request. This remains the default for the CLI and
    /// consumers that do not need coordinated window motion.
    #[must_use]
    pub const fn new(request: Request) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            request,
        }
    }

    /// Creates a protocol-v2 request that may carry a window transition.
    #[must_use]
    pub const fn v2(request: Request) -> Self {
        Self {
            version: PROTOCOL_VERSION_V2,
            request,
        }
    }

    /// Validates version-specific request semantics after deserialization.
    ///
    /// # Errors
    ///
    /// Returns a machine-readable protocol error for an unsupported version,
    /// a transition sent through v1, or invalid anchor geometry.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if !SUPPORTED_PROTOCOL_VERSIONS.contains(&self.version) {
            return Err(ProtocolError::incompatible(self.version));
        }

        self.request.validate_for_version(self.version)
    }
}

/// Operations supported by Melibea's local protocol.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    List,
    Subscribe,
    Minimize {
        window_id: Option<u64>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_present_transition"
        )]
        transition: Option<WindowTransition>,
    },
    Restore {
        window_id: u64,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_present_transition"
        )]
        transition: Option<WindowTransition>,
    },
    Close {
        window_id: u64,
    },
}

/// Deserializes a transition only when the field contains a real transition.
///
/// Serde normally maps both an absent optional field and an explicit JSON
/// `null` to `None`. Protocol v1 must reject *any* transition field, while v2
/// names omission (not `null`) as the request for compositor-local motion, so
/// a present `null` is deliberately malformed at both versions.
fn deserialize_present_transition<'de, D>(
    deserializer: D,
) -> Result<Option<WindowTransition>, D::Error>
where
    D: Deserializer<'de>,
{
    WindowTransition::deserialize(deserializer).map(Some)
}

impl Request {
    /// Creates a minimize action with protocol-v1-compatible wire shape.
    #[must_use]
    pub const fn minimize(window_id: Option<u64>) -> Self {
        Self::Minimize {
            window_id,
            transition: None,
        }
    }

    /// Creates a protocol-v2 minimize action with an explicit transition.
    #[must_use]
    pub const fn minimize_with_transition(
        window_id: Option<u64>,
        transition: WindowTransition,
    ) -> Self {
        Self::Minimize {
            window_id,
            transition: Some(transition),
        }
    }

    /// Creates a restore action with protocol-v1-compatible wire shape.
    #[must_use]
    pub const fn restore(window_id: u64) -> Self {
        Self::Restore {
            window_id,
            transition: None,
        }
    }

    /// Creates a protocol-v2 restore action with an explicit transition.
    #[must_use]
    pub const fn restore_with_transition(window_id: u64, transition: WindowTransition) -> Self {
        Self::Restore {
            window_id,
            transition: Some(transition),
        }
    }

    fn validate_for_version(&self, version: u32) -> Result<(), ProtocolError> {
        let transition = match self {
            Self::Minimize { transition, .. } | Self::Restore { transition, .. } => {
                transition.as_ref()
            }
            Self::List | Self::Subscribe | Self::Close { .. } => None,
        };

        if version == PROTOCOL_VERSION && transition.is_some() {
            return Err(ProtocolError::invalid_request(
                "window transitions require Melibea protocol version 2",
            ));
        }

        if let Some(transition) = transition {
            transition.validate().map_err(|error| {
                ProtocolError::invalid_request(format!("invalid window transition: {error}"))
            })?;
        }

        Ok(())
    }
}

/// Output-local logical geometry supplied by a shell for one transition.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BubbleAnchor {
    pub output: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl BubbleAnchor {
    /// Creates and validates one output-local logical rectangle.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty or oversized output name, non-finite
    /// coordinates, non-positive dimensions, or overflowing edges.
    pub fn new(
        output: impl Into<String>,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    ) -> Result<Self, BubbleAnchorError> {
        let anchor = Self {
            output: output.into(),
            x,
            y,
            width,
            height,
        };
        anchor.validate()?;
        Ok(anchor)
    }

    /// Revalidates geometry received from an untrusted protocol peer.
    ///
    /// # Errors
    ///
    /// Returns an error when the rectangle is unsafe to forward to niri.
    pub fn validate(&self) -> Result<(), BubbleAnchorError> {
        if self.output.is_empty() {
            return Err(BubbleAnchorError::EmptyOutput);
        }
        if self.output.len() > MAX_OUTPUT_NAME_BYTES {
            return Err(BubbleAnchorError::OutputTooLong);
        }
        if ![self.x, self.y, self.width, self.height]
            .into_iter()
            .all(f64::is_finite)
        {
            return Err(BubbleAnchorError::NonFiniteGeometry);
        }
        if self.width <= 0.0 || self.height <= 0.0 {
            return Err(BubbleAnchorError::NonPositiveSize);
        }

        let right = self.x + self.width;
        let bottom = self.y + self.height;
        if !right.is_finite() || !bottom.is_finite() || right <= self.x || bottom <= self.y {
            return Err(BubbleAnchorError::InvalidEdges);
        }

        Ok(())
    }
}

/// Why a bubble anchor is unsafe to forward to the compositor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BubbleAnchorError {
    EmptyOutput,
    OutputTooLong,
    NonFiniteGeometry,
    NonPositiveSize,
    InvalidEdges,
}

impl std::fmt::Display for BubbleAnchorError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyOutput => formatter.write_str("output name is empty"),
            Self::OutputTooLong => write!(
                formatter,
                "output name exceeds {MAX_OUTPUT_NAME_BYTES} UTF-8 bytes"
            ),
            Self::NonFiniteGeometry => formatter.write_str("geometry must be finite"),
            Self::NonPositiveSize => formatter.write_str("width and height must be positive"),
            Self::InvalidEdges => {
                formatter.write_str("geometry edges overflow or lose positive extent")
            }
        }
    }
}

impl std::error::Error for BubbleAnchorError {}

/// Optional visual behavior attached to one minimize or restore action.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WindowTransition {
    Anchored { anchor: BubbleAnchor },
    Disabled,
}

impl WindowTransition {
    /// Creates an anchored transition from already validated geometry.
    #[must_use]
    pub const fn anchored(anchor: BubbleAnchor) -> Self {
        Self::Anchored { anchor }
    }

    /// Validates any untrusted geometry carried by the transition.
    ///
    /// # Errors
    ///
    /// Returns an error when an anchored transition contains invalid geometry.
    pub fn validate(&self) -> Result<(), BubbleAnchorError> {
        match self {
            Self::Anchored { anchor } => anchor.validate(),
            Self::Disabled => Ok(()),
        }
    }
}

/// Server message envelope. Every response and subscription event carries the
/// version used to encode it.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ServerMessage {
    pub version: u32,
    pub message: Message,
}

impl ServerMessage {
    #[must_use]
    pub const fn new(message: Message) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            message,
        }
    }

    /// Encodes one normal reply or subscription event using the requesting
    /// client's supported protocol version.
    #[must_use]
    pub const fn for_version(version: u32, message: Message) -> Self {
        Self { version, message }
    }
}

/// One response or subscription event.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    /// Complete authoritative projection. This is always the first state
    /// message sent to a ready subscriber.
    Snapshot { revision: u64, windows: Vec<Window> },
    /// Ordered mutations that transform the preceding revision into this one.
    Changes {
        revision: u64,
        changes: Vec<WindowChange>,
    },
    /// Result accepted from niri for one requested operation.
    ActionResult(ActionResult),
    /// The daemon is connected but has no current authoritative niri snapshot.
    Unavailable { revision: u64, reason: String },
    /// Invalid request, incompatible version, or failed operation.
    Error(ProtocolError),
}

/// Stable shell-facing representation of one minimized family.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct Window {
    pub id: u64,
    pub app_id: Option<String>,
    pub title: Option<String>,
    /// Optional desktop-entry or themed-icon identity. Niri 26.04 does not
    /// currently provide one, so version 1 normally leaves this field empty.
    pub icon_name: Option<String>,
}

impl From<&MinimizedWindow> for Window {
    fn from(window: &MinimizedWindow) -> Self {
        Self {
            id: window.id.0,
            app_id: window.app_id.clone(),
            title: window.title.clone(),
            icon_name: None,
        }
    }
}

/// Sequential change within one atomic revision.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WindowChange {
    Added {
        index: usize,
        window: Window,
    },
    Updated {
        index: usize,
        window: Window,
    },
    Moved {
        window_id: u64,
        from_index: usize,
        to_index: usize,
    },
    Removed {
        index: usize,
        window_id: u64,
    },
}

/// Operation represented in an action response.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Minimize,
    Restore,
    Close,
}

/// Semantic result forwarded from niri without claiming more lifecycle work
/// than the compositor acknowledged.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ActionResult {
    pub operation: Operation,
    pub requested_id: Option<u64>,
    pub window_id: Option<u64>,
    pub status: ActionStatus,
}

/// Stable Melibea spelling of niri's window action statuses.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionStatus {
    Applied,
    AlreadyInRequestedState,
    CloseRequested,
    WindowNotFound,
    Blocked,
    /// Compatibility result from an older experimental niri that only
    /// acknowledged request handling.
    LegacyHandled,
}

/// Machine-readable protocol error.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ProtocolError {
    pub code: ErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub supported_versions: Vec<u32>,
}

impl ProtocolError {
    #[must_use]
    pub fn incompatible(requested: u32) -> Self {
        Self {
            code: ErrorCode::IncompatibleVersion,
            message: format!(
                "unsupported Melibea protocol version {requested}; supported versions are 1 and 2"
            ),
            supported_versions: SUPPORTED_PROTOCOL_VERSIONS.to_vec(),
        }
    }

    #[must_use]
    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::InvalidRequest,
            message: message.into(),
            supported_versions: Vec::new(),
        }
    }

    #[must_use]
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::Unavailable,
            message: message.into(),
            supported_versions: Vec::new(),
        }
    }

    #[must_use]
    pub fn action_failed(message: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::ActionFailed,
            message: message.into(),
            supported_versions: Vec::new(),
        }
    }
}

/// Stable error categories for programmatic consumers.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    IncompatibleVersion,
    InvalidRequest,
    Unavailable,
    ActionFailed,
}

/// Computes the sequential mutations that transform `previous` into `next`.
/// Applying the returned changes in order reproduces `next` exactly.
#[must_use]
pub fn diff_windows(previous: &[Window], next: &[Window]) -> Vec<WindowChange> {
    let mut current = previous.to_vec();
    let mut changes = Vec::new();

    for index in (0..current.len()).rev() {
        if !next.iter().any(|window| window.id == current[index].id) {
            let window_id = current.remove(index).id;
            changes.push(WindowChange::Removed { index, window_id });
        }
    }

    for (target_index, next_window) in next.iter().enumerate() {
        match current
            .iter()
            .position(|window| window.id == next_window.id)
        {
            None => {
                current.insert(target_index, next_window.clone());
                changes.push(WindowChange::Added {
                    index: target_index,
                    window: next_window.clone(),
                });
            }
            Some(current_index) => {
                if current_index != target_index {
                    let window = current.remove(current_index);
                    current.insert(target_index, window);
                    changes.push(WindowChange::Moved {
                        window_id: next_window.id,
                        from_index: current_index,
                        to_index: target_index,
                    });
                }
                if current[target_index] != *next_window {
                    current[target_index] = next_window.clone();
                    changes.push(WindowChange::Updated {
                        index: target_index,
                        window: next_window.clone(),
                    });
                }
            }
        }
    }

    debug_assert_eq!(current, next);
    changes
}

#[cfg(test)]
mod tests {
    use super::{
        BubbleAnchor, BubbleAnchorError, ClientRequest, ErrorCode, PROTOCOL_VERSION,
        PROTOCOL_VERSION_V2, Request, Window, WindowChange, WindowTransition, diff_windows,
    };

    fn window(id: u64, title: &str) -> Window {
        Window {
            id,
            app_id: Some("org.example.App".to_owned()),
            title: Some(title.to_owned()),
            icon_name: None,
        }
    }

    fn anchor() -> BubbleAnchor {
        BubbleAnchor::new("DP-1", 12.5, 24.25, 48.0, 48.0).expect("valid anchor")
    }

    #[test]
    fn protocol_v1_action_json_remains_byte_compatible() {
        let minimize = serde_json::to_string(&ClientRequest::new(Request::minimize(Some(42))))
            .expect("serialize v1 minimize");
        let restore = serde_json::to_string(&ClientRequest::new(Request::restore(42)))
            .expect("serialize v1 restore");

        assert_eq!(
            minimize,
            r#"{"version":1,"request":{"type":"minimize","window_id":42}}"#
        );
        assert_eq!(
            restore,
            r#"{"version":1,"request":{"type":"restore","window_id":42}}"#
        );
    }

    #[test]
    fn protocol_v2_serializes_anchored_and_disabled_transitions_exactly() {
        let anchored = ClientRequest::v2(Request::minimize_with_transition(
            Some(42),
            WindowTransition::anchored(anchor()),
        ));
        let disabled = ClientRequest::v2(Request::restore_with_transition(
            42,
            WindowTransition::Disabled,
        ));

        assert_eq!(
            serde_json::to_string(&anchored).expect("serialize anchored transition"),
            r#"{"version":2,"request":{"type":"minimize","window_id":42,"transition":{"type":"anchored","anchor":{"output":"DP-1","x":12.5,"y":24.25,"width":48.0,"height":48.0}}}}"#
        );
        assert_eq!(
            serde_json::to_string(&disabled).expect("serialize disabled transition"),
            r#"{"version":2,"request":{"type":"restore","window_id":42,"transition":{"type":"disabled"}}}"#
        );
        assert_eq!(anchored.version, PROTOCOL_VERSION_V2);
    }

    #[test]
    fn protocol_v1_rejects_a_transition_without_changing_its_wire_shape() {
        let request = ClientRequest {
            version: PROTOCOL_VERSION,
            request: Request::restore_with_transition(42, WindowTransition::Disabled),
        };

        let error = request.validate().expect_err("v2 feature must fail on v1");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(error.supported_versions.is_empty());
    }

    #[test]
    fn explicit_null_transition_is_not_confused_with_an_absent_field() {
        let v1 = r#"{"version":1,"request":{"type":"minimize","window_id":42,"transition":null}}"#;
        let v2 = r#"{"version":2,"request":{"type":"restore","window_id":42,"transition":null}}"#;

        assert!(serde_json::from_str::<ClientRequest>(v1).is_err());
        assert!(serde_json::from_str::<ClientRequest>(v2).is_err());
    }

    #[test]
    fn anchor_validation_bounds_identity_geometry_and_edge_arithmetic() {
        assert_eq!(
            BubbleAnchor::new("", 0.0, 0.0, 1.0, 1.0),
            Err(BubbleAnchorError::EmptyOutput)
        );
        assert_eq!(
            BubbleAnchor::new("x".repeat(257), 0.0, 0.0, 1.0, 1.0),
            Err(BubbleAnchorError::OutputTooLong)
        );
        assert_eq!(
            BubbleAnchor::new("DP-1", f64::NAN, 0.0, 1.0, 1.0),
            Err(BubbleAnchorError::NonFiniteGeometry)
        );
        assert_eq!(
            BubbleAnchor::new("DP-1", 0.0, 0.0, 0.0, 1.0),
            Err(BubbleAnchorError::NonPositiveSize)
        );
        assert_eq!(
            BubbleAnchor::new("DP-1", f64::MAX, 0.0, f64::MAX, 1.0),
            Err(BubbleAnchorError::InvalidEdges)
        );
    }

    #[test]
    fn diff_is_sequential_for_remove_move_add_and_update() {
        let previous = vec![window(1, "one"), window(2, "old"), window(3, "three")];
        let next = vec![window(3, "three"), window(2, "new"), window(4, "four")];

        assert_eq!(
            diff_windows(&previous, &next),
            vec![
                WindowChange::Removed {
                    index: 0,
                    window_id: 1,
                },
                WindowChange::Moved {
                    window_id: 3,
                    from_index: 1,
                    to_index: 0,
                },
                WindowChange::Updated {
                    index: 1,
                    window: window(2, "new"),
                },
                WindowChange::Added {
                    index: 2,
                    window: window(4, "four"),
                },
            ]
        );
    }
}
