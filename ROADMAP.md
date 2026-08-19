# Melibea roadmap

This roadmap orders Melibea's work by evidence. Only one implementation
milestone is active at a time; a deployed trial may remain under observation
after its code work pauses. Estimates describe likely solo-development effort,
not delivery commitments.

## Product sequence

```text
M0 Foundation (done)
  -> M1 Focus-responsive geometry experiment (deployed trial)
      -> M2 Reliable daily-use controller (after comfort evidence)
  -> M3 Native minimization design spike (complete, gate passed)
      -> M4 Usable native minimization (complete)
      -> M5 Versioned shell contract (complete)
      -> M6 Celestina bubble integration (complete)
      -> M7 Coordinated bubble motion (active)
      -> M8 Optional expansions
```

## M0 — Foundation

**Status:** done

**Evidence:** commit `f5d7942`

**Effort:** completed

### Outcome

Melibea exists as an independent Rust repository with a minimal executable,
pure attention-policy types, tests, project boundaries, and one active product
milestone.

### Delivered

- Independent Rust package with `unsafe` forbidden.
- Explicit `focused`, `unfocused`, and `preserve` width behavior.
- Unit coverage for width selection and invalid proportions.
- Minimal diagnostic CLI skeleton.
- Product constraints in `PROJECT.md`.
- No dependency on Celestina, Piri, or a fork of niri.

### Exit evidence

- `cargo test` passes.
- `cargo clippy --all-targets -- -D warnings` passes.
- The initial repository is published on `main`.

## M1 — Focus-responsive geometry experiment

**Status:** deployed trial; implementation paused while comfort is observed

**Estimated effort:** several focused sessions to one week

### Outcome

In a real niri session, a configured terminal expands when focused and returns
to its compact width when focus returns to the editor:

```text
Editor focused:   Terminal 10% | Editor 90%
Terminal focused: Terminal 50% | Editor 90% ->
```

The widths are independent rather than redistributed to total 100%; niri's
scrollable strip may overflow the output normally.

### In

- Typed configuration for application matching and width behavior.
- Connection to the niri IPC event stream.
- Initial window and workspace snapshot followed by live events.
- In-memory window registry and current-focus state.
- Rules matching `app_id`; optional title matching only when required.
- Focused, unfocused, and `preserve` width policies.
- Dry-run mode that explains intended actions without changing layout.
- Live width actions through existing niri IPC.
- Unit tests using recorded or constructed event sequences.

### Out

- Minimization and bubbles.
- Celestina integration.
- Changes to niri source.
- Floating-window policy.
- Dialog, popup, transient, or fullscreen behavior beyond safe exclusion.
- Visual clipping that preserves the client's full width.
- Persistence across compositor restarts.
- A general plugin system.

### Work

1. Define configuration types, width parsing, rule priority, and validation.
   **Done.**
2. Define a pure transition engine for current and previous focus.
   **Done.**
3. Adapt niri snapshots and event-stream messages into the internal registry.
   **Done, including the live Unix-socket transport.**
4. Add dry-run diagnostics that identify the event, matched rule, previous
   state, selected policy, and intended niri action.
   **Done through the read-only `observe` command.**
5. Apply width actions while discarding decisions made obsolete by newer focus
   events.
   **Done with targeted window actions, generation-based coalescing, and a
   controlled rapid-focus cycle.**
6. Recover from IPC disconnection by rebuilding state from a fresh snapshot.
   **Done with clean reader cancellation, fresh event/action sockets, and a
   one-second reconnect delay.**
7. Exercise the 10% terminal / 90% editor workflow first in a controlled
   session and then during sustained daily work.
   **Controlled terminal cycle passed: 942 px -> 178 px at 10%, 50% action on
   focus, and 10% action on focus loss. Original geometry and focus were
   restored. The supervised daily trial started on 2026-08-14 through
   `melibea.service`; a bounded `status` health check now exposes snapshot and
   rule coverage. The author confirmed the live focus behavior works as
   intended on 2026-08-14; comfort over sustained use remains pending.**
8. Reload configuration safely during the sustained trial.
   **Done and live-validated: valid edits rebuild from a fresh snapshot;
   invalid edits preserve the last known-good policy and do not restart the
   service.**

