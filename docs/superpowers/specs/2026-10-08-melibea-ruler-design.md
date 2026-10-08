# melibea-ruler — design

Roadmap milestone: **M10 — Screen ruler as a companion binary**
([ROADMAP.md](../../../ROADMAP.md#m10--screen-ruler-as-a-companion-binary)).

Status: design approved 2026-10-08, not yet implemented.

## Purpose

Measure on-screen distances in logical pixels, with edges found automatically
rather than eyeballed, in the spirit of PowerToys' Screen Ruler.

Usage is a **one-shot measurement**: a bind freezes the screen, one thing is
measured, the result is copied to the clipboard, and the ruler exits. It is not
a toolbar that stays open.

Two measurements, nothing else:

- **Box** — the bounds of the element under the pointer (a button, a panel, an
  image), found by colour.
- **Spacing** — from the pointer, the distance to the nearest edge in each of
  the four directions: the gap the pointer is standing in.

Results are in **logical pixels**: the unit used in code and in niri's
configuration, and the same number for the same element on every output.

## Decisions settled before design

### Licence

The ruler is MIT, like Melibea, and lives in this repository. Every piece it
needs is permissive:

| Crate | Licence | Use |
|---|---|---|
| `smithay-client-toolkit` | MIT | Wayland client, layer-shell, seat, shm |
| `wayland-client`, `wayland-protocols`, `wayland-protocols-wlr` | MIT | `wlr-screencopy`, viewporter, fractional scale |
| `tiny-skia` | BSD-3-Clause | drawing lines and the label background |
| `ab_glyph` | MIT OR Apache-2.0 | rasterising the label text |

The roadmap required this question answered before any code; it is. The niri
fork already implements `wlr-screencopy`, so the ruler needs nothing new from
the compositor.

`tiny-skia` and `ab_glyph` are not in the local cargo cache, so the first build
needs network access.

### Approach: freeze, then measure

At launch the ruler captures every output once and shows the captures, frozen,
in full-screen overlay surfaces. All measuring happens on that frozen picture.

Rejected alternatives:

- **A live, transparent overlay.** Every capture would include the overlay's
  own guides and labels, and edge detection would find them. Avoiding that
  means hiding, capturing and re-showing on every pointer move: flicker and
  latency, for a one-shot tool that gains nothing from being live.
- **Inside the niri fork, like the magnifier.** The ruler has no business in
  the compositor, and it would add GPL code to the fork for no rendering
  benefit. The roadmap already decided against it.

## Architecture

### Repository layout

The root `Cargo.toml` becomes a workspace with two packages:

- `melibea` — unchanged: same code, same four dependencies (`regex`, `serde`,
  `serde_json`, `toml`). A second binary inside this package would have pulled
  the graphical dependencies into the daemon's dependency graph, which the
  roadmap rules out.
- `ruler/` — new package producing the `melibea-ruler` binary, with the same
  lint settings (`unsafe_code = "forbid"`, clippy `all` and `pedantic`).

### Modules

| Module | Responsibility | Depends on |
|---|---|---|
| `capture` | Capture each output once through `wlr-screencopy` into an in-memory image. The only code that speaks the capture protocol. | Wayland |
| `edges` | Given an image and a point, find edges: box and spacing. Pure functions. | nothing |
| `overlay` | Layer surfaces, pointer and keyboard, drawing the frozen picture and the guides. | Wayland, `tiny-skia`, `ab_glyph` |
| `main` | Argument parsing, the single-instance lock, wiring, exit. | all of the above |

`edges` takes an image type of its own (width, height, stride, pixels) rather
than a Wayland buffer, so it is tested without a compositor.

## Lifecycle of one measurement

1. **Single instance.** Take an `flock` on a file in `$XDG_RUNTIME_DIR`. If
   another ruler holds it, exit silently: a second ruler would capture the
   first one's guides. The kernel releases the lock if the process dies, so a
   crash never leaves the ruler blocked.
2. **Capture.** Capture every enabled output, without the cursor, before
   showing anything. If any capture fails, exit with an error (see
   [Errors](#errors)); never continue with some outputs missing.
3. **Show.** Create one layer surface per output on the `overlay` layer,
   anchored to all four edges, exclusive zone `-1`, keyboard interactivity
   `exclusive`. Each shows its own capture at native resolution: the buffer is
   the physical size of the capture, presented through viewporter and
   fractional scale, so DP-1 shows its real 3840×2160 pixels.
4. **Measure.** On pointer motion, recompute and redraw only on the output
   under the pointer.
5. **Finish.**
   - **Left click**: copy the measurement with `wl-copy`, then exit.
   - **Space**: switch between box and spacing.
   - **Wheel**: raise or lower the tolerance.
   - **Escape or right click**: exit without copying.

It is launched from a bind in the `tools` binding mode (for example `R`). It
does not use `melibea.service` or any shell.

## Edge detection

All detection runs on the frozen capture in **physical pixels**, so an edge is
found to the pixel even on a scaled output. Only the result is converted.

### Reference and tolerance

- The reference colour is the pixel under the pointer.
- A pixel matches if no channel (R, G or B) differs from the reference by more
  than the **tolerance**. Default 30 out of 255, as in PowerToys.
- The wheel adjusts the tolerance for the current run; it returns to the
  default on the next launch. `--tolerance N` sets the default.
- Pixels are compared with the reference, not with their neighbour, so a
  gradient eventually ends instead of running to the edge of the screen.

### Spacing

Walk from the pointer in each of the four directions until the first pixel
that does not match, or the edge of the output. The result is
horizontal = left + right and vertical = up + down.

### Box

Four rays are not enough: the label text inside a button would stop them at
the first letter. Instead:

- Flood-fill the connected region of pixels that match the reference
  (4-connected), starting at the pointer.
- The box is the bounding rectangle of that region. The fill flows around text
  and icons inside an element, so it gives the bounds of the whole element.
- The last filled region is kept. While the pointer stays inside it, the box is
  unchanged and nothing is recomputed. A new fill happens only when the
  pointer enters a different region. This matters because a large region, such
  as a window background on a 4K output, is about 8 million pixels.
- A region that reaches the edge of the output ends there.

### Conversion to logical pixels

Physical measurement ÷ output scale, rounded to the nearest integer. On DP-1
(scale 1.5) a 150-pixel-wide button measures 100.

### Known limits

- **Soft edges.** A diffuse shadow or a gradient with no sharp edge gives a
  box whose size depends on the tolerance; the wheel is there for this.
- **Elements without their own background.** Text drawn straight on the
  surrounding background has no box of its own: the fill escapes into the
  background.

## What is drawn

The frozen capture is shown **unchanged**: not dimmed, not tinted. A ruler for
edges and colours has to show the picture as it is. Everything the ruler draws
goes on top and covers as little as possible.

- **Cursor.** The system cursor is hidden. The ruler draws a thin crosshair,
  1 logical pixel wide, centred exactly on the measured pixel, because an arrow
  would cover the very spot being measured.
- **Visibility.** Every line uses one fixed accent colour with a 1-pixel dark
  outline, so it reads on white and on black alike. There is no theming: the
  ruler depends on no shell.
- **Spacing mode.** Four lines from the crosshair to the edge found in each
  direction, each ending in a short perpendicular tick.
- **Box mode.** The outline of the rectangle found, drawn **outside** the
  region so its edge pixels stay visible.
- **Label.** A dark, semi-transparent pill with the size in Noto Sans Mono at
  about 13 logical pixels, for example `412 × 38`. Below it, smaller, the mode,
  tolerance and output: `box · tol 30 · DP-1`. In box mode the label sits
  outside the box at its top-left corner, moved inwards when it would leave the
  output; in spacing mode it sits next to the crosshair. The font is found with
  `fc-match monospace`.
- **On click.** Copy and exit at once, with no notification. The clipboard
  gets `412x38`, without spaces, ready to paste into configuration or code.

## Several outputs

- One surface per enabled output, each with its own capture and scale.
- A measurement never crosses outputs: fills and rays stop at the edge of the
  output under the pointer.
- When the pointer moves to another output, the guides follow it, and the
  output it left goes back to the clean picture.
- If an output is connected or disconnected while measuring, the ruler exits:
  measuring a capture of a layout that no longer exists would give false
  numbers.
- An output switched off by the blackout tool is not an enabled output, so it
  is simply not captured.

## Errors

| Problem | Behaviour |
|---|---|
| The compositor lacks `wlr-screencopy` or refuses a capture | Exit before showing anything. Message on stderr and one notification, since a failure launched from a bind is otherwise invisible. |
| Another ruler is open | Exit silently (the lock in step 1). |
| `wl-copy` is missing or fails | Exit anyway, with a notification that the copy failed. |

No error leaves an overlay on screen or the keyboard grabbed.

## Performance

Each output's capture is kept once in memory. On pointer motion only the
damaged area is redrawn — the old guides and label, and the new ones — rather
than the 33 MB of a 4K frame.

## Headless mode

```text
melibea-ruler --measure box|spacing --at X,Y --output NAME [--tolerance N]
```

Captures that output, measures at logical point `X,Y` in output-local
coordinates, prints the result in the clipboard format (`412x38`) and exits,
showing nothing. It serves scripts, and above all it makes the exit criteria
checkable without a person in front of the screen.

## Verification

### Unit tests (`edges`, no Wayland)

Synthetic images in memory, with expected results written by hand:

- A rectangle with "text" pixels inside: the box is the rectangle, not the
  text.
- An anti-aliased edge, with tolerance just below and just above the edge's
  difference.
- A region touching the output edge: the box and rays stop there.
- A gradient: the fill stops where the difference from the reference passes
  the tolerance.
- A single isolated pixel: a 1×1 box.
- Logical conversion at scales 1, 1.5 and 2, including rounding.

### Roadmap exit criteria

| Criterion | How it is shown |
|---|---|
| A measurement matches a known-size window to the pixel. | In a nested niri, a test window drawing a solid box of known size; the headless mode must return exactly that size. |
| Edge detection finds a real UI element's bounds from a point inside it. | In the nest, a GTK button with a text label; the box must equal the size GTK allocates it, with the label inside not cutting the box short. |
| It works on each output of a mixed-DPI, mixed-refresh session. | Nests at scale 1 and 1.5: the same box measures the same in logical pixels. The final check across the three real outputs is done by hand in the real session, since a nest cannot reproduce their refresh rates. |
| It runs with `melibea.service` stopped. | The ruler never opens Melibea's socket. Shown by running it in the nest, where the service never runs. |

## Out of scope

As in the roadmap, and from the design questions:

- Any dependency on the `melibea` daemon, Celestina, or another shell.
- Annotation, drawing, screenshot editing, colour picking and OCR.
- Horizontal-only and vertical-only measurements, and manual drag-a-rectangle
  measurement: not needed in practice.
- A toolbar or a ruler that stays open across several measurements.
- Physical-pixel results.
- Theming, persisted tolerance, and configurable colours.
