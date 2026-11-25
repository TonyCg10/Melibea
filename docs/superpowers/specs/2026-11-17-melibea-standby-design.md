# Standby — design

Roadmap milestone: **M12 — Standby**
([ROADMAP.md](../../../ROADMAP.md#m12--standby)).

Status: proposed 2026-11-17, widened the same day from minimized windows to a
suspend shortcut, then narrowed on 2026-11-24 to games, with the author's
decisions recorded in [Decisions](#decisions). Implementation plan:
[2026-11-24-melibea-standby.md](../plans/2026-11-24-melibea-standby.md).

## Purpose

Suspend a game the way a console does: its processes stop completely, using no
CPU or GPU, and resume exactly where they were. Games are what cost resources
in this session; a browser, a chat client or the shell are not suspended.

Two ways in:

- **Suspend shortcut.** One bind toggles the focused game: the first press
  suspends it where it is, tiled or fullscreen, and the window keeps its place
  showing its last frame, blurred and dimmed; the next press on that window
  resumes it.
- **Standby after minimizing.** A game whose rule names a delay is suspended
  once it has stayed minimized that long, and resumed when it is restored.

The game keeps its RAM and VRAM while suspended, as on a console. What is freed
is CPU, GPU time and power.

## What the session looks like today

Measured on 2026-11-24 in the author's session, read-only:

- Victoria 3 runs through Bottles (Flatpak) with Proton-CachyOS. niri reports
  its window as app id `steam_proton`, title `Victoria 3`, with the pid of
  `xwayland-satellite`, which lives in `niri.service`.
- The game process (`argv[0]` `Z:\…\victoria3.exe`, `comm` `Main Thread`) lives
  in `app-flatpak-com.usebottles.bottles-*.scope`, beside the Bottles
  interface, `xdg-dbus-proxy`, the prefix's `wineserver` and Wine services.
- Steam (Flatpak) runs as two `app-flatpak-com.valvesoftware.Steam-*.scope`
  units; a game started by Steam runs under the client's unit.

So a game cannot be found from its window's pid, and its unit holds things
that must keep running. Both shape the design below.

## Who owns what

Each layer keeps the authority it already has; nothing moves.

| Concern | Owner | Why there |
|---|---|---|
| The toggle shortcut | niri config, `spawn "melibea" "toggle-suspend"` | niri owns input; the bind just calls Melibea |
| Recognizing a game and finding its processes | **Melibea**, from `/proc` and `games.toml` | it is the deterministic policy owner |
| Moving the game into a unit of its own, freezing and thawing it | **systemd** user manager, asked by Melibea | cgroup v2 freezer; systemd keeps unit state coherent on stop and logout |
| Keeping the last frame on screen, blurred and dimmed | **niri fork** | a render effect on the window's own texture; a client cannot do it without capturing the screen every frame |
| Not sending input to a suspended window | **niri fork** | niri owns input routing; events sent to a frozen client only pile up |
| Reporting that a window is suspended | **niri fork** IPC (`is_suspended`) and **Melibea** protocol v3 | niri states the visual fact, Melibea the process fact |
| A card with the game's identity on top | **the future game application** | presentation; Melibea and niri never draw application identity |

In one sentence: **niri makes it look and feel frozen, Melibea decides and
asks systemd to freeze exactly the game, and a later game application may
explain it to the user.**

## Recognizing a game

A window is a game when, checked in this order:

1. A rule in `$XDG_CONFIG_HOME/melibea/games.toml` matches it. Rules are
   resolved top to bottom, first match wins, like `[[attention]]`.
2. Otherwise, its app id is `steam_app_<N>` with `N` a decimal Steam app id.

```toml
[[game]]
app_id = "^steam_proton$"     # regex, optional
title = "^Victoria 3$"        # regex, optional; at least one of the two
process = "victoria3.exe"     # optional, see below
standby_after = "5m"          # optional: suspend after staying minimized this long
```

A game's processes are found from one `/proc` snapshot of the user's
processes:

- **By process name** when the rule names `process`: every process whose
  `argv[0]`, cut at the last `/` or `\`, equals it ignoring ASCII case. Wine
  sets `argv[0]` to the Windows path and `comm` to a thread name, so `comm` is
  not used.
- **By Steam app id** otherwise: every process whose environment holds
  `SteamAppId=<N>`, where `N` comes from the window's `steam_app_<N>` app id. A
  rule without `process` on a window that is not `steam_app_<N>` recognizes
  nothing.

The **game set** is those processes and all their descendants. The
**game's windows** are every window recognized as the same game (same rule and
process name, or same Steam app id) plus every window whose pid is in the game
set; they are suspended and resumed together.

A missing `games.toml` means no rules; Steam games still work. An invalid one
refuses suspension with the parse error and never affects resume or recovery.

## Separating the game

Freezing the unit a game runs in would also freeze Bottles, the prefix's
`wineserver`, or the whole Steam client. Melibea therefore asks systemd to move
the game set into a transient scope of its own and freezes only that:

```text
StartTransientUnit("app-melibea-game-<lowest root pid>.scope", "fail",
                   PIDs = <game set not already in an app-melibea-game-* scope>,
                   CollectMode = "inactive-or-failed")
```

through `busctl --user`, so Melibea gains no D-Bus dependency. Melibea then
re-reads `/proc` until every moved pid reports the new unit, bounded to two
seconds. A game process forked meanwhile is caught by one more discovery and
move; if game processes still remain outside, suspension is refused.

The scope outlives the suspension: the game stays in it until it exits, and a
later suspension reuses it. Melibea freezes and thaws only units named
`app-melibea-game-*.scope`, which only it creates.

Rejected alternatives:

- **Freezing the Flatpak unit.** Stops Bottles or the Steam client with the
  game, and with Bottles also every other game of the prefix.
- **`SIGSTOP` / `SIGCONT`.** Visible to parents doing job control, undone by
  any stray `SIGCONT`, and racy across a process tree.
- **Writing `cgroup.freeze` or `cgroup.procs` directly.** systemd would not
  know, so `systemctl stop`, logout and `FreezerState` would disagree.

## The safety rule

Freezing the wrong process can freeze the compositor, Melibea or the shell
and lock the session. A game is frozen only when all of these hold at that
moment, read from `/proc`, never cached:

1. The window is recognized as a game.
2. The game set is not empty.
3. Every process in the game set belongs to the user, is not Melibea itself,
   is not `niri`, `xwayland-satellite` or `Xwayland` by `comm`, and runs in a
   unit named `app-*.scope` or `app-*.service` directly under the user
   manager's `app.slice`. This refuses `niri.service` and `session.slice`.
4. After separating, every process in each target unit is in the game set.
5. Every target unit is named `app-melibea-game-<pid>.scope`.

A refusal leaves everything untouched and running, and is reported as a
`blocked` result with a reason (`not_a_game`, `game_not_running`,
`protected_process`, `game_escaped`, `invalid_rules`, `systemd`).

## Suspend sequence

```text
bind ─▶ melibea toggle-suspend
          ├─ focused window already suspended? ─▶ resume sequence
          ├─ safety rule passes?  no ─▶ "blocked: <reason>", nothing changes
          ├─ separate the game into app-melibea-game-<pid>.scope
          ├─ ledger += entry
          ├─ niri: set-window-suspended for each game window   (blur on, input off)
          └─ systemctl --user freeze UNIT…
```

Visual first, freeze second: the blur covers the window before the client
stops. If the freeze fails, Melibea thaws what it froze, clears the niri flags
and removes the ledger entry.

Resume runs in reverse: thaw, confirm `FreezerState=running`, then clear the
niri flags, so the window takes input only once its client can answer. Resume
reads the ledger, never `games.toml`.

While suspended, niri:

- renders the window's last buffer through its blur passes, dimmed, with the
  values of a `suspend { }` block;
- sends it no keyboard, pointer or touch events and gives it no keyboard
  focus, but still lets the user focus its tile and reports it focused in IPC,
  so the toggle can target it;
- keeps it in the layout and in `Windows` IPC with `is_suspended: true`;
- never scans it out directly: what it draws is its own texture, not the
  client's surface.

## Standby after minimizing

A minimized game whose rule names `standby_after` is suspended once niri has
reported it minimized for that long, with the niri flag set like any other
suspension. When the window leaves the minimized list, by Melibea's restore or
by niri directly, Melibea resumes it at once; until it does, the restored
window shows blurred rather than looking live and dropping input. Melibea's own
restore and close actions resume a suspended window before forwarding.

## Recovery

Frozen processes must never outlive the reason they were frozen.

- Melibea records each suspension in `$XDG_RUNTIME_DIR/melibea/standby.json`
  (units, windows, identity, whether niri was flagged) before freezing, and
  removes it after thawing.
- On start and after every niri reconnection it reconciles: a ledger entry
  with no suspended niri window left is thawed and dropped; a niri window
  flagged suspended with no ledger entry has its flag cleared; an entry whose
  units no longer exist is dropped.
- `melibea thaw-all` thaws every unit in the ledger and every loaded
  `app-melibea-game-*.scope`, so a lost or corrupt ledger still releases
  everything; it clears niri flags when niri answers. The service unit runs it
  from `ExecStopPost`, so a crash or stop of Melibea releases everything.
- A reboot clears everything: the ledger lives in the runtime directory.

## Protocol

Melibea protocol v3. Version 1 and 2 payloads stay byte-for-byte; only the
`supported_versions` list in an `incompatible_version` error grows to `[1, 2, 3]`.

- `suspend`, `resume` and `toggle_suspend` actions with an optional window id
  (the focused window by default). Their `action_result` uses operations
  `suspend` and `resume` and the usual statuses; `blocked` carries a `reason`.
- `list_suspended`, and on v3 subscriptions a `suspended` message carrying the
  complete ordered list (id, app id, title) with its own revision: sent after
  the first snapshot and on every change.

Registering a unit for a launcher (`register_unit`) waits for the game
application.

## Changes to the Niri fork

- A per-window suspended flag set by `set-window-suspended --id W true|false`,
  also over IPC, answering with the semantic window action result.
- Rendering a suspended window's texture through the existing blur, plus a
  dim, configured by `suspend { blur-passes blur-offset dim }`.
- Input and keyboard-focus exclusion for suspended windows.
- `is_suspended` in the `Window` and `MinimizedWindow` IPC records, carried by
  the usual change events.

None of this touches process state; niri never freezes anything.

## Known effects

- Audio stops; online sessions may time out.
- The X server keeps queueing events for a frozen X11 game; input exclusion
  removes most of them, and the spike measures what is left over ten minutes.
- D-Bus calls to a frozen client block until their timeout.
- A Bottles prefix's `wineserver` keeps running: it is shared by the prefix and
  is not in the game set.

## Validation plan

A spike before any code, then nested-niri checks per task:

1. Move a synthetic process out of a scope into `app-melibea-game-*` with
   `busctl --user`, freeze it, see its CPU time stop, thaw it.
2. Do the same with Victoria 3 out of the Bottles unit, frozen ten minutes
   while the author keeps using the session; Bottles keeps working; the game
   resumes intact.
3. Do the same with a Steam game found by `SteamAppId`; the Steam client keeps
   working and registers the game's exit afterwards.
4. Blur, dim, input and focus exclusion in a nested niri.
5. `SIGKILL` Melibea while a game is frozen; `ExecStopPost` thaws it and a
   restart clears the niri flag.

## Exit criteria

- The toggle bind freezes the focused game, shows it blurred, and the next
  press wakes it with its state intact, for a Bottles game and a Steam game.
- Only the game freezes: Bottles, the prefix's `wineserver` and the Steam
  client keep running.
- Standby after minimizing works for games whose rule names a delay, and only
  for them.
- With no bind pressed and no `standby_after`, behavior and protocol v1/v2
  output are unchanged byte for byte.
- No request can freeze niri, Melibea, the shell, `xwayland-satellite`, or a
  process outside the game; every refusal is explained.
- No frozen unit survives Melibea stopping, crashing or restarting.

## Out of scope

- Suspending applications that are not games.
- The game application: library, cover art, play time, the card over a
  suspended game, and `register_unit`.
- Drawing application identity anywhere in niri or Melibea.
- Saving process state to disk, or resuming after a reboot.
- Battery or thermal policy that suspends without the user asking.

## Decisions

Taken by the author on 2026-11-24:

1. **Games only.** Browsers, chat clients and the shell are never suspended.
2. **Separate the game** into its own scope rather than freezing the Flatpak
   unit it runs in. If the spike shows systemd refuses the move, the work stops
   and the alternative goes back to the author.
3. **Windows of one game** are suspended and resumed together.
4. **No Celestina work** in this milestone: no card, no launcher scopes.
5. **M12 becomes the active milestone**, starting with the spike; M10 stays
   awaiting its real-session check.
