# AGENTS.md — Melibea

Melibea is an MIT Rust daemon that controls attention and surface state for
the niri compositor: focus-responsive column widths and native minimization
over a small versioned local protocol. `ruler/` is a separate companion
binary, `melibea-ruler`. Repository text and commit subjects are English;
speak to the author in Spanish.

## Read before acting

Read now, completely: this file and [ROADMAP.md](ROADMAP.md) (what is live,
what is planned, where the Niri fork lives).

Read when the trigger applies, before acting on that matter:

- [PROTOCOL.md](PROTOCOL.md) and [PROTOCOL-V2.md](PROTOCOL-V2.md): when a change
  touches the wire format, `list`/`subscribe`/actions, or what Celestina sends.
- [PROJECT.md](PROJECT.md): when the request's scope or a product boundary is
  unclear.
- [README.md](README.md): when you need the commands or the configuration.
- `docs/superpowers/specs/` and `docs/superpowers/plans/`: when working on the
  milestone they design.
- `docs/history/`: only to answer a question about past work. The M7
  hand-off there is superseded; its `/tmp` paths and hashes no longer exist.

## Boundaries

- Melibea is independent. Celestina consumes the protocol but owns no
  Melibea policy and no authoritative window state.
- niri alone owns surfaces, layout, focus, rendering, input, minimization
  lifetime, restoration and animation. Melibea never becomes a second state
  authority.
- Behavior is deterministic, explicit and reversible: no AI, learning or
  heuristic classification.
- Native minimization takes a window out of layout and navigation; a hidden
  workspace or off-screen scratchpad is not a substitute.
- State changes never wait for an animation to finish.
- Code that changes niri itself (M4, M7, M9, M11) belongs in the fork at
  `../NIRI-MELIBEA`, a separate GPL-3.0 repository, not here.

## Checks

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

`ruler/tests/nest.sh` runs the ruler against a nested niri.

## The author's live session

- Never restart niri or `melibea.service`, and never touch the live niri
  config, without the author's request.
- `scripts/install.sh` builds, installs `melibea` and `melibea-ruler` into
  `~/.local/bin` and removes `target/`; the running daemon picks up the new
  binary at its next restart. Pass `--keep-target` while iterating.
- The fork's `scripts/install.sh` installs niri into
  `~/.local/lib/celestina/niri`; it takes effect at the next login.
- Test compositor changes in a nested niri. Never `pkill -f niri` from a tool
  shell: the pattern matches the shell itself. Stop processes by pid.
- No commit or push without the author's request.
