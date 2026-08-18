//! Deterministic width selection for focus transitions.

use std::{error::Error, fmt, str::FromStr};

/// A width behavior applied to a niri column.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WidthPolicy {
    /// Use a fraction of the output width, in the range `0.0 < value <= 1.0`.
    Proportion(Proportion),
    /// Leave the column width unchanged.
    Preserve,
}

impl WidthPolicy {
    /// Creates a proportional width when the supplied value is valid.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidProportion`] when the value is not finite or falls
    /// outside `0.0 < value <= 1.0`.
    pub fn proportion(value: f64) -> Result<Self, InvalidProportion> {
        if value.is_finite() && value > 0.0 && value <= 1.0 {
            Ok(Self::Proportion(Proportion(value)))
        } else {
            Err(InvalidProportion(value))
        }
    }
}

impl FromStr for WidthPolicy {
    type Err = WidthParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let value = input.trim();
        if value == "preserve" {
            return Ok(Self::Preserve);
        }

        let proportion = if let Some(percent) = value.strip_suffix('%') {
            parse_number(percent, input)? / 100.0
        } else {
            parse_number(value, input)?
        };

        Self::proportion(proportion).map_err(WidthParseError::InvalidProportion)
    }
}

impl fmt::Display for WidthPolicy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Proportion(value) => write!(formatter, "{}%", value.get() * 100.0),
            Self::Preserve => formatter.write_str("preserve"),
        }
    }
}

fn parse_number(value: &str, original: &str) -> Result<f64, WidthParseError> {
    value
        .parse::<f64>()
        .map_err(|_| WidthParseError::InvalidNumber(original.to_owned()))
}

/// A rejected proportional width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InvalidProportion(pub f64);

/// A validated fraction of output width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Proportion(f64);

impl Proportion {
    /// Returns the fraction in the range `0.0 < value <= 1.0`.
    #[must_use]
    pub const fn get(self) -> f64 {
        self.0
    }
}

impl fmt::Display for InvalidProportion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "proportion must be finite and greater than 0 through 1, got {}",
            self.0
        )
    }
}

impl Error for InvalidProportion {}

/// A width value that could not be parsed or validated.
#[derive(Clone, Debug, PartialEq)]
pub enum WidthParseError {
    /// The input was neither a decimal proportion nor a percentage.
    InvalidNumber(String),
    /// The parsed number fell outside the supported range.
    InvalidProportion(InvalidProportion),
}

impl fmt::Display for WidthParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidNumber(value) => write!(
                formatter,
                "expected `preserve`, a proportion such as `0.5`, or a percentage such as `50%`; got `{value}`"
            ),
            Self::InvalidProportion(error) => error.fmt(formatter),
        }
    }
}

impl Error for WidthParseError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidNumber(_) => None,
            Self::InvalidProportion(error) => Some(error),
        }
    }
}

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

    fn width(value: f64) -> WidthPolicy {
        WidthPolicy::proportion(value).expect("valid test width")
    }

    #[test]
    fn selects_width_from_focus_state() {
        let policy = AttentionPolicy {
            focused: width(0.5),
            unfocused: width(0.1),
        };

        assert_eq!(policy.width_for(true), width(0.5));
        assert_eq!(policy.width_for(false), width(0.1));
    }

    #[test]
    fn rejects_invalid_proportions() {
        assert!(WidthPolicy::proportion(0.0).is_err());
        assert!(WidthPolicy::proportion(-0.1).is_err());
        assert!(WidthPolicy::proportion(1.1).is_err());
        assert!(WidthPolicy::proportion(f64::NAN).is_err());
    }

    #[test]
    fn preserve_is_an_explicit_policy() {
        let policy = AttentionPolicy {
            focused: width(0.9),
            unfocused: WidthPolicy::Preserve,
        };

        assert_eq!(policy.width_for(false), WidthPolicy::Preserve);
    }

    #[test]
    fn parses_percent_decimal_and_preserve_widths() {
        assert_eq!("50%".parse(), Ok(width(0.5)));
        assert_eq!("0.1".parse(), Ok(width(0.1)));
        assert_eq!("preserve".parse(), Ok(WidthPolicy::Preserve));
    }

    #[test]
    fn rejects_ambiguous_or_empty_widths() {
        assert!("50".parse::<WidthPolicy>().is_err());
        assert!("0%".parse::<WidthPolicy>().is_err());
        assert!("".parse::<WidthPolicy>().is_err());
    }
}
