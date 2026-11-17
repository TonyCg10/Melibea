# Standby for minimized applications — design

Roadmap milestone: **M12 — Standby for minimized applications**
([ROADMAP.md](../../../ROADMAP.md#m12--standby-for-minimized-applications)).

Status: proposed 2026-11-17, awaiting the author's decisions in
[Open decisions](#open-decisions). No code yet.

## Purpose

A minimized window today leaves the layout but its process keeps running:
timers, animations, polling, JavaScript, background rendering. Standby extends
minimization the way a console's suspend does: once an application has been
minimized for a while, its processes are frozen, using no CPU at all, and they
resume exactly where they were the moment it is restored.

```text
visible ──minimize──▶ minimized ──after delay──▶ minimized + frozen
   ▲                     │                              │
   └──────restore────────┘◀────────thaw, then restore───┘
```

Restoring a frozen window is instant on screen: niri keeps the tile's last
committed buffer, so the old frame appears at once and the client starts
answering a few milliseconds later, after the thaw.

## What freezing means here

The **cgroup v2 freezer**, driven through systemd:

```sh
systemctl --user freeze app-niri-firefox-4242.scope
systemctl --user thaw   app-niri-firefox-4242.scope
```

Rejected alternatives:

- **`SIGSTOP` / `SIGCONT` on the window's pid.** Freezes one process, not its
  children (the shell, the build running in a terminal, a browser's content
  processes). It is visible to parents doing job control, and a stray
  `SIGCONT` from anyone undoes it.
- **Writing `cgroup.freeze` directly.** Works, but systemd would not know the
  unit is frozen. Going through systemd means `systemctl --user stop`, logout
  and `FreezerState` all stay coherent, and a frozen unit is visible to anyone
  with `systemctl --user list-units`.
- **Freezing inside niri.** niri owns surfaces, not process policy. Melibea is
  already the deterministic, configured policy owner for minimized windows.

Requirements: cgroup v2 and systemd ≥ 246 (the freeze verb). Both already
hold wherever niri runs as a systemd service, which is also the only case in
which niri puts spawned applications into their own `app-niri-*.scope`.

## Ownership

| Concern | Owner |
|---|---|
| Whether a window is minimized | niri (unchanged) |
| Which processes belong to a window | the kernel and systemd, read by Melibea |
| Whether and when to freeze | Melibea, from configuration |
| The freeze itself | systemd's user manager |
| How a frozen bubble looks | the shell (Celestina), from a state flag |

The Niri fork is **not modified** for the core of this milestone. niri already
reports `pid` for visible windows in `Windows` and for minimized ones in
`MinimizedWindowsChanged`, and its spawn path already creates one transient
scope per launched application.

## The safety rule

Freezing the wrong cgroup can freeze the shell, the compositor, or Melibea
itself, which would lock the session. So a unit is frozen only when **all** of
the following hold at that moment, checked from `/proc`, never cached:

1. The window's pid resolves, through `/proc/<pid>/cgroup`, to a unit named
   `app-*.scope` or `app-*.service` under the user manager.
2. The unit contains neither niri's pid nor Melibea's own.
3. No **visible** window has a pid inside that unit. One process with several
   windows (a browser, a single-instance terminal) is frozen only when every
   one of its windows is minimized.
4. Every process in the unit descends from a pid that owns one of the minimized
   windows being frozen. This refuses a unit whose root is something else: a
   shell that launched an application without its own scope, or a terminal
   that is still visible.
5. The minimized window matched a rule that opts it in (see
   [Configuration](#configuration)).

Rule 4 is what excludes `xwayland-satellite` (spawned into niri's own cgroup),
Celestina and anything it launched without a scope, and applications started
from a terminal that is still open.

If any check fails, the window simply stays minimized and running, and
`melibea status` explains which rule refused it.

## Configuration

Opt-in per application, never global, because freezing has consequences only
the user can weigh (see [Known effects](#known-effects)):

```toml
[[standby]]
app_id = "^(firefox|chromium|code)$"
after = "2m"          # minimized this long before freezing; "0s" is immediate

[[standby]]
app_id = "^kitty$"
title = "^btop"
after = "30s"
```

Same matching semantics as `[[attention]]`: top to bottom, first match wins,
both matchers must match when both are present. With no `[[standby]]` rules,
Melibea behaves exactly as today.

The delay exists because most minimizations are short. Freezing and thawing
are cheap, but a delay keeps a quick minimize/restore from paying even that,
and keeps background work like a finishing download from being interrupted
by a moment's minimization.

## Lifecycle

**Freeze.** When a minimized window's delay elapses, Melibea re-runs the
safety rule and runs `systemctl --user freeze UNIT`. A window that is restored
or closed before the delay elapses cancels its timer.

**Restore through Melibea** (Celestina's path). Thaw first, wait for
`systemctl` to return (it waits for the cgroup to report thawed), then send
niri's restore. The client is already running when its tile reappears.

**Restore through niri directly** (`niri msg action restore-window`, a niri
bind). niri restores the tile with its last buffer; Melibea sees `Restored` in
the event stream and thaws at once. The window is visible but unresponsive for
the few milliseconds in between.

**Close through Melibea.** Thaw first, then send close: a frozen client cannot
answer `xdg_toplevel.close`.

**Close through niri directly.** Not intercepted: the request reaches a frozen
client and waits there until something thaws it. Documented as a known gap;
Celestina always goes through Melibea.

**Another window of the same unit becomes visible.** A frozen unit cannot map
new windows, but before its delay elapses a running application may open one.
Rule 3 is checked again at freeze time, and if a frozen unit ever gains a
visible window anyway (one of its windows restored through niri directly), it
is thawed immediately.

**Clipboard.** If a frozen client owns the current selection, a paste in
another application would hang waiting for it. Melibea cannot see the
selection owner through niri IPC today, so the first version thaws nothing on
paste and lists this under known effects. See open decision 3.

## Recovery

Frozen processes must never outlive the reason they were frozen.

- Melibea writes every unit it freezes to a ledger,
  `$XDG_RUNTIME_DIR/melibea/standby`, before freezing it, and removes it after
  thawing it. It never thaws a unit it did not freeze.
- On start, and after every niri reconnection, Melibea reconciles the ledger
  against a fresh snapshot: any ledger unit whose windows are not all
  minimized, or whose rule no longer opts it in, is thawed.
- `melibea thaw-all` thaws every ledger unit. The systemd unit gains
  `ExecStopPost=%h/.local/bin/melibea thaw-all`, so a crash or a stop of
  Melibea releases everything it held.
- If niri itself dies, its clients are disconnected. A frozen client learns
  that only when thawed, so `ExecStopPost` and the next start's reconciliation
  matter here too. A reboot clears everything regardless: the ledger lives in
  the runtime directory.

## Protocol

Shells need to know a bubble is in standby to show it (dimmed, a moon or
snowflake). Protocol v1 and v2 state payloads are fixed, so this is a
**protocol v3**: each minimized entry gains

```json
{ "id": 42, "app_id": "firefox", "title": "…", "standby": true }
```

and a change in `standby` is an ordinary ordered `updated` change carrying a
new revision. v1 and v2 subscribers see exactly what they see today. No new
actions: restore and close already do the right thing.

## Known effects

Freezing is a real suspension, not a power hint, and these follow from it.
They are why the feature is opt-in per application.

- **Audio and video stop.** A frozen music player is silent.
- **Network sessions may drop.** An SSH session in a frozen terminal times out
  after the server's keep-alive; a frozen chat client misses its websocket.
- **D-Bus calls to a frozen client block** until their timeout (25 s by
  default) — notifications actions, MPRIS, portals.
- **Clipboard ownership**, as above.
- **Wayland events are queued for the frozen client.** Minimized windows get
  no frame callbacks or input, so traffic is small (output changes, keymap,
  selection offers), but libwayland disconnects a client whose buffer fills.
  This is the main thing the spike must measure, see below.

## Validation plan

A spike in a disposable nested niri before any daemon integration, in the
same style as M3:

1. Confirm that a nested niri running as a systemd service spawns into
   `app-niri-*.scope`, and that `systemctl --user freeze` on it drops the
   client's CPU to zero (`/proc/<pid>/stat` utime stops moving).
2. Freeze a minimized GTK window and a minimized Firefox, then generate
   compositor traffic for 10 minutes: output scale changes, keymap changes,
   clipboard copies in other clients. The frozen clients must survive the
   thaw. If libwayland disconnects them, the fix belongs in the Niri fork
   (raising the client buffer limit with `wl_display_set_default_max_buffer_size`,
   or not sending non-essential events to frozen clients) and this design is
   revised before M12 continues.
3. Restore through niri directly and through Melibea; time from request to the
   client's first new commit.
4. Kill Melibea with `SIGKILL` while a unit is frozen; `ExecStopPost` must thaw
   it. Restart Melibea with a stale ledger; reconciliation must thaw it.
5. Negative cases for every clause of the safety rule: an application launched
   from a visible terminal, a browser with one window minimized and one visible,
   `xwayland-satellite`, an application launched by Celestina without a scope.
   None may freeze.

## Exit criteria

- With no `[[standby]]` rules, behavior and protocol v1/v2 output are
  unchanged byte for byte.
- An opted-in application minimized past its delay uses no CPU, and restores
  through Celestina with its state intact.
- No configuration can freeze niri, Melibea, the shell, or a unit with a
  visible window; each refusal is explained by `melibea status`.
- No frozen unit survives Melibea stopping, crashing, or restarting.
- Unit tests cover the safety rule against constructed `/proc` and cgroup
  trees, timer cancellation, ledger reconciliation, and v3 projection.

## Out of scope

- Thermal or battery policy (freezing on its own when not minimized).
- Freezing windows that are merely on another workspace or off screen.
- Saving process state to disk (true hibernation of an application).
- Manual `freeze`/`thaw` commands for windows that are not minimized.
- Changes to the Niri fork, unless spike step 2 shows they are required.

## Open decisions

1. **Default delay** when a rule omits `after`. Proposed: rules must state it,
   no default.
2. **Bubble presentation** of standby in Celestina: dimmed, an icon, or
   nothing. Shell work, but it decides whether v3 is worth adding in the first
   version or can follow later.
3. **Clipboard.** Accept the known effect, or extend the Niri fork to report
   the selection owner's pid so Melibea can refuse to freeze it.
4. **Milestone order.** M10 (the ruler) is planned and not started; the roadmap
   keeps one active milestone. Which goes first?
