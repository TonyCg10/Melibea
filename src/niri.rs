//! Transport-neutral adaptation of niri 26.04 event-stream JSON.

use std::{collections::BTreeMap, error::Error, fmt};

use serde::Deserialize;
use serde_json::Value;

use crate::{
    config::Config,
    minimization::MinimizedWindow,
    transition::{
        AttentionEngine, ExclusionReason, FocusTransition, WindowEligibility, WindowId,
        WindowRecord,
    },
};

/// Window fields used by Melibea from niri's public IPC representation.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct NiriWindow {
    pub id: u64,
    pub title: Option<String>,
    pub app_id: Option<String>,
    pub is_focused: bool,
    pub is_floating: bool,
}

impl NiriWindow {
    fn window_id(&self) -> WindowId {
        WindowId(self.id)
    }

    fn into_record(self) -> WindowRecord {
        let eligibility = if self.is_floating {
            WindowEligibility::Excluded(ExclusionReason::Floating)
        } else {
            WindowEligibility::Managed
        };

        WindowRecord {
            id: WindowId(self.id),
            app_id: self.app_id,
            title: self.title,
            eligibility,
        }
    }
}

/// Subset of niri events relevant to attention state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NiriEvent {
    WindowsChanged {
        windows: Vec<NiriWindow>,
    },
    MinimizedWindowsChanged {
        windows: Vec<MinimizedWindow>,
    },
    WindowOpenedOrChanged {
        window: NiriWindow,
    },
    WindowClosed {
        id: WindowId,
    },
    WindowFocusChanged {
        id: Option<WindowId>,
    },
    /// A valid event that Melibea deliberately does not consume.
    Ignored {
        name: String,
    },
}

/// Parses one newline-delimited event from `niri msg --json event-stream`.
///
/// Unknown event variants are retained as [`NiriEvent::Ignored`] for forward
/// compatibility. Fields unknown to Melibea are ignored by Serde.
///
/// # Errors
///
/// Returns an error when the line is not JSON, is not a one-variant event
/// envelope, or a recognized event has an incompatible payload.
pub fn parse_event_line(line: &str) -> Result<NiriEvent, NiriEventError> {
    let value: Value = serde_json::from_str(line).map_err(NiriEventError::Json)?;
    let Value::Object(object) = value else {
        return Err(NiriEventError::InvalidEnvelope);
    };

    if object.len() != 1 {
        return Err(NiriEventError::InvalidEnvelope);
    }

    let (name, payload) = object
        .into_iter()
        .next()
        .ok_or(NiriEventError::InvalidEnvelope)?;

    match name.as_str() {
        "WindowsChanged" => {
            let payload: WindowsChangedPayload = decode_payload(&name, payload)?;
            Ok(NiriEvent::WindowsChanged {
                windows: payload.windows,
            })
        }
        "MinimizedWindowsChanged" => {
            let payload: MinimizedWindowsChangedPayload = decode_payload(&name, payload)?;
            Ok(NiriEvent::MinimizedWindowsChanged {
                windows: payload
                    .windows
                    .into_iter()
                    .map(NiriMinimizedWindow::into_window)
                    .collect(),
            })
        }
        "WindowOpenedOrChanged" => {
            let payload: WindowChangedPayload = decode_payload(&name, payload)?;
            Ok(NiriEvent::WindowOpenedOrChanged {
                window: payload.window,
            })
        }
        "WindowClosed" => {
            let payload: WindowIdPayload = decode_payload(&name, payload)?;
            Ok(NiriEvent::WindowClosed {
                id: WindowId(payload.id),
            })
        }
        "WindowFocusChanged" => {
            let payload: OptionalWindowIdPayload = decode_payload(&name, payload)?;
            Ok(NiriEvent::WindowFocusChanged {
                id: payload.id.map(WindowId),
            })
        }
        _ => Ok(NiriEvent::Ignored { name }),
    }
}

fn decode_payload<T>(name: &str, payload: Value) -> Result<T, NiriEventError>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_value(payload).map_err(|source| NiriEventError::Payload {
        event: name.to_owned(),
        source,
    })
}

