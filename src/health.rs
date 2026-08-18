//! Read-only health summary derived from an authoritative niri snapshot.

use std::{error::Error, fmt};

use crate::{
    config::{Config, WindowIdentity},
    niri::NiriWindow,
    transition::WindowId,
};

/// Rule and eligibility information for the currently focused window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FocusedWindowHealth {
    pub id: WindowId,
    pub app_id: Option<String>,
    pub title: Option<String>,
    pub matched_rule: Option<usize>,
    pub is_floating: bool,
}

/// One-shot health report for configuration and current niri windows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HealthReport {
    pub total_windows: usize,
    pub matched_tiled_windows: usize,
    pub unmatched_tiled_windows: usize,
    pub floating_windows: usize,
    pub matched_floating_windows: usize,
    pub matches_per_rule: Vec<usize>,
    pub focused: Option<FocusedWindowHealth>,
}

impl HealthReport {
    /// Builds a deterministic report from a full niri window snapshot.
    ///
    /// # Errors
    ///
    /// Rejects a semantically invalid snapshot with multiple focused windows.
    pub fn from_windows(config: &Config, windows: &[NiriWindow]) -> Result<Self, HealthError> {
        let focused = windows
            .iter()
            .filter(|window| window.is_focused)
            .collect::<Vec<_>>();
        if focused.len() > 1 {
            return Err(HealthError::MultipleFocusedWindows(
                focused.iter().map(|window| WindowId(window.id)).collect(),
            ));
        }

        let mut matches_per_rule = vec![0; config.attention_rules().len()];
        let mut matched_tiled_windows = 0;
        let mut unmatched_tiled_windows = 0;
        let mut floating_windows = 0;
        let mut matched_floating_windows = 0;

        for window in windows {
            let matched_rule = resolve_rule(config, window);
            if window.is_floating {
                floating_windows += 1;
                if matched_rule.is_some() {
                    matched_floating_windows += 1;
                }
            } else if let Some(rule_index) = matched_rule {
                matched_tiled_windows += 1;
                matches_per_rule[rule_index] += 1;
            } else {
                unmatched_tiled_windows += 1;
            }
        }

        let focused = focused.first().map(|window| FocusedWindowHealth {
            id: WindowId(window.id),
            app_id: window.app_id.clone(),
            title: window.title.clone(),
            matched_rule: resolve_rule(config, window),
            is_floating: window.is_floating,
        });

        Ok(Self {
            total_windows: windows.len(),
            matched_tiled_windows,
            unmatched_tiled_windows,
            floating_windows,
            matched_floating_windows,
            matches_per_rule,
            focused,
        })
    }
}

fn resolve_rule(config: &Config, window: &NiriWindow) -> Option<usize> {
    config
        .resolve(WindowIdentity {
            app_id: window.app_id.as_deref(),
            title: window.title.as_deref(),
        })
        .map(|resolved| resolved.index)
}

/// Invalid compositor state discovered by a health check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HealthError {
    MultipleFocusedWindows(Vec<WindowId>),
}

impl fmt::Display for HealthError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MultipleFocusedWindows(ids) => {
                write!(formatter, "niri reports multiple focused windows: {ids:?}")
            }
        }
    }
}

impl Error for HealthError {}

#[cfg(test)]
mod tests {
    use super::{HealthError, HealthReport};
    use crate::{config::Config, niri::NiriWindow, transition::WindowId};

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
        .expect("valid config")
    }

    fn window(id: u64, app_id: &str, focused: bool, floating: bool) -> NiriWindow {
        NiriWindow {
            id,
            title: Some("title".to_owned()),
            app_id: Some(app_id.to_owned()),
            is_focused: focused,
            is_floating: floating,
        }
    }

    #[test]
    fn summarizes_matches_exclusions_and_focus() {
        let windows = vec![
            window(10, "kitty", false, false),
            window(20, "code", true, false),
            window(30, "kitty", false, true),
            window(40, "firefox", false, false),
        ];

        let report = HealthReport::from_windows(&config(), &windows).expect("valid snapshot");

        assert_eq!(report.total_windows, 4);
        assert_eq!(report.matched_tiled_windows, 2);
        assert_eq!(report.unmatched_tiled_windows, 1);
        assert_eq!(report.floating_windows, 1);
        assert_eq!(report.matched_floating_windows, 1);
        assert_eq!(report.matches_per_rule, vec![1, 1]);
        assert_eq!(report.focused.expect("focused").id, WindowId(20));
    }

    #[test]
    fn rejects_multiple_focused_windows() {
        let windows = vec![
            window(10, "kitty", true, false),
            window(20, "code", true, false),
        ];

        assert_eq!(
            HealthReport::from_windows(&config(), &windows),
            Err(HealthError::MultipleFocusedWindows(vec![
                WindowId(10),
                WindowId(20)
            ]))
        );
    }
}
