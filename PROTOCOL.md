# Melibea local protocol v1

This document freezes the shell-facing boundary delivered by M5. Niri remains
authoritative for mapped surfaces and minimized state. Melibea mirrors the
latest complete niri snapshot, serializes consumer access, and forwards
actions; a shell never owns window lifetime.

## Transport

- Unix stream socket at `$MELIBEA_SOCKET` when set.
- Otherwise `$XDG_RUNTIME_DIR/melibea.sock`.
- The daemon creates the socket with mode `0600` and refuses to replace a
  regular file or a socket that already accepts connections.
- One newline-delimited JSON request is accepted per connection, up to 64 KiB.
- `subscribe` keeps its connection open; every other operation returns one
  message and closes.

Every envelope carries `"version": 1`. An unsupported client version receives
an `incompatible_version` error with `supported_versions: [1]`.

## Requests

```json
{"version":1,"request":{"type":"list"}}
{"version":1,"request":{"type":"subscribe"}}
{"version":1,"request":{"type":"minimize","window_id":42}}
{"version":1,"request":{"type":"minimize","window_id":null}}
{"version":1,"request":{"type":"restore","window_id":42}}
{"version":1,"request":{"type":"close","window_id":42}}
```

A null `window_id` is valid only for `minimize` and asks niri to resolve the
focused window.

## State messages

A ready `list` response and the first ready message on every subscription are
a complete snapshot:

```json
{"version":1,"message":{"type":"snapshot","revision":7,"windows":[{"id":42,"app_id":"org.example.App","title":"Example","icon_name":null}]}}
```

The daemon then sends ordered incremental revisions:

```json
{"version":1,"message":{"type":"changes","revision":8,"changes":[{"type":"removed","index":0,"window_id":42}]}}
```

Change variants are `added`, `updated`, `moved`, and `removed`. They are
sequential: applying them in array order to the preceding revision reproduces
the new ordered state exactly. Revisions are monotonic for the lifetime of the
daemon and advance on a material state change or a fresh authoritative
resynchronization.

If the niri event stream is unavailable, subscribers stay connected and
receive:

```json
{"version":1,"message":{"type":"unavailable","revision":8,"reason":"niri connection lost"}}
```

No incremental change follows `unavailable`. The next state message is a fresh
snapshot, which closes the resynchronization gap.

## Action results

```json
{"version":1,"message":{"type":"action_result","operation":"restore","requested_id":42,"window_id":42,"status":"applied"}}
```

Statuses preserve niri's semantics:

- `applied`
- `already_in_requested_state`
- `close_requested`
- `window_not_found`
- `blocked`
- `legacy_handled`, only for compatibility with the older experimental niri
  acknowledgement

`close_requested` does not claim that a window has closed. Consumers remove a
bubble only after the authoritative state revision removes it.

## Errors

Error codes are `incompatible_version`, `invalid_request`, `unavailable`, and
`action_failed`. Clients must use the code rather than parse the human-readable
message.

## Ordering guarantee

Snapshot publication, subscription registration, and state-change broadcast
run through one broker queue. A subscriber therefore observes either the state
before a concurrent niri update followed by that update, or the state after
it; it cannot miss the update between `list` and event registration because
`subscribe` is one atomic operation.
