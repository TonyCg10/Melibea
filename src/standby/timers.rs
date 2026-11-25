//! Standby after minimizing: which minimized games are due for suspension.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

/// Minimized periods of windows, and which ones have already fired.
#[derive(Debug, Default)]
pub struct StandbyTimers {
    /// Every minimized window: when its period began and its rule's delay.
    minimized: BTreeMap<u64, (Instant, Option<Duration>)>,
    /// Windows whose current period has already been reported due.
    fired: BTreeSet<u64>,
}

impl StandbyTimers {
    /// Replaces the minimized list with niri's latest one, each window with
    /// its delay (none when its rule names no `standby_after`). A window
    /// already listed keeps the start of its period; a new one starts at
    /// `now`. Returns the ids that left the list, ascending.
    pub fn update(&mut self, now: Instant, minimized: &[(u64, Option<Duration>)]) -> Vec<u64> {
        let current: BTreeMap<u64, Option<Duration>> = minimized.iter().copied().collect();
        let left: Vec<u64> = self
            .minimized
            .keys()
            .filter(|id| !current.contains_key(id))
            .copied()
            .collect();
        for id in &left {
            self.minimized.remove(id);
            self.fired.remove(id);
        }
        for (id, delay) in current {
            self.minimized
                .entry(id)
                .and_modify(|(_, known)| *known = delay)
                .or_insert((now, delay));
        }
        left
    }

    /// The windows whose delay has elapsed at `now`, ascending. Each window is
    /// reported at most once per minimized period.
    pub fn due(&mut self, now: Instant) -> Vec<u64> {
        let due: Vec<u64> = self
            .minimized
            .iter()
            .filter(|(id, (since, delay))| {
                !self.fired.contains(id)
                    && delay.is_some_and(|delay| now.saturating_duration_since(*since) >= delay)
            })
            .map(|(id, _)| *id)
            .collect();
        self.fired.extend(due.iter().copied());
        due
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::StandbyTimers;

    const FIVE_MINUTES: Duration = Duration::from_secs(300);

    #[test]
    fn a_game_is_due_after_its_delay() {
        let t0 = Instant::now();
        let mut timers = StandbyTimers::default();
        assert!(timers.update(t0, &[(7, Some(FIVE_MINUTES))]).is_empty());

        assert!(timers.due(t0 + Duration::from_secs(299)).is_empty());
        assert_eq!(timers.due(t0 + Duration::from_secs(300)), vec![7]);
        assert!(timers.due(t0 + Duration::from_secs(400)).is_empty());
    }

    #[test]
    fn a_window_without_delay_never_fires() {
        let t0 = Instant::now();
        let mut timers = StandbyTimers::default();
        timers.update(t0, &[(7, None)]);

        assert!(timers.due(t0 + Duration::from_hours(24)).is_empty());
    }

    #[test]
    fn leaving_the_minimized_list_resets_and_is_reported() {
        let t0 = Instant::now();
        let mut timers = StandbyTimers::default();
        timers.update(t0, &[(7, Some(FIVE_MINUTES)), (8, None)]);
        assert_eq!(timers.due(t0 + FIVE_MINUTES), vec![7]);

        let left = timers.update(t0 + Duration::from_secs(310), &[]);
        assert_eq!(left, vec![7, 8]);

        // Minimized again: a new period starts and fires once more.
        let t1 = t0 + Duration::from_secs(320);
        assert!(timers.update(t1, &[(7, Some(FIVE_MINUTES))]).is_empty());
        assert!(timers.due(t1 + Duration::from_secs(299)).is_empty());
        assert_eq!(timers.due(t1 + FIVE_MINUTES), vec![7]);
    }

    #[test]
    fn staying_minimized_keeps_the_period() {
        let t0 = Instant::now();
        let mut timers = StandbyTimers::default();
        timers.update(t0, &[(7, Some(FIVE_MINUTES))]);
        // A later list that still holds the window does not restart its clock.
        assert!(
            timers
                .update(t0 + Duration::from_secs(200), &[(7, Some(FIVE_MINUTES))])
                .is_empty()
        );

        assert_eq!(timers.due(t0 + FIVE_MINUTES), vec![7]);
    }

    #[test]
    fn a_zero_delay_is_due_at_once() {
        let t0 = Instant::now();
        let mut timers = StandbyTimers::default();
        timers.update(t0, &[(7, Some(Duration::ZERO))]);

        assert_eq!(timers.due(t0), vec![7]);
        assert!(timers.due(t0).is_empty());
    }
}
