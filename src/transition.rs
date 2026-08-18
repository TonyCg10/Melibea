//! Pure focus-transition decisions with no compositor side effects.

use std::collections::BTreeMap;

use crate::{
    attention::{Proportion, WidthPolicy},
    config::{Config, WindowIdentity},
};

/// Stable window identity supplied by the niri adapter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WindowId(pub u64);

/// Window metadata required by attention-rule resolution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowRecord {
    pub id: WindowId,
    pub app_id: Option<String>,
    pub title: Option<String>,
    pub eligibility: WindowEligibility,
}

impl WindowRecord {
    fn identity(&self) -> WindowIdentity<'_> {
        WindowIdentity {
            app_id: self.app_id.as_deref(),
            title: self.title.as_deref(),
        }
    }
}

/// Whether Melibea may apply attention policy to this window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowEligibility {
    Managed,
    Excluded(ExclusionReason),
}

/// A deterministic reason for excluding a window from mutation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExclusionReason {
    Floating,
}

/// Whether a decision describes a window gaining or losing focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusState {
    Focused,
    Unfocused,
}

impl FocusState {
    const fn is_focused(self) -> bool {
        matches!(self, Self::Focused)
    }
}

/// Explainable result for one side of a focus transition.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DecisionOutcome {
    /// Set a concrete proportional width from the matching rule.
    Resize {
        rule_index: usize,
        width: Proportion,
    },
    /// The matching rule explicitly leaves this width unchanged.
    Preserve { rule_index: usize },
    /// The window is known but no configured rule matches it.
    Unmanaged,
    /// The window is known but deliberately excluded from mutation.
    Excluded { reason: ExclusionReason },
    /// Focus referenced a window not present in the current registry.
    UnknownWindow,
}

/// One explainable decision produced by a focus transition.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FocusDecision {
    pub window_id: WindowId,
    pub state: FocusState,
    pub outcome: DecisionOutcome,
}

/// All decisions caused by one observed focus state.
#[derive(Clone, Debug, PartialEq)]
pub struct FocusTransition {
    /// Monotonically increasing observation number used to reject stale work.
    pub generation: u64,
    pub previous: Option<WindowId>,
    pub current: Option<WindowId>,
    /// Previous-focus decision first, current-focus decision second.
    pub decisions: Vec<FocusDecision>,
}

/// Deterministic attention state independent from niri transport and actions.
#[derive(Debug)]
pub struct AttentionEngine {
    config: Config,
    windows: BTreeMap<WindowId, WindowRecord>,
    focused: Option<WindowId>,
    generation: u64,
}

impl AttentionEngine {
    /// Creates an empty engine using validated configuration.
    #[must_use]
    pub fn new(config: Config) -> Self {
        Self {
            config,
            windows: BTreeMap::new(),
            focused: None,
            generation: 0,
        }
    }

    /// Inserts a new window or replaces its current metadata.
    pub fn upsert_window(&mut self, window: WindowRecord) {
        self.windows.insert(window.id, window);
    }

    /// Removes a closed window and clears focus if it owned current focus.
    pub fn remove_window(&mut self, window_id: WindowId) -> bool {
        if self.focused == Some(window_id) {
            self.focused = None;
        }
        self.windows.remove(&window_id).is_some()
    }

    /// Returns the currently observed focused window.
    #[must_use]
    pub const fn focused_window(&self) -> Option<WindowId> {
        self.focused
    }

    /// Observes the latest focus and returns pure, explainable decisions.
    ///
    /// Every observation advances the generation, including duplicate focus
    /// events. This lets a future executor discard an older transition even
    /// when the latest observation requires no mutation.
    pub fn observe_focus(&mut self, current: Option<WindowId>) -> FocusTransition {
        let generation = self.next_generation();
        let previous = self.focused;
        self.focused = current;

        let mut decisions = Vec::with_capacity(2);
        if previous != current {
            if let Some(window_id) = previous {
                decisions.push(self.decision_for(window_id, FocusState::Unfocused));
            }
            if let Some(window_id) = current {
                decisions.push(self.decision_for(window_id, FocusState::Focused));
            }
        }

        FocusTransition {
            generation,
            previous,
            current,
            decisions,
        }
    }

