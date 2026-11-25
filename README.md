# Melibea

Melibea is a deterministic attention and surface-state controller for the
[niri](https://github.com/niri-wm/niri) Wayland compositor.

The first feature under validation is focus-responsive geometry: selected
columns may expand when focused and return to a compact width after losing
focus. Melibea is independent from any desktop shell and now exposes native
minimization state through a small versioned local protocol. Celestina can
consume that contract, but it does not own Melibea policy or window lifetime.

Native minimization now continues alongside the deployed geometry trial.
A separate niri 26.04 branch provides native minimize, restore, close, query,
events, transient-family handling, relational restoration, and compositor-local
motion without using a hidden workspace. `melibea run` projects its state over
protocol v1 with gapless subscriptions and semantic actions, extended by a
backward-compatible protocol-v2 motion hint for coordinated window-to-bubble
travel. Celestina 1.0.0 uses that public boundary for its compact overlapping
bubble group and selector, and downgrades to protocol v1 without the hint when
a Melibea it talks to does not speak v2, so minimize and restore keep working
without the travel. The author confirmed that icons and titles need no
preview. The compositor patch remains an experimental maintained checkout; see
[ROADMAP.md](ROADMAP.md) for what is validated in a nested harness versus
deployed in the real session.

## Configuration

Melibea reads `~/.config/melibea/config.toml`, or
`$XDG_CONFIG_HOME/melibea/config.toml` when `XDG_CONFIG_HOME` is set. It does
not create this file automatically. Start from `config.example.toml` when you
are ready to run a real configuration.

```sh
cargo run -- --config config.example.toml check-config
cargo run -- --config config.example.toml observe
cargo run -- --config config.example.toml run
cargo run -- status
cargo run -- list
cargo run -- minimize [WINDOW_ID]
cargo run -- restore WINDOW_ID
cargo run -- close WINDOW_ID
cargo run -- subscribe
cargo test --workspace
```

`check-config` performs strict parsing and validation without contacting niri
or changing any window. Rules are resolved from top to bottom; the first rule
whose configured matchers all match wins.

`observe` opens niri's live event stream and prints the width actions Melibea
would choose. It is a read-only diagnostic: it never sends a layout action.
Stop it with `Ctrl+C`.

`status` validates the selected configuration, waits up to two seconds for
niri's authoritative window snapshot, and reports tiled matches, unmatched
windows, floating exclusions, focus, and matches per rule. It does not mutate
niri. Process supervision remains visible through `systemctl --user status
melibea.service`.

`list`, `minimize`, `restore`, `close`, and `subscribe` use the stable local
Melibea v1 service rather than speaking niri IPC directly. Omitting the id from
`minimize` targets the focused window. `subscribe` prints an initial snapshot
and then ordered JSON-line changes. `minimized` and `close-window` remain CLI
aliases for compatibility. Stock niri 26.04 does not provide the native events
or actions, so these operations require the separate experimental checkout.

`run` is the explicit mutating mode and the protocol daemon. It evaluates the
initial snapshot, applies configured widths to matching windows by their niri
window id, follows focus changes, and serves `$MELIBEA_SOCKET` or
`$XDG_RUNTIME_DIR/melibea.sock` with mode `0600`. Use `observe` first with the
same configuration. If niri IPC fails, subscribers receive `unavailable` and
the next successful connection begins with a fresh snapshot.

While running, Melibea also checks the configuration source once per second.
A valid edit replaces the policy and rebuilds from a fresh niri snapshot. An
invalid or temporarily unreadable edit is reported but never replaces the last
known-good policy or stops the controller.

## Standby for games

Melibea can suspend a game: it moves the game's processes into a systemd
scope of their own (`app-melibea-game-<pid>.scope`), freezes that scope, and
asks niri to draw the window blurred and dimmed and to send it no input.
Resuming thaws the scope first and clears the niri flag afterwards. See the
[design](docs/superpowers/specs/2026-11-17-melibea-standby-design.md) and
[PROTOCOL-V3.md](PROTOCOL-V3.md).

Most games need no rule. A window is a game when its app id is `steam_app_<N>`,
or when the process that owns it has `SteamAppId=<N>` in its environment (any
Steam game, native or Proton) or `WINESERVERSOCKET` (Wine, Proton, Bottles,
Lutris, Heroic). For a native Wayland window that process is niri's pid; for an
X11 window niri reports `xwayland-satellite`, so Melibea asks the X server
(X-Resource) which process owns the window, and recognizes it only when exactly
one does. `WINEPREFIX` alone never marks a game.

Rules remain for native non-Steam games and for `standby_after`. They are in
`~/.config/melibea/games.toml`, or `$XDG_CONFIG_HOME/melibea/games.toml` when
`XDG_CONFIG_HOME` is set, resolved from top to bottom with the first match
winning, and they take precedence over detection:

```toml
[[game]]                      # a native non-Steam game found by its executable
app_id = "^mygame$"           # regex, optional
title = "^My Game$"           # regex, optional; at least one of the two
process = "mygame"            # this, or steam_app_id, or neither
standby_after = "5m"          # optional: suspend after staying minimized this long
```

Name `app_id` in every rule and anchor `title` with `^`: a rule with only a
title also matches a browser tab named after the game ("Terraria Wiki —
Mozilla Firefox"). Melibea suspends together only the windows that share the
target's app id or whose process belongs to the game, but a title-only rule
can still make a browser tab the target.

A missing file means no rules; an invalid one refuses suspension and turns off
standby after minimizing, but never affects resume or recovery.

```sh
melibea suspend [WINDOW_ID]          # the focused window's game when the id is omitted
melibea resume [WINDOW_ID]
melibea toggle-suspend [WINDOW_ID]
melibea thaw-all                     # works without the daemon
```

`suspend`, `resume` and `toggle-suspend` go through the running daemon over
protocol v3. A refusal prints its reason and changes nothing. `thaw-all` thaws
every unit in the ledger (`$XDG_RUNTIME_DIR/melibea/standby.json`) and every
loaded `app-melibea-game-*.scope`, and clears niri's flags when niri answers.
The service unit runs it from `ExecStopPost`, so stopping or crashing the
daemon releases every game. On start and after every niri reconnection the
daemon reconciles the ledger, niri's flags and systemd's freezer, and logs
what it changed.

A minimized game whose rule names `standby_after` is suspended once it has
stayed minimized that long. When it leaves the minimized list, by Melibea's
restore or by niri directly, it is resumed at once. Melibea's `restore` and
`close` resume a suspended window before forwarding the action.

An example niri bind:

```kdl
binds {
    Mod+Shift+P allow-inhibiting=false { spawn "melibea" "toggle-suspend"; }
}
```

`allow-inhibiting=false` keeps the bind working while a native Wayland game
inhibits keyboard shortcuts; otherwise the game would swallow the toggle.

The suspended look needs the Niri fork, configured by its `suspend { }` block
(these are the defaults):

```kdl
suspend {
    blur-passes 3    // 1 to 31
    blur-offset 3.0  // 0 to 100
    dim 0.35         // 0 to 1
}
```

## Daily trial

Build and install the binary with:

```sh
scripts/install.sh
```

It builds the workspace in release, installs `~/.local/bin/melibea` and
`~/.local/bin/melibea-ruler` (or under `$MELIBEA_PREFIX/bin`) by an atomic
rename, checks the installed bytes against the build, and then removes
`target/`, which nothing reads after the install and the next build
regenerates. `--keep-target` leaves it. A running
daemon keeps the previous binary until it restarts.

The repository includes [`contrib/systemd/melibea.service`](contrib/systemd/melibea.service).
Install it as a user unit, then let niri restart it at session startup:

```kdl
spawn-at-startup "systemctl" "--user" "restart" "melibea.service"
```

The unit is deliberately not enabled on `default.target`: niri owns session
startup so the current `NIRI_SOCKET` is available. Runtime diagnostics are
available with `journalctl --user -u melibea.service`.

## Screen ruler

`melibea-ruler` is a second binary in this repository, separate from the daemon
and independent of it. It freezes the screen, then measures on the frozen
picture: from the pointer it finds the box of the element under it, or the gap
it stands in, in logical pixels, and a click copies the measurement. Escape
leaves. Run it from the `tools` binding mode, where it is bound to `M`
(Mod+H, then M).

For scripts there is a headless mode that shows nothing:

```sh
melibea-ruler --measure box|spacing --at X,Y --output NAME [--tolerance N]
```

It prints the result (for example `412x38`) and exits. See the
[design](docs/superpowers/specs/2026-10-08-melibea-ruler-design.md).

## Current state

Typed configuration, deterministic rule resolution, the pure focus transition
engine, a minimal direct niri IPC transport, and targeted width execution are
implemented. `observe` remains read-only; `run` applies the same decisions.
A controlled live cycle has applied 10% unfocused and 50% focused widths on
niri 26.04, including rapid focus alternation, and the author has confirmed the
deployed behavior works as intended. Sustained comfort remains under
observation. A separate nested end-to-end cycle validated protocol-v1 list,
subscribe, minimize, restore, close, semantic responses, and ordered revisions
against real GTK and Kitty clients. The Celestina integration reconstructed a
two-window group, restored one entry and retained the other until Niri's later
authoritative close revision.

See [PROTOCOL.md](PROTOCOL.md) for the frozen v1 wire contract,
[PROTOCOL-V2.md](PROTOCOL-V2.md) for the action-scoped motion extension and
[PROTOCOL-V3.md](PROTOCOL-V3.md) for standby,
[PROJECT.md](PROJECT.md) for product boundaries, and [ROADMAP.md](ROADMAP.md)
for the active milestone. Agents start with [AGENTS.md](AGENTS.md); completed
milestones and the superseded M7 hand-off are in [docs/history](docs/history/).