#[derive(Deserialize)]
struct WindowsChangedPayload {
    windows: Vec<NiriWindow>,
}

#[derive(Deserialize)]
struct MinimizedWindowsChangedPayload {
    windows: Vec<NiriMinimizedWindow>,
}

#[derive(Deserialize)]
struct NiriMinimizedWindow {
    id: u64,
    title: Option<String>,
    app_id: Option<String>,
}

impl NiriMinimizedWindow {
    fn into_window(self) -> MinimizedWindow {
        MinimizedWindow {
            id: WindowId(self.id),
            app_id: self.app_id,
            title: self.title,
        }
    }
}

#[derive(Deserialize)]
struct WindowChangedPayload {
    window: NiriWindow,
}

#[derive(Deserialize)]
struct WindowIdPayload {
    id: u64,
}

#[derive(Deserialize)]
struct OptionalWindowIdPayload {
    id: Option<u64>,
}

/// Applies transport-neutral niri events to Melibea's pure attention engine.
#[derive(Debug)]
pub struct NiriAdapter {
    engine: AttentionEngine,
    known_windows: BTreeMap<WindowId, WindowSignature>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct WindowSignature {
    title: Option<String>,
    app_id: Option<String>,
    is_floating: bool,
}

impl From<&NiriWindow> for WindowSignature {
    fn from(window: &NiriWindow) -> Self {
        Self {
            title: window.title.clone(),
            app_id: window.app_id.clone(),
            is_floating: window.is_floating,
        }
    }
}

impl NiriAdapter {
    /// Creates an empty adapter using validated Melibea configuration.
    #[must_use]
    pub fn new(config: Config) -> Self {
        Self {
            engine: AttentionEngine::new(config),
            known_windows: BTreeMap::new(),
        }
    }

    /// Applies one event and returns a focus transition when attention must be
    /// reconsidered.
    ///
    /// # Errors
    ///
    /// Rejects an invalid full snapshot containing more than one focused
    /// window without partially applying it.
    pub fn apply(&mut self, event: NiriEvent) -> Result<Option<FocusTransition>, NiriAdapterError> {
        match event {
            NiriEvent::WindowsChanged { windows } => self.replace_windows(windows).map(Some),
            NiriEvent::WindowOpenedOrChanged { window } => {
                let focused = window.is_focused;
                let window_id = window.window_id();
                let signature = WindowSignature::from(&window);
                let relevant_change = self
                    .known_windows
                    .insert(window_id, signature.clone())
                    .as_ref()
                    != Some(&signature);
                self.engine.upsert_window(window.into_record());

                Ok((focused && relevant_change).then(|| self.apply_focus(Some(window_id))))
            }
            NiriEvent::WindowClosed { id } => {
                self.known_windows.remove(&id);
                self.engine.remove_window(id);
                Ok(None)
            }
            NiriEvent::WindowFocusChanged { id } => Ok(Some(self.apply_focus(id))),
            NiriEvent::MinimizedWindowsChanged { .. } | NiriEvent::Ignored { .. } => Ok(None),
        }
    }

    fn replace_windows(
        &mut self,
        windows: Vec<NiriWindow>,
    ) -> Result<FocusTransition, NiriAdapterError> {
        let focused = windows
            .iter()
            .filter(|window| window.is_focused)
            .map(NiriWindow::window_id)
            .collect::<Vec<_>>();

        if focused.len() > 1 {
            return Err(NiriAdapterError::MultipleFocusedWindows(focused));
        }

        let next_windows = windows
            .iter()
            .map(|window| (window.window_id(), WindowSignature::from(window)))
            .collect::<BTreeMap<_, _>>();
        for removed in self
            .known_windows
            .keys()
            .filter(|id| !next_windows.contains_key(id))
            .copied()
        {
            self.engine.remove_window(removed);
        }

        for window in windows {
            self.engine.upsert_window(window.into_record());
        }

        self.known_windows = next_windows;
        Ok(self.engine.synchronize_focus(focused.first().copied()))
    }

