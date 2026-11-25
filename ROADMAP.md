# Melibea roadmap

This roadmap orders Melibea's work by evidence. Only one implementation
milestone is active at a time; a deployed trial may remain under observation
after its code work pauses. Estimates describe likely solo-development effort,
not delivery commitments.

## Product sequence

```text
M0 Foundation (done)
  -> M1 Focus-responsive geometry experiment (deployed trial)
      -> M2 Reliable daily-use controller (after comfort evidence)
  -> M3 Native minimization design spike (complete, gate passed)
      -> M4 Usable native minimization (complete)
      -> M5 Versioned shell contract (complete)
      -> M6 Celestina bubble integration (complete)
      -> M7 Coordinated bubble motion (complete, one criterion deferred)
      -> M8 Optional expansions
      -> M12 Standby for games (built, in daily use)
  -> M9 Screen magnifier (complete, in the Niri fork)
      -> M10 Screen ruler companion binary (built; awaiting real-session check)
  -> M11 Binding modes (complete, in the Niri fork)
```

M9, M10 and M11 are session tools rather than surface-state work, so they sit
outside the M1-M8 line. M9 and M11 live in the Niri fork, which owns rendering
and input; M10 is a client and touches neither Niri nor the `melibea` daemon.

## Completed milestones

Their outcome, scope and evidence are in
[docs/history/roadmap-completed-milestones.md](docs/history/roadmap-completed-milestones.md).

- **M0 — Foundation** — done
- **M3 — Native minimization design spike** — complete
- **M4 — Usable native minimization** — complete; optional live multi-output confirmation deferred until a safe suitable backend is available
- **M5 — Versioned Melibea client contract** — complete
- **M6 — Celestina bubble integration** — complete
- **M7 — Coordinated bubble motion** — complete, with one exit criterion deferred
- **M9 — Screen magnifier in the compositor** — complete — in the Niri fork as `3050705`, installed and in daily use
- **M11 — Binding modes in the compositor** — complete — in the Niri fork as `fd3dba4` and `77f1f09`, installed and in daily use

## M1 — Focus-responsive geometry experiment

**Status:** deployed trial; implementation paused while comfort is observed

**Estimated effort:** several focused sessions to one week

### Outcome

In a real niri session, a configured terminal expands when focused and returns
to its compact width when focus returns to the editor:

```text
Editor focused:   Terminal 10% | Editor 90%
Terminal focused: Terminal 50% | Editor 90% ->
```

The widths are independent rather than redistributed to total 100%; niri's
scrollable strip may overflow the output normally.

### In

- Typed configuration for application matching and width behavior.
- Connection to the niri IPC event stream.
- Initial window and workspace snapshot followed by live events.
- In-memory window registry and current-focus state.
- Rules matching `app_id`; optional title matching only when required.
- Focused, unfocused, and `preserve` width policies.
- Dry-run mode that explains intended actions without changing layout.
- Live width actions through existing niri IPC.
- Unit tests using recorded or constructed event sequences.

### Out

- Minimization and bubbles.
- Celestina integration.
- Changes to niri source.
- Floating-window policy.
- Dialog, popup, transient, or fullscreen behavior beyond safe exclusion.
- Visual clipping that preserves the client's full width.
- Persistence across compositor restarts.
- A general plugin system.

### Work

1. Define configuration types, width parsing, rule priority, and validation.
   **Done.**
2. Define a pure transition engine for current and previous focus.
   **Done.**
3. Adapt niri snapshots and event-stream messages into the internal registry.
   **Done, including the live Unix-socket transport.**
4. Add dry-run diagnostics that identify the event, matched rule, previous
   state, selected policy, and intended niri action.
   **Done through the read-only `observe` command.**
5. Apply width actions while discarding decisions made obsolete by newer focus
   events.
   **Done with targeted window actions, generation-based coalescing, and a
   controlled rapid-focus cycle.**
6. Recover from IPC disconnection by rebuilding state from a fresh snapshot.
   **Done with clean reader cancellation, fresh event/action sockets, and a
   one-second reconnect delay.**
