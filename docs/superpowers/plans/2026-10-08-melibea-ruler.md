# melibea-ruler Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `melibea-ruler`, a one-shot screen ruler for niri that freezes every output, measures an element's box or the spacing around the pointer in logical pixels, copies the result and exits.

**Architecture:** A second Cargo package in a new workspace, so the `melibea` daemon keeps its four dependencies. Pure modules (`cli`, `image`, `units`, `edges`, `draw`, `lock`) carry the logic and the tests; `capture` speaks `wlr-screencopy` once per output; `overlay` shows the frozen captures in layer-shell surfaces and handles pointer and keyboard. A headless `--measure` mode makes the roadmap's exit criteria checkable without a person.

**Tech Stack:** Rust 2024 (toolchain 1.98), `smithay-client-toolkit` 0.19.2, `wayland-client` 0.31, `wayland-protocols` 0.32 (viewporter), `wayland-protocols-wlr` 0.3 (screencopy), `ab_glyph` 0.2.32, `wl-copy`, `fc-match`, `notify-send`.

**Spec:** [docs/superpowers/specs/2026-10-08-melibea-ruler-design.md](../specs/2026-10-08-melibea-ruler-design.md)

## Global Constraints

- Licence MIT; every new dependency must be permissive (MIT, Apache-2.0, BSD).
- The `melibea` package's dependencies stay exactly `regex`, `serde`, `serde_json`, `toml`.
- `ruler/` uses the same lints as `melibea`: `unsafe_code = "forbid"`, clippy `all` and `pedantic` at `warn`; `cargo clippy --workspace --all-targets` must be warning-free in `ruler/`.
- Results are logical pixels: physical measurement ÷ output scale, rounded to nearest.
- Default tolerance 30 (0–255, max per-channel difference against the reference pixel).
- Box uses a 4-connected flood fill; spacing walks four rays; both stop at the output edge.
- Clipboard text `412x38`; on-screen label `412 × 38` with a detail line `box · tol 30 · DP-1`.
- Captures are taken before any surface is shown, without the cursor; any capture failure aborts before showing anything.
- Single instance via `flock` on `$XDG_RUNTIME_DIR/melibea-ruler.lock`; a second instance exits silently with status 0.
- The frozen picture is shown unchanged (not dimmed or tinted).
- The ruler never talks to `melibea.service`, Celestina or Noctalia.
- Wayland's `Xrgb8888` is laid out in memory as B, G, R, X bytes per pixel.

## Deviation from the spec, decided while planning

The spec lists `tiny-skia` for drawing. Every line the ruler draws is horizontal or vertical, and the label background is a plain rectangle, so a few axis-aligned fill functions in `draw.rs` cover all of it and are directly testable. `tiny-skia` is not added. Task 8 records this in the spec.

The spec also says a redraw does not touch the whole 4K frame. This plan copies the frozen
frame into a fresh buffer on every redraw (one memcpy, about 33 MB on DP-1) and damages only
the changed area, so the compositor uploads and recomposites only that area. Reusing one
buffer per output would need double buffering to avoid writing into a buffer the compositor
still holds. Redraws are paced by frame callbacks, so the copy happens at most once per output
refresh. If Task 7's manual check shows the ruler lagging on DP-1, switch to two buffers per
surface and restore only the previous guides' rectangle; until then the simpler path stands.

## File Structure

```text
Cargo.toml                 modify: add [workspace]
scripts/install.sh         modify: build and install both binaries
ruler/Cargo.toml           create: package melibea-ruler
ruler/src/main.rs          create: wiring, exit codes, notifications, clipboard
ruler/src/cli.rs           create: argument parsing (Command, Mode)
ruler/src/image.rs         create: Image (frozen frame) and colour matching
ruler/src/units.rs         create: physical <-> logical, result formatting
ruler/src/edges.rs         create: Bounds, spacing(), Region, region()
ruler/src/draw.rs          create: Rect, Canvas, guides, label placement
ruler/src/text.rs          create: font loading and the two-line label
ruler/src/lock.rs          create: single-instance lock
ruler/src/capture.rs       create: wlr-screencopy of every output
ruler/src/overlay.rs       create: layer surfaces, input, redraw
ruler/tests/probes/box.py      create: nested-session probe, solid box of known size
ruler/tests/probes/button.py   create: nested-session probe, GTK button
ruler/tests/nest.sh            create: runs the exit-criteria checks in a nested niri
docs/superpowers/specs/2026-10-08-melibea-ruler-design.md   modify: record the tiny-skia deviation
ROADMAP.md                 modify: M10 status
~/.config/niri/config.kdl  modify (outside the repo): `M` in the `tools` mode
```

---

### Task 1: Workspace and argument parsing

**Files:**
- Modify: `Cargo.toml`
- Create: `ruler/Cargo.toml`, `ruler/src/main.rs`, `ruler/src/cli.rs`

**Interfaces:**
- Produces: `cli::Mode { Box, Spacing }` with `toggled(self) -> Mode` and `name(self) -> &'static str`; `cli::Command::{Interactive { tolerance: u8 }, Measure { mode: Mode, at: (f64, f64), output: String, tolerance: u8 }}`; `cli::parse<I: IntoIterator<Item = String>>(args: I) -> Result<Command, String>`; `cli::USAGE: &str`; `cli::DEFAULT_TOLERANCE: u8 = 30`.

- [ ] **Step 1: Add the workspace to the root manifest**

Append to `Cargo.toml` (the `[package]` section stays as it is):

```toml
[workspace]
members = ["ruler"]
```

- [ ] **Step 2: Create `ruler/Cargo.toml`**

```toml
[package]
name = "melibea-ruler"
version = "0.1.0"
edition = "2024"
description = "One-shot screen ruler for niri: element bounds and spacing in logical pixels"
license = "MIT"
publish = false

[dependencies]
ab_glyph = "0.2.32"
smithay-client-toolkit = { version = "0.19.2", default-features = false, features = ["xkbcommon"] }
wayland-client = "0.31"
wayland-protocols = { version = "0.32", features = ["client"] }
wayland-protocols-wlr = { version = "0.3", features = ["client"] }

[lints.rust]
unsafe_code = "forbid"

[lints.clippy]
all = "warn"
pedantic = "warn"
```

- [ ] **Step 3: Write the failing tests in `ruler/src/cli.rs`**

```rust
//! Command-line arguments.
//!
//! Two shapes only: no arguments opens the interactive ruler, and `--measure` with `--at` and
//! `--output` measures once without showing anything. The second exists so a measurement can
//! be scripted and checked.

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
            parse(args(&["--measure", "box", "--at", "10.5,20", "--output", "DP-1"])),
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
        assert!(parse(args(&["--measure", "circle", "--at", "1,1", "--output", "X"])).is_err());
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
```

And `ruler/src/main.rs`:

```rust
//! `melibea-ruler`: a one-shot screen ruler for niri.
//!
//! See `docs/superpowers/specs/2026-10-08-melibea-ruler-design.md`.

mod cli;

use std::process::ExitCode;

fn main() -> ExitCode {
    match cli::parse(std::env::args().skip(1)) {
        Ok(command) => {
            eprintln!("melibea-ruler: {command:?} is not wired up yet");
            ExitCode::FAILURE
        }
        Err(message) => {
            eprintln!("melibea-ruler: {message}\n{}", cli::USAGE);
            ExitCode::from(2)
        }
    }
}
```

- [ ] **Step 4: Run the tests to verify they fail**

Run: `cargo test -p melibea-ruler`
Expected: compile errors — `parse`, `Command`, `Mode` not found. (The first run downloads `ab_glyph`; it needs network.)

- [ ] **Step 5: Implement `cli.rs` above the test module**

```rust
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
                    other => return Err(format!("--measure must be box or spacing, got {other:?}")),
                });
            }
            "--at" => {
                let value = next_value(&mut args, "--at")?;
                at = Some(
                    parse_point(&value).ok_or_else(|| format!("--at must be X,Y, got {value:?}"))?,
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
```

- [ ] **Step 6: Run the tests and check the daemon's dependencies are untouched**

