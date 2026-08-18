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
protocol v1 with gapless subscriptions and semantic actions. Celestina 0.32.0
uses that public boundary for its compact overlapping bubble group and
selector. The author confirmed that icons and titles need no preview; active M7
work adds only a backward-compatible protocol-v2 motion hint and coordinated
window-to-bubble travel. The compositor patch remains an experimental
maintained checkout.

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
cargo test
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

## Daily trial

The repository includes [`contrib/systemd/melibea.service`](contrib/systemd/melibea.service).
Install it as a user unit, then let niri restart it at session startup:

```kdl
spawn-at-startup "systemctl" "--user" "restart" "melibea.service"
```

The unit is deliberately not enabled on `default.target`: niri owns session
startup so the current `NIRI_SOCKET` is available. Runtime diagnostics are
available with `journalctl --user -u melibea.service`.

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

See [PROTOCOL.md](PROTOCOL.md) for the frozen v1 wire contract and
[PROTOCOL-V2.md](PROTOCOL-V2.md) for the action-scoped motion extension,
[PROJECT.md](PROJECT.md) for product boundaries, and [ROADMAP.md](ROADMAP.md)
for the active milestone. Work paused during the final M7 integration is
recorded in [M7-HANDOFF.md](M7-HANDOFF.md); a new session should read that file
before applying patches or touching the live compositor.
