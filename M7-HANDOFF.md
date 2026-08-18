# M7 paused checkpoint and cross-session handoff

> ## Superseded in part on 2026-08-18 after a reboot destroyed `/tmp`
>
> The machine rebooted at 13:25 on 2026-08-18 and emptied `/tmp`. Every volatile
> artifact this document points at is gone and unrecoverable: the Niri M7 patch,
> the preserved nested Niri binary, the nested harness, and the isolated
> Celestina copy. All hashes and `/tmp` paths below are historical records, not
> live truth.
>
> What survived, and what was rebuilt afterwards:
>
> - Melibea M7 is committed at `m7-protocol-v2` (`258cfb0`) instead of sitting
>   uncommitted. Its wire form is unchanged.
> - The Niri M6 baseline survived in `/home/toni/CODIGO/NIRI-MELIBEA` and the M7
>   motion layer was **reimplemented from scratch** on top of it. It now lives in
>   that real checkout as uncommitted work, not in a `/tmp` patch.
> - The nested harness was rebuilt at
>   `/home/toni/CODIGO/.m7-recovery-2026-08-18/nested/nest.py`, without Celestina.
> - A persistent backup of both worktrees lives in
>   `/home/toni/CODIGO/.m7-recovery-2026-08-18/`.
>
> The Celestina sections below (P2-A, P2-B, the completion sequence, and the
> activation plan) are **still outstanding and were not attempted**. That work
> must be reconstructed from the current real Celestina checkout, because the
> isolated copy and its patch no longer exist.
>
> See "Reimplemented M7 state" at the end of this file for what is now true.

**Checkpoint:** `M7-PAUSED-2026-08-18`

**Authority:** the author explicitly paused the work and asked for a durable,
self-contained handoff. This document records state; it does not authorize a
deployment, service restart, live configuration change, commit, push, reset,
clean, stash, or unrelated refactor.

**Current outcome:** protocol v2 in Melibea and the Niri motion implementation
are complete enough for final integration. Celestina is paused in an isolated
copy with one newly added provider-side guard not yet revalidated and one known
host-parser defect still unfixed. Nothing from M7 has been activated in the
real desktop session, and the full nested matrix has not run.

No worker or reviewer remains active at this checkpoint; both in-progress
Celestina tasks were explicitly interrupted when the author requested the
pause.

## Resume in one minute

1. Read this file completely.
2. In Celestina, run `python3 scripts/agent-context.py celestina` and read every
   returned document before editing anything.
3. Verify all paths and hashes in this file; `/tmp` is volatile and live state
   can drift.
4. Continue in `/tmp/celestina-m7-work`. Do **not** apply the existing
   Celestina patch: it predates the last interrupted edit.