Run: `cargo test -p melibea-ruler && cargo test -p melibea && cargo tree -p melibea --depth 1 -e normal`
Expected: all tests pass; the tree lists only `regex`, `serde`, `serde_json`, `toml` under `melibea`.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock ruler/
git commit -m "Start melibea-ruler as a second package in a workspace"
```

---

### Task 2: Frozen frames and units

**Files:**
- Create: `ruler/src/image.rs`, `ruler/src/units.rs`
- Modify: `ruler/src/main.rs` (add `mod image; mod units;`)

**Interfaces:**
- Produces: `image::Image` with `new(width: u32, height: u32, stride: u32, data: Vec<u8>) -> Image`, `from_rgb(width: u32, height: u32, f: impl Fn(u32, u32) -> [u8; 3]) -> Image`, `width(&self) -> u32`, `height(&self) -> u32`, `stride(&self) -> u32`, `data(&self) -> &[u8]`, `rgb(&self, x: u32, y: u32) -> [u8; 3]`, `flip_vertical(&mut self)`; `image::matches(a: [u8; 3], b: [u8; 3], tolerance: u8) -> bool`.
- Produces: `units::scale(physical_width: u32, logical_width: i32) -> f64`, `units::to_logical(physical: u32, scale: f64) -> u32`, `units::to_physical(logical: f64, scale: f64, limit: u32) -> u32`, `units::clipboard(width: u32, height: u32) -> String`, `units::label(width: u32, height: u32) -> String`.

- [ ] **Step 1: Write the failing tests**

`ruler/src/image.rs`:

```rust
//! A frozen frame, in the layout niri's captures arrive in.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixels_are_read_from_bgrx_memory() {
        // One pixel: B=1, G=2, R=3, X=255.
        let image = Image::new(1, 1, 4, vec![1, 2, 3, 255]);
        assert_eq!(image.rgb(0, 0), [3, 2, 1]);
    }

    #[test]
    fn stride_padding_is_skipped() {
        // Two rows of one pixel each, 8 bytes per row.
        let image = Image::new(1, 2, 8, vec![1, 1, 1, 0, 9, 9, 9, 9, 2, 2, 2, 0, 9, 9, 9, 9]);
        assert_eq!(image.rgb(0, 1), [2, 2, 2]);
    }

    #[test]
    fn from_rgb_round_trips() {
        let image = Image::from_rgb(3, 2, |x, y| [x as u8, y as u8, 7]);
        assert_eq!(image.rgb(2, 1), [2, 1, 7]);
        assert_eq!(image.width(), 3);
        assert_eq!(image.height(), 2);
    }

    #[test]
    fn flip_vertical_swaps_rows() {
        let mut image = Image::from_rgb(1, 3, |_, y| [y as u8, 0, 0]);
        image.flip_vertical();
        assert_eq!(image.rgb(0, 0), [2, 0, 0]);
        assert_eq!(image.rgb(0, 2), [0, 0, 0]);
    }

    #[test]
    fn matching_is_per_channel_and_inclusive() {
        assert!(matches([100, 100, 100], [130, 70, 100], 30));
        assert!(!matches([100, 100, 100], [131, 100, 100], 30));
        assert!(!matches([100, 100, 100], [100, 100, 69], 30));
        assert!(matches([0, 0, 0], [0, 0, 0], 0));
        assert!(!matches([0, 0, 0], [0, 0, 1], 0));
    }
}
```

`ruler/src/units.rs`:

```rust
//! Physical and logical pixels, and how results are written.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_comes_from_the_capture_and_the_logical_size() {
        assert!((scale(3840, 2560) - 1.5).abs() < 1e-9);
        assert!((scale(1920, 1920) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn logical_sizes_round_to_nearest() {
        assert_eq!(to_logical(150, 1.5), 100);
        assert_eq!(to_logical(151, 1.5), 101); // 100.67
        assert_eq!(to_logical(1, 1.5), 1); // 0.67
        assert_eq!(to_logical(1920, 1.0), 1920);
        assert_eq!(to_logical(3840, 2.0), 1920);
    }

    #[test]
    fn logical_points_map_into_the_capture() {
        assert_eq!(to_physical(100.4, 1.5, 3840), 150);
        assert_eq!(to_physical(0.0, 1.5, 3840), 0);
        // The far edge and beyond clamp to the last pixel.
        assert_eq!(to_physical(2560.0, 1.5, 3840), 3839);
        assert_eq!(to_physical(9999.0, 1.0, 1920), 1919);
    }

    #[test]
    fn results_are_written_two_ways() {
        assert_eq!(clipboard(412, 38), "412x38");
        assert_eq!(label(412, 38), "412 × 38");
    }
}
```

Add `mod image;` and `mod units;` to `main.rs`.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p melibea-ruler`
Expected: compile errors for the missing items.

- [ ] **Step 3: Implement `image.rs`**

```rust
/// A frozen frame: `Xrgb8888`, which in memory is B, G, R, X per pixel.
#[derive(Clone, Debug)]
pub struct Image {
    width: u32,
    height: u32,
    stride: u32,
    data: Vec<u8>,
}

impl Image {
    pub fn new(width: u32, height: u32, stride: u32, data: Vec<u8>) -> Image {
        assert!(stride >= width * 4, "stride shorter than a row");
        assert!(data.len() >= stride as usize * height as usize, "data shorter than the frame");
        Image {
            width,
            height,
            stride,
            data,
        }
    }

    pub fn from_rgb(width: u32, height: u32, f: impl Fn(u32, u32) -> [u8; 3]) -> Image {
        let stride = width * 4;
        let mut data = vec![0; stride as usize * height as usize];
        for y in 0..height {
            for x in 0..width {
                let [r, g, b] = f(x, y);
                let i = y as usize * stride as usize + x as usize * 4;
                data[i..i + 4].copy_from_slice(&[b, g, r, 0xff]);
            }
        }
        Image::new(width, height, stride, data)
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn stride(&self) -> u32 {
        self.stride
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn rgb(&self, x: u32, y: u32) -> [u8; 3] {
        let i = y as usize * self.stride as usize + x as usize * 4;
        [self.data[i + 2], self.data[i + 1], self.data[i]]
    }

    /// Captures can arrive bottom-up; the flag says so, and this puts them right.
    pub fn flip_vertical(&mut self) {
        let row = self.stride as usize;
        let rows = self.height as usize;
        for y in 0..rows / 2 {
            let (top, bottom) = self.data.split_at_mut((rows - 1 - y) * row);
            top[y * row..(y + 1) * row].swap_with_slice(&mut bottom[..row]);
        }
    }
}

/// Whether two colours are the same within `tolerance` on every channel.
pub fn matches(a: [u8; 3], b: [u8; 3], tolerance: u8) -> bool {
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= tolerance)
}
```

- [ ] **Step 4: Implement `units.rs`**

```rust
/// The scale of an output, read from its capture rather than from the advertised integer
/// scale: niri's scales are fractional, and the capture's width is the truth.
pub fn scale(physical_width: u32, logical_width: i32) -> f64 {
    f64::from(physical_width) / f64::from(logical_width)
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a positive pixel count divided by a positive scale, rounded"
)]
pub fn to_logical(physical: u32, scale: f64) -> u32 {
    (f64::from(physical) / scale).round() as u32
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to 0..limit before the cast"
)]
pub fn to_physical(logical: f64, scale: f64, limit: u32) -> u32 {
    let last = f64::from(limit.saturating_sub(1));
    (logical * scale).floor().clamp(0.0, last) as u32
}

/// What is copied: ready to paste into configuration or code.
pub fn clipboard(width: u32, height: u32) -> String {
    format!("{width}x{height}")
}

/// What is shown.
pub fn label(width: u32, height: u32) -> String {
    format!("{width} × {height}")
}
```

- [ ] **Step 5: Run the tests**

Run: `cargo test -p melibea-ruler`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add ruler/src
git commit -m "Give the ruler frozen frames and logical units"
```

---

### Task 3: Edge detection

**Files:**
- Create: `ruler/src/edges.rs`
- Modify: `ruler/src/main.rs` (add `mod edges;`)

**Interfaces:**
- Consumes: `image::Image`, `image::matches`.
- Produces: `edges::Bounds { x0: u32, y0: u32, x1: u32, y1: u32 }` (inclusive) with `width(&self) -> u32`, `height(&self) -> u32`; `edges::spacing(image: &Image, x: u32, y: u32, tolerance: u8) -> Bounds` (x0..x1 along the pointer's row, y0..y1 along its column); `edges::Region` with `bounds(&self) -> Bounds`, `contains(&self, x: u32, y: u32) -> bool`; `edges::region(image: &Image, x: u32, y: u32, tolerance: u8) -> Region`.

- [ ] **Step 1: Write the failing tests**

```rust
//! Finding edges in a frozen frame, in physical pixels.
//!
//! A pixel belongs if no channel differs from the reference — the pixel under the pointer —
//! by more than the tolerance. Comparing with the reference rather than with the neighbour is
//! what makes a gradient end somewhere.

#[cfg(test)]
mod tests {
    use super::*;

    const BG: [u8; 3] = [255, 255, 255];
    const FG: [u8; 3] = [40, 90, 200];

    /// 40×30 white, with a blue button at x 10..=29, y 5..=14, "text" inside it at x 15..=17,
    /// y 8..=10.
    fn button() -> Image {
        Image::from_rgb(40, 30, |x, y| {
            let in_button = (10..=29).contains(&x) && (5..=14).contains(&y);
            let in_text = (15..=17).contains(&x) && (8..=10).contains(&y);
            if in_text {
                [0, 0, 0]
            } else if in_button {
                FG
            } else {
                BG
            }
        })
    }

    #[test]
    fn the_box_is_the_element_not_the_text_inside_it() {
        let region = region(&button(), 12, 6, 30);
        assert_eq!(region.bounds(), Bounds { x0: 10, y0: 5, x1: 29, y1: 14 });
        assert_eq!(region.bounds().width(), 20);
        assert_eq!(region.bounds().height(), 10);
    }

    #[test]
    fn the_region_knows_what_it_covers() {
        let region = region(&button(), 12, 6, 30);
        assert!(region.contains(29, 14));
        assert!(!region.contains(16, 9), "the text is a hole in the region");
        assert!(!region.contains(5, 5), "outside the button");
    }

    #[test]
    fn a_region_reaching_the_frame_edge_stops_there() {
        let region = region(&button(), 0, 0, 30);
        assert_eq!(region.bounds(), Bounds { x0: 0, y0: 0, x1: 39, y1: 29 });
    }

    #[test]
    fn diagonal_neighbours_do_not_connect() {
        // A white pixel at (0,0) and one at (1,1), black elsewhere.
        let image = Image::from_rgb(2, 2, |x, y| if x == y { BG } else { [0, 0, 0] });
        assert_eq!(region(&image, 0, 0, 0).bounds(), Bounds { x0: 0, y0: 0, x1: 0, y1: 0 });
    }

    #[test]
    fn an_isolated_pixel_is_a_one_by_one_box() {
        let image = Image::from_rgb(5, 5, |x, y| if (x, y) == (2, 2) { FG } else { BG });
        let bounds = region(&image, 2, 2, 30).bounds();
        assert_eq!((bounds.width(), bounds.height()), (1, 1));
    }

    #[test]
    fn an_antialiased_edge_is_in_or_out_by_the_tolerance() {
        // Columns: 0..=4 at 100, column 5 at 125, 6..=9 at 200.
        let image = Image::from_rgb(10, 1, |x, _| match x {
            0..=4 => [100; 3],
            5 => [125; 3],
            _ => [200; 3],
        });
        assert_eq!(region(&image, 0, 0, 24).bounds().x1, 4);
        assert_eq!(region(&image, 0, 0, 25).bounds().x1, 5);
    }

    #[test]
    fn a_gradient_ends_where_it_leaves_the_tolerance() {
        // Each column one step brighter than the last.
        let image = Image::from_rgb(100, 1, |x, _| [x as u8; 3]);
        assert_eq!(region(&image, 10, 0, 30).bounds(), Bounds { x0: 0, y0: 0, x1: 40, y1: 0 });
    }

    #[test]
    fn spacing_is_the_gap_around_the_pointer() {
        // Inside the button, away from the text: row 6 spans the button, column 12 too.
        let bounds = spacing(&button(), 12, 6, 30);
        assert_eq!(bounds, Bounds { x0: 10, y0: 5, x1: 29, y1: 14 });
    }

    #[test]
    fn spacing_stops_at_the_first_different_pixel() {
        // On row 9 the text starts at x 15, so the ray to the right stops at 14.
        let bounds = spacing(&button(), 12, 9, 30);
        assert_eq!((bounds.x0, bounds.x1), (10, 14));
    }

    #[test]
    fn spacing_stops_at_the_frame_edge() {
        let bounds = spacing(&button(), 2, 2, 30);
        assert_eq!(bounds, Bounds { x0: 0, y0: 0, x1: 39, y1: 29 });
    }
}
```

Add `mod edges;` to `main.rs`.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p melibea-ruler edges`
Expected: compile errors for `Bounds`, `region`, `spacing`.

- [ ] **Step 3: Implement above the tests**

```rust
use crate::image::{Image, matches};

/// An inclusive rectangle in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounds {
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
}

impl Bounds {
    pub fn width(&self) -> u32 {
        self.x1 - self.x0 + 1
    }

    pub fn height(&self) -> u32 {
        self.y1 - self.y0 + 1
    }
}

/// The run of matching pixels along the pointer's row (`x0..=x1`) and column (`y0..=y1`).
pub fn spacing(image: &Image, x: u32, y: u32, tolerance: u8) -> Bounds {
    let reference = image.rgb(x, y);
    let same = |px: u32, py: u32| matches(image.rgb(px, py), reference, tolerance);

    let mut bounds = Bounds { x0: x, y0: y, x1: x, y1: y };
    while bounds.x0 > 0 && same(bounds.x0 - 1, y) {
        bounds.x0 -= 1;
    }
    while bounds.x1 + 1 < image.width() && same(bounds.x1 + 1, y) {
        bounds.x1 += 1;
    }
    while bounds.y0 > 0 && same(x, bounds.y0 - 1) {
        bounds.y0 -= 1;
    }
    while bounds.y1 + 1 < image.height() && same(x, bounds.y1 + 1) {
        bounds.y1 += 1;
    }
    bounds
}

/// The connected area of one colour, as filled from a point.
pub struct Region {
    bounds: Bounds,
    width: u32,
    mask: Vec<bool>,
}

impl Region {
    pub fn bounds(&self) -> Bounds {
        self.bounds
    }

    /// Whether a point is in the filled area itself, not merely inside its bounds.
    pub fn contains(&self, x: u32, y: u32) -> bool {
        x < self.width
            && self
                .mask
                .get(y as usize * self.width as usize + x as usize)
                .copied()
                .unwrap_or(false)
    }
}

/// Fills the 4-connected area matching the pixel at `(x, y)`.
///
/// Four rays would stop at the first letter of a button's label. The fill flows around the
/// label instead, so its bounding box is the whole button.
pub fn region(image: &Image, x: u32, y: u32, tolerance: u8) -> Region {
    let (width, height) = (image.width(), image.height());
    let index = |px: u32, py: u32| py as usize * width as usize + px as usize;
    let reference = image.rgb(x, y);

    let mut mask = vec![false; width as usize * height as usize];
    let mut bounds = Bounds { x0: x, y0: y, x1: x, y1: y };
    let mut stack = vec![(x, y)];
    mask[index(x, y)] = true;

    while let Some((px, py)) = stack.pop() {
        bounds.x0 = bounds.x0.min(px);
        bounds.y0 = bounds.y0.min(py);
        bounds.x1 = bounds.x1.max(px);
        bounds.y1 = bounds.y1.max(py);

        let neighbours = [
            (px > 0).then(|| (px - 1, py)),
            (px + 1 < width).then(|| (px + 1, py)),
            (py > 0).then(|| (px, py - 1)),
            (py + 1 < height).then(|| (px, py + 1)),
        ];
        for (nx, ny) in neighbours.into_iter().flatten() {
            let i = index(nx, ny);
            if !mask[i] && matches(image.rgb(nx, ny), reference, tolerance) {
                mask[i] = true;
                stack.push((nx, ny));
            }
        }
    }

    Region {
        bounds,
        width,
        mask,
    }
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p melibea-ruler`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add ruler/src
git commit -m "Find an element's box and the spacing around a point"
```

---

### Task 4: Drawing, text and the label

**Files:**
- Create: `ruler/src/draw.rs`, `ruler/src/text.rs`
- Modify: `ruler/src/main.rs` (add `mod draw; mod text;`)

**Interfaces:**
- Consumes: `edges::Bounds`.
- Produces: `draw::Rgba = [u8; 4]` (straight alpha, r g b a); constants `ACCENT`, `OUTLINE`, `LABEL_BG`, `LABEL_FG`, `LABEL_DIM`; `draw::Rect { x: i32, y: i32, w: i32, h: i32 }` with `union(self, other: Rect) -> Rect`; `draw::Canvas<'a>` with `new(data: &'a mut [u8], width: u32, height: u32, stride: u32) -> Canvas<'a>`, `blend(&mut self, x: i32, y: i32, color: Rgba)`, `fill(&mut self, rect: Rect, color: Rgba) -> Option<Rect>`, `size(&self) -> (i32, i32)`; `draw::crosshair(canvas, at: (i32, i32), px: i32) -> Rect`; `draw::spacing_guides(canvas, bounds: Bounds, at: (i32, i32), px: i32) -> Rect`; `draw::box_guides(canvas, bounds: Bounds, px: i32) -> Rect`; `draw::place_label(size: (i32, i32), preferred: (i32, i32), area: (i32, i32)) -> (i32, i32)`.
- Produces: `text::Text` with `load() -> Result<Text, String>`, `from_font(font: FontVec) -> Text`; `text::Label<'a> { main: &'a str, detail: &'a str }`; `text::label_size(&Text, &Label, px: i32) -> (i32, i32)`; `text::draw_label(&Text, &mut Canvas, &Label, at: (i32, i32), px: i32) -> Rect`.

