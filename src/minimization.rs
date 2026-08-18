//! Pure minimized-window state projected from a future native niri contract.
//!
//! niri remains authoritative for whether a mapped surface is minimized.
//! Melibea mirrors that state so a CLI or shell can render and operate a
//! deterministic bubble list without owning surface lifetime.

use std::{collections::BTreeSet, error::Error, fmt};

use crate::transition::WindowId;

/// Metadata needed to identify one minimized window outside the compositor.
///
/// Icon lookup deliberately remains a shell concern. `app_id` is the stable
/// hint a shell can use for desktop-entry resolution while `title` helps users
/// distinguish multiple windows of the same application.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MinimizedWindow {
    pub id: WindowId,
    pub app_id: Option<String>,
    pub title: Option<String>,
}

/// Authoritative minimized-window change received from the compositor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MinimizationEvent {
    /// Complete ordered state, used on startup and after reconnecting.
    Snapshot { windows: Vec<MinimizedWindow> },
    /// A visible mapped window entered native minimized state.
    Minimized { window: MinimizedWindow },
    /// Metadata changed while the surface remained minimized.
    MetadataChanged { window: MinimizedWindow },
    /// A minimized window returned to a visible layout.
    Restored { id: WindowId },
    /// The client destroyed a window while it was minimized.
    Closed { id: WindowId },
}

/// Why an entry disappeared from the bubble projection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemovalReason {
    Restored,
    Closed,
}

/// Explainable result of applying one authoritative event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegistryChange {
    SnapshotReplaced {
        previous_count: usize,
        current_count: usize,
    },
    Added {
        id: WindowId,
        index: usize,
    },
    Updated {
        id: WindowId,
        index: usize,
    },
    Removed {
        id: WindowId,
        index: usize,
        reason: RemovalReason,
    },
    Unchanged,
    /// Incremental metadata or removal referred to an entry absent from the
    /// latest authoritative state. Melibea never fabricates it.
    UnknownWindow {
        id: WindowId,
    },
}

/// Ordered, transport-neutral projection of native minimized windows.
#[derive(Debug, Default)]
pub struct MinimizedRegistry {
    windows: Vec<MinimizedWindow>,
    revision: u64,
}

impl MinimizedRegistry {
    /// Returns the complete bubble order exposed to consumers.
    #[must_use]
    pub fn windows(&self) -> &[MinimizedWindow] {
        &self.windows
    }

    /// Monotonic local revision advanced only by material state changes.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Applies one compositor event atomically.
    ///
    /// Snapshot order is authoritative. A duplicate identifier rejects the
    /// whole snapshot without modifying the last known-good state.
    ///
    /// # Errors
    ///
    /// Returns [`MinimizationError::DuplicateWindow`] for an invalid snapshot.
    pub fn apply(&mut self, event: MinimizationEvent) -> Result<RegistryChange, MinimizationError> {
        match event {
            MinimizationEvent::Snapshot { windows } => self.replace_snapshot(windows),
            MinimizationEvent::Minimized { window } => Ok(self.minimized(window)),
            MinimizationEvent::MetadataChanged { window } => Ok(self.metadata_changed(window)),
            MinimizationEvent::Restored { id } => Ok(self.remove(id, RemovalReason::Restored)),
            MinimizationEvent::Closed { id } => Ok(self.remove(id, RemovalReason::Closed)),
        }
    }

    fn replace_snapshot(
        &mut self,
        windows: Vec<MinimizedWindow>,
    ) -> Result<RegistryChange, MinimizationError> {
        let mut ids = BTreeSet::new();
        for window in &windows {
            if !ids.insert(window.id) {
                return Err(MinimizationError::DuplicateWindow(window.id));
            }
        }

        if self.windows == windows {
            return Ok(RegistryChange::Unchanged);
        }

        let previous_count = self.windows.len();
        let current_count = windows.len();
        self.windows = windows;
        self.advance_revision();
        Ok(RegistryChange::SnapshotReplaced {
            previous_count,
            current_count,
        })
    }

    fn minimized(&mut self, window: MinimizedWindow) -> RegistryChange {
        if let Some(index) = self.position(window.id) {
            if self.windows[index] == window {
                return RegistryChange::Unchanged;
            }

            let id = window.id;
            self.windows[index] = window;
            self.advance_revision();
            return RegistryChange::Updated { id, index };
        }

        let id = window.id;
        let index = self.windows.len();
        self.windows.push(window);
        self.advance_revision();
        RegistryChange::Added { id, index }
    }

    fn metadata_changed(&mut self, window: MinimizedWindow) -> RegistryChange {
        let id = window.id;
        let Some(index) = self.position(id) else {
            return RegistryChange::UnknownWindow { id };
        };

        if self.windows[index] == window {
            return RegistryChange::Unchanged;
        }

        self.windows[index] = window;
        self.advance_revision();
        RegistryChange::Updated { id, index }
    }

    fn remove(&mut self, id: WindowId, reason: RemovalReason) -> RegistryChange {
        let Some(index) = self.position(id) else {
            return RegistryChange::UnknownWindow { id };
        };

        self.windows.remove(index);
        self.advance_revision();
        RegistryChange::Removed { id, index, reason }
    }