    fn apply_focus(&mut self, focused: Option<WindowId>) -> FocusTransition {
        if self.engine.focused_window() == focused {
            self.engine.reevaluate_focus()
        } else {
            self.engine.observe_focus(focused)
        }
    }
}

/// An invalid event-stream line.
#[derive(Debug)]
pub enum NiriEventError {
    Json(serde_json::Error),
    InvalidEnvelope,
    Payload {
        event: String,
        source: serde_json::Error,
    },
}

impl fmt::Display for NiriEventError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(error) => write!(formatter, "invalid niri event JSON: {error}"),
            Self::InvalidEnvelope => {
                formatter.write_str("niri event must contain exactly one event variant")
            }
            Self::Payload { event, source } => {
                write!(formatter, "invalid `{event}` payload: {source}")
            }
        }
    }
}

impl Error for NiriEventError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            Self::InvalidEnvelope => None,
            Self::Payload { source, .. } => Some(source),
        }
    }
}

/// A semantically invalid niri event sequence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NiriAdapterError {
    MultipleFocusedWindows(Vec<WindowId>),
}

impl fmt::Display for NiriAdapterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MultipleFocusedWindows(ids) => {
                write!(
                    formatter,
                    "niri snapshot contains multiple focused windows: {ids:?}"
                )
            }
        }
    }
}

impl Error for NiriAdapterError {}

#[cfg(test)]
mod tests {
    use super::{NiriAdapter, NiriAdapterError, NiriEvent, parse_event_line};
    use crate::{
        attention::WidthPolicy,
        config::Config,
        transition::{DecisionOutcome, ExclusionReason, WindowId},
    };

    fn config() -> Config {
        Config::parse(
            r#"
                [[attention]]
                app_id = "^kitty$"
                focused_width = "50%"
                unfocused_width = "10%"

                [[attention]]
                app_id = "^code$"
                focused_width = "90%"
                unfocused_width = "preserve"
            "#,
        )
        .expect("valid test configuration")
    }

    fn window_json(id: u64, app_id: &str, focused: bool, floating: bool) -> String {
        format!(
            r#"{{"id":{id},"title":"window","app_id":"{app_id}","pid":10,"workspace_id":1,"is_focused":{focused},"is_floating":{floating},"is_urgent":false,"layout":{{"pos_in_scrolling_layout":[1,1],"tile_size":[100.0,100.0],"window_size":[100,100],"tile_pos_in_workspace_view":null,"window_offset_in_tile":[0.0,0.0]}},"focus_timestamp":null}}"#
        )
    }

    fn assert_proportion(outcome: DecisionOutcome, expected: f64) {
        let DecisionOutcome::Resize { width, .. } = outcome else {
            panic!("expected resize decision, got {outcome:?}")
        };
        let actual = width.get();
        assert!(
            (actual - expected).abs() < f64::EPSILON,
            "expected proportion {expected}, got {actual}"
        );
    }