`px` is one logical pixel in physical pixels for the output being drawn: `scale.round().max(1.0)` as `i32`.

- [ ] **Step 1: Write the failing tests**

`ruler/src/draw.rs`:

```rust
//! What the ruler draws over the frozen picture.
//!
//! Every line is horizontal or vertical and every fill a plain rectangle, so this is a handful
//! of clipped rectangle fills into the `Xrgb8888` buffer rather than a vector library. Each
//! drawing function returns the rectangle it touched, which is what gets damaged and, on the
//! next frame, restored.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edges::Bounds;

    fn canvas_data(width: u32, height: u32) -> Vec<u8> {
        // White, opaque: B G R X.
        [255u8, 255, 255, 255].repeat(width as usize * height as usize)
    }

    fn pixel(data: &[u8], width: u32, x: u32, y: u32) -> [u8; 3] {
        let i = (y * width * 4 + x * 4) as usize;
        [data[i + 2], data[i + 1], data[i]]
    }

    #[test]
    fn opaque_colour_is_written_in_bgr_order() {
        let mut data = canvas_data(2, 1);
        Canvas::new(&mut data, 2, 1, 8).blend(1, 0, [10, 20, 30, 255]);
        assert_eq!(pixel(&data, 2, 1, 0), [10, 20, 30]);
        assert_eq!(pixel(&data, 2, 0, 0), [255, 255, 255]);
    }

    #[test]
    fn half_alpha_mixes_with_the_picture() {
        let mut data = canvas_data(1, 1);
        Canvas::new(&mut data, 1, 1, 4).blend(0, 0, [0, 0, 0, 128]);
        // 255 * 127 / 255 = 127.
        assert_eq!(pixel(&data, 1, 0, 0), [127, 127, 127]);
    }

    #[test]
    fn drawing_off_the_canvas_is_ignored() {
        let mut data = canvas_data(1, 1);
        let mut canvas = Canvas::new(&mut data, 1, 1, 4);
        canvas.blend(-1, 0, [0, 0, 0, 255]);
        canvas.blend(0, 1, [0, 0, 0, 255]);
        assert_eq!(pixel(&data, 1, 0, 0), [255, 255, 255]);
    }

    #[test]
    fn fills_are_clipped_and_report_what_they_touched() {
        let mut data = canvas_data(10, 10);
        let mut canvas = Canvas::new(&mut data, 10, 10, 40);
        let touched = canvas.fill(Rect { x: 8, y: -2, w: 5, h: 4 }, [0, 0, 0, 255]);
        assert_eq!(touched, Some(Rect { x: 8, y: 0, w: 2, h: 2 }));
        assert_eq!(canvas.fill(Rect { x: 20, y: 0, w: 5, h: 5 }, [0, 0, 0, 255]), None);
    }

    #[test]
    fn rects_union() {
        let a = Rect { x: 0, y: 0, w: 2, h: 2 };
        let b = Rect { x: 5, y: 1, w: 1, h: 4 };
        assert_eq!(a.union(b), Rect { x: 0, y: 0, w: 6, h: 5 });
    }

    #[test]
    fn the_box_outline_leaves_the_measured_pixels_alone() {
        let mut data = canvas_data(30, 30);
        let bounds = Bounds { x0: 10, y0: 10, x1: 19, y1: 19 };
        let touched = box_guides(&mut Canvas::new(&mut data, 30, 30, 120), bounds, 1);
        for (x, y) in [(10, 10), (19, 19), (10, 19), (19, 10), (15, 15)] {
            assert_eq!(pixel(&data, 30, x, y), [255, 255, 255], "({x},{y}) was painted");
        }
        assert_ne!(pixel(&data, 30, 9, 9), [255, 255, 255], "the outline is just outside");
        assert!(touched.x <= 9 && touched.y <= 9);
        assert!(touched.x + touched.w >= 21 && touched.y + touched.h >= 21);
    }

    #[test]
    fn spacing_guides_cover_the_gap_and_report_it() {
        let mut data = canvas_data(30, 30);
        let bounds = Bounds { x0: 5, y0: 8, x1: 24, y1: 20 };
        let touched = spacing_guides(&mut Canvas::new(&mut data, 30, 30, 120), bounds, (12, 14), 1);
        assert_ne!(pixel(&data, 30, 5, 14), [255, 255, 255], "the row line reaches x0");
        assert_ne!(pixel(&data, 30, 12, 20), [255, 255, 255], "the column line reaches y1");
        assert!(touched.x <= 5 && touched.x + touched.w >= 25);
        assert!(touched.y <= 8 && touched.y + touched.h >= 21);
    }

    #[test]
    fn the_label_stays_on_the_output() {
        let area = (100, 50);
        assert_eq!(place_label((20, 10), (10, 10), area), (10, 10));
        assert_eq!(place_label((20, 10), (90, 45), area), (80, 40));
        assert_eq!(place_label((20, 10), (-5, -8), area), (0, 0));
    }
}
```

`ruler/src/text.rs`:

