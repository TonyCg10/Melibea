//! Deterministic width selection for focus transitions.

/// A width behavior applied to a niri column.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WidthPolicy {
    /// Use a fraction of the output width, in the inclusive range `0.0..=1.0`.
    Proportion(f64),
    /// Leave the column width unchanged.
    Preserve,
}

impl WidthPolicy {
    /// Creates a proportional width when the supplied value is valid.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidProportion`] when the value is not finite or falls
    /// outside `0.0..=1.0`.
    pub fn proportion(value: f64) -> Result<Self, InvalidProportion> {
        if value.is_finite() && (0.0..=1.0).contains(&value) {
            Ok(Self::Proportion(value))
        } else {
            Err(InvalidProportion(value))
        }
    }
}

/// A rejected proportional width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InvalidProportion(pub f64);

/// Widths applied while a matching column is focused or unfocused.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AttentionPolicy {
    pub focused: WidthPolicy,
    pub unfocused: WidthPolicy,
}

impl AttentionPolicy {
    #[must_use]
    pub const fn width_for(self, focused: bool) -> WidthPolicy {
        if focused {
            self.focused
        } else {
            self.unfocused
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AttentionPolicy, WidthPolicy};

    #[test]
    fn selects_width_from_focus_state() {
        let policy = AttentionPolicy {
            focused: WidthPolicy::proportion(0.5).expect("valid test width"),
            unfocused: WidthPolicy::proportion(0.1).expect("valid test width"),
        };

        assert_eq!(policy.width_for(true), WidthPolicy::Proportion(0.5));
        assert_eq!(policy.width_for(false), WidthPolicy::Proportion(0.1));
    }

    #[test]
    fn rejects_invalid_proportions() {
        assert!(WidthPolicy::proportion(-0.1).is_err());
        assert!(WidthPolicy::proportion(1.1).is_err());
        assert!(WidthPolicy::proportion(f64::NAN).is_err());
    }

    #[test]
    fn preserve_is_an_explicit_policy() {
        let policy = AttentionPolicy {
            focused: WidthPolicy::Proportion(0.9),
            unfocused: WidthPolicy::Preserve,
        };

        assert_eq!(policy.width_for(false), WidthPolicy::Preserve);
    }
}