    /// Re-resolves the current focus after its metadata or configuration
    /// becomes available, without inventing a focus change.
    pub fn reevaluate_focus(&mut self) -> FocusTransition {
        let generation = self.next_generation();
        let current = self.focused;
        let decisions = current
            .map(|window_id| self.decision_for(window_id, FocusState::Focused))
            .into_iter()
            .collect();

        FocusTransition {
            generation,
            previous: current,
            current,
            decisions,
        }
    }

    /// Rebuilds desired attention state for every registered window.
    ///
    /// This is used after a full compositor snapshot or reconnect. Unlike a
    /// focus delta, it emits an explicit decision for every known window so an
    /// already-open inactive window can immediately recover its compact width.
    pub fn synchronize_focus(&mut self, current: Option<WindowId>) -> FocusTransition {
        let generation = self.next_generation();
        let previous = self.focused;
        self.focused = current;
        let decisions = self
            .windows
            .keys()
            .copied()
            .map(|window_id| {
                let state = if Some(window_id) == current {
                    FocusState::Focused
                } else {
                    FocusState::Unfocused
                };
                self.decision_for(window_id, state)
            })
            .collect();

        FocusTransition {
            generation,
            previous,
            current,
            decisions,
        }
    }

    fn next_generation(&mut self) -> u64 {
        self.generation = self.generation.saturating_add(1);
        self.generation
    }