```rust
//! The label: the size in large digits, the mode and tolerance underneath.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::draw::Canvas;

    #[test]
    fn longer_text_is_wider() {
        let text = Text::load().expect("a monospace font via fc-match");
        let short = label_size(&text, &Label { main: "1 × 1", detail: "box" }, 1);
        let long = label_size(&text, &Label { main: "1234 × 5678", detail: "box" }, 1);
        assert!(long.0 > short.0);
        assert!(short.1 > 0);
    }

    #[test]
    fn the_label_paints_only_inside_what_it_reports() {
        let text = Text::load().expect("a monospace font via fc-match");
        let (width, height) = (200u32, 80u32);
        let mut data = [255u8, 255, 255, 255].repeat((width * height) as usize);
        let label = Label { main: "412 × 38", detail: "box · tol 30 · DP-1" };
        let touched = draw_label(
            &text,
            &mut Canvas::new(&mut data, width, height, width * 4),
            &label,
            (10, 10),
            1,
        );
        for y in 0..height {
            for x in 0..width {
                let i = ((y * width + x) * 4) as usize;
                let inside = (touched.x..touched.x + touched.w).contains(&(x as i32))
                    && (touched.y..touched.y + touched.h).contains(&(y as i32));
                if !inside {
                    assert_eq!(&data[i..i + 3], &[255, 255, 255], "({x},{y}) painted outside");
                }
            }
        }
        assert_eq!((touched.x, touched.y), (10, 10));
    }
}
```

Add `mod draw;` and `mod text;` to `main.rs`.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p melibea-ruler draw text`
Expected: compile errors for the missing items.

- [ ] **Step 3: Implement `draw.rs` above its tests**

```rust
use crate::edges::Bounds;

/// A colour as r, g, b, a with straight (not premultiplied) alpha.
pub type Rgba = [u8; 4];

/// The guides: one colour that reads on light and dark alike, thanks to the outline.
pub const ACCENT: Rgba = [0x4c, 0xd6, 0xff, 0xff];
pub const OUTLINE: Rgba = [0x00, 0x00, 0x00, 0xc8];
pub const LABEL_BG: Rgba = [0x10, 0x10, 0x14, 0xd8];
pub const LABEL_FG: Rgba = [0xf2, 0xf2, 0xf2, 0xff];
pub const LABEL_DIM: Rgba = [0xb0, 0xb0, 0xb8, 0xff];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn union(self, other: Rect) -> Rect {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let right = (self.x + self.w).max(other.x + other.w);
        let bottom = (self.y + self.h).max(other.y + other.h);
        Rect {
            x,
            y,
            w: right - x,
            h: bottom - y,
        }
    }
}

/// An `Xrgb8888` buffer to draw into.
pub struct Canvas<'a> {
    data: &'a mut [u8],
    width: i32,
    height: i32,
    stride: usize,
}

impl<'a> Canvas<'a> {
    #[allow(clippy::cast_possible_wrap, reason = "output sizes are far below i32::MAX")]
    pub fn new(data: &'a mut [u8], width: u32, height: u32, stride: u32) -> Canvas<'a> {
        Canvas {
            data,
            width: width as i32,
            height: height as i32,
            stride: stride as usize,
        }
    }

    pub fn size(&self) -> (i32, i32) {
        (self.width, self.height)
    }

    #[allow(clippy::cast_sign_loss, reason = "x and y are checked non-negative first")]
    pub fn blend(&mut self, x: i32, y: i32, color: Rgba) {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return;
        }
        let i = y as usize * self.stride + x as usize * 4;
        let [r, g, b, a] = color;
        let alpha = u16::from(a);
        // B, G, R in memory.
        for (offset, channel) in [(0, b), (1, g), (2, r)] {
            let under = u16::from(self.data[i + offset]);
            let mixed = (u16::from(channel) * alpha + under * (255 - alpha)) / 255;
            self.data[i + offset] = u8::try_from(mixed).unwrap_or(u8::MAX);
        }
    }

    /// Fills `rect`, clipped to the canvas; returns the part actually touched.
    pub fn fill(&mut self, rect: Rect, color: Rgba) -> Option<Rect> {
        let x0 = rect.x.max(0);
        let y0 = rect.y.max(0);
        let x1 = (rect.x + rect.w).min(self.width);
        let y1 = (rect.y + rect.h).min(self.height);
        if x0 >= x1 || y0 >= y1 {
            return None;
        }
        for y in y0..y1 {
            for x in x0..x1 {
                self.blend(x, y, color);
            }
        }
        Some(Rect {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        })
    }
}

/// An axis-aligned accent line `px` thick with a `px` outline on both sides.
fn line(canvas: &mut Canvas, from: (i32, i32), to: (i32, i32), px: i32) -> Rect {
    let x = from.0.min(to.0);
    let y = from.1.min(to.1);
    let core = Rect {
        x,
        y,
        w: (from.0 - to.0).abs() + px,
        h: (from.1 - to.1).abs() + px,
    };
    let outline = Rect {
        x: core.x - px,
        y: core.y - px,
        w: core.w + 2 * px,
        h: core.h + 2 * px,
    };
    canvas.fill(outline, OUTLINE);
    canvas.fill(core, ACCENT);
    outline
}

/// A small cross centred on the measured pixel, standing in for the hidden system cursor.
pub fn crosshair(canvas: &mut Canvas, at: (i32, i32), px: i32) -> Rect {
    let arm = 8 * px;
    let horizontal = line(canvas, (at.0 - arm, at.1), (at.0 + arm, at.1), px);
    let vertical = line(canvas, (at.0, at.1 - arm), (at.0, at.1 + arm), px);
    horizontal.union(vertical)
}

#[allow(clippy::cast_possible_wrap, reason = "output sizes are far below i32::MAX")]
pub fn spacing_guides(canvas: &mut Canvas, bounds: Bounds, at: (i32, i32), px: i32) -> Rect {
    let (x0, y0, x1, y1) = (bounds.x0 as i32, bounds.y0 as i32, bounds.x1 as i32, bounds.y1 as i32);
    let tick = 4 * px;
    let mut touched = line(canvas, (x0, at.1), (x1, at.1), px);
    touched = touched.union(line(canvas, (at.0, y0), (at.0, y1), px));
    touched = touched.union(line(canvas, (x0, at.1 - tick), (x0, at.1 + tick), px));
    touched = touched.union(line(canvas, (x1, at.1 - tick), (x1, at.1 + tick), px));
    touched = touched.union(line(canvas, (at.0 - tick, y0), (at.0 + tick, y0), px));
    touched.union(line(canvas, (at.0 - tick, y1), (at.0 + tick, y1), px))
}

/// The box's outline, drawn just outside it so its edge pixels stay visible.
#[allow(clippy::cast_possible_wrap, reason = "output sizes are far below i32::MAX")]
pub fn box_guides(canvas: &mut Canvas, bounds: Bounds, px: i32) -> Rect {
    let left = bounds.x0 as i32 - px;
    let top = bounds.y0 as i32 - px;
    let right = bounds.x1 as i32 + 1;
    let bottom = bounds.y1 as i32 + 1;
    let mut touched = line(canvas, (left, top), (right, top), px);
    touched = touched.union(line(canvas, (left, bottom), (right, bottom), px));
    touched = touched.union(line(canvas, (left, top), (left, bottom), px));
    touched.union(line(canvas, (right, top), (right, bottom), px))
}

/// Moves a label of `size` from `preferred` just enough to stay inside `area`.
pub fn place_label(size: (i32, i32), preferred: (i32, i32), area: (i32, i32)) -> (i32, i32) {
    (
        preferred.0.clamp(0, (area.0 - size.0).max(0)),
        preferred.1.clamp(0, (area.1 - size.1).max(0)),
    )
}
```

Note on `box_guides`: the outline of a `px`-thick line extends `px` beyond its core, so the core is placed one `px` outside the box (`left = x0 - px`, `right = x1 + 1`) and its own outline never reaches the box's pixels on the inside. If the test `the_box_outline_leaves_the_measured_pixels_alone` fails, the fix is to push the core out by one more `px`, not to drop the outline.

- [ ] **Step 4: Implement `text.rs` above its tests**

```rust
use std::process::Command;

use ab_glyph::{Font, FontVec, PxScale, ScaleFont, point};

use crate::draw::{Canvas, LABEL_BG, LABEL_DIM, LABEL_FG, Rect, Rgba};

/// Logical sizes; multiplied by `px` for each output.
const MAIN_SIZE: i32 = 13;
const DETAIL_SIZE: i32 = 10;
const PADDING: i32 = 6;
const GAP: i32 = 2;

pub struct Text {
    font: FontVec,
}

pub struct Label<'a> {
    pub main: &'a str,
    pub detail: &'a str,
}

impl Text {
    /// The system's monospace font, as fontconfig names it.
    pub fn load() -> Result<Text, String> {
        let output = Command::new("fc-match")
            .args(["-f", "%{file}", "monospace"])
            .output()
            .map_err(|e| format!("cannot run fc-match: {e}"))?;
        let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if path.is_empty() {
            return Err("fc-match found no monospace font".to_owned());
        }
        let bytes = std::fs::read(&path).map_err(|e| format!("cannot read {path}: {e}"))?;
        let font = FontVec::try_from_vec(bytes).map_err(|_| format!("{path} is not a usable font"))?;
        Ok(Text::from_font(font))
    }

    pub fn from_font(font: FontVec) -> Text {
        Text { font }
    }

    #[allow(clippy::cast_possible_truncation, reason = "glyph metrics are small")]
    fn line_size(&self, s: &str, size: f32) -> (i32, i32) {
        let scaled = self.font.as_scaled(PxScale::from(size));
        let width: f32 = s.chars().map(|c| scaled.h_advance(self.font.glyph_id(c))).sum();
        (width.ceil() as i32, scaled.height().ceil() as i32)
    }

    #[allow(clippy::cast_possible_truncation, reason = "glyph metrics are small")]
    fn draw_line(&self, canvas: &mut Canvas, s: &str, at: (i32, i32), size: f32, color: Rgba) {
        let scaled = self.font.as_scaled(PxScale::from(size));
        let mut caret = 0.0f32;
        for c in s.chars() {
            let id = self.font.glyph_id(c);
            let glyph = id.with_scale_and_position(size, point(caret, scaled.ascent()));
            caret += scaled.h_advance(id);
            if let Some(outlined) = self.font.outline_glyph(glyph) {
                let origin = outlined.px_bounds().min;
                outlined.draw(|gx, gy, coverage| {
                    let alpha = (f32::from(color[3]) * coverage).round() as u8;
                    canvas.blend(
                        at.0 + origin.x as i32 + gx as i32,
                        at.1 + origin.y as i32 + gy as i32,
                        [color[0], color[1], color[2], alpha],
                    );
                });
            }
        }
    }
}