7. Exercise the 10% terminal / 90% editor workflow first in a controlled
   session and then during sustained daily work.
   **Controlled terminal cycle passed: 942 px -> 178 px at 10%, 50% action on
   focus, and 10% action on focus loss. Original geometry and focus were
   restored. The supervised daily trial started on 2026-08-14 through
   `melibea.service`; a bounded `status` health check now exposes snapshot and
   rule coverage. The author confirmed the live focus behavior works as
   intended on 2026-08-14; comfort over sustained use remains pending.**
8. Reload configuration safely during the sustained trial.
   **Done and live-validated: valid edits rebuild from a fresh snapshot;
   invalid edits preserve the last known-good policy and do not restart the
   service.**

### Exit

- Rapid focus changes converge on the latest focused column.
- A matching terminal follows its configured focused and unfocused widths.
- A `preserve` editor is never contracted by Melibea.
- Excluded or unmatched windows cause no mutation.
- Stopping or disconnecting Melibea leaves niri usable and reports a clear
  error.
- The interaction remains comfortable during a sustained real-work session.

### Gate to M2

The author confirms that automatic focus resizing saves effort without causing
unacceptable camera movement, terminal reflow, or responsive-layout churn. If
this fails, adjust or stop the experiment before adding more architecture.

## M2 — Reliable daily-use controller

**Status:** planned

**Estimated effort:** one to two weeks after M1

### Outcome

The geometry controller can remain enabled during normal niri use, preserves
manual intent, handles niri's column structures correctly, and recovers from
ordinary process and IPC failures.

### Intended scope

- Distinguish window focus, effective root window, and column focus.
- Treat tabbed or vertically stacked windows as one column where appropriate.
- Keep manual width separate from temporary focus overrides.
- Restore the correct active width after an inactive contraction.
- Make animations interruptible by relying on current state, not a command
  backlog.
- Reload valid configuration without losing the last known-good policy.
- Provide useful structured diagnostics and a health/status command.
- Characterize fullscreen, floating, transient, and interactive-resize events
  before admitting any of them into policy.

### Exit

- Restarting Melibea reconstructs state from niri without moving unmatched
  columns.
- Changing tabs inside one column does not cause spurious width oscillation.
- Manual and temporary widths remain distinguishable in tests and real use.
- Invalid configuration is rejected without replacing the working policy.
- A multi-hour daily session reveals no stale-event or reconnect corruption.

### Geometry-native follow-up

Decide from evidence whether existing IPC actions are sufficient. If two-step
resize transitions expose visible or inconsistent intermediate states, define
the narrowest atomic geometry action niri would need. This assessment is
independent from the native minimization spike.

## M8 — Optional expansions

**Status:** later, uncommitted

Candidates, admitted only by demonstrated daily value:

- Native visual contraction without resizing client content.
- Profiles or policies scoped by named workspace.
- Manual grouping of minimized bubbles.
- Urgency and attention cues that do not steal focus.
- Support for additional shell consumers.
- Upstreamable general niri actions discovered by Melibea.

Each candidate requires its own active milestone before implementation.

## M10 — Screen ruler as a companion binary

**Status:** built — `melibea-ruler`, installed by `scripts/install.sh` and bound to
`M` in the `tools` binding mode (Mod+H, then M). Awaiting the real-session check.
Design: [docs/superpowers/specs/2026-10-08-melibea-ruler-design.md](docs/superpowers/specs/2026-10-08-melibea-ruler-design.md).

Met in a nested niri: a known-size 300x200 box measured exactly at scales 1 and
1.5, in both box and spacing modes; the headless mode; running with no
`melibea.service`; the single-instance lock; Escape; and the frozen picture
shown unchanged. Awaiting the person's check: a GTK button's bounds from a
point inside it, the pointer-driven behaviour (crosshair, labels, wheel,
click-to-copy), one measurement on each of the three real outputs (mixed DPI
and refresh rate), and that pointer motion on DP-1 (3840×2160, scale 1.5,
165 Hz) feels smooth. Every redraw copies the whole frozen frame, and that copy
was never measured on that output; if it lags, the fallback is two buffers per
surface, restoring only the previous guides' rectangle. Horizontal-only and vertical-only measurements were dropped
from scope during design.

**Estimated effort:** one to two focused sessions after M9

### Outcome

Measuring distances on screen in pixels, with edges found automatically rather
than eyeballed, in the spirit of PowerToys' Screen Ruler.