    #[test]
    fn parses_realistic_window_snapshot_and_ignores_extra_fields() {
        let line = format!(
            r#"{{"WindowsChanged":{{"windows":[{}]}}}}"#,
            window_json(7, "kitty", true, false)
        );

        let event = parse_event_line(&line).expect("valid event");
        let NiriEvent::WindowsChanged { windows } = event else {
            panic!("expected window snapshot")
        };

        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].id, 7);
        assert_eq!(windows[0].app_id.as_deref(), Some("kitty"));
    }

    #[test]
    fn parses_authoritative_minimized_window_snapshot() {
        let event = parse_event_line(
            r#"{"MinimizedWindowsChanged":{"windows":[{"id":42,"title":"scratch","app_id":"kitty","pid":10,"is_urgent":false,"focus_timestamp":null}]}}"#,
        )
        .expect("valid minimized snapshot");

        let NiriEvent::MinimizedWindowsChanged { windows } = event else {
            panic!("expected minimized window snapshot")
        };
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].id, WindowId(42));
        assert_eq!(windows[0].app_id.as_deref(), Some("kitty"));
        assert_eq!(windows[0].title.as_deref(), Some("scratch"));
    }

    #[test]
    fn retains_unknown_event_name_without_rejecting_stream() {
        let event = parse_event_line(r#"{"FutureEvent":{"value":1}}"#).expect("valid event");
        assert_eq!(
            event,
            NiriEvent::Ignored {
                name: "FutureEvent".to_owned()
            }
        );
    }

    #[test]
    fn rejects_invalid_event_envelope() {
        assert!(parse_event_line(r#"{"WindowClosed":{"id":1},"extra":{}}"#).is_err());
    }

    #[test]
    fn snapshot_then_focus_change_drives_attention_engine() {
        let mut adapter = NiriAdapter::new(config());
        let snapshot = format!(
            r#"{{"WindowsChanged":{{"windows":[{},{}]}}}}"#,
            window_json(10, "kitty", true, false),
            window_json(20, "code", false, false)
        );

        let initial = adapter
            .apply(parse_event_line(&snapshot).expect("valid snapshot"))
            .expect("valid sequence")
            .expect("focus transition");
        assert_proportion(initial.decisions[0].outcome, 0.5);

        let focused = adapter
            .apply(
                parse_event_line(r#"{"WindowFocusChanged":{"id":20}}"#).expect("valid focus event"),
            )
            .expect("valid sequence")
            .expect("focus transition");

        assert_proportion(focused.decisions[0].outcome, 0.1);
        assert_eq!(
            focused.decisions[1].outcome,
            DecisionOutcome::Resize {
                rule_index: 1,
                width: match WidthPolicy::proportion(0.9).expect("valid width") {
                    WidthPolicy::Proportion(width) => width,
                    WidthPolicy::Preserve => unreachable!(),
                }
            }
        );
    }

    #[test]
    fn focused_floating_window_is_explained_as_excluded() {
        let mut adapter = NiriAdapter::new(config());
        let snapshot = format!(
            r#"{{"WindowsChanged":{{"windows":[{}]}}}}"#,
            window_json(10, "kitty", true, true)
        );

        let transition = adapter
            .apply(parse_event_line(&snapshot).expect("valid snapshot"))
            .expect("valid sequence")
            .expect("focus transition");

        assert_eq!(
            transition.decisions[0].outcome,
            DecisionOutcome::Excluded {
                reason: ExclusionReason::Floating
            }
        );
    }

    #[test]
    fn metadata_after_focus_reevaluates_unknown_window() {
        let mut adapter = NiriAdapter::new(config());
        let focus = adapter
            .apply(NiriEvent::WindowFocusChanged {
                id: Some(WindowId(10)),
            })
            .expect("valid sequence")
            .expect("focus transition");
        assert_eq!(focus.decisions[0].outcome, DecisionOutcome::UnknownWindow);

        let opened = format!(
            r#"{{"WindowOpenedOrChanged":{{"window":{}}}}}"#,
            window_json(10, "kitty", true, false)
        );
        let reevaluated = adapter
            .apply(parse_event_line(&opened).expect("valid event"))
            .expect("valid sequence")
            .expect("reevaluation");

        assert_proportion(reevaluated.decisions[0].outcome, 0.5);
    }

    #[test]
    fn repeated_focused_window_metadata_does_not_create_feedback_work() {
        let mut adapter = NiriAdapter::new(config());
        let opened = format!(
            r#"{{"WindowOpenedOrChanged":{{"window":{}}}}}"#,
            window_json(10, "kitty", true, false)
        );

        assert!(
            adapter
                .apply(parse_event_line(&opened).expect("first event"))
                .expect("valid sequence")
                .is_some()
        );
        assert!(
            adapter
                .apply(parse_event_line(&opened).expect("repeated event"))
                .expect("valid sequence")
                .is_none()
        );
    }

    #[test]
    fn rejects_snapshot_with_multiple_focused_windows_atomically() {
        let mut adapter = NiriAdapter::new(config());
        let snapshot = format!(
            r#"{{"WindowsChanged":{{"windows":[{},{}]}}}}"#,
            window_json(10, "kitty", true, false),
            window_json(20, "code", true, false)
        );

        let error = adapter
            .apply(parse_event_line(&snapshot).expect("valid snapshot syntax"))
            .expect_err("snapshot must be rejected");

        assert_eq!(
            error,
            NiriAdapterError::MultipleFocusedWindows(vec![WindowId(10), WindowId(20)])
        );
    }
}