#[allow(clippy::cast_precision_loss, reason = "font sizes are small integers")]
pub fn label_size(text: &Text, label: &Label, px: i32) -> (i32, i32) {
    let main = text.line_size(label.main, (MAIN_SIZE * px) as f32);
    let detail = text.line_size(label.detail, (DETAIL_SIZE * px) as f32);
    (
        main.0.max(detail.0) + 2 * PADDING * px,
        main.1 + GAP * px + detail.1 + 2 * PADDING * px,
    )
}

#[allow(clippy::cast_precision_loss, reason = "font sizes are small integers")]
pub fn draw_label(text: &Text, canvas: &mut Canvas, label: &Label, at: (i32, i32), px: i32) -> Rect {
    let (w, h) = label_size(text, label, px);
    let background = Rect { x: at.0, y: at.1, w, h };
    canvas.fill(background, LABEL_BG);

    let main_size = (MAIN_SIZE * px) as f32;
    let main_height = text.line_size(label.main, main_size).1;
    let inner = (at.0 + PADDING * px, at.1 + PADDING * px);
    text.draw_line(canvas, label.main, inner, main_size, LABEL_FG);
    text.draw_line(
        canvas,
        label.detail,
        (inner.0, inner.1 + main_height + GAP * px),
        (DETAIL_SIZE * px) as f32,
        LABEL_DIM,
    );
    background
}
```

- [ ] **Step 5: Run the tests**

Run: `cargo test -p melibea-ruler`
Expected: PASS. If `the_label_paints_only_inside_what_it_reports` fails, a glyph's bounds extend past the line height (descenders); widen `PADDING` or measure each glyph's `px_bounds` in `line_size` rather than painting outside the reported rectangle.

- [ ] **Step 6: Commit**

```bash
git add ruler/src
git commit -m "Draw the ruler's guides and label"
```

---

### Task 5: Single-instance lock

**Files:**
- Create: `ruler/src/lock.rs`
- Modify: `ruler/src/main.rs` (add `mod lock;`)

**Interfaces:**
- Produces: `lock::acquire() -> Result<Option<std::fs::File>, String>` (uses `$XDG_RUNTIME_DIR`); `lock::acquire_in(dir: &Path) -> Result<Option<File>, String>`. `Ok(None)` means another ruler holds it. The lock lasts while the returned `File` lives.

- [ ] **Step 1: Write the failing test**

```rust
//! One ruler at a time: a second one would capture the first one's guides.
//!
//! `flock` rather than a pid file, because the kernel drops the lock when the process dies, so
//! a crashed ruler never leaves the next one locked out.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_ruler_is_turned_away_until_the_first_is_gone() {
        let dir = std::env::temp_dir().join(format!("melibea-ruler-lock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let first = acquire_in(&dir).unwrap();
        assert!(first.is_some());
        assert!(acquire_in(&dir).unwrap().is_none());

        drop(first);
        assert!(acquire_in(&dir).unwrap().is_some());

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p melibea-ruler lock`
Expected: compile error, `acquire_in` not found.

- [ ] **Step 3: Implement**

```rust
use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;

pub fn acquire() -> Result<Option<File>, String> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").ok_or("XDG_RUNTIME_DIR is not set")?;
    acquire_in(Path::new(&dir))
}

pub fn acquire_in(dir: &Path) -> Result<Option<File>, String> {
    let path = dir.join("melibea-ruler.lock");
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(e)) => Err(format!("cannot lock {}: {e}", path.display())),
    }
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p melibea-ruler`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add ruler/src
git commit -m "Keep the ruler to one instance"
```

---

### Task 6: Capturing every output, and the headless mode

**Files:**
- Create: `ruler/src/capture.rs`, `ruler/tests/probes/box.py`, `ruler/tests/nest.sh`
- Modify: `ruler/src/main.rs`

**Interfaces:**
- Consumes: `image::Image`, `units::*`, `edges::{spacing, region, Bounds}`, `cli::{Command, Mode}`.
- Produces: `capture::Capture { name: String, logical: (i32, i32), image: Image }` with `scale(&self) -> f64`; `capture::capture_all(conn: &wayland_client::Connection) -> Result<Vec<Capture>, String>`; in `main.rs`, `measure(capture: &Capture, mode: Mode, at: (u32, u32), tolerance: u8) -> (u32, u32)` returning logical width and height.

This task's checks are the first two exit criteria, run in a nested niri. There is no unit test for screencopy; the nest is the test.

- [ ] **Step 1: Write the nested probe `ruler/tests/probes/box.py`**

```python
# A green window with a magenta box of exactly 300x200 logical pixels centred in it.
# Used by nest.sh: measuring anywhere inside the box must give 300x200 at any scale.
import gi
gi.require_version("Gtk", "4.0")
from gi.repository import Gtk, Gdk

css = b"""
window { background: #00ff00; }
.target { background: #ff00ff; }
"""

def on_activate(app):
    provider = Gtk.CssProvider()
    provider.load_from_data(css)
    Gtk.StyleContext.add_provider_for_display(
        Gdk.Display.get_default(), provider, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION)
    box = Gtk.Box()
    box.add_css_class("target")
    box.set_size_request(300, 200)
    box.set_halign(Gtk.Align.CENTER)
    box.set_valign(Gtk.Align.CENTER)
    window = Gtk.ApplicationWindow(application=app, title="RULER-PROBE")
    window.set_child(box)
    window.present()

app = Gtk.Application(application_id="org.melibea.RulerProbe")
app.connect("activate", on_activate)
app.run(None)
```

- [ ] **Step 2: Write `ruler/tests/nest.sh`**

```sh
#!/bin/sh
# Exit-criteria checks for melibea-ruler in a nested niri.
#
# usage: ruler/tests/nest.sh SCALE
#
# Starts a nested niri at the given output scale, opens the probe window, finds the window's
# centre over niri's IPC, and measures there with the headless mode. Prints the measurement and
# PASS or FAIL. The nested window briefly takes focus on the host session.
set -eu

scale=${1:?usage: nest.sh SCALE}
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ruler=${RULER:-$here/../../target/debug/melibea-ruler}
work=$(mktemp -d)
trap 'kill "$niri_pid" 2>/dev/null || true; rm -rf "$work"' EXIT

cat > "$work/niri.kdl" <<EOF
output "winit" { scale $scale; }
layout { gaps 0; focus-ring { off; } border { off; } }
prefer-no-csd
EOF

niri -c "$work/niri.kdl" > "$work/niri.log" 2>&1 &
niri_pid=$!
sleep 3
display=$(grep -oE 'Wayland socket: wayland-[0-9]+' "$work/niri.log" | grep -oE 'wayland-[0-9]+')
socket=$(grep -oE '/run/user/[0-9]+/niri\.wayland-[0-9]+\.[0-9]+\.sock' "$work/niri.log" | head -n 1)

WAYLAND_DISPLAY=$display GDK_BACKEND=wayland python3 "$here/probes/box.py" &
sleep 3

centre=$(NIRI_SOCKET=$socket niri msg --json windows | python3 -c '
import json, sys
w = [w for w in json.load(sys.stdin) if w["title"] == "RULER-PROBE"][0]["layout"]
tx, ty = w["tile_pos_in_workspace_view"]
ox, oy = w["window_offset_in_tile"]
ww, wh = w["window_size"]
print(f"{tx + ox + ww / 2},{ty + oy + wh / 2}")
')

for mode in box spacing; do
    result=$(WAYLAND_DISPLAY=$display "$ruler" --measure "$mode" --at "$centre" --output winit)
    if [ "$result" = 300x200 ]; then verdict=PASS; else verdict=FAIL; fi
    echo "scale $scale $mode at $centre: $result $verdict"
done
```

(`spacing` from the centre of a solid box also spans the whole box, so both modes must give `300x200`.)

- [ ] **Step 3: Implement `capture.rs`**

