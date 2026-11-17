# Standby — design

Roadmap milestone: **M12 — Standby**
([ROADMAP.md](../../../ROADMAP.md#m12--standby)).

Status: proposed 2026-11-17, revised the same day after the author widened
the scope from minimized windows to a dedicated suspend shortcut. Awaiting the
decisions in [Open decisions](#open-decisions). No code yet.

## Purpose

Suspend an application the way a console suspends a game: its processes stop
completely, using no CPU, and resume exactly where they were.

Two ways in:

- **Suspend shortcut.** A bind suspends the focused window right where it is,
  visible, tiled or fullscreen. The window keeps its place and shows its last
  frame, blurred, with the application's identity drawn on top. The same bind,
  or focusing the window and pressing a resume bind, wakes it.
- **Standby after minimizing.** An opted-in application that stays minimized
  past a configured delay is suspended in the background.

The target experience, reached in stages, is a fullscreen game frozen on its
last frame, blurred, with the game's icon and name over it, and later a
console-like game application in the author's suite that owns that screen.

## Who owns what

This is the boundary the rest of the design follows. Each layer keeps the
authority it already has; nothing moves.

| Concern | Owner | Why there |
|---|---|---|
| The suspend and resume shortcuts | niri config, `spawn "melibea" "suspend"` | niri owns input; the bind just calls Melibea |
| Deciding whether a window may be suspended | **Melibea** | it is the deterministic policy owner, and already decides for minimized windows |
| Finding the processes behind a window | **Melibea**, from `/proc` and cgroups | the compositor knows a pid, not a process tree |
| Freezing and thawing processes | **systemd** user manager, asked by Melibea | cgroup v2 freezer; systemd keeps unit state coherent on stop and logout |
| Keeping the last frame on screen | **niri** (already does: a client that stops committing keeps its last buffer) | niri owns surfaces and rendering |
| Blurring and dimming that frame | **niri fork** | a render effect on a window's own texture, like the magnifier; a client cannot do it without capturing the screen every frame |
| Not sending input to a suspended window | **niri fork** | niri owns input routing; events sent to a frozen client only pile up in its socket |
| Reporting that a window is suspended | **niri fork** IPC (`is_suspended`) and **Melibea** protocol v3 | niri states the visual fact, Melibea the process fact |
| The card on top: icon, name, play time, "press to resume" | **the shell or the game application** | presentation; Melibea and niri never draw application identity |
| Game metadata: cover art, title, store id, play time | **the game application**, outside these repos | only the launcher knows what a process *is* as a game |
| Telling Melibea which unit a game runs in | **the game application**, through Melibea's protocol | see [Games under Xwayland](#games-under-xwayland) |

In one sentence: **niri makes it look and feel frozen, Melibea decides and
freezes it, systemd does the freezing, and the shell or game app explains it
to the user.**

## Freezing

The cgroup v2 freezer, through systemd:

```sh
systemctl --user freeze app-niri-firefox-4242.scope
systemctl --user thaw   app-niri-firefox-4242.scope
```

Rejected alternatives:

- **`SIGSTOP` / `SIGCONT`.** Stops one process, not its children (a game's
  helper processes, a browser's content processes), is visible to parents doing
  job control, and any stray `SIGCONT` undoes it.
- **Writing `cgroup.freeze` directly.** Works, but systemd would not know, so
  `systemctl stop`, logout and `FreezerState` would disagree with reality.
- **Freezing inside niri.** niri owns surfaces, not process policy.

Requirements: cgroup v2 and systemd ≥ 246, which already hold wherever niri
runs as a systemd service — the only case in which niri spawns applications
into their own `app-niri-*.scope`.

## The safety rule

Freezing the wrong cgroup can freeze the compositor, Melibea or the shell and
lock the session. A unit is frozen only when all of the following hold at that
moment, read from `/proc`, never cached:

1. The target resolves, through `/proc/<pid>/cgroup`, to a unit named
   `app-*.scope` or `app-*.service` under the user manager.
2. The unit contains neither niri's pid nor Melibea's.
3. Every window whose pid is in the unit is being suspended together. For the
   shortcut, that means the unit owns only the focused window, or the shortcut
   suspends all of that unit's windows at once (open decision 1). For standby
   after minimizing, every one of the unit's windows is minimized.
4. Every process in the unit descends from a pid that owns one of those
   windows, or from the root process a game application registered for it.
   This refuses a shell, or a still-visible terminal, that launched something
   without its own scope.
5. For standby after minimizing only: a `[[standby]]` rule opts the window in.
   The shortcut is an explicit request and needs no rule.

A refusal leaves the window untouched and running. The shortcut reports why
through Melibea's action result, and `melibea status` explains it.

## Suspend sequence

```text
bind ─▶ melibea suspend
          ├─ safety rule passes?  no ─▶ result "blocked: <reason>", nothing changes
          ├─ ledger += unit
          ├─ niri: set-window-suspended --id W true   (blur on, input off)
          └─ systemctl --user freeze UNIT
```

Visual first, freeze second: the blur covers the window before the client
stops, so there is no visible moment of a frozen window that still looks live
and silently drops input. If the freeze fails, Melibea clears the niri flag
and removes the ledger entry.

Resume runs in reverse: thaw, wait for systemd to report it thawed, then clear
niri's flag, so the window takes input only once its client can answer.

While suspended, niri:

- keeps rendering the window's last buffer, blurred and dimmed, with the blur
  radius and dim taken from a `suspend { }` config block;
- sends it no keyboard, pointer or touch events and does not give it keyboard
  focus, but still lets the user focus its tile, so resume can target it;
- keeps it in the layout and in `Windows` IPC, with `is_suspended: true`;
- does not unredirect it for direct scanout, since the blur needs composition.

## Standby after minimizing

Unchanged from the first draft, now as the second entry point:

```toml
[[standby]]
app_id = "^(firefox|code)$"
after = "2m"   # minimized this long before freezing; "0s" is immediate
```

Matched like `[[attention]]`: top to bottom, first match wins. With no rules,
nothing is suspended in the background. Melibea freezes when the delay
elapses, thaws before restoring or closing on its own action paths, and thaws
at once when it sees niri restore the window directly. A minimized window
needs no blur: it is not on screen.

## Games under Xwayland

The hardest case for the end goal. A game running through Proton or another X11
path is reported by niri with the pid of `xwayland-satellite`, not the game's.
Satellite lives in niri's own cgroup, so the safety rule refuses it, correctly:
freezing it would freeze every X11 application.

So for these games, Melibea cannot find the processes from the window alone.
The fix belongs to whoever launches the game:

- **The future game application** launches each game in its own unit
  (`systemd-run --user --scope --unit=app-game-<id>`) and registers it with
  Melibea over the protocol: "window W, or the next fullscreen window on output
  O, belongs to unit U". Melibea still applies rules 1, 2 and 4 to that unit.
- **Until then**, games launched through Steam run inside Steam's own unit.
  Suspending them would freeze the Steam client too. That is refused by default
  and could later become a deliberate opt-in.

Native Wayland games launched through niri `spawn` need nothing: their window
pid is their own and their scope is their own.

## Recovery

Frozen processes must never outlive the reason they were frozen.

- Melibea records each unit in `$XDG_RUNTIME_DIR/melibea/standby` before
  freezing it and removes it after thawing. It never thaws a unit it did not
  freeze.
- On start and after every niri reconnection it reconciles the ledger against
  a fresh snapshot and thaws anything no longer justified, also clearing niri's
  suspended flag.
- `melibea thaw-all` thaws every ledger unit; the service unit runs it from
  `ExecStopPost`, so a crash or stop of Melibea releases everything.
- If niri restarts, its suspended flags are gone with it; reconciliation thaws
  the units. A reboot clears everything: the ledger lives in the runtime
  directory.

## Protocol

Melibea protocol v3 (v1 and v2 payloads stay fixed):

- `suspend` and `resume` actions, optional window id, focused window by default,
  with the same semantic results as minimize (`applied`, `already_in_state`,
  `window_not_found`, `blocked` with a reason).
- `register_unit`, for launchers: binds a window, or the next fullscreen window
  on an output, to a unit they started.
- A `suspended` list in state: window id, app id, title, output, whether it is
  fullscreen, and whether it is also minimized. Ordered, revisioned and
  snapshot-first like the minimized list, so a shell or game application can
  draw its card from the first message and follow changes without races.

The card's position comes from niri's window geometry, which the shell already
reads; for a fullscreen window it is simply the output.

## Changes to the Niri fork

Now required, where the first draft needed none:

- A per-window `suspended` flag set by a new action
  `set-window-suspended --id W true|false`, also over IPC.
- Rendering a suspended window's texture through the existing blur passes, plus
  a dim, configured by `suspend { blur-radius dim }`.
- Input exclusion for suspended windows, and no direct scanout for them.
- `is_suspended` in the `Window` IPC record and its change event.

None of this touches process state; niri never freezes anything.

## Known effects

- Audio and video stop; network sessions may time out (SSH, multiplayer).
- D-Bus calls to a frozen client block until their timeout (25 s by default).
- A paste from a frozen client's selection hangs until it is thawed.
- Wayland events still queue for the frozen client; libwayland disconnects a
  client whose buffer fills. Input exclusion removes most of that traffic,
  but the spike must measure what remains.
- A suspended game keeps its RAM and VRAM, as on a console.

## Validation plan

A spike in a disposable nested niri before daemon integration, as in M3:

1. Freeze a native Wayland game or demo and a GTK window through their
   `app-niri-*` scopes; CPU time must stop and resume on thaw.
2. Hold them frozen ten minutes while generating compositor traffic (scale and
   keymap changes, clipboard copies elsewhere); they must survive the thaw.
3. Blur and input exclusion in the fork: a suspended fullscreen window shows its
   blurred last frame, takes no input, and resumes on the bind.
4. `SIGKILL` Melibea while a unit is frozen; `ExecStopPost` must thaw it and a
   restart must clear any niri flag left behind.
5. Every clause of the safety rule refuses: an application launched from a
   visible terminal, one browser window of two, `xwayland-satellite`, a Steam
   game, anything Celestina launched without a scope.

## Exit criteria

- The suspend bind freezes the focused eligible window, shows it blurred, and
  the resume bind wakes it with its state intact.
- Standby after minimizing works for opted-in applications only.
- With no binds and no `[[standby]]` rules, behavior and protocol v1/v2 output
  are unchanged byte for byte.
- No request can freeze niri, Melibea, the shell, `xwayland-satellite`, or a
  unit with a window that is not being suspended; every refusal is explained.
- No frozen unit survives Melibea stopping, crashing or restarting.

## Out of scope for these repositories

- The game application: library, cover art, play time, the console-style
  suspended screen. It is a protocol consumer like Celestina.
- Drawing application identity anywhere in niri or Melibea.
- Saving process state to disk, or resuming after a reboot.
- Battery or thermal policy that suspends without the user asking.

## Open decisions

1. **Shortcut on a multi-window application.** Suspend all of that unit's
   windows together, or refuse unless the focused window is its only one?
   Proposed: suspend all, each blurred, since they freeze together anyway.
2. **The card before the game application exists.** Celestina draws a minimal
   one (icon and name) from protocol v3, or nothing until the game app.
3. **Steam games.** Refuse, or allow freezing Steam's whole unit as an explicit
   opt-in.
4. **Milestone order.** M10 (the ruler) is planned and not started, and the
   roadmap keeps one active milestone.
