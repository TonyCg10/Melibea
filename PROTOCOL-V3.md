# Melibea local protocol v3

Protocol v3 extends [protocol v2](PROTOCOL-V2.md) with standby for games:
suspending and resuming a game's processes, and a list of the suspended
windows. Protocols [v1](PROTOCOL.md) and v2 are frozen; v3 changes none of
their bytes. Niri still owns every mapped surface and all minimized state.

## Compatibility

- The daemon accepts versions 1, 2 and 3. Version 3 accepts everything
  version 2 does, with the same payloads, carrying `"version": 3`.
- Suspension requests are valid only at version 3. At version 1 or 2 they
  receive `invalid_request` with the message
  `suspension requires Melibea protocol version 3`, and nothing is executed.
- An unsupported version receives `incompatible_version` at version 1 with
  `supported_versions: [1, 2, 3]` and the message
  `unsupported Melibea protocol version N; supported versions are 1, 2 and 3`.
- `action_result` gains an optional `reason`. It is omitted whenever it is
  absent, so v1 and v2 results are byte-identical to before.

## Requests

Each takes an optional `window_id`; `null` means the focused window.

```json
{"version": 3, "request": {"type": "suspend", "window_id": 608}}
{"version": 3, "request": {"type": "resume", "window_id": null}}
{"version": 3, "request": {"type": "toggle_suspend", "window_id": null}}
{"version": 3, "request": {"type": "list_suspended"}}
```

`suspend` freezes the processes of the window's game, `resume` thaws them, and
`toggle_suspend` does whichever applies. They are answered with one
`action_result` whose `operation` is `suspend` or `resume` (`toggle_suspend`
reports the operation it performed). `list_suspended` is answered with a
`suspended` message.

```json
{"version":3,"message":{"type":"action_result","operation":"suspend","requested_id":608,"window_id":608,"status":"applied"}}
```

Statuses are the existing ones: `applied`, `already_in_requested_state`,
`window_not_found` and `blocked`. `requested_id` carries the resolved window
when the request named none.

## Suspended list

```json
{"version":3,"message":{"type":"suspended","revision":1,"windows":[{"id":608,"app_id":"steam_app_1","title":"Game"}]}}
```

The list has its own `revision`, starting at 0 with an empty list and
advancing only when the list changes; it is independent of the `snapshot` and
`changes` revision. A v3 `subscribe` receives its usual first message
(`snapshot` or `unavailable`), then a `suspended` message with the current
list even when it is empty, then a `suspended` message with the complete new
list at every change. Subscribers at version 1 and 2 never receive
`suspended`.

## Blocked reasons

A `blocked` result carries `reason` as `<code>: <description>`. The codes:

| Code | Meaning |
|---|---|
| `not_a_game` | The window matches no rule in `games.toml`. |
| `game_not_running` | No process of the game is running. |
| `protected_process` | A process that must never be frozen is in the game. |
| `game_escaped` | Game processes stayed outside the game scope. |
| `invalid_rules` | `games.toml` is invalid. |
| `systemd` | A systemd call failed. |
| `ledger` | The standby ledger could not be read or written. |
| `compositor` | niri refused or could not be reached. |

The description after the colon is for people and may change; match the code.