```rust
//! Capturing every output once, through `wlr-screencopy`.
//!
//! This is the only code that speaks the capture protocol. It runs before anything is shown,
//! so nothing of the ruler's own is ever in a capture.

use smithay_client_toolkit::{
    delegate_output, delegate_registry, delegate_shm,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shm::{
        Shm, ShmHandler,
        slot::{Buffer, SlotPool},
    },
};
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle, WEnum,
    globals::registry_queue_init,
    protocol::{wl_output::WlOutput, wl_shm},
};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
};

use crate::image::Image;
use crate::units;

pub struct Capture {
    pub name: String,
    pub logical: (i32, i32),
    pub image: Image,
}

impl Capture {
    pub fn scale(&self) -> f64 {
        units::scale(self.image.width(), self.logical.0)
    }
}

struct Frame {
    name: String,
    logical: (i32, i32),
    frame: ZwlrScreencopyFrameV1,
    /// width, height, stride, format of the shm buffer the compositor asked for.
    shm: Option<(i32, i32, i32, wl_shm::Format)>,
    /// Every buffer option has been described and a copy can be requested.
    described: bool,
    buffer: Option<Buffer>,
    y_invert: bool,
    result: Option<Result<(), String>>,
}

struct CaptureState {
    registry_state: RegistryState,
    output_state: OutputState,
    shm: Shm,
    frames: Vec<Frame>,
}

pub fn capture_all(conn: &Connection) -> Result<Vec<Capture>, String> {
    let (globals, mut queue) =
        registry_queue_init(conn).map_err(|e| format!("cannot read the compositor's globals: {e}"))?;
    let qh = queue.handle();
    let manager: ZwlrScreencopyManagerV1 = globals
        .bind(&qh, 1..=3, ())
        .map_err(|_| "the compositor does not offer wlr-screencopy".to_owned())?;
    let shm = Shm::bind(&globals, &qh).map_err(|_| "the compositor does not offer wl_shm".to_owned())?;

    let mut state = CaptureState {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        shm,
        frames: Vec::new(),
    };
    let wayland = |e: wayland_client::DispatchError| format!("Wayland error while capturing: {e}");
    // The first round trip announces the outputs, the second their names and sizes.
    queue.roundtrip(&mut state).map_err(wayland)?;
    queue.roundtrip(&mut state).map_err(wayland)?;

    let outputs: Vec<(WlOutput, String, (i32, i32))> = state
        .output_state
        .outputs()
        .filter_map(|output| {
            let info = state.output_state.info(&output)?;
            Some((output, info.name?, info.logical_size?))
        })
        .collect();
    if outputs.is_empty() {
        return Err("there are no outputs to capture".to_owned());
    }

    for (index, (output, name, logical)) in outputs.into_iter().enumerate() {
        // 0: without the cursor.
        let frame = manager.capture_output(0, &output, &qh, index);
        state.frames.push(Frame {
            name,
            logical,
            frame,
            shm: None,
            described: false,
            buffer: None,
            y_invert: false,
            result: None,
        });
    }

    let mut pool = SlotPool::new(4096, &state.shm).map_err(|e| format!("cannot create a shm pool: {e}"))?;
    while state.frames.iter().any(|frame| frame.result.is_none()) {
        queue.blocking_dispatch(&mut state).map_err(wayland)?;
        for frame in &mut state.frames {
            if frame.result.is_some() || frame.buffer.is_some() || !frame.described {
                continue;
            }
            let Some((width, height, stride, format)) = frame.shm else {
                frame.result = Some(Err("the compositor offered no buffer format the ruler reads".to_owned()));
                continue;
            };
            let (buffer, _) = pool
                .create_buffer(width, height, stride, format)
                .map_err(|e| format!("cannot allocate a capture buffer: {e}"))?;
            frame.frame.copy(buffer.wl_buffer());
            frame.buffer = Some(buffer);
        }
    }

    let mut captures = Vec::new();
    for frame in state.frames {
        frame.result.unwrap_or(Ok(())).map_err(|e| format!("capturing {}: {e}", frame.name))?;
        let (width, height, stride, _) = frame.shm.ok_or("a capture finished without a buffer")?;
        let buffer = frame.buffer.ok_or("a capture finished without a buffer")?;
        let pixels = buffer
            .canvas(&mut pool)
            .ok_or("a capture buffer is still in use")?
            .to_vec();
        let mut image = Image::new(
            u32::try_from(width).map_err(|_| "negative capture width")?,
            u32::try_from(height).map_err(|_| "negative capture height")?,
            u32::try_from(stride).map_err(|_| "negative capture stride")?,
            pixels,
        );
        if frame.y_invert {
            image.flip_vertical();
        }
        frame.frame.destroy();
        captures.push(Capture {
            name: frame.name,
            logical: frame.logical,
            image,
        });
    }
    Ok(captures)
}

impl Dispatch<ZwlrScreencopyManagerV1, ()> for CaptureState {
    fn event(
        _: &mut Self,
        _: &ZwlrScreencopyManagerV1,
        _: <ZwlrScreencopyManagerV1 as Proxy>::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, usize> for CaptureState {
    fn event(
        state: &mut Self,
        proxy: &ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwlr_screencopy_frame_v1::Event;
        let frame = &mut state.frames[*index];
        match event {
            Event::Buffer {
                format,
                width,
                height,
                stride,
            } => {
                if let WEnum::Value(format @ (wl_shm::Format::Xrgb8888 | wl_shm::Format::Argb8888)) = format {
                    if let (Ok(w), Ok(h), Ok(s)) = (i32::try_from(width), i32::try_from(height), i32::try_from(stride)) {
                        frame.shm.get_or_insert((w, h, s, format));
                    }
                }
                // Before version 3 there is no buffer_done: the one buffer event is all there is.
                if proxy.version() < 3 {
                    frame.described = true;
                }
            }
            Event::BufferDone => frame.described = true,
            Event::Flags { flags } => {
                if let WEnum::Value(flags) = flags {
                    frame.y_invert = flags.contains(zwlr_screencopy_frame_v1::Flags::YInvert);
                }
            }
            Event::Ready { .. } => frame.result = Some(Ok(())),
            Event::Failed => frame.result = Some(Err("the compositor refused the capture".to_owned())),
            _ => {}
        }
    }
}

impl OutputHandler for CaptureState {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {}
}

impl ShmHandler for CaptureState {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for CaptureState {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState];
}

delegate_output!(CaptureState);
delegate_shm!(CaptureState);
delegate_registry!(CaptureState);
```

- [ ] **Step 4: Wire the headless mode in `main.rs`**

Replace `main.rs` with:

```rust
//! `melibea-ruler`: a one-shot screen ruler for niri.
//!
//! See `docs/superpowers/specs/2026-10-08-melibea-ruler-design.md`.

mod capture;
mod cli;
mod draw;
mod edges;
mod image;
mod lock;
mod text;
mod units;

use std::process::ExitCode;

use capture::Capture;
use cli::{Command, Mode};
use wayland_client::Connection;

fn main() -> ExitCode {
    let command = match cli::parse(std::env::args().skip(1)) {
        Ok(command) => command,
        Err(message) => {
            eprintln!("melibea-ruler: {message}\n{}", cli::USAGE);
            return ExitCode::from(2);
        }
    };
    match command {
        Command::Measure {
            mode,
            at,
            output,
            tolerance,
        } => headless(mode, at, &output, tolerance),
        Command::Interactive { .. } => {
            eprintln!("melibea-ruler: the interactive ruler arrives in the next task");
            ExitCode::FAILURE
        }
    }
}

/// Measures at one point, prints the result and exits, showing nothing.
fn headless(mode: Mode, at: (f64, f64), output: &str, tolerance: u8) -> ExitCode {
    let result = Connection::connect_to_env()
        .map_err(|e| format!("cannot connect to the compositor: {e}"))
        .and_then(|conn| capture::capture_all(&conn))
        .and_then(|captures| {
            let capture = captures.iter().find(|c| c.name == output).ok_or_else(|| {
                let names: Vec<&str> = captures.iter().map(|c| c.name.as_str()).collect();
                format!("no output named {output:?}; there are {names:?}")
            })?;
            let scale = capture.scale();
            let point = (
                units::to_physical(at.0, scale, capture.image.width()),
                units::to_physical(at.1, scale, capture.image.height()),
            );
            let (width, height) = measure(capture, mode, point, tolerance);
            Ok(units::clipboard(width, height))
        });
    match result {
        Ok(line) => {
            println!("{line}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("melibea-ruler: {message}");
            ExitCode::FAILURE
        }
    }
}

/// The measurement at a physical point, in logical pixels.
fn measure(capture: &Capture, mode: Mode, at: (u32, u32), tolerance: u8) -> (u32, u32) {
    let bounds = match mode {
        Mode::Box => edges::region(&capture.image, at.0, at.1, tolerance).bounds(),
        Mode::Spacing => edges::spacing(&capture.image, at.0, at.1, tolerance),
    };
    let scale = capture.scale();
    (
        units::to_logical(bounds.width(), scale),
        units::to_logical(bounds.height(), scale),
    )
}
```

`draw`, `lock` and `text` are unused until Task 7; add `#![allow(dead_code)]` at the top of `main.rs` for this task only and remove it in Task 7.

- [ ] **Step 5: Build and run the nest at scale 1 and 1.5**

Run:
```bash
cargo build -p melibea-ruler && chmod +x ruler/tests/nest.sh \
  && ruler/tests/nest.sh 1 && ruler/tests/nest.sh 1.5
```
Expected: four lines, every one `300x200 PASS`. A `FAIL` at 1.5 only points at `units` or at the scale being read from the capture; a `FAIL` at 1 points at `capture` or `edges`.

- [ ] **Step 6: Check it does not depend on the daemon**

Run: `grep -rn "melibea.sock\|MELIBEA_SOCKET\|melibea::" ruler/src || echo "no reference to the daemon"`
Expected: `no reference to the daemon`. (The nest never runs `melibea.service`, so Step 5 already ran without it.)

- [ ] **Step 7: Commit**

```bash
git add ruler/
git commit -m "Capture every output and measure from the command line"
```

---

### Task 7: The interactive overlay

**Files:**
- Create: `ruler/src/overlay.rs`, `ruler/tests/probes/button.py`
- Modify: `ruler/src/main.rs`

**Interfaces:**
- Consumes: `capture::Capture`, `cli::Mode`, `edges::{region, spacing, Bounds, Region}`, `draw::*`, `text::*`, `units::*`, `lock::acquire`.
- Produces: `overlay::Outcome::{Copy(String), Cancel, Fail(String)}`; `overlay::run(conn: &Connection, captures: Vec<Capture>, tolerance: u8, text: Text) -> Outcome`.

- [ ] **Step 1: Implement `overlay.rs`**

