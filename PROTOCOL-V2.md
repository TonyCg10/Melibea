# Melibea local protocol v2

Protocol v2 extends the frozen [protocol v1](PROTOCOL.md) only for coordinated
minimize and restore motion. State, ordering, action-result, availability, and
error semantics remain unchanged. Niri still owns every mapped surface and all
authoritative minimized state.

## Compatibility

- The daemon accepts versions 1 and 2.
- A version-1 request without a transition behaves exactly as documented in
  `PROTOCOL.md`.
- Version 1 rejects a transition field rather than silently ignoring it.
- Version-2 state and action-result messages use the same payload shapes as
  version 1 and carry `"version": 2`.
- An unsupported version receives `incompatible_version` with
  `supported_versions: [1, 2]`.

## Transition hints

A minimize or restore request may carry one optional transition. It is an
ephemeral presentation hint attached to that action; Melibea validates and
forwards it but never stores it.

Anchored motion:

```json
{
  "version": 2,
  "request": {
    "type": "minimize",
    "window_id": null,
    "transition": {
      "type": "anchored",
      "anchor": {
        "output": "DP-1",
        "x": 1874.0,
        "y": 9.0,
        "width": 22.0,
        "height": 22.0
      }
    }
  }
}
```

```json
{
  "version": 2,
  "request": {
    "type": "restore",
    "window_id": 42,
    "transition": {
      "type": "anchored",
      "anchor": {
        "output": "DP-1",
        "x": 1874.0,
        "y": 9.0,
        "width": 22.0,
        "height": 22.0
      }
    }
  }
}
```

Reduced motion:

```json
{
  "version": 2,
  "request": {
    "type": "restore",
    "window_id": 42,
    "transition": {"type": "disabled"}
  }
}
```

Omitting `transition` asks Niri to use its ordinary compositor-local motion.
An explicit JSON `null` is not omission and is rejected as malformed.
`disabled` changes state without spatial, scale, or opacity animation for that
action.

## Anchor coordinate space

The anchor is a finite, non-empty rectangle in logical coordinates local to
the named output. The shell owns that rectangle because it draws the bubble;
Niri owns output topology, transforms, scale, clipping, and conversion into its
render spaces.

The first implementation accepts anchored travel only when the affected window
and anchor are on the same current output. A missing output, out-of-bounds
rectangle, or otherwise unusable anchor degrades to Niri's ordinary local
transition. It never blocks, delays, or reverses the requested lifecycle state.

## Authority and privacy

The transition does not enter minimized state, restoration anchors, layout,
focus, or persistence. Action results continue to describe the state change,
not animation completion. Protocol v2 exposes no application surface pixels
and creates no preview API.
