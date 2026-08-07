# Melibea

Melibea is a deterministic attention and surface-state controller for the
[niri](https://github.com/niri-wm/niri) Wayland compositor.

The first feature under validation is focus-responsive geometry: selected
columns may expand when focused and return to a compact width after losing
focus. Melibea is independent from any desktop shell. A future version may
expose native minimization state to optional consumers such as Celestina.

## Current state

The repository contains the initial domain model and command-line skeleton.
It does not connect to a running niri session or resize windows yet.

```sh
cargo run -- status
cargo test
```

See [PROJECT.md](PROJECT.md) for product boundaries and [ROADMAP.md](ROADMAP.md)
for the active milestone.