```rust
//! The interactive ruler: one full-screen layer surface per output showing its frozen capture,
//! with the guides for the measurement under the pointer drawn on top.

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_keyboard, delegate_layer, delegate_output, delegate_pointer,
    delegate_registry, delegate_seat, delegate_shm, delegate_simple,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState, SimpleGlobal},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers},
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
    },
    shell::{
        WaylandSurface,
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
    },
    shm::{Shm, ShmHandler, slot::SlotPool},
};
use wayland_client::{
    Connection, Dispatch, QueueHandle,
    globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
};
use wayland_protocols::wp::viewporter::client::{
    wp_viewport::{self, WpViewport},
    wp_viewporter::WpViewporter,
};

use crate::capture::Capture;
use crate::cli::Mode;
use crate::draw::{self, Canvas, Rect};
use crate::edges::{self, Bounds, Region};
use crate::text::{self, Label, Text};
use crate::units;

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const TOLERANCE_STEP: u8 = 5;

pub enum Outcome {
    Copy(String),
    Cancel,
    Fail(String),
}

struct Surface {
    capture: Capture,
    layer: LayerSurface,
    viewport: WpViewport,
    configured: bool,
    frame_pending: bool,
    dirty: bool,
    /// What the last frame drew over the picture: damaged again on the next one.
    drawn: Option<Rect>,
}

struct Overlay {
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    compositor: CompositorState,
    shm: Shm,
    viewporter: SimpleGlobal<WpViewporter, 1>,
    pool: SlotPool,
    surfaces: Vec<Surface>,
    pointer: Option<wl_pointer::WlPointer>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    /// The surface under the pointer and the physical pixel it points at.
    hover: Option<(usize, (u32, u32))>,
    mode: Mode,
    tolerance: u8,
    /// The last filled region, kept while the pointer stays inside it.
    cached: Option<(usize, u8, Region)>,
    text: Text,
    outcome: Option<Outcome>,
    /// Set once the surfaces exist: from then on, any output change ends the ruler.
    ready: bool,
}

pub fn run(conn: &Connection, captures: Vec<Capture>, tolerance: u8, text: Text) -> Outcome {
    match setup_and_run(conn, captures, tolerance, text) {
        Ok(outcome) => outcome,
        Err(message) => Outcome::Fail(message),
    }
}

fn setup_and_run(conn: &Connection, captures: Vec<Capture>, tolerance: u8, text: Text) -> Result<Outcome, String> {
    let (globals, mut queue) =
        registry_queue_init(conn).map_err(|e| format!("cannot read the compositor's globals: {e}"))?;
    let qh = queue.handle();
    let wayland = |e: wayland_client::DispatchError| format!("Wayland error: {e}");

    let compositor = CompositorState::bind(&globals, &qh).map_err(|_| "no wl_compositor")?;
    let layer_shell = LayerShell::bind(&globals, &qh).map_err(|_| "no layer shell")?;
    let shm = Shm::bind(&globals, &qh).map_err(|_| "no wl_shm")?;
    let viewporter = SimpleGlobal::<WpViewporter, 1>::bind(&globals, &qh).map_err(|_| "no wp_viewporter")?;
    let largest = captures
        .iter()
        .map(|c| c.image.stride() as usize * c.image.height() as usize)
        .max()
        .unwrap_or(4096);
    let pool = SlotPool::new(largest * 2, &shm).map_err(|e| format!("cannot create a shm pool: {e}"))?;

    let mut state = Overlay {
        registry_state: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
        output_state: OutputState::new(&globals, &qh),
        compositor,
        shm,
        viewporter,
        pool,
        surfaces: Vec::new(),
        pointer: None,
        keyboard: None,
        hover: None,
        mode: Mode::Box,
        tolerance,
        cached: None,
        text,
        outcome: None,
        ready: false,
    };
    queue.roundtrip(&mut state).map_err(wayland)?;
    queue.roundtrip(&mut state).map_err(wayland)?;

    for capture in captures {
        let output = state
            .output_state
            .outputs()
            .find(|o| state.output_state.info(o).and_then(|i| i.name).as_deref() == Some(capture.name.as_str()))
            .ok_or_else(|| format!("output {} disappeared before the ruler opened", capture.name))?;
        let surface = state.compositor.create_surface(&qh);
        let viewport = state
            .viewporter
            .get()
            .map_err(|_| "no wp_viewporter")?
            .get_viewport(&surface, &qh, ());
        let layer = layer_shell.create_layer_surface(&qh, surface, Layer::Overlay, Some("melibea-ruler"), Some(&output));
        layer.set_anchor(Anchor::all());
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
        layer.set_size(0, 0);
        layer.commit();
        state.surfaces.push(Surface {
            capture,
            layer,
            viewport,
            configured: false,
            frame_pending: false,
            dirty: false,
            drawn: None,
        });
    }
    state.ready = true;

    while state.outcome.is_none() {
        queue.blocking_dispatch(&mut state).map_err(wayland)?;
    }
    Ok(state.outcome.take().unwrap_or(Outcome::Cancel))
}

impl Overlay {
    fn surface_index(&self, surface: &wl_surface::WlSurface) -> Option<usize> {
        self.surfaces.iter().position(|s| s.layer.wl_surface() == surface)
    }

    fn physical(&self, index: usize, position: (f64, f64)) -> (u32, u32) {
        let capture = &self.surfaces[index].capture;
        let scale = capture.scale();
        (
            units::to_physical(position.0, scale, capture.image.width()),
            units::to_physical(position.1, scale, capture.image.height()),
        )
    }

    fn bounds(&mut self, index: usize, at: (u32, u32)) -> Bounds {
        let image = &self.surfaces[index].capture.image;
        match self.mode {
            Mode::Spacing => edges::spacing(image, at.0, at.1, self.tolerance),
            Mode::Box => {
                if let Some((cached_index, cached_tolerance, region)) = &self.cached {
                    if *cached_index == index && *cached_tolerance == self.tolerance && region.contains(at.0, at.1) {
                        return region.bounds();
                    }
                }
                let region = edges::region(image, at.0, at.1, self.tolerance);
                let bounds = region.bounds();
                self.cached = Some((index, self.tolerance, region));
                bounds
            }
        }
    }

    /// The text to copy for what is under the pointer now.
    fn current_result(&mut self) -> Option<String> {
        let (index, at) = self.hover?;
        let bounds = self.bounds(index, at);
        let scale = self.surfaces[index].capture.scale();
        Some(units::clipboard(
            units::to_logical(bounds.width(), scale),
            units::to_logical(bounds.height(), scale),
        ))
    }

    #[allow(clippy::cast_possible_truncation, reason = "scales are small")]
    #[allow(clippy::cast_possible_wrap, reason = "output sizes are far below i32::MAX")]
    fn redraw(&mut self, index: usize, qh: &QueueHandle<Self>) {
        {
            let surface = &mut self.surfaces[index];
            if !surface.configured {
                return;
            }
            if surface.frame_pending {
                surface.dirty = true;
                return;
            }
        }

        let hover_here = self.hover.filter(|(i, _)| *i == index).map(|(_, at)| at);
        let bounds = hover_here.map(|at| self.bounds(index, at));

        let Self { pool, surfaces, text, mode, tolerance, .. } = self;
        let surface = &mut surfaces[index];
        let image = &surface.capture.image;
        let (width, height, stride) = (image.width(), image.height(), image.stride());
        let Ok((buffer, canvas)) = pool.create_buffer(width as i32, height as i32, stride as i32, wl_shm::Format::Xrgb8888) else {
            return;
        };
        canvas.copy_from_slice(&image.data()[..canvas.len()]);

        let mut drawn = None;
        if let (Some(at), Some(bounds)) = (hover_here, bounds) {
            let scale = surface.capture.scale();
            let px = scale.round().max(1.0) as i32;
            let mut c = Canvas::new(canvas, width, height, stride);
            let at_i = (at.0 as i32, at.1 as i32);
            let guides = match mode {
                Mode::Box => draw::box_guides(&mut c, bounds, px),
                Mode::Spacing => draw::spacing_guides(&mut c, bounds, at_i, px),
            };
            let cross = draw::crosshair(&mut c, at_i, px);
            let main = units::label(units::to_logical(bounds.width(), scale), units::to_logical(bounds.height(), scale));
            let detail = format!("{} · tol {} · {}", mode.name(), tolerance, surface.capture.name);
            let label = Label { main: &main, detail: &detail };
            let size = text::label_size(text, &label, px);
            let preferred = match mode {
                Mode::Box => (bounds.x0 as i32 - px, bounds.y0 as i32 - px - size.1 - 4 * px),
                Mode::Spacing => (at_i.0 + 12 * px, at_i.1 + 12 * px),
            };
            let place = draw::place_label(size, preferred, c.size());
            let label_rect = text::draw_label(text, &mut c, &label, place, px);
            drawn = Some(guides.union(cross).union(label_rect));
        }

        let wl_surface = surface.layer.wl_surface();
        let damage = match (surface.drawn, drawn) {
            (Some(old), Some(new)) => Some(old.union(new)),
            (old, new) => old.or(new),
        };
        match damage {
            Some(d) => wl_surface.damage_buffer(d.x, d.y, d.w, d.h),
            None => wl_surface.damage_buffer(0, 0, width as i32, height as i32),
        }
        surface.drawn = drawn;
        wl_surface.frame(qh, wl_surface.clone());
        surface.frame_pending = true;
        if buffer.attach_to(wl_surface).is_ok() {
            surface.layer.commit();
        }
    }

    fn finish(&mut self, outcome: Outcome) {
        if self.outcome.is_none() {
            self.outcome = Some(outcome);
        }
    }
}

impl LayerShellHandler for Overlay {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {
        self.finish(Outcome::Cancel);
    }

    fn configure(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        layer: &LayerSurface,
        _: LayerSurfaceConfigure,
        _: u32,
    ) {
        let Some(index) = self.surfaces.iter().position(|s| &s.layer == layer) else {
            return;
        };
        let surface = &mut self.surfaces[index];
        // The buffer is the capture at its physical size; the viewport presents it at the
        // output's logical size, so a scaled output shows its real pixels one to one.
        surface.viewport.set_destination(surface.capture.logical.0, surface.capture.logical.1);
        surface.configured = true;
        self.redraw(index, qh);
    }
}

impl CompositorHandler for Overlay {
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: i32) {}
    fn transform_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: wl_output::Transform) {}
    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
    fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}

    fn frame(&mut self, _: &Connection, qh: &QueueHandle<Self>, surface: &wl_surface::WlSurface, _: u32) {
        let Some(index) = self.surface_index(surface) else {
            return;
        };
        let s = &mut self.surfaces[index];
        s.frame_pending = false;
        if std::mem::take(&mut s.dirty) {
            self.redraw(index, qh);
        }
    }
}

impl PointerHandler for Overlay {
    fn pointer_frame(&mut self, _: &Connection, qh: &QueueHandle<Self>, pointer: &wl_pointer::WlPointer, events: &[PointerEvent]) {
        for event in events {
            let Some(index) = self.surface_index(&event.surface) else {
                continue;
            };
            match event.kind {
                PointerEventKind::Enter { serial } => {
                    // The ruler draws its own crosshair on the measured pixel.
                    pointer.set_cursor(serial, None, 0, 0);
                    self.hover = Some((index, self.physical(index, event.position)));
                    self.redraw(index, qh);
                }
                PointerEventKind::Leave { .. } => {
                    if self.hover.is_some_and(|(i, _)| i == index) {
                        self.hover = None;
                        self.redraw(index, qh);
                    }
                }
                PointerEventKind::Motion { .. } => {
                    self.hover = Some((index, self.physical(index, event.position)));
                    self.redraw(index, qh);
                }
                PointerEventKind::Press { button: BTN_LEFT, .. } => {
                    let outcome = self.current_result().map_or(Outcome::Cancel, Outcome::Copy);
                    self.finish(outcome);
                }
                PointerEventKind::Press { button: BTN_RIGHT, .. } => self.finish(Outcome::Cancel),
                PointerEventKind::Axis { vertical, .. } if vertical.absolute != 0.0 => {
                    // Wheel up widens the tolerance, wheel down narrows it.
                    self.tolerance = if vertical.absolute < 0.0 {
                        self.tolerance.saturating_add(TOLERANCE_STEP)
                    } else {
                        self.tolerance.saturating_sub(TOLERANCE_STEP)
                    };
                    self.redraw(index, qh);
                }
                _ => {}
            }
        }
    }
}

impl KeyboardHandler for Overlay {
    fn enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: &wl_surface::WlSurface, _: u32, _: &[u32], _: &[Keysym]) {}
    fn leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: &wl_surface::WlSurface, _: u32) {}
    fn release_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, _: KeyEvent) {}
    fn update_modifiers(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, _: Modifiers, _: u32) {}

    fn press_key(&mut self, _: &Connection, qh: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, event: KeyEvent) {
        match event.keysym {
            Keysym::Escape => self.finish(Outcome::Cancel),
            Keysym::space => {
                self.mode = self.mode.toggled();
                if let Some((index, _)) = self.hover {
                    self.redraw(index, qh);
                }
            }
            _ => {}
        }
    }
}

impl SeatHandler for Overlay {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(&mut self, _: &Connection, qh: &QueueHandle<Self>, seat: wl_seat::WlSeat, capability: Capability) {
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            self.keyboard = self.seat_state.get_keyboard(qh, &seat, None).ok();
        }
        if capability == Capability::Pointer && self.pointer.is_none() {
            self.pointer = self.seat_state.get_pointer(qh, &seat).ok();
        }
    }

    fn remove_capability(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat, capability: Capability) {
        if capability == Capability::Keyboard {
            if let Some(keyboard) = self.keyboard.take() {
                keyboard.release();
            }
        }
        if capability == Capability::Pointer {
            if let Some(pointer) = self.pointer.take() {
                pointer.release();
            }
        }
    }
}

impl OutputHandler for Overlay {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    // Measuring a capture of a layout that no longer exists gives false numbers.
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        if self.ready {
            eprintln!("melibea-ruler: an output was connected; closing");
            self.finish(Outcome::Cancel);
        }
    }
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        if self.ready {
            eprintln!("melibea-ruler: an output was disconnected; closing");
            self.finish(Outcome::Cancel);
        }
    }
}

impl ShmHandler for Overlay {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl AsMut<SimpleGlobal<WpViewporter, 1>> for Overlay {
    fn as_mut(&mut self) -> &mut SimpleGlobal<WpViewporter, 1> {
        &mut self.viewporter
    }
}

impl Dispatch<WpViewport, ()> for Overlay {
    fn event(_: &mut Self, _: &WpViewport, _: wp_viewport::Event, (): &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl ProvidesRegistryState for Overlay {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

delegate_compositor!(Overlay);
delegate_output!(Overlay);
delegate_shm!(Overlay);
delegate_seat!(Overlay);
delegate_keyboard!(Overlay);
delegate_pointer!(Overlay);
delegate_layer!(Overlay);
delegate_registry!(Overlay);
delegate_simple!(Overlay, WpViewporter, 1);
```