    fn position(&self, id: WindowId) -> Option<usize> {
        self.windows.iter().position(|window| window.id == id)
    }

    fn advance_revision(&mut self) {
        self.revision = self.revision.saturating_add(1);
    }
}

/// Invalid authoritative minimized-window state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MinimizationError {
    DuplicateWindow(WindowId),
}

impl fmt::Display for MinimizationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateWindow(id) => {
                write!(formatter, "minimized-window snapshot repeats id {}", id.0)
            }
        }
    }
}

impl Error for MinimizationError {}

#[cfg(test)]
mod tests {
    use super::{
        MinimizationError, MinimizationEvent, MinimizedRegistry, MinimizedWindow, RegistryChange,
        RemovalReason,
    };
    use crate::transition::WindowId;

    fn window(id: u64, title: &str) -> MinimizedWindow {
        MinimizedWindow {
            id: WindowId(id),
            app_id: Some("org.example.App".to_owned()),
            title: Some(title.to_owned()),
        }
    }

    #[test]
    fn snapshot_establishes_authoritative_bubble_order() {
        let mut registry = MinimizedRegistry::default();

        let change = registry
            .apply(MinimizationEvent::Snapshot {
                windows: vec![window(20, "second"), window(10, "first")],
            })
            .expect("valid snapshot");

        assert_eq!(
            change,
            RegistryChange::SnapshotReplaced {
                previous_count: 0,
                current_count: 2,
            }
        );
        assert_eq!(
            registry
                .windows()
                .iter()
                .map(|window| window.id)
                .collect::<Vec<_>>(),
            vec![WindowId(20), WindowId(10)]
        );
        assert_eq!(registry.revision(), 1);
    }

    #[test]
    fn invalid_snapshot_is_rejected_atomically() {
        let mut registry = MinimizedRegistry::default();
        registry
            .apply(MinimizationEvent::Minimized {
                window: window(10, "kept"),
            })
            .expect("valid event");
        let revision = registry.revision();

        let error = registry.apply(MinimizationEvent::Snapshot {
            windows: vec![window(20, "duplicate"), window(20, "duplicate")],
        });

        assert_eq!(error, Err(MinimizationError::DuplicateWindow(WindowId(20))));
        assert_eq!(registry.windows(), &[window(10, "kept")]);
        assert_eq!(registry.revision(), revision);
    }

    #[test]
    fn repeated_minimize_updates_in_place_without_reordering() {
        let mut registry = MinimizedRegistry::default();
        registry
            .apply(MinimizationEvent::Snapshot {
                windows: vec![window(10, "old"), window(20, "other")],
            })
            .expect("valid snapshot");

        let change = registry
            .apply(MinimizationEvent::Minimized {
                window: window(10, "new"),
            })
            .expect("valid event");

        assert_eq!(
            change,
            RegistryChange::Updated {
                id: WindowId(10),
                index: 0,
            }
        );
        assert_eq!(registry.windows()[0].title.as_deref(), Some("new"));
        assert_eq!(registry.windows()[1].id, WindowId(20));
    }

    #[test]
    fn unknown_metadata_never_fabricates_a_bubble() {
        let mut registry = MinimizedRegistry::default();

        let change = registry
            .apply(MinimizationEvent::MetadataChanged {
                window: window(99, "unknown"),
            })
            .expect("valid event");

        assert_eq!(change, RegistryChange::UnknownWindow { id: WindowId(99) });
        assert!(registry.windows().is_empty());
        assert_eq!(registry.revision(), 0);
    }

    #[test]
    fn restore_and_client_close_are_distinct_removals() {
        let mut registry = MinimizedRegistry::default();
        registry
            .apply(MinimizationEvent::Snapshot {
                windows: vec![window(10, "restore"), window(20, "close")],
            })
            .expect("valid snapshot");

        let restored = registry
            .apply(MinimizationEvent::Restored { id: WindowId(10) })
            .expect("valid restore");
        let closed = registry
            .apply(MinimizationEvent::Closed { id: WindowId(20) })
            .expect("valid close");

        assert_eq!(
            restored,
            RegistryChange::Removed {
                id: WindowId(10),
                index: 0,
                reason: RemovalReason::Restored,
            }
        );
        assert_eq!(
            closed,
            RegistryChange::Removed {
                id: WindowId(20),
                index: 0,
                reason: RemovalReason::Closed,
            }
        );
        assert!(registry.windows().is_empty());
        assert_eq!(registry.revision(), 3);
    }

    #[test]
    fn identical_state_does_not_advance_revision() {
        let mut registry = MinimizedRegistry::default();
        let event = MinimizationEvent::Minimized {
            window: window(10, "same"),
        };
        registry.apply(event.clone()).expect("valid event");
        let revision = registry.revision();

        assert_eq!(
            registry.apply(event).expect("valid event"),
            RegistryChange::Unchanged
        );
        assert_eq!(registry.revision(), revision);
    }
}
