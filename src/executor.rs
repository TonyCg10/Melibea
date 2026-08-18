//! Coalescing of pure focus decisions into targeted niri resize actions.

use std::collections::BTreeMap;

use crate::{
    attention::Proportion,
    transition::{DecisionOutcome, FocusState, FocusTransition, WindowId},
};

/// A concrete, still-pending mutation for one niri window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResizeAction {
    pub generation: u64,
    pub window_id: WindowId,
    pub state: FocusState,
    pub width: Proportion,
}

/// Latest unexecuted resize per window.
///
/// Every decision, including a no-op decision, supersedes older work for the
/// same window. This prevents a queued contraction from running after newer
/// metadata says that the window must be preserved or excluded.
#[derive(Debug, Default)]
pub struct PendingResizes {
    latest_generation: BTreeMap<WindowId, u64>,
    pending: BTreeMap<WindowId, ResizeAction>,
}

impl PendingResizes {
    /// Incorporates every decision in one transition.
    pub fn ingest(&mut self, transition: &FocusTransition) {
        for decision in &transition.decisions {
            let latest = self
                .latest_generation
                .entry(decision.window_id)
                .or_default();
            if transition.generation < *latest {
                continue;
            }
            *latest = transition.generation;

            match decision.outcome {
                DecisionOutcome::Resize { width, .. } => {
                    self.pending.insert(
                        decision.window_id,
                        ResizeAction {
                            generation: transition.generation,
                            window_id: decision.window_id,
                            state: decision.state,
                            width,
                        },
                    );
                }
                DecisionOutcome::Preserve { .. }
                | DecisionOutcome::Unmanaged
                | DecisionOutcome::Excluded { .. }
                | DecisionOutcome::UnknownWindow => {
                    self.pending.remove(&decision.window_id);
                }
            }
        }
    }

    /// Removes and returns the most relevant pending action.
    ///
    /// Newer generations run first. Within one transition, expansion of the
    /// focused window precedes contraction of windows that lost focus.
    pub fn pop_next(&mut self) -> Option<ResizeAction> {
        let window_id = self
            .pending
            .values()
            .max_by_key(|action| {
                (
                    action.generation,
                    matches!(action.state, FocusState::Focused),
                )
            })?
            .window_id;
        self.pending.remove(&window_id)
    }

    /// Drops all queued and generation state for a closed window.
    pub fn forget_window(&mut self, window_id: WindowId) {
        self.pending.remove(&window_id);
        self.latest_generation.remove(&window_id);
    }

    /// Clears queued work before ingesting an authoritative full snapshot.
    pub fn reset(&mut self) {
        self.pending.clear();
        self.latest_generation.clear();
    }

    /// Whether no mutation is waiting to be sent.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::PendingResizes;
    use crate::{
        attention::{Proportion, WidthPolicy},
        transition::{DecisionOutcome, FocusDecision, FocusState, FocusTransition, WindowId},
    };

    const TERMINAL: WindowId = WindowId(10);
    const EDITOR: WindowId = WindowId(20);

    fn width(value: f64) -> Proportion {
        match WidthPolicy::proportion(value).expect("valid width") {
            WidthPolicy::Proportion(width) => width,
            WidthPolicy::Preserve => unreachable!(),
        }
    }

    fn resize(
        generation: u64,
        window_id: WindowId,
        state: FocusState,
        value: f64,
    ) -> FocusTransition {
        FocusTransition {
            generation,
            previous: None,
            current: None,
            decisions: vec![FocusDecision {
                window_id,
                state,
                outcome: DecisionOutcome::Resize {
                    rule_index: 0,
                    width: width(value),
                },
            }],
        }
    }

    #[test]
    fn newer_resize_supersedes_pending_width_for_same_window() {
        let mut pending = PendingResizes::default();
        pending.ingest(&resize(1, TERMINAL, FocusState::Focused, 0.5));
        pending.ingest(&resize(2, TERMINAL, FocusState::Unfocused, 0.1));

        let action = pending.pop_next().expect("pending resize");
        assert_eq!(action.generation, 2);
        assert_eq!(action.width, width(0.1));
        assert!(pending.is_empty());
    }

    #[test]
    fn newer_no_op_cancels_an_older_resize() {
        let mut pending = PendingResizes::default();
        pending.ingest(&resize(1, TERMINAL, FocusState::Focused, 0.5));
        pending.ingest(&FocusTransition {
            generation: 2,
            previous: Some(TERMINAL),
            current: Some(TERMINAL),
            decisions: vec![FocusDecision {
                window_id: TERMINAL,
                state: FocusState::Focused,
                outcome: DecisionOutcome::Preserve { rule_index: 0 },
            }],
        });

        assert!(pending.is_empty());
    }

    #[test]
    fn stale_transition_cannot_restore_cancelled_work() {
        let mut pending = PendingResizes::default();
        pending.ingest(&FocusTransition {
            generation: 2,
            previous: None,
            current: Some(TERMINAL),
            decisions: vec![FocusDecision {
                window_id: TERMINAL,
                state: FocusState::Focused,
                outcome: DecisionOutcome::Unmanaged,
            }],
        });
        pending.ingest(&resize(1, TERMINAL, FocusState::Focused, 0.5));

        assert!(pending.is_empty());
    }

    #[test]
    fn focused_resize_runs_before_contraction_from_same_generation() {
        let mut pending = PendingResizes::default();
        let mut transition = resize(3, TERMINAL, FocusState::Unfocused, 0.1);
        transition.decisions.push(FocusDecision {
            window_id: EDITOR,
            state: FocusState::Focused,
            outcome: DecisionOutcome::Resize {
                rule_index: 1,
                width: width(0.9),
            },
        });
        pending.ingest(&transition);

        assert_eq!(pending.pop_next().expect("focused").window_id, EDITOR);
        assert_eq!(pending.pop_next().expect("unfocused").window_id, TERMINAL);
    }

    #[test]
    fn forgetting_closed_window_cancels_pending_resize() {
        let mut pending = PendingResizes::default();
        pending.ingest(&resize(1, TERMINAL, FocusState::Focused, 0.5));

        pending.forget_window(TERMINAL);

        assert!(pending.is_empty());
    }

    #[test]
    fn reset_discards_work_from_state_before_full_snapshot() {
        let mut pending = PendingResizes::default();
        pending.ingest(&resize(1, TERMINAL, FocusState::Focused, 0.5));

        pending.reset();

        assert!(pending.is_empty());
    }
}