    fn decision_for(&self, window_id: WindowId, state: FocusState) -> FocusDecision {
        let outcome =
            self.windows
                .get(&window_id)
                .map_or(DecisionOutcome::UnknownWindow, |window| {
                    if let WindowEligibility::Excluded(reason) = window.eligibility {
                        return DecisionOutcome::Excluded { reason };
                    }

                    self.config.resolve(window.identity()).map_or(
                        DecisionOutcome::Unmanaged,
                        |resolved| match resolved.rule.policy().width_for(state.is_focused()) {
                            WidthPolicy::Proportion(width) => DecisionOutcome::Resize {
                                rule_index: resolved.index,
                                width,
                            },
                            WidthPolicy::Preserve => DecisionOutcome::Preserve {
                                rule_index: resolved.index,
                            },
                        },
                    )
                });

        FocusDecision {
            window_id,
            state,
            outcome,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AttentionEngine, DecisionOutcome, ExclusionReason, FocusDecision, FocusState,
        WindowEligibility, WindowId, WindowRecord,
    };
    use crate::{attention::WidthPolicy, config::Config};

    const TERMINAL: WindowId = WindowId(10);
    const EDITOR: WindowId = WindowId(20);
    const BROWSER: WindowId = WindowId(30);

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

    fn window(id: WindowId, app_id: &str) -> WindowRecord {
        WindowRecord {
            id,
            app_id: Some(app_id.to_owned()),
            title: None,
            eligibility: WindowEligibility::Managed,
        }
    }

    fn resize(rule_index: usize, value: f64) -> DecisionOutcome {
        let WidthPolicy::Proportion(width) =
            WidthPolicy::proportion(value).expect("valid test width")
        else {
            unreachable!("constructor returned a concrete proportion")
        };
        DecisionOutcome::Resize { rule_index, width }
    }

    fn engine() -> AttentionEngine {
        let mut engine = AttentionEngine::new(config());
        engine.upsert_window(window(TERMINAL, "kitty"));
        engine.upsert_window(window(EDITOR, "code"));
        engine.upsert_window(window(BROWSER, "firefox"));
        engine
    }

    #[test]
    fn models_editor_terminal_editor_sequence() {
        let mut engine = engine();

        let editor_focused = engine.observe_focus(Some(EDITOR));
        assert_eq!(
            editor_focused.decisions,
            vec![FocusDecision {
                window_id: EDITOR,
                state: FocusState::Focused,
                outcome: resize(1, 0.9),
            }]
        );

        let terminal_focused = engine.observe_focus(Some(TERMINAL));
        assert_eq!(
            terminal_focused.decisions,
            vec![
                FocusDecision {
                    window_id: EDITOR,
                    state: FocusState::Unfocused,
                    outcome: DecisionOutcome::Preserve { rule_index: 1 },
                },
                FocusDecision {
                    window_id: TERMINAL,
                    state: FocusState::Focused,
                    outcome: resize(0, 0.5),
                },
            ]
        );

        let editor_refocused = engine.observe_focus(Some(EDITOR));
        assert_eq!(
            editor_refocused.decisions,
            vec![
                FocusDecision {
                    window_id: TERMINAL,
                    state: FocusState::Unfocused,
                    outcome: resize(0, 0.1),
                },
                FocusDecision {
                    window_id: EDITOR,
                    state: FocusState::Focused,
                    outcome: resize(1, 0.9),
                },
            ]
        );
    }

    #[test]
    fn duplicate_focus_advances_generation_without_reapplying_width() {
        let mut engine = engine();

        let first = engine.observe_focus(Some(TERMINAL));
        let duplicate = engine.observe_focus(Some(TERMINAL));

        assert_eq!(first.generation, 1);
        assert_eq!(duplicate.generation, 2);
        assert_eq!(duplicate.previous, Some(TERMINAL));
        assert!(duplicate.decisions.is_empty());
    }

    #[test]
    fn explains_unmanaged_and_unknown_windows() {
        let mut engine = engine();

        let unmanaged = engine.observe_focus(Some(BROWSER));
        assert_eq!(unmanaged.decisions[0].outcome, DecisionOutcome::Unmanaged);

        let unknown = engine.observe_focus(Some(WindowId(999)));
        assert_eq!(unknown.decisions.len(), 2);
        assert_eq!(unknown.decisions[0].outcome, DecisionOutcome::Unmanaged);
        assert_eq!(unknown.decisions[1].outcome, DecisionOutcome::UnknownWindow);
    }

    #[test]
    fn closing_focused_window_prevents_a_resize_for_the_dead_window() {
        let mut engine = engine();
        engine.observe_focus(Some(TERMINAL));

        assert!(engine.remove_window(TERMINAL));
        assert_eq!(engine.focused_window(), None);

        let next = engine.observe_focus(Some(EDITOR));
        assert_eq!(next.previous, None);
        assert_eq!(next.decisions.len(), 1);
        assert_eq!(next.decisions[0].window_id, EDITOR);
    }

    #[test]
    fn updated_metadata_changes_later_rule_resolution() {
        let mut engine = engine();
        engine.observe_focus(Some(BROWSER));
        engine.observe_focus(None);

        engine.upsert_window(window(BROWSER, "kitty"));
        let transition = engine.observe_focus(Some(BROWSER));

        assert_eq!(transition.decisions[0].outcome, resize(0, 0.5));
    }

    #[test]
    fn reevaluates_focus_when_metadata_arrives_after_focus() {
        let mut engine = AttentionEngine::new(config());

        let unknown = engine.observe_focus(Some(TERMINAL));
        assert_eq!(unknown.decisions[0].outcome, DecisionOutcome::UnknownWindow);

        engine.upsert_window(window(TERMINAL, "kitty"));
        let reevaluated = engine.reevaluate_focus();

        assert_eq!(reevaluated.previous, Some(TERMINAL));
        assert_eq!(reevaluated.current, Some(TERMINAL));
        assert_eq!(reevaluated.decisions[0].outcome, resize(0, 0.5));
    }

    #[test]
    fn snapshot_synchronization_decides_for_focused_and_inactive_windows() {
        let mut engine = AttentionEngine::new(config());
        engine.upsert_window(window(TERMINAL, "kitty"));
        engine.upsert_window(window(EDITOR, "code"));

        let transition = engine.synchronize_focus(Some(EDITOR));

        assert_eq!(transition.decisions.len(), 2);
        assert_eq!(
            transition.decisions,
            vec![
                FocusDecision {
                    window_id: TERMINAL,
                    state: FocusState::Unfocused,
                    outcome: resize(0, 0.1),
                },
                FocusDecision {
                    window_id: EDITOR,
                    state: FocusState::Focused,
                    outcome: resize(1, 0.9),
                },
            ]
        );
    }

    #[test]
    fn explains_excluded_windows_without_resolving_rules() {
        let mut engine = engine();
        engine.upsert_window(WindowRecord {
            id: TERMINAL,
            app_id: Some("kitty".to_owned()),
            title: None,
            eligibility: WindowEligibility::Excluded(ExclusionReason::Floating),
        });

        let transition = engine.observe_focus(Some(TERMINAL));

        assert_eq!(
            transition.decisions[0].outcome,
            DecisionOutcome::Excluded {
                reason: ExclusionReason::Floating
            }
        );
    }
}