### Why a second binary rather than a plugin

`Add no plugin framework` remains a settled decision: a loader, a stable API,
and versioning are infrastructure for many consumers, and this has one. This
milestone adds no extension point of any kind.

It is also not part of the `melibea` daemon. That daemon has no graphical code
at all — its whole dependency list is `regex`, `serde`, `serde_json` and
`toml` — while a ruler needs a layer-shell surface, drawing, pointer grabs, and
pixel reads. Those belong to a client, not to a surface-state controller.

So `melibea-ruler` is a separate program that happens to live in this
repository: one repository, two binaries, no framework, and no daemon carrying a
rendering stack it never uses.

### In

- A layer-shell overlay that measures between two points in logical pixels
  (dropped during design: it measures one point's surrounding box or gap).
- Edge detection by colour difference, with an adjustable tolerance, so a
  bounding box can be found from one point inside it.
- Bounding-box, horizontal-only, and vertical-only measurements (horizontal-only
  and vertical-only were dropped during design).
- Copying a measurement to the clipboard.
- Per-output correctness on mixed-DPI outputs, reporting logical pixels and
  naming the output.

### Out

- Any dependency on the `melibea` daemon. The ruler measures pixels; it has
  nothing to say about window state, and must run with the daemon stopped.
- Any dependency on Celestina or another shell.
- Annotation, drawing, or screenshot editing.
- Colour picking and OCR, which the session already answers: `PickColor` is a
  native Niri IPC action, and screencopy plus an OCR program covers text.

### Licence boundary to settle first

Reading pixels means speaking `wlr-screencopy`. Melibea deliberately
implements Niri's wire protocol directly rather than linking the GPL
`niri-ipc` crate, to stay MIT. The same question has to be answered for
whichever screencopy client library this uses **before** any code is written;
if the only practical binding is GPL, then either the ruler is not MIT, or it
does not live here.

### Exit

- A measurement matches a known-size window to the pixel.
- Edge detection finds a real UI element's bounds from a point inside it.
- It works on each output of a mixed-DPI, mixed-refresh session.
- It runs with `melibea.service` stopped.

## M12 — Standby

**Status:** built and in daily use since 2026-11-25; two checks remain (see
"Real session" below). Fork `f3219392`, Melibea `cbc4069`, both installed.

**Design:** [docs/superpowers/specs/2026-11-17-melibea-standby-design.md](docs/superpowers/specs/2026-11-17-melibea-standby-design.md)

**Plan:** [docs/superpowers/plans/2026-11-24-melibea-standby.md](docs/superpowers/plans/2026-11-24-melibea-standby.md)

**Estimated effort:** not estimated; the spike comes first, as M3 came before
M4, and sizes the rest.

### Spike

Passed on 2026-11-25, by hand with `busctl --user call … StartTransientUnit`
and `systemctl --user freeze|thaw`, on systemd 262 with cgroup v2:

- **Synthetic.** A `yes` process moved out of a service into
  `app-melibea-game-<pid>.scope`; freezing took 4 ms; its CPU ticks stayed at
  128 for 3 s and rose to 225 a second after the thaw.
- **Bottles, Victoria 3.** The game process (`argv[0]` `…\victoria3.exe`, `comm`
  `Main Thread`) moved alone out of the Bottles Flatpak unit, whose other 16
  processes, including `wineserver`, stayed. Frozen ten minutes: CPU usage of
  its scope unchanged (56.7 ms before and after), Bottles usable throughout,
  no warning in the user journal, the game alive and running after the thaw.
  **Finding:** the author's clicks on the frozen window were queued through
  xwayland-satellite and Xwayland and acted in the game after the thaw. The
  fork's input exclusion (plan task 3) is what protects the game's state, and
  the real-session check now tests it.
- **Steam, tModLoader (app 1281930), a native game under Xwayland.** Its window
  is app id `dotnet`, not `steam_app_1281930`: Steam names windows only for
  Proton games, so `[[game]]` rules gained `steam_app_id`. The five processes
  carrying `SteamAppId=1281930` sat in three Steam units (client, launcher and
  the game's container); they and one descendant moved into one game scope.
  Frozen two minutes: CPU unchanged, the Steam client usable, the game intact
  after the thaw, and Steam registered its exit.

The gate passed: moving a game's processes out of a Flatpak unit works.

### Real session

Checked by the author on 2026-11-25 with the installed fork and daemon, no
`games.toml`, bind `Mod+B` then `S` (the `bubbles` binding mode):

- Victoria 3 (Bottles, Proton-CachyOS) recognized without a rule: the X server
  named `victoria3.exe` (host pid) as the window's owner and its environment
  carries `WINESERVERSOCKET`. Only that process moved into
  `app-melibea-game-<pid>.scope` and froze (CPU flat); Bottles kept running.
  The window showed blurred and dimmed, clicks and typing on it never reached
  the game, and the second press resumed it intact.
- A Steam game suspended and resumed the same way, by the author's report.
- The bind on a window that is not a game did nothing.
- `systemctl --user restart melibea.service` with the game frozen: the unit's
  `ExecStopPost` ran `thaw-all`, which thawed the scope, cleared the flag and
  emptied the ledger.

Not yet checked live: restoring from Celestina a minimized game that Melibea
froze, and `standby_after`. The nested end-to-end run (task 13 step 1) covered
restart reconciliation, `kill -9` plus `thaw-all`, and the compositor exiting
with a game frozen.

### Outcome

A bind toggles the focused game like a console suspends one: Melibea moves the
game's processes into a systemd scope of their own and freezes only that with
the cgroup v2 freezer, and niri keeps the window's last frame on screen,
blurred and dimmed, without delivering input, until the next press. Bottles,
the Wine server of its prefix and the Steam client keep running. A game whose
rule names a delay is also suspended after staying minimized that long.

### In

- `toggle-suspend`, `suspend` and `resume` in Melibea, bound through niri
  `spawn`.
- Game recognition: Steam games by `steam_app_<N>` and `SteamAppId`, other
  games by `[[game]]` rules in `games.toml` naming their process.
- Separating the game into `app-melibea-game-<pid>.scope` before freezing.
- A safety rule, checked from `/proc`, that never freezes niri, Melibea, the
  shell, `xwayland-satellite`, or a process outside the game.
- A ledger, reconciliation and `thaw-all` from `ExecStopPost`.
- Protocol v3: suspend, resume and toggle actions and an ordered
  suspended-window list.
- In the Niri fork: a per-window suspended flag rendered blurred and dimmed,
  input and focus exclusion, and `is_suspended` in IPC.

### Out

- Suspending applications that are not games.
- The console-style game application, `register_unit` and any drawing of
  application identity; they consume protocol v3 from outside these
  repositories.
- Saving process state to disk, or resuming after a reboot.
- Suspending without the user asking, such as for battery or thermal policy.

### Exit

- The toggle bind freezes the focused game, shows it blurred, and the next
  press wakes it with its state intact, for a Bottles game and a Steam game.
- Only the game freezes: Bottles, the prefix's Wine server and the Steam
  client keep running.
- Standby after minimizing works for games whose rule names a delay only.
- With no bind pressed and no `standby_after`, behavior and protocol v1/v2
  output are unchanged byte for byte.
- No request can freeze niri, Melibea, the shell, `xwayland-satellite`, or a
  process outside the game; every refusal is explained.
- No frozen unit survives Melibea stopping, crashing or restarting.

## Where the Niri fork lives

M4, M7, M9 and M11, and the rendering and input part of M12, are code in the
Niri fork, not in this repository. The fork is GPL-3.0, like niri, and is kept
as its own repository rather than inside this MIT one: <https://github.com/TonyCg10/niri-melibea>, branch `main`, on
top of upstream niri `8ed0da4`.

Until 2026-09-23 its only remote was a directory under `/tmp` that had already
been wiped, so the whole fork existed in one working tree. Its history was a
shallow clone, completed from upstream niri before publishing on 2026-10-08.

## Global non-goals

- A new compositor or replacement for niri.
- A general plugin collection competing with Piri.
- AI classification, habit learning, or unpredictable automatic placement.
- Contexts, scenes, or automatic project inference.
- Hidden, off-screen, or ordinary workspaces presented as real minimization.
- Requiring Celestina or any graphical shell to run Melibea.
- Reimplementing niri's layout, rendering, or focus authority in an external
  daemon.
