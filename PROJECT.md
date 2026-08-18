# Melibea project

## Purpose

Melibea makes attention visible in niri without replacing niri's scrollable
layout. A configured column can use a compact inactive width, expand when it
receives focus, and recover its prior state deterministically.

The intended daily outcome is simple: a terminal may rest at 10% beside a 90%
editor, expand to 50% when focused, and return to 10% when focus goes back to
the editor.

## Current state

Melibea is an independent Rust project. Typed configuration, strict validation,
deterministic first-match rule resolution, and a side-effect-free focus
transition engine exist. A minimal direct niri IPC transport now supplies live
snapshots and events to both a read-only diagnostic observer and an explicit
mutating controller. Resize decisions are coalesced by generation and sent to a
specific niri window id. A lost IPC session is discarded completely before
reconnecting, so decisions never cross snapshot generations.

The first controlled live cycle passed on niri 26.04: an inactive kitty window
contracted from 942 px to 178 px at 10%, accepted 50% whenever focused, and
returned to 10% on focus loss. Its original 942 px width and previous focus were
restored after the experiment.

The sustained trial is now deployed as a supervised user service. niri restarts
`melibea.service` at session startup, while systemd owns process lifetime and
journald diagnostics. Direct niri application scopes were rejected because
they did not keep this non-Wayland controller alive reliably.

The installed `melibea status` command now performs a bounded, read-only health
check against a fresh niri snapshot and explains current rule coverage without
depending on systemd.

Configuration reload is content-based rather than timestamp-based. A valid
change rebuilds controller state from a new snapshot; a malformed or unreadable
change is deduplicated in diagnostics and leaves the working policy intact. The
behavior passed a live valid-invalid-restored cycle without changing the daemon
PID.

Focus resizing is now in an author-observed daily trial and its implementation
is paused. Active development moved through the native-minimization spike and
the first shell-neutral client boundary. Melibea fills a pure ordered registry
from niri's dedicated native event and exposes it through a versioned Unix
socket contract with snapshot-first subscriptions.

A separate GPL niri 26.04 checkout now contains the native spike. Its layout
can hold a mapped tile outside all visible workspaces, process its commits,
restore it, and remove it when the client exits. Experimental IPC actions passed
a nested live cycle without replacing or restarting the host compositor. The
branch now also exposes a dedicated ordered `MinimizedWindows` query and
`MinimizedWindowsChanged` full-snapshot event. A persistent Melibea connection
observed one minimize/restore cycle as registry revisions 1 and 2 while normal
niri window IPC continued to expose only visible windows.

Melibea's minimal MIT wire client now also sends the experimental minimize,
restore, and targeted close actions. A two-window nested cycle preserved
insertion order as bubbles 0 and 1, restored only bubble 0, compacted the
remaining entry without changing its identity, and closed the remaining
minimized client without affecting the restored window.

Protocol v1 now exposes `list`, `minimize`, `restore`, `close`, and `subscribe`
without linking the GPL niri IPC crate. One broker orders snapshot publication
and subscriber registration, so a consumer cannot miss a change between its
initial state and event stream. A nested vertical test drove a real GTK window
through add, restore, add, and close revisions while preserving niri's semantic
action results. Celestina 0.32.0 now consumes that boundary as one compact
overlapping group and an accessible selector. A disposable combined session
proved reconstruction, restore and authoritative close without moving
presentation policy or surface authority into Melibea.

The author accepted icons and titles as sufficient daily identity on
2026-08-18, so M7 contains no preview mechanism. Active work is limited to a
protocol-v2 transition hint that carries one ephemeral output-local bubble
anchor with a minimize or restore request. Melibea validates and forwards that
hint but never stores it; niri remains the only renderer and lifecycle owner.

## Constraints

- Melibea must remain useful without Celestina or another shell.
- Decisions are deterministic and inspectable; there is no learned behavior.
- niri remains authoritative for surfaces, layout, focus, and rendering.
- External events and IPC data are fallible input and must not crash the daemon.
- Manual width and temporary focus overrides are distinct state.
- Floating windows, dialogs, popups, and fullscreen transitions are excluded
  until their behavior is deliberately specified.
- A future minimized window must leave the navigable layout completely. A
  storage workspace is not an acceptable final implementation.
- Celestina integration will consume a versioned contract; it will not own
  Melibea's policy or authoritative minimized-window state.

## Decisions

| Decision | Reason |
|---|---|
| Build Melibea as a separate Rust project | Its behavior belongs to niri and may serve shells other than Celestina. |
| Validate focus-responsive geometry first | It can prove daily value before modifying niri. |
| Let the geometry trial continue while the minimization spike becomes active | The deployed behavior can gather comfort evidence without blocking independent source research. |
| Start with real column resizing through IPC | It is the narrowest test of the interaction. |
| Treat visual clipping as a later native experiment | It crosses layout, rendering, input, and camera boundaries. |
| Keep one active milestone | The project is currently a focused solo experiment. |
| Add no plugin framework | Melibea has two product goals, not an open-ended extension platform. |
| Keep the MIT license and implement the small niri wire protocol directly | Linking the GPL niri-ipc crate would change Melibea's licensing; the required protocol subset is small and testable. |
| Use targeted `SetWindowWidth` actions | `SetColumnWidth` operates on whichever column is focused when handled; a window id avoids mutating the wrong target after rapid focus changes. |
| Keep minimized surfaces native to niri and bubble presentation outside niri | The compositor must own rendering, input exclusion, focus, and surface lifetime; Melibea projects deterministic state and a shell only renders it. |
| Separate lifecycle discovery from visible-window iteration in the niri spike | Reusing the normal visible iterator would leak minimized surfaces into focus, overview, MRU, IPC, or screencasting. |
| Expose a dedicated minimized-window IPC record | A minimized surface has no honest workspace or geometry; reusing the visible `Window` record would require fabricated values. |
| Keep experimental bubble actions in Melibea's minimal wire client | Shells can invoke one project boundary without linking GPL `niri-ipc` or learning the compositor wire representation. |
| Use one versioned local broker for shell consumers | Snapshot and subscription registration share an order, eliminating the list-then-subscribe race without giving shells surface authority. |
| Keep bubble anchor geometry out of protocol v1 | Presentation belongs to the shell; the delivered bubble UI now gives a later bidirectional motion contract a concrete anchor to evaluate. |
| Carry the M7 anchor only inside each protocol-v2 action | Per-action geometry cannot become stale after a shell crash and requires no second state authority, lease, or heartbeat. |
| Use output-local logical coordinates | The shell names the output it draws on while niri retains topology, transform, scale, and render-space authority. |
| Omit window previews from M7 | The author confirmed that icons and titles already distinguish the deployed bubbles; exposing application pixels would add security and lifecycle cost without demonstrated value. |

## Open questions

- Does resizing the client on every focus transition cause unacceptable text
  reflow or responsive-layout churn during real use?
- Do targeted niri width actions remain visually clean under sustained rapid
  focus changes, or will a future atomic compositor action become necessary?
- Should active width always be configured, or may Melibea restore the last
  manual width observed before contraction?
- What is the smallest niri-owned held-surface state that still receives commit
  and destruction handling while remaining absent from rendering, focus,
  overview, MRU, navigation, and screencasting?
- Which placement facts must niri retain to restore a tile into the same
  workspace, column, and neighborhood after outputs or workspaces change?
- Does the coordinated trajectory improve orientation enough to justify its
  compositor maintenance surface during sustained daily use?