5. Finish and test the two P2 items in [The exact interrupted point](#the-exact-interrupted-point).
6. Regenerate and independently review the Celestina patch.
7. Run the full isolated nested harness.
8. Only if all of that is green, apply, build, back up, deploy, and perform the
   single planned live Niri restart in the order documented below.

## Product and authority boundaries

These decisions are settled for M7:

- Melibea is an independent MIT project. Celestina may consume it but does not
  own Melibea policy or authoritative window state.
- Niri alone owns surfaces, layout, focus, rendering, input exclusion,
  minimization lifetime, restoration, and animation rendering.
- Celestina owns only the shell presentation and the current bubble-slot
  rectangle.
- Melibea validates and forwards an action-scoped transition hint. It never
  stores an anchor, creates a lease, or becomes a second state authority.
- Behavior is deterministic, explicit, and reversible. There is no AI,
  learning, heuristic activity classification, or per-window bureaucracy.
- Native minimization removes a window from normal layout and navigation. A
  hidden workspace or off-screen scratchpad is not an acceptable substitute.
- Icons and titles are sufficient. M7 contains no live/static previews and no
  application-pixel API.
- State changes never wait for animation completion.

The M7 transition contract is:

```text
WindowTransition
  anchored { anchor: BubbleAnchor }
  disabled

BubbleAnchor
  output: String
  x, y, width, height: finite f64
```

The rectangle uses output-local logical coordinates. `transition` is optional
only for protocol v2 minimize/restore actions. Omission requests ordinary Niri
motion. Explicit JSON `null` is invalid. Protocol v1 rejects the field and
retains its exact previous wire representation. Invalid, stale, out-of-bounds,
missing-output, or cross-output anchors fall back to generic local Niri motion.
`disabled` changes state immediately without spatial, scale, or opacity motion.

Canonical project documents:

- `PROJECT.md`
- `ROADMAP.md`, active milestone M7
- `PROTOCOL.md`, frozen v1
- `PROTOCOL-V2.md`, action-scoped v2 extension

## Repository and worktree manifest

All revisions and hashes below were verified on 2026-08-18. Recheck them on
resume rather than treating this table as live truth.

| Area | Path | Branch / base | State at pause |
|---|---|---|---|
| Melibea | `/home/toni/CODIGO/MELIBEA` | `main`, `f5d7942e38033e56764c9bac5ecd30d92ad726c5` | M1-M7 work is uncommitted; protocol v2 source is here. Preserve all changes. |
| Niri real checkout | `/home/toni/CODIGO/NIRI-MELIBEA` | `codex/melibea-native-minimization`, base `8ed0da44d974c32c6877d2f4630c314da0717ecb` | Dirty M6 baseline only; M7 patch still applies cleanly. Do not reset or clean it. |
| Niri M7 source copy | `/tmp/niri-m7-work` | same base/index model | Completed M7 source used to generate the final patch. Volatile. |
| Celestina real checkout | `/home/toni/CODIGO/CELESTINA` | `main`, `c79588c0a45a399aeb8c7dc8e7046278920fb9b3` | Dirty BUBBLE-1 plus unrelated author work, including `denseglass.cpp/.h`. No M7 patch applied. |
| Celestina M7 isolated copy | `/tmp/celestina-m7-work` | same Celestina base | BUBBLE-1 is the staged baseline; M7 is the unstaged/intent-to-add delta. One P2 fix is present but unverified; one remains. |
| Nested harness | `/tmp/melibea-m7-nested` | not a repository | Static checks and external preflight passed; full compositor run never started. |

### Dirty-worktree rules

- Do not use `git reset`, `git checkout`, `git clean`, `git stash`, or
  `git apply --index` in any of the three repositories.
- Do not stage, commit, or push unless the author separately asks for it.
- Preserve unrelated Celestina edits, especially `celestina/src/denseglass.cpp`
  and `celestina/src/denseglass.h`.
- In `/tmp/celestina-m7-work`, the index deliberately represents the BUBBLE-1
  baseline and the unstaged diff represents M7. Do not casually stage or
  unstage files: that destroys the clean incremental patch boundary.
- A normal `git diff` in the real Celestina checkout is not an M7 patch because
  it also contains BUBBLE-1 and unrelated author changes.

## Artifact manifest

### Final Niri M7 patch

```text
path:   /tmp/melibea-m7-niri.patch
bytes:  107024
sha256: d18232eb9c853a3cbabf1e1dfa1640c37a3fef512429837077e93faf709a5d1d
scope:  16 files, 1803 insertions, 144 deletions
```

It contains the new tracked module `src/layout/window_transition.rs`; any
replacement patch must include its `new file mode 100644` entry. The patch was
applied to an exact materialization of the M6 index, compared byte-for-byte,
reverse-checked, reversed, and reapply-checked. It still passed
`git apply --check` against the real Niri checkout at this pause.

The preserved nested release binary is:

```text
path:   /tmp/niri-m7-target/release/niri
bytes:  135883728
sha256: 7248ee6666f4213cda8ce000b8fcc14ba9d91311d39b0b9152b29a6eecc7ec57
version: niri 26.04 (v26.04-modified)
```

### Stale Celestina patch — do not apply

```text
path:   /tmp/melibea-m7-celestina.patch
bytes:  111448
sha256: 8178070c74e07b375c400838746ff5d1004cbf9b42c1ced4b5007f3ea1070a3b
```

This patch is internally valid and still applies to the real Celestina
checkout, but it predates the provider-side readiness fix added immediately
before the pause. It also lacks the still-pending `confirmed` parser fix. It is
only a reconstruction base if the isolated copy disappears; it is not a final
integration artifact.

The current paused unstaged diff in `/tmp/celestina-m7-work` had this identity:

```text
bytes:  112961
lines:  2891
sha256: 5f559f188ca12d9d3f7963d31a9fcbdb0ff0e879d9055e415676c82df5ab5ccb
```

The deliberate staged BUBBLE-1 baseline diff in that copy had:

```text
bytes:  145765
sha256: f114fe29309dbd065fc14982ca12b56a04a0492fcc52a1bea5bc878ecb487a04
```

Use these hashes only to detect drift at the first resumed inspection. The
unstaged hash must change when the two remaining fixes are completed.

## Completed work

### Melibea

The real Melibea worktree already contains the dual protocol implementation:

- daemon accepts versions 1 and 2;
- each subscriber retains its negotiated version;
- v1 JSON remains exact and v1 rejects a `transition` field, including `null`;
- v2 minimize/restore accept optional `anchored` or `disabled` transitions;
- unsupported future versions return `incompatible_version` with `[1, 2]`
  before attempting to decode a future request body;
- transition validation is strict and forwarding is action-scoped;
- CLI commands remain v1 and therefore retain compatibility;
- daemon diagnostics correctly advertise v1/v2.

Last recorded clean evidence before the pause:

- 73 library tests and 12 binary tests passed;
- `cargo fmt --check` passed;
- strict Clippy passed;
- `git diff --check` passed.

Do not rerun this suite merely to rediscover state, but do rerun it before a
final build or if any Melibea source changes.

### Niri

The final patch implements:

- optional IPC transition fields without changing legacy serialization;
- same-output validation for the whole transient family before visual cleanup;
- output identity, hot-unplug bounds, and overview reprojection;
- correct scrolling viewport/content coordinate conversion;
- compositor-local transforms for anchored trajectories;
- tiled and floating minimize/restore motion;
- exact closing-snapshot cancellation by window identity;
- `disabled` cleanup across open/close/layout/tile/floating movement while
  preserving the distinct legacy global `window-movement off` behavior;
- generic restore cancellation of only the exact stale close artifact while
  retaining ordinary open motion.

Last recorded evidence:

- `cargo check --offline --tests` passed;
- all 138 layout tests passed;
- 9 transition tests, 4 anchor tests, 1 disabled-motion test, 1 global-off vs
  disabled regression, 1 generic cleanup/open regression, 7 `niri-ipc` tests,
  its doctest, and the `niri-config` conversion test passed;
- strict Clippy passed for `niri-ipc` and `niri-config`;
- Niri Clippy passed with only two explicitly inherited allowances;
- focused rustfmt and `git diff --check` passed.

Known environmental/semantic limits:

- the complete Niri library suite reached the EGL-backed tests and stalled in
  this environment; the final layout subset is green, but do not claim the EGL
  suite passed;
- global `cargo fmt --all --check` reports only inherited formatting in
  `src/protocols/foreign_toplevel.rs`, which is outside M7;
- a very fast anchored restore-to-minimize reversal guarantees immediate state,
  exact cancellation, and no ghost, but may normalize one frame to restored
  geometry before beginning the new close. Fractionally continuous reversal is
  explicitly outside M7.

### Celestina before the final interruption

The isolated M7 copy already includes:

- v1 subscription compatibility and v2 minimize/restore actions;
- v1 close action;
- strict action options, exact window IDs, transition validation, and hostile
  input rejection;
- authoritative presence confirmation for minimize and authoritative absence
  confirmation for restore/close;
- a bounded separate shell request ledger with an 8-second outer timeout;
- a current, output-local anchor queried at action time;
- one permanent 22 by 22 logical bubble slot that is non-painting, input-inert,
  hidden from accessibility, present before the first bubble, and stable at the
  front/right edge while the group grows left;
- reduced-motion forwarding as `disabled`;
- strict public `celestina msg minimize` option handling;
- exact per-operation provider option keys;
- shell-side gating on `providers["melibea"]["available"]` being the actual
  boolean `true`, not global helper availability or a truthy string;
- BUBBLE-2 plan/roadmap/evidence and `VAL-BUBBLE-2` coverage text.

Before the two late P2 findings, recorded evidence included Rust tests and
strict Clippy, the focal C++ build, BubbleGroup 6/6, BubbleSelector 12/12,
qmllint, architecture/diff guards, and the shell-service test under a private
D-Bus session at 18/18. These results do not validate the newly added guard or
the still-missing parser change.

## The exact interrupted point

There are two confirmed P2 defects. Resume here; do not broaden scope.

### P2-A — provider could act after its authoritative projection was withdrawn

Problem: the shell frame may still say Melibea is available for a short time
after the provider subscription has already withdrawn its projection. The
provider previously checked only pending capacity, then opened a separate
action connection. It could minimize a window even though it no longer had an
authoritative projection from which to publish the bubble or confirm success.

Current WIP state: `/tmp/celestina-m7-work/celestina/src/provider_adapter/melibea.rs`
already contains an interrupted fix:

- `BridgeState::can_dispatch` rejects `!projection.ready()` and then checks
  pending capacity/duplicate request IDs;
- `request_action` validates and encodes first, then calls
  `lock_state().can_dispatch(request_id)` immediately before
  `UnixStream::connect`;
- regressions named
  `capacity_and_duplicate_ids_are_refused_before_an_action_can_run` and
  `dispatch_requires_a_ready_authoritative_projection` are present.

This code was interrupted before a completion report. Inspect it, preserve the
guard immediately before socket IO, and test it. The unavoidable loss *after*
that guard remains an ordinary race handled by reserve/arm, reconnect/failure,
and the bounded timeout; do not invent rollback of an already accepted Niri
state change.

### P2-B — the C++ host rejects the adapter's terminal `confirmed` result

Problem: the Rust adapter emits `accepted` and then terminal `confirmed`, but
`readResult` in
`/tmp/celestina-m7-work/celestina/src/providerstates.cpp` currently accepts
only `accepted` and `failed`. It therefore drops a valid confirmation, leaving
the shell's Melibea ledger pending until its 8-second timeout even though Niri
already completed the action.

Current WIP state: **not fixed**. The paused isolated code still contains the
two-state condition:

```cpp
if (outcome != QStringLiteral("accepted")
    && outcome != QStringLiteral("failed"))
```

Required narrow fix:

1. admit `confirmed` as a valid result state in `readResult`;
2. add a parser/effect regression proving it becomes a result/answer rather
   than an invalid frame;
3. add or extend a host/ledger path test proving `accepted` leaves the Melibea
   request pending and `confirmed` settles it successfully without waiting for
   expiry;
4. retain rejection of unknown states and hostile IDs/reasons.

`MelibeaRequests::acknowledge` already treats `confirmed` as terminal, so do not
duplicate that policy elsewhere.

## Required Celestina completion sequence

1. Recheck the WIP identity and current diff:

   ```sh
   cd /tmp/celestina-m7-work
   git status --short --branch
   git diff --check
   git diff --binary | sha256sum
   ```

2. Complete P2-A and P2-B without touching the staged BUBBLE-1 baseline.
3. Run focused provider-core, provider-adapter, provider-state,
   MelibeaRequests, shell-service, BubbleGroup, and BubbleSelector tests.
4. Run strict Rust Clippy, QML lint, architecture guard, and diff check.
5. Rebuild the isolated Celestina host and both adapters. Existing
   `/tmp/celestina-m7-cmake` contents are not reliable: `ctest -N` showed many
   test executables missing after cache cleanup.
6. Run the shell-service test with a private bus, not the live user bus:

   ```sh
   dbus-run-session -- env QT_QPA_PLATFORM=offscreen CELESTINA_SHELL_SCALE=1 \
     /tmp/celestina-m7-cmake/celestina-shell-service-test
   ```

7. Run the repository's canonical verification discovered by
   `scripts/agent-context.py`; distinguish sandbox/environment failures from
   product failures and do not overclaim skipped tests.
8. Require a final stable-delta review with zero unresolved P1/P2 findings.
9. Regenerate the incremental M7 patch only after the last code/test fix:

   ```sh
   cd /tmp/celestina-m7-work
   git diff --check
   git diff --binary --output=/tmp/melibea-m7-celestina.patch
   sha256sum /tmp/melibea-m7-celestina.patch
   wc -c -l /tmp/melibea-m7-celestina.patch
   git -C /home/toni/CODIGO/CELESTINA apply --check \
     /tmp/melibea-m7-celestina.patch
   ```

10. Confirm the regenerated patch excludes the staged BUBBLE-1 baseline,
    `denseglass.cpp/.h`, and unrelated version-history work unless those paths
    are genuinely part of the already registered M7 unit.

If `/tmp/celestina-m7-work` is missing, do not edit the real checkout to
reconstruct blindly. Create a new isolated copy of the current real Celestina
worktree, preserve its BUBBLE-1 baseline, apply the stale patch only inside that
copy, implement both P2 fixes, then repeat the full sequence above.

## Nested end-to-end validation

Harness path: `/tmp/melibea-m7-nested`

Architecture:

```text
host Wayland socket only
  -> private dbus-run-session
     -> nested Niri winit
        -> isolated runtime/config/cache/data/state directories
        -> Melibea daemon behind an exact-byte Unix proxy
        -> private Celestina host and adapters
        -> disposable Kitty clients
```

It never receives the host `NIRI_SOCKET`, never edits HOME configuration, never
uses the user bus, never invokes `systemctl`, and cleans up only PIDs whose PID,
`/proc` start time, command-line fragment, and private runtime directory still
match the run record. It uses no `pkill` or `killall`. Evidence is preserved in
a unique `/tmp/melibea-m7-nested-run.*` directory.

Static validation and the external `--preflight` already passed, including
Niri/Melibea config validation, Python compilation, exact-byte proxy self-test,
artifact selection, style resolution, and the preserved Niri hash. The full
run never started.

After the final Celestina build is selected, rerun preflight and then the full
matrix with the same explicit artifact paths:

```sh
/tmp/melibea-m7-nested/run.sh --preflight
/tmp/melibea-m7-nested/run.sh
```

Unix sockets and the nested GUI may require execution outside the filesystem
sandbox. Request only the narrow permission necessary to run this harness; do
not broaden it into live-session authority.

The matrix must cover:

- shell-originated anchored minimize into an empty slot;
- a second anchored minimize proving the front slot/anchor does not move as the
  group grows;
- tiled and floating windows;
- anchored restore;
- transition omission producing generic Niri motion;
- missing/unknown output producing generic fallback without failed lifecycle;
- `disabled` minimize and restore with immediate state and no spatial/scale
  animation;
- immediate minimize/restore reversal with no ghost or stale minimized state;
- exact v2 NDJSON captured by the proxy;
- authoritative Niri/Melibea state and timed screenshots.

The winit backend exposes only one output. It cannot prove real multi-output
hotplug or migration; keep those cases in author validation. IPC state alone
cannot prove a one-frame ghost, so inspect the preserved screenshots rather
than declaring success from JSON only.

If `/tmp/melibea-m7-nested` is missing, reconstruct it as an isolated
`dbus-run-session -> niri winit -> inner harness` with the invariants above.
Do not replace it with a test against the live session.

## Live state snapshot and activation guard

This was the last read-only live audit on 2026-08-18. It may drift and must be
rechecked immediately before activation:

- Niri: `niri.service`, executable
  `/home/toni/.local/lib/celestina/niri`, old hash
  `db7af1422405cb6865a0fa8fa0820013a94fea60cc9d4c17e58e5de4fa930d49`.
- Melibea: `melibea.service`, executable `/home/toni/.local/bin/melibea`, old
  hash `5f67abc8972157f7c8f5eb8879ef914e23975066e4e7e0868d0dae7405bd1cb2`.
- Niri starts/restarts Melibea through `spawn-at-startup systemctl --user
  restart melibea.service`.
- Celestina has no service or active autostart. Its last audited live D-Bus
  owner executed the build-tree host
  `/home/toni/CODIGO/CELESTINA/celestina/build/celestina` inside the Codex/Claude
  application scope. Never stop that whole scope.
- Installed Celestina host hash was
  `86240d2c0fce60761b146508fad4a945c2d8df0510c5eff7190e3e9d7b0756ae`;
  it differed from the live build-tree host.
- Live Niri config: `/home/toni/.config/niri/config.kdl`.
- Last verified binds:

  ```kdl
  Mod+N repeat=false { minimize-window; }
  Mod+M repeat=false { spawn "celestina" "msg" "bubbles-toggle"; }
  ```

- No `spawn-at-startup "celestina"` existed.

At this pause, no M7 binary was installed, no Niri or Celestina M7 patch was
applied to a real checkout, no live config was changed, and no
Niri/Melibea/Celestina process was restarted for M7. Melibea's uncommitted M7
source is intentionally present in its real development worktree, as recorded
above; it is not the installed daemon.

## Final application and single-restart plan

Do not begin this section until Melibea, final Celestina, final Niri, final
review, and the nested matrix are all green.

1. Recheck every real worktree and patch with `git status`, `git diff --check`,
   SHA-256, and `git apply --check`.
2. Apply the final Niri and Celestina patches without `--index`. Do not alter
   the Melibea worktree boundary or unrelated dirty changes.
3. Build and verify Melibea and Niri from their real checkouts. Build Niri from
   `/home/toni/CODIGO/NIRI-MELIBEA`; do not use Celestina's
   `build-patched-niri.sh`, which reconstructs another source tree and would
   omit the combined M6/M7 work.
4. Before replacing anything, create a persistent backup directory outside
   `/tmp`, record hashes, and copy:
   - installed Niri and Melibea binaries;
   - the complete installed Celestina bundle, wrapper, desktop entry, style,
     and adapters;
   - current Niri config and relevant user-unit/drop-in files;
   - the live Celestina build-tree host/adapters if still used;
   - all final patches and dirty-worktree diffs/inventories.
5. Resolve the exact owner PID of `org.celestina.Shell` with `busctl --user
   status`, validate a numeric PID and exact `/proc/PID/exe` against the allowed
   build/bundle path, send `SIGTERM` only to that PID, and wait boundedly. Do not
   use `pkill`, stop the surrounding application scope, or kill by name.
6. From `/home/toni/CODIGO/CELESTINA/celestina`, run the canonical
   `scripts/complete-production.sh`. It builds, verifies, deploys, and reports
   status without activating the live shell.
7. Install Niri and Melibea atomically through temporary files in their target
   directories followed by `mv`; never overwrite a running executable in
   place.
8. Restart only `melibea.service` first and verify its new PID, mapped binary
   hash, v1/v2 daemon diagnostics, and socket behavior.
9. Add `spawn-at-startup "celestina"` to Niri config but temporarily retain the
   old native `Mod+N { minimize-window; }` bind. `spawn-at-startup` runs only on
   Niri startup, whereas binds reload live.
10. Validate the config with the newly built Niri.
11. Perform the one planned `systemctl --user restart niri.service`.
12. Verify the new Niri process/hash, Melibea service/socket, new Celestina
    D-Bus owner and installed host/adapters, then inspect logs.
13. Only after all three are healthy, change the live-reloaded bind to:

    ```kdl
    Mod+N repeat=false { spawn "celestina" "msg" "minimize"; }
    ```

14. Exercise one real minimize/restore, reduced-motion case, bubble grouping,
    selector, and recovery/reconstruction cycle. Record author-only perceptual
    and multi-output observations in Celestina `VALIDATION.md` rather than
    claiming them from automated tests.

This ordering avoids any window where the new key bind points at an old shell
or old Melibea daemon.

## Rollback boundary

Before the single Niri restart, rollback can restore old files/config and
restart only Melibea/Celestina; Niri is still running the old mapped binary.

After the new Niri process starts:

- the bind can immediately return to native `minimize-window` through live
  config reload;
- prior files can be restored from the persistent backup;
- but complete binary rollback necessarily requires a second Niri restart.

Do not claim otherwise: an already running compositor cannot replace its own
mapped executable bytes without another process start.

## M7 exit checklist

M7 is not complete until every item is true:

- [ ] P2-A provider-side projection guard reviewed and green.
- [ ] P2-B `confirmed` host parser/ledger path reviewed and green.
- [ ] Final Celestina patch regenerated after those fixes, hashed, complete,
      cleanly applicable, and free of unrelated work.
- [ ] Final adversarial review has no unresolved P1/P2 finding.
- [ ] Melibea v1 exact compatibility and v2 tests remain green.
- [ ] Niri patch and targeted motion/lifecycle tests remain green.
- [ ] Full nested harness passes and its state plus screenshots are reviewed.
- [ ] Persistent pre-activation backup and rollback manifest exist.
- [ ] Real Niri, Melibea, and Celestina bytes match the verified artifacts.
- [ ] Exactly one planned Niri restart activates the new stack.
- [ ] `Mod+N` is switched only after the new stack is healthy.
- [ ] Real-session author validation records multi-output, perceptual motion,
      reduced motion, selector, and reconstruction results.

Until those boxes are satisfied, the honest status remains: **M7 active,
paused before final Celestina correction and isolated end-to-end validation**.


## Reimplemented M7 state (2026-08-18, after the `/tmp` loss)

### Melibea

Committed on branch `m7-protocol-v2` as `258cfb0`. Unchanged in behavior:
73 library tests, 12 binary tests, `cargo fmt --check`, and `git diff --check`
all pass.

### Niri

The M7 motion layer was rewritten on the surviving M6 baseline in
`/home/toni/CODIGO/NIRI-MELIBEA` (branch `codex/melibea-native-minimization`,
uncommitted, per this document's dirty-worktree rules). It is materially
smaller than the lost version because it reuses M6's existing close/open
visuals rather than introducing a parallel animation path.

Changes:

- `niri-ipc`: `WindowTransition` (`anchored`/`disabled`) and `BubbleAnchor`,
  plus an optional `transition` field on `MinimizeWindow`/`RestoreWindow`.
  Legacy serialization is byte-identical and pinned by test.
- `niri-config`: dedicated `MinimizeWindowWithTransition` /
  `RestoreWindowWithTransition` variants, unreachable from a key bind, so the
  bind-reachable variants keep their exact prior meaning.
- `src/layout/window_transition.rs`: new module resolving a hint against live
  output state and reprojecting an output-local anchor into workspace-local
  coordinates. Every unusable case degrades to generic motion.
- `Layout::resolve_window_transition` resolves at action time and stores
  nothing.
- `ClosingWindow` and `OpenAnimation` gained optional travel; both deliberately
  bypass the close/open shaders, which are in-place dissolves that would fight a
  trajectory.
- `src/input/mod.rs`: the three duplicated minimize/restore arms were unified
  into `act_minimize`/`act_restore`, which accept the hint.

**One real integration defect was found and fixed.** Melibea already emitted
`{"type":"anchored",...}` (internally tagged, lowercase) toward Niri, while the
new `niri-ipc` enum defaulted to `{"Anchored":{...}}`. The two would not have
interoperated. `niri-ipc` now uses `#[serde(tag = "type", rename_all =
"lowercase")]`, matching the frozen shell-facing spelling, and both sides pin
the exact string in tests.

Evidence:

- 140 layout tests pass, including 9 new `window_transition` unit tests and 6
  new `Layout`-level tests.
- 8 `niri-ipc` tests, its doctest, and 20 `niri-config` tests pass.
- Strict Clippy is clean for `niri-ipc` and `niri-config`. The Niri lib reports
  three warnings, all in files M7 does not touch
  (`ipc/client.rs`, `protocols/ext_workspace.rs`,
  `protocols/output_management.rs`).
- `git diff --check` passes and the M7 files are rustfmt-clean.

Not claimed: the EGL-backed Niri suite was not run, and `cargo fmt --all
--check` still reports inherited drift in `src/protocols/foreign_toplevel.rs`,
which M7 leaves untouched.

### Nested validation

`/home/toni/CODIGO/.m7-recovery-2026-08-18/nested/nest.py` rebuilds the isolated
`dbus-run-session -> nested niri (winit) -> Melibea -> disposable client` harness
and keeps the original invariants: no host `NIRI_SOCKET`, no HOME configuration
edits, no user bus, no `systemctl`, and teardown that matches PID, `/proc` start
time, command-line fragment, and private runtime directory before signalling
anything. It uses no `pkill` or `killall`. Celestina is out of scope.

All 12 checks pass against a real nested compositor:

- protocol v1 rejects a `transition` field, and v1 without one still applies;
- anchored minimize and restore apply, and the window really leaves and rejoins
  the visible layout;
- `disabled` minimize and restore apply and leave consistent state;
- an unknown output falls back without failing the lifecycle change;
- immediate minimize/restore reversal leaves no duplicate, ghost, or stale
  minimized state;
- protocol v1 `list` still works after v2 traffic.

Exact v2 NDJSON is preserved per run under `evidence/protocol.ndjson`.

**Not proven by this harness**, and still author-only validation: perceptual
motion quality, multi-output hotplug and migration (winit exposes one output),
and the one-frame ghost case, which needs screenshots rather than IPC state.

### Celestina: both P2 items are done, in an isolated copy only

Work happened exclusively in `/home/toni/CODIGO/.m7-recovery-2026-08-18/celestina-nest`,
a copy of the real worktree whose index holds BUBBLE-1 plus the author's
unrelated work as the baseline, so the unstaged diff is exactly the new delta.
**The real checkout at `/home/toni/CODIGO/CELESTINA` was never modified**, and
nothing was applied, built into, or deployed from it.

Both defects turned out to be **live in the deployed BUBBLE-1 baseline**, not
merely artifacts of the lost M7 copy. The handoff described them as M7 findings
because that is where they were noticed.

**P2-B — the host dropped every terminal `confirmed`.** `celestina-shell-core`
emits `state: "confirmed"` for any provider whose effect something still has to
observe, while `readResult` admitted only `accepted` and `failed` and turned the
confirmation into an invalid frame. `RequestLedger::result` already handled
`confirmed` correctly, so the fix is confined to the parser. The reason this
survived is that `requestledger_test.cpp` drove the ledger directly and never
crossed the parser; the new regression drives the same decision
`ShellProvidersClient::applyLine` makes on each line, so the join is covered.
Both new tests were verified to fail with the fix reverted and pass with it.

**P2-A — an action could run without an authoritative projection.**
`request_action` connected and acted first, and capacity was only checked
afterwards in `reserve`, so a refused action could already have moved a window
in Niri. `BridgeState::can_dispatch` now rejects a withdrawn projection, a full
pending table, and a duplicate request id, and `request_action` validates and
encodes before calling it immediately ahead of `UnixStream::connect`. Losing the
projection after that guard remains an ordinary race handled by reserve/arm,
reconnection failure, and the bounded timeout; no accepted Niri state change is
rolled back.

Evidence, all from the isolated copy:

- 14 provider-states tests and 15 request-ledger tests pass under
  `QT_QPA_PLATFORM=offscreen`.
- 129 Rust tests pass and strict Clippy is clean.
- The architecture contract passes. The language contract reports only
  pre-existing debt in `siderita/`, which this delta does not touch.
- `git diff --check` passes.

Incremental patch, containing only the five files this work changed and none of
the BUBBLE-1 baseline or unrelated author work:

```text
path:   /home/toni/CODIGO/.m7-recovery-2026-08-18/celestina-p2-fixes.patch
bytes:  13618
lines:  299
sha256: b0fe1d2ef58ffbb8077df9bd649546eef49d627f11397f7f6dea237ff03bdffe
scope:  5 files, 185 insertions, 7 deletions
```

It passes `git apply --check` against the real checkout. It was deliberately
**not** applied.

### Celestina M7 delta: rewritten as far as the wire and the adapter

The delta lost with `/tmp` is being rewritten in the same isolated copy. Two of
its four parts are complete and tested; the two that remain are shell plumbing.

**Done — protocol v2 in `celestina-shell-core::melibea`.** `PROTOCOL_VERSION_V2`
and `SUPPORTED_PROTOCOL_VERSIONS` were added, `decode_message` now reads a v1 or
v2 reply, and `WindowTransition`/`BubbleAnchor` carry an action-scoped hint.
`Request` gained a `Minimize` variant and an optional `transition` on minimize
and restore, skipped entirely when absent so an explicit `null` can never be
sent. `Eq` was dropped from the request types because an anchor carries `f64`.

The version is chosen by the request, not by the shell:
`action_with_transition` stays on v1 whenever there is no hint, so a Melibea
that speaks only v1 still serves every ordinary action. Close refuses a hint
rather than sending one Melibea would ignore, because it has no destination to
travel to. One pre-existing test asserted that version 2 was incompatible; that
assertion encoded the v1-only assumption this work repeals, so it now names a
genuinely unsupported version and a new test owns the v1/v2 boundary.

**Done — the provider adapter.** It serves `minimize` as well as `restore` and
`close`, reads the hint from each action's own options rather than remembering
one, and refuses an unknown kind, a missing output, a missing coordinate, or an
unusable rectangle locally instead of sending it on.

Confirmation is directional: minimize settles when the window becomes **present**
in the projection, restore and close when it becomes **absent**. `Settles`
carries that per request, so neither ever settles on the action's own optimistic
reply.

The option keys are flat, matching the existing `window_id` idiom:
`transition` is `"anchored"` or `"disabled"`; an anchored hint adds
`anchor_output`, `anchor_x`, `anchor_y`, `anchor_width`, `anchor_height`.

**Started — the permanent anchor slot.** `celestina/qml/BubbleAnchorSlot.qml` is
a non-painting, input-inert, accessibility-ignored 22x22 item that converts its
position to output-local logical coordinates by subtracting the screen origin
from `mapToGlobal`, and builds the transition options at action time.
Reduced motion wins over the anchor: it sends `disabled` rather than travel to a
different place.

It is placed in the panel's bubble cluster, after the group, which is the edge
the group grows away from. `PanelCluster` gained `showsGlass`, defaulting to
`hasContent`, so the cluster can reserve space for something invisible without a
glass pill sitting under nothing; every existing caller is unchanged.

Evidence: 130 Rust tests in `celestina` and 346 in `celestina-shell-core` pass,
strict Clippy is clean for both, and `qmllint` is clean for
`BubbleAnchorSlot.qml`, `PanelCluster.qml`, and `Panel.qml`.

### Both plumbing routes are decided and wired

The two open questions were how the anchor reaches an action that has no anchor slot of
its own. They were answered differently on purpose, because the two callers differ.

**Restore, from the selector: a snapshot taken when the surface opens.** The selector is a
separate surface from the panel, so the overlay controller reads `bubbleAnchorRect()` off
the opener panel and hands it over as an initial property, exactly as it already hands over
`openerRect` and `attachmentAnchorRect`. A snapshot is honest here: the overlay is anchored
to that panel for its whole short life, so a panel that had moved would already have
misplaced the card. Following the existing contract beat inventing a parallel live-query
mechanism. The property is inserted only for `BubbleSelector`, since no other overlay
declares it.

Unlike every other rectangle on that path, it is **not** translated into the carrier: it
stays in the compositor's output-local logical space, because it is going to Niri rather
than into the surface's own layout.

**Minimize, from a keybind or the CLI: a live query.** There is no surface to snapshot, so
`ShellService` asks for the anchor at action time. It resolves the **focused output** from
`NiriClient::focusedOutput()` — the focused window is the one Niri will minimize, so its
monitor is the one whose bubbles it should travel to.

`ShellService` does not take a `PanelManager` for this. It takes `BubbleAnchorSource`, a
two-method interface in the spirit of `RequestSink`: one rectangle and one preference. That
is what lets the verb's contract be tested without a QML engine, a screen, or a mapped
panel — the shell-service test could not even link `panelmanager.cpp`.

**A focused minimize names no window.** `Request::Minimize` carries
`window_id: Option<u64>` and sends an explicit JSON `null`, which PROTOCOL.md defines as
"resolve the focused window". This is not a convenience: reading focus in the shell and
acting on it afterwards would race, and Niri is the only authority on what is focused at the
instant the action lands. Melibea's reply names the window it resolved, and that resolved id
is what the confirmation then watches for.

Gating is strict: `providers["melibea"]["available"]` is checked as an actual boolean, since
a truthy string would claim availability for a provider that had published nothing. A window
id the caller does name must parse as an exact decimal. Every refusal is a D-Bus error
through `refuse`, never a silent zero.

`celestina msg minimize` needed no CLI registration: `msg` passes a verb straight to the
service. It is documented in `celestina/README.md`.

### Evidence

All from the isolated copy, with `QT_QPA_PLATFORM=offscreen` forced throughout:

- 14 shell-service tests, 14 provider-states tests, and 15 request-ledger tests pass; the
  shell-service run used a private `dbus-run-session`.
- 133 Rust tests in `celestina` and 347 in `celestina-shell-core` pass.
- Strict Clippy is clean for both crates.
- `qmllint` is clean for `BubbleAnchorSlot.qml`, `BubbleSelector.qml`, `BubbleGroup.qml`,
  `Panel.qml`, and `PanelCluster.qml`.
- The full `celestina` shell target builds.
- The architecture contract passes. The language and documentation contracts report only
  pre-existing debt in `siderita/`, which this delta does not touch.

### The nested matrix passed

Run `run-20260818-153357` under `/home/toni/CODIGO/.m7-recovery-2026-08-18/nested`, against
the debug Niri and Melibea binaries built at 14:18 from the current M7 source. All 12 checks
passed and the nested Niri log contains no panic or error.

What it proved on real compositor traffic, with the exact wire bytes preserved in
`evidence/protocol.ndjson`:

- v1 **rejects** a transition field rather than ignoring it, answering `invalid_request`
  with "window transitions require Melibea protocol version 2";
- a v1 minimize and restore with no transition still apply, byte-identical to before;
- anchored minimize and restore apply under v2, and the window really leaves and re-enters
  the visible layout (`minimized=[2]`, then empty);
- `disabled` applies and leaves state consistent;
- an anchor naming an output that does not exist still returns `applied` — it degrades to
  Niri's ordinary motion and never blocks or reverses the lifecycle change;
- an immediate reversal leaves no duplicate and no ghost (`minimized=[2] visible=[]`);
- a protocol-v1 `list` still works after v2 traffic on the same daemon.

Melibea published ten ordered revisions across the cycles, alternating one bubble and none.

**Scope limit, stated plainly:** this harness exercises Melibea against Niri only. Celestina
is deliberately out of it, so none of the Celestina delta — the anchor slot, the two anchor
routes, the shell-originated minimize — has been seen running. Its evidence is still only
unit-level.

### What M7 still needs

- QML tests for the anchor slot, and a `VAL-BUBBLE-2` matrix covering multi-output,
  reduced motion, and assistive technology.
- BUBBLE-2 plan, roadmap, and evidence text.
- A combined nested run that includes Celestina, which is what would actually show a
  minimize ending at the bubble and a restore beginning there.
- Tiled and floating windows and transient families, which the current matrix does not
  separate.
- A final adversarial review of the combined delta.
- The persistent pre-activation backup and the single planned Niri restart.

One judgement to confirm with the author: the anchor slot reserves permanent panel space,
so the bubble cluster no longer collapses to nothing when there are no minimized windows.
`PanelCluster` gained `showsGlass` so no glass pill sits under it, but the reserved width
is a visible layout decision and should be looked at on real hardware.

### Isolated-copy manifest

```text
copy:   /home/toni/CODIGO/.m7-recovery-2026-08-18/celestina-nest
build:  /home/toni/CODIGO/.m7-recovery-2026-08-18/celestina-build
patch:  /home/toni/CODIGO/.m7-recovery-2026-08-18/celestina-m7.patch
bytes:  67536
lines:  1576
sha256: 2724432f5baa282972dd18f095b0a5f9888cc2662c8fb805092cc4337a5c419d
scope:  21 files, 1028 insertions, 42 deletions
```

This lives on persistent storage rather than `/tmp`, which is what destroyed the
previous attempt. The index holds BUBBLE-1 plus the author's unrelated work, so
the unstaged diff is exactly this delta; `BubbleAnchorSlot.qml` and
`bubbleanchorsource.h` are intent-to-add, so the patch carries both
`new file mode` entries. It passes `git apply --check`
against the real checkout and was deliberately not applied.

### Environment warning

`scripts/check-architecture-contract.sh` includes a QML visual contract that
starts a QML engine. Running it without `QT_QPA_PLATFORM=offscreen` crashed the
GPU on this machine mid-session. Force the offscreen platform for it and for
every Qt test, and prefer bounded build parallelism over `-j$(nproc)`.