### Exit

- Rapid focus changes converge on the latest focused column.
- A matching terminal follows its configured focused and unfocused widths.
- A `preserve` editor is never contracted by Melibea.
- Excluded or unmatched windows cause no mutation.
- Stopping or disconnecting Melibea leaves niri usable and reports a clear
  error.
- The interaction remains comfortable during a sustained real-work session.

### Gate to M2

The author confirms that automatic focus resizing saves effort without causing
unacceptable camera movement, terminal reflow, or responsive-layout churn. If
this fails, adjust or stop the experiment before adding more architecture.

## M2 — Reliable daily-use controller

**Status:** planned

**Estimated effort:** one to two weeks after M1

### Outcome

The geometry controller can remain enabled during normal niri use, preserves
manual intent, handles niri's column structures correctly, and recovers from
ordinary process and IPC failures.

### Intended scope

- Distinguish window focus, effective root window, and column focus.
- Treat tabbed or vertically stacked windows as one column where appropriate.
- Keep manual width separate from temporary focus overrides.
- Restore the correct active width after an inactive contraction.
- Make animations interruptible by relying on current state, not a command
  backlog.
- Reload valid configuration without losing the last known-good policy.
- Provide useful structured diagnostics and a health/status command.
- Characterize fullscreen, floating, transient, and interactive-resize events
  before admitting any of them into policy.

### Exit

- Restarting Melibea reconstructs state from niri without moving unmatched
  columns.
- Changing tabs inside one column does not cause spurious width oscillation.
- Manual and temporary widths remain distinguishable in tests and real use.
- Invalid configuration is rejected without replacing the working policy.
- A multi-hour daily session reveals no stale-event or reconnect corruption.

### Geometry-native follow-up

Decide from evidence whether existing IPC actions are sufficient. If two-step
resize transitions expose visible or inconsistent intermediate states, define
the narrowest atomic geometry action niri would need. This assessment is
independent from the native minimization spike.

## M3 — Native minimization design spike

**Status:** complete

**Estimated effort:** about one week of focused research and prototype work

### Outcome

A bounded niri prototype proves whether a mapped toplevel can enter a genuine
`Minimized` state outside tiled and floating layouts, then return without a
hidden, off-screen, or navigable storage workspace.

### State hypothesis

```text
Mapped surface
  |- Tiled
  |- Floating
  `- Minimized
