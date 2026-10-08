//! Command-line arguments.
//!
//! Two shapes only: no arguments opens the interactive ruler, and `--measure` with `--at` and
//! `--output` measures once without showing anything. The second exists so a measurement can
//! be scripted and checked.

pub const DEFAULT_TOLERANCE: u8 = 30;

pub const USAGE: &str = "usage: melibea-ruler [--tolerance N]
       melibea-ruler --measure box|spacing --at X,Y --output NAME [--tolerance N]";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The bounds of the element under the pointer.
    Box,
    /// The gap around the pointer, horizontally and vertically.
    Spacing,
}

impl Mode {
    pub fn toggled(self) -> Mode {
        match self {
            Mode::Box => Mode::Spacing,
            Mode::Spacing => Mode::Box,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Mode::Box => "box",
            Mode::Spacing => "spacing",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Interactive {
        tolerance: u8,
    },
    /// `at` is in output-local logical pixels.
    Measure {
        mode: Mode,
        at: (f64, f64),
        output: String,
        tolerance: u8,
    },
}

pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Command, String> {
    let mut tolerance = DEFAULT_TOLERANCE;
    let mut mode = None;
    let mut at = None;
    let mut output = None;

    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--tolerance" => {
                let value = next_value(&mut args, "--tolerance")?;
                tolerance = value
                    .parse()
                    .map_err(|_| format!("--tolerance must be 0-255, got {value:?}"))?;
            }
            "--measure" => {
                mode = Some(match next_value(&mut args, "--measure")?.as_str() {
                    "box" => Mode::Box,
                    "spacing" => Mode::Spacing,
                    other => {
                        return Err(format!("--measure must be box or spacing, got {other:?}"));
                    }
                });
            }
            "--at" => {
                let value = next_value(&mut args, "--at")?;
                at = Some(
                    parse_point(&value)
                        .ok_or_else(|| format!("--at must be X,Y, got {value:?}"))?,
                );
            }
            "--output" => output = Some(next_value(&mut args, "--output")?),
            other => return Err(format!("unknown argument {other:?}")),
        }
    }

    match (mode, at, output) {
        (None, None, None) => Ok(Command::Interactive { tolerance }),
        (Some(mode), Some(at), Some(output)) => Ok(Command::Measure {
            mode,
            at,
            output,
            tolerance,
        }),
        _ => Err("--measure, --at and --output go together".to_owned()),
    }
}

fn next_value(args: &mut impl Iterator<Item = String>, name: &str) -> Result<String, String> {
    args.next().ok_or_else(|| format!("{name} needs a value"))
}

fn parse_point(value: &str) -> Option<(f64, f64)> {
    let (x, y) = value.split_once(',')?;
    let x: f64 = x.trim().parse().ok()?;
    let y: f64 = y.trim().parse().ok()?;
    (x.is_finite() && y.is_finite() && x >= 0.0 && y >= 0.0).then_some((x, y))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn no_arguments_opens_the_ruler_with_the_default_tolerance() {
        assert_eq!(parse(args(&[])), Ok(Command::Interactive { tolerance: 30 }));
    }

    #[test]
    fn tolerance_can_be_set() {
        assert_eq!(
            parse(args(&["--tolerance", "12"])),
            Ok(Command::Interactive { tolerance: 12 })
        );
    }

    #[test]
    fn a_full_measure_command_parses() {
        assert_eq!(
            parse(args(&[
                "--measure",
                "box",
                "--at",
                "10.5,20",
                "--output",
                "DP-1"
            ])),
            Ok(Command::Measure {
                mode: Mode::Box,
                at: (10.5, 20.0),
                output: "DP-1".to_owned(),
                tolerance: 30,
            })
        );
    }

    #[test]
    fn measure_without_its_companions_is_refused() {
        assert!(parse(args(&["--measure", "spacing"])).is_err());
        assert!(parse(args(&["--at", "1,1", "--output", "DP-1"])).is_err());
    }

    #[test]
    fn bad_values_are_refused() {
        assert!(
            parse(args(&[
                "--measure",
                "circle",
                "--at",
                "1,1",
                "--output",
                "X"
            ]))
            .is_err()
        );
        assert!(parse(args(&["--tolerance", "300"])).is_err());
        assert!(parse(args(&["--tolerance"])).is_err());
        assert!(parse(args(&["--measure", "box", "--at", "1;1", "--output", "X"])).is_err());
        assert!(parse(args(&["--measure", "box", "--at", "-1,1", "--output", "X"])).is_err());
        assert!(parse(args(&["--frobnicate"])).is_err());
    }

    #[test]
    fn mode_toggles_and_names_itself() {
        assert_eq!(Mode::Box.toggled(), Mode::Spacing);
        assert_eq!(Mode::Spacing.toggled(), Mode::Box);
        assert_eq!(Mode::Box.name(), "box");
        assert_eq!(Mode::Spacing.name(), "spacing");
    }
}
