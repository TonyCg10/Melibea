# Melibea project

## Purpose

Melibea makes attention visible in niri without replacing niri's scrollable
layout. A configured column can use a compact inactive width, expand when it
receives focus, and recover its prior state deterministically.

The intended daily outcome is simple: a terminal may rest at 10% beside a 90%
editor, expand to 50% when focused, and return to 10% when focus goes back to
the editor.

## Current state

Melibea is an independent Rust project. The initial executable and pure domain
model exist, but niri IPC integration and configuration loading are not yet
implemented.

## Constraints

- Melibea must remain useful without Celestina or another shell.
- Decisions are deterministic and inspectable; there is no learned behavior.
- niri remains authoritative for surfaces, layout, focus, and rendering.
- External events and IPC data are fallible input and must not crash the daemon.
- Manual width and temporary focus overrides are distinct state.
- Floating windows, dialogs, popups, and fullscreen transitions are excluded
  until their behavior is deliberately specified.
- A future minimized window must leave the navigable layout completely. A
  storage workspace is not an acceptable final implementation.
- Celestina integration will consume a versioned contract; it will not own
  Melibea's policy or authoritative minimized-window state.

## Decisions

| Decision | Reason |
|---|---|
| Build Melibea as a separate Rust project | Its behavior belongs to niri and may serve shells other than Celestina. |
| Validate focus-responsive geometry first | It can prove daily value before modifying niri. |
| Start with real column resizing through IPC | It is the narrowest test of the interaction. |
| Treat visual clipping as a later native experiment | It crosses layout, rendering, input, and camera boundaries. |
| Keep one active milestone | The project is currently a focused solo experiment. |
| Add no plugin framework | Melibea has two product goals, not an open-ended extension platform. |

## Open questions

- Does resizing the client on every focus transition cause unacceptable text
  reflow or responsive-layout churn during real use?
- Can existing niri IPC actions apply transitions cleanly, or will an atomic
  compositor action become necessary?
- Should active width always be configured, or may Melibea restore the last
  manual width observed before contraction?