```

A minimized surface remains alive but:

- has no layout position;
- is not rendered;
- cannot receive input or focus;
- is absent from overview and workspace navigation;
- retains enough compositor-owned placement data for restoration.

### Work

1. Identify the niri owners and invariants for mapped, tiled, floating, focused,
   rendered, and IPC-visible toplevels.
   **Initial niri 26.04 source map done. `Layout` owns tiled and floating
   `Tile<Mapped>` values; `remove_window` already yields a reusable
   `RemovedTile`, but visible-window iteration is also consumed by focus, IPC,
   MRU, and screencasting. A minimized surface therefore needs lifecycle
   discoverability separate from visible/navigation iteration.**
2. Define the minimum state and transitions without committing to a public
   protocol.
   **Done for the spike. Melibea has a transport-neutral ordered registry for
   authoritative snapshot, minimize, metadata, restore, and client-close
   transitions. The niri prototype holds the removed `Tile<Mapped>` plus its
   preferred workspace, keeps it discoverable for commits and destruction,
   and excludes it from normal visible-window iteration.**
3. Prototype minimize and restore actions in a disposable niri branch.
   **Done for the first bounded prototype in the separate GPL checkout
   `/home/toni/CODIGO/NIRI-MELIBEA`, branch
   `codex/melibea-native-minimization`, based exactly on niri 26.04 commit
   `8ed0da4`. Experimental targeted and focused minimize actions plus targeted
   restore compile across all targets. The branch also exposes a dedicated
   ordered minimized-window query and full-snapshot event instead of fabricating
   visible-window geometry. Melibea parses that event into its pure registry.**
4. Exercise closing, output removal, workspace removal, and compositor shutdown
   while a surface is minimized.
   **Done for the bounded spike. Unit tests pass for removal from visible
   layout, restoration after dynamic workspace deletion, client close,
   preservation of fullscreen, restoration to an original output that still
   exists, and fallback to the active output after the original output is
   removed. A nested live cycle passed:
   Kitty id 2 was visible, `windows` became empty while its PID remained alive,
   restore returned the same id and focus, and targeted close while minimized
   removed the client cleanly. A second nested cycle proved the query and event
   bridge: Melibea reported `bubble=0` for id 2, then a persistent connection
   advanced from revision 1 with one bubble to revision 2 with none after
   restore. A multi-window nested cycle then minimized ids 2 and 3 through
   Melibea, preserved their insertion order, restored id 2 independently, and
   closed id 3 while it remained minimized. A combined Celestina plus Melibea
   nested cycle then shut the compositor down with an editor retained: the
   client exited, the nested socket disappeared, niri returned success, the
   host session stayed healthy, and restart began with an empty minimized
   snapshot. The one-output winit nest cannot visually exercise output
   removal, so that fallback is proven at layout level rather than claimed as
   a live multi-output result.**
5. Measure the patch surface and decide whether it can be maintained or proposed
   upstream as a general primitive.
   **Final spike measurement: nine tracked niri files carry about 700 added
   lines and five removed lines, including tests and CLI/IPC plumbing. The layout
   ownership change is localized. Output fallback, shutdown, fullscreen, and
   screencast boundaries are now characterized: minimize uses niri's existing
   target-stop path for window casts, while the visible window iterators used
   by screencast rendering exclude retained tiles. The remaining semantic risk
   is transient toplevel ownership, not broad layout or rendering ownership.**

### Exit

- A visible window disappears completely without entering another workspace.
- It remains alive and closes cleanly if its client exits.
- It restores by one deterministic rule.
- There is no residual focus, input, overview, or navigation entry.
- The required niri change is understood well enough to accept or reject M4.

### Gate to M4

Proceed only if native minimization does not require a broad rewrite of niri's
layout and rendering ownership. Otherwise record the blocking boundary rather
than shipping a fake workspace-based implementation.

**Decision: pass.** The spike changes one layout owner and uses existing niri
paths for surface lifecycle, output migration, screencast shutdown, actions,
and IPC. The remaining work is product semantics and restoration fidelity, so
M4 may proceed without turning Melibea into a hidden-workspace implementation.

### Known spike gaps

- Existing `Windows` IPC still represents minimize/restore as close/open because
  it intentionally enumerates only visible layout windows. Consumers must use
  the new dedicated minimized snapshot; this boundary is now live-validated.
- Melibea exposes only a diagnostic projection. A stable shell-facing contract
  and actual Celestina bubble UI remain later milestones.
- Experimental Melibea commands can minimize, restore, and close by id, but
  they are CLI probes rather than the versioned shell contract planned for M5.
- The experimental niri IPC now returns a semantic `WindowActionResult` for
  targeted minimize, restore, and close actions. Melibea accepts this response
  while remaining backward-compatible with the earlier `Handled` reply. All
  five semantic statuses are now validated against a nested current build.
- The M3 prototype reinserted automatically. M4 now retains nearest-first
  relational anchors for a tiled column, vertical position, and floating stack;
  a raw index is used only when no original neighbor survives.
- Popup surfaces disappear with their retained parent tile. Separately mapped
  transient toplevels now follow the deterministic family policy in both tests
  and a live GTK probe.
- Minimize and restore now have a compositor-local transition foundation. The
  retained family leaves and returns in one frame using niri's existing close
  and open visuals; lifecycle state changes immediately and never waits for
  animation completion. A trajectory to Celestina's actual bubble anchor is
  intentionally deferred until that shell contract exists.

## M4 — Usable native minimization

**Status:** complete; optional live multi-output confirmation deferred until a
safe suitable backend is available

**Estimated effort:** two to four weeks after a successful M3

### Outcome

niri provides stable minimize, restore, close, query, and event behavior for
multiple live minimized surfaces. Melibea consumes that state without becoming
its authority.

### Current baseline

- Native ordered state, query, full-snapshot event, and targeted actions work.
- Multiple windows, client close, compositor shutdown, workspace deletion,
  output fallback, fullscreen preservation, and screencast stop are covered.
- Melibea remains a replaceable consumer; niri owns every mapped surface and
  the authoritative minimized state.

### Work

1. Freeze output and workspace fallback semantics.
   **Done at layout level. Original output plus workspace wins only while both
   exist; otherwise restore uses the active output and workspace.**
2. Preserve a relational restore anchor so reinsertion prefers an existing
   original neighbor, then the closest surviving position.
   **Done. Tiled windows restore within a surviving original column or between
   the nearest surviving original columns. Floating windows recover their
   stack relation. Both paths use the original index only as a final fallback,
   and four dedicated layout tests cover immediate neighbors, a more distant
   surviving neighbor, vertical column position, and floating order.**
3. Define one deterministic policy for separately mapped transient toplevels;
   popup surfaces already follow their retained parent tile.
   **Done at code and layout-test level. A toplevel root and every separately
   mapped descendant form one minimized family and one public bubble.
   Minimizing any member retains the whole family; new descendants join an
   already-minimized parent without becoming visible; detaching and reparenting
   split or merge families deterministically; restoring returns the root first
   and descendants afterwards; closing a root promotes a surviving member. A
   live GTK probe minimized an existing root/child family as one public bubble,
   then created another child while its root was minimized: it never entered
   the visible layout and appeared with the root after restore.**
4. Replace request-only acknowledgement with an observable resulting revision
   or explicit semantic result for minimize, restore, and close.
   **Done at protocol, server, CLI, and Melibea-client level. Minimize and
   restore distinguish `Applied`, `AlreadyInRequestedState`,
   `WindowNotFound`, and `Blocked`. Close returns `CloseRequested` rather than
   claiming the client has already destroyed its surface; consumers observe
   final removal through the authoritative minimized snapshot event. When a
   transient descendant is minimized, the response resolves to the public
   family-root id. JSON compatibility and Melibea parser tests cover the new
   shape and legacy `Handled` remains accepted during the experiment. A nested
   wire probe observed `Applied`, `AlreadyInRequestedState`, `WindowNotFound`,
   `Blocked`, and `CloseRequested`, plus the subsequent authoritative snapshot
   events.**
5. Add coordinated minimize/restore motion without making animation the owner
   of lifecycle state.
   **Done for the compositor-local foundation. Before retaining a family, niri
   snapshots every visible root and transient member and starts their existing
   close visual from the current geometry in the same render turn. Restore
   reinserts authoritative state first, then starts the existing open visual
   for every family member. A nested build passed normal minimize/restore, an
   immediate minimize-to-restore reversal, and fullscreen minimize/restore
   while preserving the same public id and final registry state. Exact motion
   between a window and a shell bubble remains M7 work because it requires a
   versioned Celestina anchor rather than a compositor-hardcoded coordinate.**
6. Repeat the nested suite with fullscreen and, when a suitable backend is
   available, live multi-output removal.
   **Fullscreen passed in a live one-output winit nest: id and fullscreen
   geometry survived the animated cycle. Live output removal remains pending
   because the current winit backend exposes only one test output.**

### Intended compositor contract

```text
minimize-window <window-id>
restore-window <window-id>
close-minimized-window <window-id>
minimized-windows
window-minimized
window-restored
```

The experimental Rust/JSON spellings are now exercised end to end. Melibea
protocol v1 deliberately hides these compositor-specific names from shells.

### Restoration policy to validate

1. Restore to the original workspace and output when both still exist.
2. Reinsert beside the original neighbor when that relation still exists.
3. Otherwise use the closest valid original position.
4. If the output disappeared, use the active output.
5. If the workspace disappeared, use the active workspace.
6. Recover the active/manual width, never the temporary inactive width.

### Exit

- Multiple minimized windows remain independently addressable.
- Client exit removes minimized state without residue.
- Output and workspace removal follow documented fallback rules.
- Melibea restart does not lose or fabricate compositor-owned state.
- Tests cover minimize, restore, close, and invalid identifiers.

### Gate to M5

The native contract and restoration semantics are stable enough that a shell
consumer will not need access to niri internals.

## M5 — Versioned Melibea client contract

**Status:** complete

**Estimated effort:** several days

### Outcome

Any shell or CLI can list and operate minimized windows through a small,
versioned Melibea contract without becoming responsible for their lifetime.

### Intended scope

- Full state snapshot before incremental events.
- `list`, `minimize`, `restore`, `close`, and `subscribe` operations.
- Stable window identity, `app_id`, title, and optional icon identity.
- Explicit protocol version and compatibility errors.
- Reference CLI covering every operation.
- Reconnection that cannot miss state changes between snapshot and subscription.

### Delivered

- Protocol v1 over a mode-`0600` Unix stream socket, resolved from
  `MELIBEA_SOCKET` or `XDG_RUNTIME_DIR`.
- One newline-delimited request per connection and a persistent `subscribe`
  operation.
- Atomic snapshot-first subscription followed by sequential `added`,
  `updated`, `moved`, and `removed` changes with monotonic daemon revisions.
- Explicit `unavailable` followed by a fresh snapshot after authoritative
  resynchronization; no incremental event crosses that boundary.
- Typed compatibility, request, availability, and action errors.
- Preservation of all five niri semantic action outcomes plus a named legacy
  acknowledgement state.
- Stable id, `app_id`, title, and nullable icon identity without exposing niri
  layout records.
- Reference CLI commands for every operation. `minimized` and `close-window`
  remain compatibility aliases.
- Contract tests for ordering, incompatible versions, action forwarding,
  unavailable/resnapshot behavior, and safe socket ownership.
- A nested end-to-end GTK cycle observed snapshot revision 1, minimize add at
  revision 2, restore removal at revision 3, second minimize at revision 4,
  and authoritative close removal at revision 5. Stopping the subscriber did
  not affect the minimized surface or compositor state.

### Exit

- The CLI can operate every minimized window without Celestina.
- Restarting a consumer has no effect on minimized surfaces.
- An incompatible client receives a clear protocol-version failure.
- Contract tests cover snapshot, incremental updates, and reconnection.

### Gate to M6

The contract is sufficient to build a shell UI using only public Melibea data
and actions. Celestina-specific work receives its own plan in the Celestina
repository when this gate passes.

## M6 — Celestina bubble integration

**Status:** complete

**Estimated effort:** one to two weeks after M5

### Outcome

Celestina Shell presents minimized windows as a compact, overlapping bubble
group and restores or closes a selected window through Melibea.

### Melibea responsibility

- Keep the public contract stable.
- Supply authoritative snapshots and events derived from niri.
- Remain usable if Celestina is stopped or restarted.
- Never move presentation policy into the Melibea daemon.

### Celestina responsibility

The consuming repository will separately own:

- bubble placement and styling;
- overlapping icon presentation and count;
- keyboard and assistive-technology behavior;
- the expanded icon/title selector;
- restore and close interactions.

Live previews, coordinated trajectory animation, project contexts, and semantic
grouping remain outside this integration milestone.

### Delivered

- Celestina 0.32.0 consumes protocol v1 from its existing aggregate provider
  process and reconstructs ordered state after either side reconnects.
- One compact overlapping panel group opens a title/app-identity selector with
  pointer, keyboard and accessible restore/close routes.
- Accepted actions never mutate the visual inventory. A later authoritative
  Melibea revision removes the row and a restore retires the chooser only after
  Niri has handed focus back to the recovered surface.
- A disposable combined Niri/Melibea/Celestina session proved two-window
  reconstruction, independent restore and close through revision 5. Celestina's
  full 0.32.0 production suite passed and the verified bundle was deployed.

### Exit

- Minimizing a window removes it from niri's strip and adds one shell bubble.
- Multiple minimized windows form one compact, operable group.
- Restarting Celestina reconstructs the group from a fresh snapshot.
- Restoring or closing an item updates both niri and the shell without residue.

### Gate to M7

Icons and titles have been used long enough to show whether live previews are
actually necessary for disambiguation.

**Decision: pass without previews on 2026-08-18.** The author confirmed that
the deployed icon-and-title interaction works as intended and needs no window
preview. This answers the M7 product decision only; it does not claim the
multi-output, reduced-motion, or assistive-technology cases in Celestina's
complete `VAL-BUBBLE-1` matrix.

## M7 — Coordinated bubble motion

**Status:** complete, with one exit criterion deferred

**Estimated effort:** several focused sessions to two weeks

### Outcome

Minimize and restore transitions visually connect a window with its shell
bubble without making animation responsible for window state.

### In

- Protocol v2 with an optional, action-scoped output-local logical anchor.
- Explicit per-action reduced-motion behavior.
- Backward-compatible protocol-v1 clients and compositor actions without a
  transition hint.
- A stable Celestina anchor that exists before the first minimized window and
  does not move when the overlapping group grows.
- Niri-owned minimize and restore trajectories between the current window
  geometry and that anchor.
- Deterministic fallback to the existing compositor-local transition when the
  anchor or output is unavailable or invalid.
- Automated compatibility, geometry, state-authority, and interruption tests.

### Out

- Live or static previews of another application's pixels.
- Persistent anchor registration, leases, heartbeats, or shell-owned window
  state.
- Cross-output snapshot travel; the first anchored transition is valid only
  when the window and bubble belong to the same output.
- Custom duration, easing, or shader policy in the shell protocol.
- A taskbar, semantic grouping, contexts, or learned behavior.

### Work

1. Freeze protocol v2 while keeping every protocol-v1 request and response
   compatible.
2. Forward one optional transition hint from a shell through Melibea to Niri
   without storing it.
3. Reserve one non-visual bubble slot in Celestina and route both minimize and
   restore through its current output-local rectangle.
4. Animate the retained Niri snapshot toward that rectangle and the restored
   tile away from it, while committing lifecycle state immediately.
5. Exercise disabled motion, invalid or removed outputs, immediate reversal,
   tiled and floating windows, and transient families before one final session
   activation.

The M7 interruption contract is state and artifact safety, not fractional
reversibility of animation progress. A new action cancels the matching retained
snapshot by window identity and starts its own transition without delaying the
lifecycle change. In the narrow restore-to-minimize race, Niri may normalize
one frame to the restored geometry before beginning the close; it must not
leave a duplicate surface, closing ghost, or stale minimized state.

### Exit

- Existing protocol-v1 clients behave exactly as before.
- A protocol-v2 client can request anchored or disabled motion per action.
- Minimizing the first window has a real destination before its bubble exists.
- Adding more bubbles does not move the front bubble or its anchor.
- Niri remains correct when no usable anchor exists and never waits for an
  animation to change authoritative state.
- A nested session shows minimize ending at the bubble and restore beginning
  there for the same window identity.
- Reduced motion changes state without spatial or scale animation.

Melibea will not grant arbitrary clients access to other applications' surface
pixels.

### Deployed state at close, 2026-08-19

Protocol v2 is implemented and tested end-to-end in a nested harness (41 of 41
checks: version negotiation, anchored and disabled motion, tiled, floating, and
transient-family minimize/restore, and the real Celestina shell driven with
key input). Celestina 1.0.0 speaks it, including a downgrade: when Melibea
answers `incompatible_version`, the same action is resent under protocol v1
without the transition, since a refused envelope means nothing moved and
resending is safe.

The real session's installed Melibea and the companion Niri M7 patch were
**not** rebuilt or redeployed in this closing session; the Niri M7 source
copy was lost with `/tmp` per [M7-HANDOFF.md](M7-HANDOFF.md) and was never
reconstructed. Minimize and restore work today in the real session through
that v1 downgrade path — correctly, but without the coordinated visual
travel this milestone was built to add. The fifth exit criterion above, "a
nested session shows minimize ending at the bubble," was met only inside the
harness, not in the deployed session.

## M8 — Optional expansions

**Status:** later, uncommitted

Candidates, admitted only by demonstrated daily value:

- Native visual contraction without resizing client content.
- Profiles or policies scoped by named workspace.
- Manual grouping of minimized bubbles.
- Urgency and attention cues that do not steal focus.
- Support for additional shell consumers.
- Upstreamable general niri actions discovered by Melibea.

Each candidate requires its own active milestone before implementation.

## Global non-goals

- A new compositor or replacement for niri.
- A general plugin collection competing with Piri.
- AI classification, habit learning, or unpredictable automatic placement.
- Contexts, scenes, or automatic project inference.
- Hidden, off-screen, or ordinary workspaces presented as real minimization.
- Requiring Celestina or any graphical shell to run Melibea.
- Reimplementing niri's layout, rendering, or focus authority in an external
  daemon.