Notes for the implementer:
- `pointer.set_cursor(serial, None, 0, 0)` is `wl_pointer::set_cursor`; a `None` surface hides the cursor over this surface.
- `self.redraw` borrows `pool`, `surfaces` and `text` separately through the destructuring `let Self { .. } = self;` — keep it that way, or the borrow checker rejects calling `self.bounds` while a canvas is alive (`bounds` is computed before the destructuring for that reason).
- If `Anchor::all()` is not available in this SCTK version, use `Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT`.
- If `vertical.absolute` is named differently, check `smithay_client_toolkit::seat::pointer::AxisScroll` in the cached source; it holds the continuous value of the wheel.

- [ ] **Step 2: Wire the interactive mode in `main.rs`**

Remove `#![allow(dead_code)]`, add `mod overlay;`, and replace the `Command::Interactive { .. }` arm and add the helpers:

```rust
        Command::Interactive { tolerance } => interactive(tolerance),
```

```rust
/// Freezes the screen, lets one thing be measured, copies it, and exits.
fn interactive(tolerance: u8) -> ExitCode {
    let _lock = match lock::acquire() {
        Ok(Some(lock)) => lock,
        // Another ruler is open: a second one would capture its guides.
        Ok(None) => return ExitCode::SUCCESS,
        Err(message) => return fail(&message),
    };
    let conn = match Connection::connect_to_env() {
        Ok(conn) => conn,
        Err(e) => return fail(&format!("cannot connect to the compositor: {e}")),
    };
    // Everything that can fail before anything is shown, fails here.
    let captures = match capture::capture_all(&conn) {
        Ok(captures) => captures,
        Err(message) => return fail(&message),
    };
    let text = match text::Text::load() {
        Ok(text) => text,
        Err(message) => return fail(&message),
    };
    match overlay::run(&conn, captures, tolerance, text) {
        overlay::Outcome::Copy(result) => copy(&result),
        overlay::Outcome::Cancel => ExitCode::SUCCESS,
        overlay::Outcome::Fail(message) => fail(&message),
    }
}

fn copy(result: &str) -> ExitCode {
    use std::io::Write as _;
    let spawned = std::process::Command::new("wl-copy")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(result.as_bytes())?;
            }
            child.wait()
        });
    match spawned {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(status) => fail(&format!("wl-copy exited with {status}; {result} was not copied")),
        Err(e) => fail(&format!("cannot run wl-copy ({e}); {result} was not copied")),
    }
}

/// A failure launched from a bind is otherwise invisible, so it is also notified.
fn fail(message: &str) -> ExitCode {
    eprintln!("melibea-ruler: {message}");
    let _ = std::process::Command::new("notify-send")
        .args(["Regla", message])
        .status();
    ExitCode::FAILURE
}
```

- [ ] **Step 3: Build, lint and run the unit tests**

Run: `cargo clippy -p melibea-ruler --all-targets && cargo test -p melibea-ruler`
Expected: no warnings in `ruler/`, all tests pass.

- [ ] **Step 4: Write the button probe `ruler/tests/probes/button.py`**

```python
# A window with one GTK button labelled "Medir". Prints the button's allocated size once it is
# shown, so a person can measure the button with the interactive ruler and compare.
import gi
gi.require_version("Gtk", "4.0")
from gi.repository import Gtk, GLib

def on_activate(app):
    button = Gtk.Button(label="Medir")
    button.set_halign(Gtk.Align.CENTER)
    button.set_valign(Gtk.Align.CENTER)
    window = Gtk.ApplicationWindow(application=app, title="RULER-BUTTON")
    window.set_child(button)
    window.present()
    GLib.timeout_add(1000, lambda: print(f"button {button.get_width()}x{button.get_height()}", flush=True) or False)

app = Gtk.Application(application_id="org.melibea.RulerButton")
app.connect("activate", on_activate)
app.run(None)
```

- [ ] **Step 5: Check the interactive ruler by hand in a nest**

Run:
```bash
niri -c /dev/null &   # a nested niri with the default config; note its socket in the log
WAYLAND_DISPLAY=wayland-N python3 ruler/tests/probes/button.py &
WAYLAND_DISPLAY=wayland-N target/debug/melibea-ruler
```
In the nested window, check each of these and note the result:
- The picture is frozen and undimmed; the system cursor is gone and the crosshair sits on the pointer.
- Box mode over the button's background (not its text) outlines the button, with the label above it showing `W × H` and `box · tol 30 · winit`. W × H equals the size the probe printed, give or take the button's own 1-pixel border.
- Space switches to spacing; the four guides reach the button's edges.
- The wheel changes the `tol` shown, and the box changes on a soft edge.
- Left click exits, and `wl-paste` in the nest prints `WxH`.
- Running a second `melibea-ruler` while one is open does nothing and exits 0.
- Escape and right click exit without copying.

- [ ] **Step 6: Commit**

```bash
git add ruler/
git commit -m "Show the frozen screen and measure under the pointer"
```

---

### Task 8: Install, bind, and close M10's paperwork

**Files:**
- Modify: `scripts/install.sh`, `docs/superpowers/specs/2026-10-08-melibea-ruler-design.md`, `ROADMAP.md`, `README.md`
- Modify (outside the repo): `~/.config/niri/config.kdl`

- [ ] **Step 1: Install both binaries**

In `scripts/install.sh`, replace the single build-and-install with a loop. Change the build line to build the whole workspace, and wrap the install block:

```sh
(cd "$repo_root" && cargo build --release --locked --workspace)

mkdir -p -- "$prefix/bin"
for name in melibea melibea-ruler; do
    built=$repo_root/target/release/$name
    destination=$prefix/bin/$name
    temporary=$(mktemp "$prefix/bin/.$name.XXXXXX")
    trap 'rm -f -- "$temporary"' EXIT HUP INT TERM
    install -m 0755 "$built" "$temporary"
    mv -f -- "$temporary" "$destination"
    trap - EXIT HUP INT TERM
    cmp -s -- "$built" "$destination" || {
        echo "install: $destination does not match $built" >&2
        exit 1
    }
    echo ">> installed $destination" >&2
done
```

Update the header comment to say it installs `melibea` and `melibea-ruler`, and the usage text is unchanged.

Run: `scripts/install.sh`
Expected: `>> installed ~/.local/bin/melibea` and `>> installed ~/.local/bin/melibea-ruler`, then `target/` removed.

- [ ] **Step 2: Add the bind to the `tools` mode**

In `~/.config/niri/config.kdl`, inside `binding-mode "tools"`, after the `R` line:

```kdl
    // Regla: congela la pantalla, mide la caja o el hueco bajo el cursor, clic copia.
    M { spawn "/home/toni/.local/bin/melibea-ruler"; }
```

Run: `~/.local/lib/celestina/niri validate -c ~/.config/niri/config.kdl`
Expected: `config is valid`. niri reloads it on save; no restart.

- [ ] **Step 3: Record the tiny-skia deviation in the spec**

In the spec's licence table, delete the `tiny-skia` row, and under it add:

```markdown
`tiny-skia` was dropped while planning: every line the ruler draws is horizontal or vertical
and the label background is a plain rectangle, so a few clipped rectangle fills cover all of
it and are tested directly.
```

- [ ] **Step 4: Update the roadmap and README**

In `ROADMAP.md`, change the product sequence line to `-> M10 Screen ruler companion binary (built; awaiting real-session check)` and M10's status to:

```markdown
**Status:** built — `melibea-ruler`, installed and bound to `M` in the `tools` mode.
Design: [docs/superpowers/specs/2026-10-08-melibea-ruler-design.md](docs/superpowers/specs/2026-10-08-melibea-ruler-design.md).

Three exit criteria are met in a nested niri (the known-size box at scales 1 and 1.5, a GTK
button's bounds, and running with no `melibea.service`). The fourth, every output of the real
mixed-DPI, mixed-refresh session, waits for one measurement on each real output. Horizontal-
and vertical-only measurements were dropped from scope during design.
```

In `README.md`, add a short section naming the second binary, what it does, and the bind.

- [ ] **Step 5: Commit and push**

```bash
git add scripts/install.sh ROADMAP.md README.md docs/superpowers/specs/2026-10-08-melibea-ruler-design.md
git commit -m "Install the ruler and record M10's state"
git push origin main
```

- [ ] **Step 6: Hand the last check to the person**

Ask for one measurement on each real output (HDMI-A-1, DP-2, DP-1), of something whose logical size is known — for example a kitty window at its `default-column-width`, or a Noctalia bar's height — and compare with the label. When all three match, M10's status becomes complete.
