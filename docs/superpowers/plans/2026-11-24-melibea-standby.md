# M12 Standby for games Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One niri bind toggles the focused game between running and frozen: only the game's processes stop, and niri shows its last frame blurred and dimmed without delivering input.

**Architecture:** Melibea recognizes the game (Steam app id or a `games.toml` rule), finds its processes in `/proc`, asks the systemd user manager through `busctl`/`systemctl` to move them into `app-melibea-game-<pid>.scope` and freezes that scope. The Niri fork owns the per-window suspended flag, its blurred rendering and input exclusion. A ledger in the runtime directory, reconciliation and `thaw-all` from `ExecStopPost` keep anything from staying frozen.

**Tech Stack:** Rust 2024 (Melibea: `regex`, `serde`, `serde_json`, `toml` only), systemd ≥ 246 tools, the Niri fork (smithay, GLES).

**Spec:** [docs/superpowers/specs/2026-11-17-melibea-standby-design.md](../specs/2026-11-17-melibea-standby-design.md)

## Global Constraints

- Melibea stays MIT with exactly its four dependencies; no `zbus`, no `nix`, no new dev-dependencies. systemd is reached only through the `busctl` and `systemctl` programs.
- `unsafe_code = "forbid"`; clippy `all` and `pedantic` with `-D warnings`.
- Every `busctl`/`systemctl` call is bounded by `SYSTEMD_TIMEOUT = 5 s` (child killed and reaped on expiry). Moved pids are confirmed from `/proc` within `MOVE_CONFIRM_TIMEOUT = 2 s`, polled every 50 ms.
- Melibea creates, freezes and thaws only units matching `^app-melibea-game-[1-9][0-9]*(-[1-9][0-9]*)?\.scope$`, and checks the name before every call.
- Files: rules `$XDG_CONFIG_HOME/melibea/games.toml` (fallback `$HOME/.config/melibea/games.toml`); ledger `$XDG_RUNTIME_DIR/melibea/standby.json`, directory `0700`, file `0600`, written by temporary file and rename.
- Protocol v1 and v2 output stays byte-for-byte except the `incompatible_version` error, whose message becomes `unsupported Melibea protocol version N; supported versions are 1, 2 and 3` and whose `supported_versions` becomes `[1, 2, 3]`. `PROTOCOL_VERSION_V3 = 3`.
- Fork defaults: `suspend { blur-passes 3; blur-offset 3.0; dim 0.35; }`; `blur-passes` 1–31, `blur-offset` finite and ≥ 0, `dim` 0–1.
- The fork is GPL-3.0 at `/home/toni/CODIGO/NIRI-MELIBEA`, branch `main`; its checks are `cargo fmt --all --check`, `cargo clippy --all --all-targets -- -D warnings`, `cargo test --all`. Melibea's checks are in its `AGENTS.md`.
- Never restart niri or `melibea.service`, edit the live niri config or install the user unit without the author. Compositor work is checked in a nested niri (see the memory note "NIRI-MELIBEA nested testing": `install.sh --keep-target`, IPC screenshots, stop by pid, never `pkill -f niri`).
- Commits: plain commits on `main` of the repository the task touches, English imperative subjects, ending with the `Co-Authored-By` trailer of the executing model. They run because the author approved executing this plan; nothing is pushed.
- Repository text is English; reports to the author are Spanish.

## Review Focus

1. **The game dies while frozen** (SIGKILL still kills a frozen cgroup, OOM): its scope disappears. Resume, toggle and reconcile must drop the entry and clear the niri flags without reporting a failure. Tests: Task 10 `resume_of_a_vanished_game_clears_flags`, `reconcile_drops_entry_whose_units_are_gone`.
2. **`games.toml` changes while a game is suspended** (rule deleted, title changed, file broken): resume and recovery never read it. Test: Task 10 `resume_ignores_the_rules_file`.
3. **Pid reuse**: a new game root gets the pid of an old `app-melibea-game-<pid>.scope` that is still loaded. Melibea picks the next free `-<k>` suffix instead of failing. Test: Task 10 `scope_name_skips_a_loaded_unit_with_the_same_pid`.
4. **The bind is pressed on a window that is not a game** (browser, Claude, the shell): nothing is moved, flagged or frozen, and the reason is printed. Test: Task 10 `refuses_a_window_that_is_not_a_game`.
5. **No runtime directory or an unwritable ledger**: suspension is refused before any process is moved or any flag is set. Test: Task 10 `unwritable_ledger_refuses_before_any_change`.

---

## Phase 0 — Gate

### Task 1: Validation spike

Manual, with the author present for steps 3–5. No code. The results go into the M12 section of `ROADMAP.md`. If step 2 or 3 shows systemd refuses moving the processes, **stop and report**: the fallback (freezing the Flatpak unit) needs a new decision from the author.

**Files:**
- Modify: `ROADMAP.md` (M12 section, new `### Spike` subsection)

- [ ] **Step 1: Confirm the tools**

Run: `systemctl --version | head -1; busctl --version | head -1; stat -fc %T /sys/fs/cgroup`
Expected: systemd ≥ 246 on both, and `cgroup2fs`.

- [ ] **Step 2: Move and freeze a synthetic process**

```bash
systemd-run --user --scope --unit=m12-spike-parent.scope -- sh -c 'yes >/dev/null & sleep 600' &
sleep 1; P=$(pgrep -n -x yes)
busctl --user call org.freedesktop.systemd1 /org/freedesktop/systemd1 org.freedesktop.systemd1.Manager \
  StartTransientUnit 'ssa(sv)a(sa(sv))' app-melibea-game-$P.scope fail 2 PIDs au 1 $P CollectMode s inactive-or-failed 0
cat /proc/$P/cgroup
systemctl --user freeze app-melibea-game-$P.scope
a=$(awk '{print $14+$15}' /proc/$P/stat); sleep 3; b=$(awk '{print $14+$15}' /proc/$P/stat); echo "$a $b"
systemctl --user thaw app-melibea-game-$P.scope; systemctl --user stop app-melibea-game-$P.scope m12-spike-parent.scope
```
Expected: the cgroup line ends in `app-melibea-game-$P.scope`; the two CPU counters are equal while frozen.

- [ ] **Step 3: Bottles game (Victoria 3)**

With the author's go-ahead, take the pid whose `argv[0]` ends in `victoria3.exe` (`pgrep -f 'victoria3\.exe'` filtered to `argv[0]`), move it and its descendants with the same `busctl` call, then freeze it for ten minutes. Meanwhile the author keeps using the session, moves the pointer over the game, types into other windows, copies and pastes elsewhere, and opens Bottles' window. Then thaw.
Expected: the move succeeds; `wineserver` and the Bottles interface stay in the Flatpak unit and keep working; `systemctl --user show -p CPUUsageNSec` of the game scope is unchanged during the freeze; after the thaw the game answers input and keeps its state; no X error in `journalctl --user -u niri -S -15min | grep -i -E 'xwayland|satellite'`.

- [ ] **Step 4: Steam game**

The author starts a small Steam game. Check `tr '\0' '\n' </proc/PID/environ | grep SteamAppId` on its processes and that its window is `steam_app_<N>` in `niri msg --json windows`. Move every process carrying `SteamAppId=<N>` (with descendants), freeze it for two minutes, thaw it, then quit the game normally.
Expected: same results as step 3; the Steam client works during the freeze and registers the game's exit.

- [ ] **Step 5: Record and commit**

Add `### Spike` to the M12 section: date, commands, measured results, and whether the gate passed. Then:

```bash
git add ROADMAP.md
git commit -m "Record the M12 standby spike"
```

---

## Phase 1 — Niri fork (in `/home/toni/CODIGO/NIRI-MELIBEA`; independent of Phase 2)

### Task 2: Suspended flag, action, IPC and configuration

**Files:**
- Modify: `niri-ipc/src/lib.rs` (`Action`, `Window`, `MinimizedWindow`)
- Modify: `niri-config/src/binds.rs` (`Action::SetWindowSuspended`, from the IPC action like `MinimizeWindowById`)
- Modify: `niri-config/src/misc.rs`, `niri-config/src/lib.rs` (`Suspend` block)
- Modify: `src/window/mapped.rs` (flag)
- Modify: `src/input/mod.rs` (action handler)
- Modify: `src/ipc/server.rs` (`WindowActionProbe`, `make_ipc_window`, `make_ipc_minimized_window`)
- Create: `src/tests/suspend.rs`; Modify: `src/tests/mod.rs`

**Interfaces:**
- Produces (IPC wire, consumed by Task 9): action `{"SetWindowSuspended":{"id":7,"suspended":true}}`, answered with `WindowActionResult` statuses `Applied`, `AlreadyInRequestedState`, `WindowNotFound`; `Window.is_suspended: bool` and `MinimizedWindow.is_suspended: bool`, both `#[serde(default)]`. CLI: `niri msg action set-window-suspended --id 7 --suspended true`.
- Produces (fork-internal, used by Tasks 3–4): `Mapped::is_suspended(&self) -> bool`, `Mapped::set_suspended(&mut self, suspended: bool) -> bool` (returns whether it changed); `niri_config::Suspend { blur_passes: u8, blur_offset: f64, dim: f64 }` at `Config::suspend`.

- [ ] **Step 1: Write the failing tests**

In `niri-ipc/src/lib.rs` tests:
```rust
#[test]
fn set_window_suspended_round_trips() {
    let action = Action::SetWindowSuspended { id: 7, suspended: true };
    let json = serde_json::to_string(&action).unwrap();
    assert_eq!(json, r#"{"SetWindowSuspended":{"id":7,"suspended":true}}"#);
    assert_eq!(serde_json::from_str::<Action>(&json).unwrap(), action);
}

#[test]
fn a_window_from_an_older_niri_is_not_suspended() {
    // A Window JSON without the field, as an older compositor sends it.
    let window: Window = serde_json::from_str(WINDOW_JSON_WITHOUT_IS_SUSPENDED).unwrap();
    assert!(!window.is_suspended);
}
```
In `niri-config` tests: `suspend { blur-passes 5; blur-offset 2.5; dim 0.5; }` parses to `Suspend { blur_passes: 5, blur_offset: 2.5, dim: 0.5 }`; an absent block gives `{ 3, 3.0, 0.35 }`; `dim 1.5`, `blur-passes 0` and `blur-offset -1` are errors.
In `src/tests/suspend.rs` (fixture as in `src/tests/secondary.rs`, actions sent through the same path `niri msg action` uses so the semantic result is observed):
- `suspending_an_unknown_window_is_not_found`: id 9999 → `WindowNotFound`.
- `suspending_twice_reports_already_suspended`: first `Applied`, second `AlreadyInRequestedState`, then `suspended: false` → `Applied`.
- `ipc_reports_the_suspended_flag`: after suspending, the IPC `Window` for that id has `is_suspended == true`; after a minimize, the `MinimizedWindow` record keeps `is_suspended == true`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --all suspend` and `cargo test -p niri-ipc set_window_suspended`
Expected: compile errors for the missing variant, field and module.

- [ ] **Step 3: Implement**

The handler finds the window among visible and minimized windows, calls `set_suspended`, queues a redraw of all outputs when it changed, and calls `update_keyboard_focus` and `refresh_pointer_contents` (Task 3 gives those their meaning). The probe resolves the status before acting, like `Minimize`. The IPC event diff already emits `WindowOpenedOrChanged` when a record changes; check that a flag change alone produces one.

- [ ] **Step 4: Run the tests and the fork checks**

Run: `cargo test --all && cargo clippy --all --all-targets -- -D warnings && cargo fmt --all --check`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add niri-ipc niri-config src/window/mapped.rs src/input/mod.rs src/ipc/server.rs src/tests
git commit -m "Add a suspended flag for windows, set over IPC and reported in window records"
```

### Task 3: Input and keyboard-focus exclusion

**Files:**
- Modify: `src/niri.rs` (`contents_under`, `update_keyboard_focus`)
- Test: `src/tests/suspend.rs`

**Interfaces:**
- Consumes: `Mapped::is_suspended` (Task 2).
- Produces: a suspended window is hit-tested as a window (click and focus-follows-mouse still focus its tile) but yields no surface; keyboard focus is `KeyboardFocus::Layout { surface: None }` while the layout's focused window is suspended, and that window's `Mapped::is_focused()` stays `true`, so IPC `is_focused` keeps naming it (Melibea's toggle targets it).

- [ ] **Step 1: Write the failing tests**

- `a_suspended_window_gets_no_keyboard_focus_but_stays_focused_in_ipc`: map one window, it is focused; suspend it → `f.niri().keyboard_focus.surface()` is `None` and the mapped window's `is_focused()` is `true`; unsuspend → `keyboard_focus.surface()` is that window's surface again.
- `pointer_over_a_suspended_window_finds_no_surface`: `contents_under` at the window's centre has `surface == None` and `window == Some(that window)`.
- `focus_leaves_and_returns_to_a_suspended_window`: map a second window and focus it → the suspended one has `is_focused() == false`; focus the suspended one again → `is_focused() == true` and keyboard surface still `None`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --all suspend`
Expected: the three new tests FAIL on the surface assertions.

- [ ] **Step 3: Implement**

In `contents_under`, the hit data of a suspended mapped window keeps the window and drops the surface. In `update_keyboard_focus`, the layout focus of a suspended window becomes `Layout { surface: None }`, and the `set_is_focused` bookkeeping that today follows the focused surface follows the layout's focused window instead, so it is set for a suspended window too. Touch goes through `contents_under` and needs nothing more.

- [ ] **Step 4: Run the tests and the fork checks**

Run: `cargo test --all && cargo clippy --all --all-targets -- -D warnings`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add src/niri.rs src/tests/suspend.rs
git commit -m "Keep input and keyboard focus away from suspended windows"
```

### Task 4: Blurred, dimmed rendering

**Files:**
- Create: `src/render_helpers/suspended.rs`; Modify: `src/render_helpers/mod.rs`
- Modify: `src/layout/tile.rs` (`render_inner`)
- Modify: `docs/wiki/Configuration:-Miscellaneous.md` (the `suspend` block), `MELIBEA-NATIVE-MINIMIZATION.md` (one section on the action)

**Interfaces:**
- Consumes: `Mapped::is_suspended`, `Config::suspend` (Task 2); `OffscreenBuffer`, `Blur`, `BlurOptions` (existing).
- Produces: `SuspendedSnapshot`, owned by the tile, whose render returns one texture element of the blurred window. It blurs again only when the window's offscreen damage, size, scale or the blur options change, so a frozen client costs one cached texture per frame.

- [ ] **Step 1: Implement**

When the window is suspended, `render_inner` skips the resize path and the plain surface path. It renders the window (`render_normal` and `render_popups`) into the snapshot's offscreen, blurs it with `BlurOptions { passes: blur_passes, offset: blur_offset }`, and pushes the texture at `window_render_loc` with `win_alpha`. Over it goes a black dim of alpha `dim` the size of `animated_window_size`, drawn through `BorderRenderElement` with the geometry corner radius when that radius is non-zero, like the blocked-out path. Without a GLES renderer or blur program it pushes the unblurred offscreen plus the dim. The client's surface elements are never pushed, so the window cannot be scanned out directly.

- [ ] **Step 2: Fork checks**

Run: `cargo test --all && cargo clippy --all --all-targets -- -D warnings && cargo fmt --all --check`
Expected: all pass.

- [ ] **Step 3: Check it in a nested niri**

Build with `scripts/install.sh --keep-target` to a nested prefix as the memory note describes, start a nested niri with a `foot` (or another moving client) tiled and a second one fullscreen, and run `niri msg action set-window-suspended --id ID --suspended true` against the nested socket for each.
Expected, checked on an IPC screenshot of the nested output: both show their last frame blurred and dimmed with rounded corners intact; typing and clicking reach neither (`foot` shows no new characters after unsuspending); `--suspended false` restores the sharp, live window; `niri msg --json windows` shows `is_suspended` following each change.

- [ ] **Step 4: Commit**

```bash
git add src/render_helpers src/layout/tile.rs docs/wiki MELIBEA-NATIVE-MINIMIZATION.md
git commit -m "Draw suspended windows as their last frame, blurred and dimmed"
```

---

## Phase 2 — Melibea building blocks (in `/home/toni/CODIGO/MELIBEA`)

All new code lives in `src/standby/` (`mod.rs` re-exports the submodules; `src/lib.rs` gains `pub mod standby;`). Tests that need files build them under `std::env::temp_dir()` with a per-test unique name, as `src/service.rs` tests already do.

### Task 5: Process table from `/proc`

**Files:**
- Create: `src/standby/mod.rs`, `src/standby/process.rs`; Modify: `src/lib.rs`

**Interfaces:**
- Produces:
```rust
pub struct ProcessInfo { pub pid: u32, pub ppid: u32, pub uid: u32, pub comm: String,
                         pub argv0: Option<String>, pub cgroup: String, pub steam_app_id: Option<u64> }
pub struct ProcessTable { /* BTreeMap<u32, ProcessInfo> */ }
impl ProcessTable {
    pub fn new(processes: impl IntoIterator<Item = ProcessInfo>) -> Self;
    pub fn read(proc_root: &Path) -> io::Result<Self>;      // every numeric entry; vanished pids skipped
    pub fn get(&self, pid: u32) -> Option<&ProcessInfo>;
    pub fn iter(&self) -> impl Iterator<Item = &ProcessInfo>;
    pub fn with_descendants(&self, roots: &BTreeSet<u32>) -> BTreeSet<u32>;
    pub fn roots(&self, set: &BTreeSet<u32>) -> BTreeSet<u32>; // members whose parent is not a member
    pub fn in_unit(&self, unit: &str) -> BTreeSet<u32>;     // cgroup's last component == unit
}
pub fn app_unit(cgroup: &str, uid: u32) -> Option<&str>;
pub fn exe_basename(argv0: &str) -> &str;
```
`uid` is the owner of `/proc/<pid>`; `cgroup` is the path after `0::`; `steam_app_id` parses `SteamAppId=<u64>` from `environ` (unreadable → `None`); `comm` and `ppid` come from `stat`, parsed after the last `)`.

- [ ] **Step 1: Write the failing tests**

- `reads_a_wine_process_from_a_fixture_proc`: a fixture pid with `stat` `4242 (Main Thread) S 4100 …`, `cmdline` `Z:\mnt\JUEGOS\victoria3.exe\0`, `environ` without Steam and `cgroup` `0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-flatpak-com.usebottles.bottles-1.scope` → `comm == "Main Thread"`, `ppid == 4100`, `argv0 == Some("Z:\\mnt\\JUEGOS\\victoria3.exe")`, `steam_app_id == None`.
- `reads_the_steam_app_id_from_the_environment`: `environ` holding `SteamAppId=1158310` → `Some(1158310)`; `SteamAppId=abc` → `None`.
- `a_process_that_vanished_is_skipped`: a pid directory without `stat` is absent, and reading does not fail.
- `descendants_follow_parent_links`: with parents 1←2←3 and 4 separate, `with_descendants({2}) == {2, 3}`, `roots({2, 3, 4}) == {2, 4}`.
- `app_unit_accepts_only_app_units_directly_under_app_slice`: the Bottles path gives its scope; `…/app.slice/app-niri-foot-9.scope` gives that; `…/session.slice/niri.service`, `…/app.slice/app-dbus-x.slice/y.scope`, another uid's path and `…/app.slice/melibea.service` give `None`.
- `exe_basename_cuts_at_either_separator`: `Z:\a\b.exe` → `b.exe`, `/usr/bin/foot` → `foot`, `foot` → `foot`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --lib standby::process`
Expected: compile error, module missing.

- [ ] **Step 3: Implement the interfaces above.**

- [ ] **Step 4: Run the checks**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add src/lib.rs src/standby
git commit -m "Read the user's processes, parents and units from /proc"
```

### Task 6: Game rules and the game set

**Files:**
- Create: `src/standby/games.rs`, `games.example.toml`

**Interfaces:**
- Consumes: `ProcessTable`, `exe_basename` (Task 5); `ConfigPathError` from `src/config.rs`.
- Produces:
```rust
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GameIdentity { Process { name: String }, SteamApp { app_id: u64 } } // name stored ASCII-lowercase
pub struct RecognizedGame { pub identity: GameIdentity, pub standby_after: Option<Duration> }
pub struct GameRules { /* Vec<GameRule> */ }
impl GameRules {
    pub fn parse(input: &str) -> Result<Self, GamesError>;
    pub fn load(path: &Path) -> Result<Self, GamesError>;   // NotFound → no rules
    pub fn recognize(&self, app_id: Option<&str>, title: Option<&str>) -> Option<RecognizedGame>;
}
pub fn default_games_path() -> Result<PathBuf, ConfigPathError>;
pub fn steam_app_id(app_id: &str) -> Option<u64>;            // "steam_app_<decimal>"
pub fn parse_delay(text: &str) -> Result<Duration, String>;  // "<u64>s" | "<u64>m" | "<u64>h"
pub fn game_set(table: &ProcessTable, identity: &GameIdentity) -> BTreeSet<u32>; // matches + descendants
```
Raw rule: `[[game]]` with `app_id`, `title` (regex, at least one), `process` (non-empty, no `/` or `\`), `steam_app_id` (u64 > 0; at most one of `process` and `steam_app_id`), `standby_after`; unknown fields rejected. A matching rule with `process` gives `Process`; with `steam_app_id` gives `SteamApp` with that id; with neither, `SteamApp` when the app id is `steam_app_<N>`, else `None`. With no matching rule, `steam_app_<N>` gives `SteamApp` with no delay.

- [ ] **Step 1: Write the failing tests**

- `a_steam_window_is_a_game_without_rules`: empty rules, `steam_app_1158310` → `SteamApp { app_id: 1158310 }`; `steam_app_12x` and `steam_proton` → `None`.
- `the_first_matching_rule_wins`: two rules matching title `^Victoria 3$` with processes `a.exe`, `b.exe` → `Process { name: "a.exe" }`.
- `a_rule_names_the_process_case_insensitively`: rule `process = "Victoria3.EXE"` → name `victoria3.exe`; `game_set` finds a process with `argv0` `Z:\x\VICTORIA3.exe` and its child, and not `start.exe` whose arguments mention `victoria3.exe`.
- `a_rule_without_process_needs_a_steam_window`: rule `title = "^X$"` matches window `steam_app_5`/`X` → `SteamApp { 5 }` with the rule's delay; window `foo`/`X` → `None`.
- `a_rule_names_a_native_steam_game`: rule `app_id = "^dotnet$"`, `steam_app_id = 1281930` on window `dotnet`/`Terraria: ¡Cava y calla!` → `SteamApp { 1281930 }`.
- `steam_game_set_follows_the_environment`: processes with `steam_app_id == Some(5)` and their descendants, not the Steam client.
- `rules_are_validated`: no matcher, invalid regex, empty `process`, `process = "a/b"`, both `process` and `steam_app_id`, `steam_app_id = 0`, `standby_after = "5x"`, unknown field → errors naming the rule number (`rule 2: …`).
- `delays_parse`: `"0s"` → 0, `"90s"` → 90 s, `"5m"` → 300 s, `"1h"` → 3600 s; `"5"`, `"m"`, `"-1s"` → errors.
- `a_missing_file_means_no_rules`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --lib standby::games`
Expected: compile error.

- [ ] **Step 3: Implement the interfaces above, and write `games.example.toml` with the Victoria 3 rule from the spec.**

- [ ] **Step 4: Run the checks**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add src/standby games.example.toml
git commit -m "Recognize games by Steam app id or by a rule naming their process"
```

### Task 7: systemd through `busctl` and `systemctl`

**Files:**
- Create: `src/standby/systemd.rs`

**Interfaces:**
- Produces:
```rust
pub const SYSTEMD_TIMEOUT: Duration = Duration::from_secs(5);
pub fn is_game_unit(name: &str) -> bool;                 // the Global Constraints pattern
pub fn game_unit_name(root_pid: u32, suffix: Option<u32>) -> String; // app-melibea-game-<pid>[-<k>].scope
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FreezerState { Running, Freezing, Frozen, Thawing, NotLoaded }
pub trait Systemd {
    fn start_scope(&self, unit: &str, pids: &BTreeSet<u32>) -> Result<(), SystemdError>;
    fn freeze(&self, unit: &str) -> Result<(), SystemdError>;
    fn thaw(&self, unit: &str) -> Result<(), SystemdError>;
    fn freezer_state(&self, unit: &str) -> Result<FreezerState, SystemdError>;
    fn game_units(&self) -> Result<Vec<String>, SystemdError>;
}
pub struct SystemdTools { /* busctl: PathBuf, systemctl: PathBuf, timeout: Duration */ }
impl SystemdTools {
    pub fn new() -> Self;                                  // "busctl", "systemctl", SYSTEMD_TIMEOUT
    pub fn with_programs(busctl: PathBuf, systemctl: PathBuf, timeout: Duration) -> Self;
}
pub enum SystemdError { NotAGameUnit(String), Spawn(io::Error), Timeout { program: String },
                        Failed { program: String, stderr: String }, Unexpected(String) }
```
Exact argv:
- `start_scope`: `busctl --user call org.freedesktop.systemd1 /org/freedesktop/systemd1 org.freedesktop.systemd1.Manager StartTransientUnit ssa(sv)a(sa(sv)) UNIT fail 2 PIDs au <count> <pid>… CollectMode s inactive-or-failed 0`, pids ascending.
- `freeze` / `thaw`: `systemctl --user freeze UNIT` / `systemctl --user thaw UNIT`.
- `freezer_state`: `systemctl --user show --property=LoadState,FreezerState UNIT`; `LoadState=not-found` → `NotLoaded`; `FreezerState` values `running|freezing|frozen|thawing`.
- `game_units`: `systemctl --user list-units --all --plain --no-legend --full app-melibea-game-*.scope`, first column, filtered by `is_game_unit`.

stderr is kept to its first 4 KiB.

- [ ] **Step 1: Write the failing tests**

Each test writes stub `busctl`/`systemctl` shell scripts into its temporary directory that append `"$0 $*"` to a log file and print the canned output, then passes them through `with_programs`.
- `start_scope_sends_the_exact_call`: pids `{30, 4}` → log line ends with `StartTransientUnit ssa(sv)a(sa(sv)) app-melibea-game-4.scope fail 2 PIDs au 2 4 30 CollectMode s inactive-or-failed 0`.
- `every_call_refuses_a_foreign_unit`: `freeze("app-flatpak-x.scope")`, `thaw("niri.service")`, `start_scope("app-melibea-game-0.scope", …)` → `NotAGameUnit`, and the log stays empty.
- `freezer_state_reads_both_properties`: outputs `LoadState=loaded\nFreezerState=frozen` → `Frozen`; `LoadState=not-found\nFreezerState=running` → `NotLoaded`.
- `a_failing_tool_reports_its_stderr`: stub exits 1 printing `Access denied` → `Failed` with that text.
- `a_hanging_tool_times_out`: stub `sleep 10`, timeout 200 ms → `Timeout` within 1 s.
- `game_units_lists_only_game_units`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --lib standby::systemd`
Expected: compile error.

- [ ] **Step 3: Implement the interfaces above.** Bounded wait: `try_wait` polled every 20 ms, then `kill` and `wait` on expiry.

- [ ] **Step 4: Run the checks**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add src/standby
git commit -m "Move, freeze and thaw game scopes through busctl and systemctl"
```

### Task 8: The ledger

**Files:**
- Create: `src/standby/ledger.rs`

**Interfaces:**
- Consumes: `GameIdentity` (Task 6).
- Produces:
```rust
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerEntry { pub units: Vec<String>, pub windows: Vec<u64>, pub identity: GameIdentity,
                         pub since_unix: u64, pub flagged: bool }
pub struct Ledger { /* path */ }
impl Ledger {
    pub fn at(path: impl Into<PathBuf>) -> Self;
    pub fn default_path() -> Result<PathBuf, LedgerError>;        // $XDG_RUNTIME_DIR/melibea/standby.json
    pub fn read(&self) -> Result<Vec<LedgerEntry>, LedgerError>;   // missing file → empty
    pub fn write(&self, entries: &[LedgerEntry]) -> Result<(), LedgerError>;
}
```
On disk: `{"version":1,"entries":[…]}`. `write` creates the directory with `0700` when missing, writes `standby.json.tmp` with `0600`, then renames it over.

- [ ] **Step 1: Write the failing tests**

`entries_round_trip`, `a_missing_file_reads_empty`, `a_corrupt_file_is_an_error`, `written_files_are_private` (directory `0700`, file `0600`), `an_unset_runtime_dir_is_an_error` (via `default_path` with the variable removed in a child process or a resolver taking the value as a parameter).

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --lib standby::ledger`
Expected: compile error.

- [ ] **Step 3: Implement the interfaces above.**

- [ ] **Step 4: Run the checks**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add src/standby
git commit -m "Record suspended games in a private ledger in the runtime directory"
```

### Task 9: niri client for windows and the suspended flag

**Files:**
- Modify: `src/niri.rs` (`NiriWindow`), `src/niri_ipc.rs` (`NiriActionClient`)

**Interfaces:**
- Consumes: the fork's wire format (Task 2). Melibea keeps speaking niri JSON directly, without the GPL crate.
- Produces:
```rust
// NiriWindow gains (both #[serde(default)]):
pub pid: Option<i32>,
pub is_suspended: bool,
pub struct NiriMinimizedWindowInfo { pub id: u64, pub title: Option<String>, pub app_id: Option<String>,
                                     pub pid: Option<i32>, #[serde(default)] pub is_suspended: bool }
impl NiriActionClient {
    pub fn windows(&mut self) -> Result<Vec<NiriWindow>, NiriActionError>;                        // "Windows"
    pub fn minimized_windows(&mut self) -> Result<Vec<NiriMinimizedWindowInfo>, NiriActionError>; // "MinimizedWindows"
    pub fn set_window_suspended(&mut self, window_id: WindowId, suspended: bool)
        -> Result<NiriWindowActionResult, NiriActionError>;
}
```
The reply shapes are `{"Ok":{"Windows":[…]}}` and the fork's `MinimizedWindows` response; check the variant names in `niri-ipc/src/lib.rs` of the fork.

- [ ] **Step 1: Write the failing tests**

In the existing `niri_ipc.rs` test style (a socket pair with a scripted reply):
- `set_window_suspended_sends_the_fork_action`: request line is `{"Action":{"SetWindowSuspended":{"id":7,"suspended":true}}}`, and a `WindowActionResult` reply maps to `Applied`.
- `windows_reads_pid_and_suspended_flag`: a `Windows` reply with `"pid":42,"is_suspended":true` and one without both fields → `Some(42)`/`true` and `None`/`false`.
- `minimized_windows_reads_the_records`.
- In `niri.rs`: an existing `WindowsChanged` line without the new fields still parses.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --lib niri`
Expected: compile errors.

- [ ] **Step 3: Implement the interfaces above.**

- [ ] **Step 4: Run the checks**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add src/niri.rs src/niri_ipc.rs
git commit -m "Read window pids and the suspended flag, and set the flag in niri"
```

---

## Phase 3 — Melibea standby

### Task 10: The standby controller

**Files:**
- Create: `src/standby/controller.rs`

**Interfaces:**
- Consumes: Tasks 5–9.
- Produces:
```rust
pub const MOVE_CONFIRM_TIMEOUT: Duration = Duration::from_secs(2);
pub struct WindowView { pub id: u64, pub app_id: Option<String>, pub title: Option<String>,
                        pub pid: Option<i32>, pub is_focused: bool, pub is_suspended: bool,
                        pub is_minimized: bool }
pub trait Compositor {
    fn windows(&mut self) -> Result<Vec<WindowView>, String>;   // visible and minimized
    fn set_suspended(&mut self, window_id: u64, suspended: bool) -> Result<(), String>;
}
pub trait ProcessSource { fn snapshot(&self) -> io::Result<ProcessTable>; }
pub struct ProcFs;                                   // ProcessTable::read("/proc")
pub struct LiveCompositor(NiriActionClient);         // merges windows() and minimized_windows()
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum StandbyOperation { Suspend, Resume }
pub enum BlockReason { NotAGame, GameNotRunning, ProtectedProcess { pid: u32, comm: String },
                       GameEscaped { pids: Vec<u32> }, InvalidRules(String), Systemd(String),
                       Ledger(String), Compositor(String) }
impl BlockReason { pub fn code(&self) -> &'static str; pub fn describe(&self) -> String; }
// codes: not_a_game, game_not_running, protected_process, game_escaped, invalid_rules,
//        systemd, ledger, compositor
pub enum StandbyOutcome {
    Applied { operation: StandbyOperation, window_id: u64, windows: Vec<u64> },
    AlreadyInRequestedState { operation: StandbyOperation, window_id: u64 },
    WindowNotFound { operation: StandbyOperation, requested_id: Option<u64> },
    Blocked { operation: StandbyOperation, window_id: Option<u64>, reason: BlockReason },
}
pub struct Standby<S: Systemd, P: ProcessSource> { /* systemd, processes, ledger, games_path, own_pid, uid */ }
impl<S: Systemd, P: ProcessSource> Standby<S, P> {
    pub fn new(systemd: S, processes: P, ledger: Ledger, games_path: PathBuf, own_pid: u32, uid: u32) -> Self;
    pub fn suspend(&mut self, niri: &mut impl Compositor, window_id: Option<u64>) -> StandbyOutcome;
    pub fn resume(&mut self, niri: &mut impl Compositor, window_id: Option<u64>) -> StandbyOutcome;
    pub fn toggle(&mut self, niri: &mut impl Compositor, window_id: Option<u64>) -> StandbyOutcome;
    pub fn is_suspended(&self, window_id: u64) -> bool;          // ledger; unreadable → false
    pub fn reconcile(&mut self, niri: &mut impl Compositor) -> Vec<String>;
    pub fn thaw_all<C: Compositor>(&mut self, niri: Option<&mut C>) -> Vec<String>;
}
```

`suspend` runs in this order. Each step that refuses returns `Blocked` before any later step runs.
1. Target: `window_id`, or the window with `is_focused`; none → `WindowNotFound`. Read the ledger and write it back unchanged, which proves it writable (error → `Ledger`). Already in the ledger → `AlreadyInRequestedState`.
2. `GameRules::load(games_path)` (error → `InvalidRules`); `recognize` → else `NotAGame`.
3. Snapshot; `game_set` empty → `GameNotRunning`. Any member owned by another uid, equal to `own_pid`, with `comm` `niri`, `xwayland-satell` or `Xwayland`, or without `app_unit` → `ProtectedProcess`.
4. Stragglers are members whose unit is not a game unit. If there are any: name = `game_unit_name(min(roots(stragglers)), None)`, or the first `Some(k)` from 1 whose `freezer_state` is `NotLoaded`. Then `start_scope` and poll snapshots until every surviving straggler reports that unit, within `MOVE_CONFIRM_TIMEOUT` (else `Systemd`). Repeat steps 3–4 once; stragglers left → `GameEscaped`.
5. Units = the units of the set. Any process in one of them that is outside the set → `ProtectedProcess`.
6. Game windows = windows recognized as the same identity, plus windows whose pid is in the set.
7. Write the ledger entry with `flagged: true` (error → `Ledger`).
8. `set_suspended(true)` for each game window. On error, clear the flags already set, remove the entry, return `Compositor`.
9. `freeze` each unit. On error, thaw the units already frozen, clear the flags, remove the entry, return `Systemd`.

`resume`: the target's entry, or `AlreadyInRequestedState` when the window exists without one, or `WindowNotFound`. Thaw each unit, skipping `NotLoaded`, and require `Running` or `NotLoaded` afterwards (else `Systemd`, entry kept). Then clear the flags of the entry's windows that still exist, and remove the entry. It never loads `games.toml`. `toggle` = `resume` when the target has an entry, else `suspend`.

`reconcile`:
- Drop entries whose units are all `NotLoaded`, clearing their windows' flags.
- Thaw and drop entries with no window that is present and `is_suspended`.
- Clear the flag of every `is_suspended` window with no entry.
- Thaw every frozen unit from `game_units()` that no entry names.
- An unreadable ledger falls back to `thaw_all`.

`thaw_all` thaws ledger units ∪ `game_units()`, clears every `is_suspended` flag when given a compositor, and writes an empty ledger. Every action yields one returned line.

- [ ] **Step 1: Write the failing tests**

Fakes in the test module:
- `FakeSystemd` records calls in order, has programmable failures and freezer states, and moves pids on `start_scope` in a table shared with `FakeProcesses`.
- `FakeCompositor` holds windows and records `set_suspended` calls.

The base fixture is the spec's session: Bottles scope with `bwrap`, `bottles`, `wineserver`, `victoria3.exe` (pid 4242); Steam client scope; satellite in `niri.service`; window 608 `steam_proton`/`Victoria 3` with the satellite's pid; rule `title = "^Victoria 3$"`, `process = "victoria3.exe"`.
- `suspend_moves_only_the_bottles_game_then_flags_then_freezes`: calls are exactly `start_scope("app-melibea-game-4242.scope", {4242})`, `set_suspended(608, true)`, `freeze("app-melibea-game-4242.scope")`; `wineserver` and `bottles` stay in the Flatpak unit; the ledger holds one entry with windows `[608]`.
- `suspend_moves_a_steam_game_by_app_id`: as the spike found for tModLoader, processes with `SteamAppId=5` sit in three Steam units (`steam-runtime-launch-client` in the client's, `reaper` in the launcher's, the game in its container's); all of them and their descendants move into one game scope, and the Steam client, `bwrap` and `xdg-dbus-proxy` do not.
- `refuses_a_window_that_is_not_a_game`: focused Floorp → `Blocked(NotAGame)`, no systemd or compositor call, ledger untouched.
- `refuses_a_game_set_with_a_protected_process`: the rule's process runs in `niri.service` → `Blocked(ProtectedProcess)`, no calls.
- `refuses_a_game_that_is_not_running`.
- `reuses_a_game_unit_without_moving`: the game already in `app-melibea-game-4242.scope` → no `start_scope`.
- `scope_name_skips_a_loaded_unit_with_the_same_pid`: `app-melibea-game-4242.scope` loaded with other pids → `start_scope("app-melibea-game-4242-1.scope", …)`.
- `escaped_processes_refuse_after_the_second_discovery`: the fake forks a new game child after each move → `Blocked(GameEscaped)`, nothing frozen.
- `freeze_failure_rolls_back_flags_and_ledger`: `set_suspended(608, false)` follows, the ledger is empty, result `Blocked(Systemd)`.
- `unwritable_ledger_refuses_before_any_change`: ledger path under a read-only directory → `Blocked(Ledger)`, with no systemd and no compositor call.
- `the_windows_of_one_game_change_together`: two `steam_app_5` windows → both flagged and both cleared on resume.
- `resume_thaws_before_clearing_flags`: call order `thaw`, `freezer_state`, `set_suspended(608, false)`; entry removed.
- `resume_ignores_the_rules_file`: rules file replaced by invalid TOML after suspend → resume `Applied`.
- `resume_of_a_vanished_game_clears_flags`: units `NotLoaded` → `Applied`, flags cleared, entry removed.
- `toggle_suspends_then_resumes`.
- `reconcile_thaws_an_entry_without_a_suspended_window`: niri restarted, so no window is flagged → thawed, entry dropped.
- `reconcile_clears_a_flag_without_an_entry`.
- `reconcile_drops_entry_whose_units_are_gone`.
- `thaw_all_releases_listed_game_units_with_a_corrupt_ledger`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --lib standby::controller`
Expected: compile error.

- [ ] **Step 3: Implement the interfaces and sequences above.**

- [ ] **Step 4: Run the checks**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add src/standby
git commit -m "Suspend and resume a game by separating it into its own scope"
```

### Task 11: Protocol v3, broker and CLI

**Files:**
- Modify: `src/protocol.rs`, `src/service.rs`, `src/main.rs`, `src/standby/controller.rs` (`outcome_result`)
- Create: `PROTOCOL-V3.md`; Modify: `AGENTS.md` (read trigger lists `PROTOCOL-V3.md`)

**Interfaces:**
- Consumes: `StandbyOutcome`, `BlockReason` (Task 10).
- Produces:
```rust
pub const PROTOCOL_VERSION_V3: u32 = 3;
pub const SUPPORTED_PROTOCOL_VERSIONS: [u32; 3] = [1, 2, 3];
// Request gains (valid only with version 3; v1/v2 → invalid_request
// "suspension requires Melibea protocol version 3"):
Suspend { window_id: Option<u64> }, Resume { window_id: Option<u64> },
ToggleSuspend { window_id: Option<u64> }, ListSuspended,
// Operation gains Suspend, Resume.
// ActionResult gains: #[serde(default, skip_serializing_if = "Option::is_none")] pub reason: Option<String>
// Message gains: Suspended { revision: u64, windows: Vec<SuspendedWindow> }
pub struct SuspendedWindow { pub id: u64, pub app_id: Option<String>, pub title: Option<String> }
impl Service { pub fn publish_suspended(&self, windows: &[SuspendedWindow]) -> Result<(), ServiceStopped>; }
impl ClientRequest { pub const fn v3(request: Request) -> Self; }
impl ServiceClient { pub fn request_v3(&self, request: Request) -> Result<ServerMessage, ClientError>; }
pub fn outcome_result(outcome: &StandbyOutcome) -> ActionResult; // in src/standby/controller.rs
```
The broker keeps the suspended list and its own revision. `publish_suspended` with an identical list does nothing; otherwise the revision advances and `Suspended` goes to subscribers whose version is ≥ 3. A v3 `subscribe` gets its usual first message, then `Suspended` with the current list, even when empty. `ListSuspended` answers `Suspended`. Version 3 accepts everything version 2 does. `outcome_result` maps `Blocked` to status `blocked` with `reason = Some("<code>: <describe>")` and `WindowNotFound` to `window_not_found`.

CLI: `melibea suspend [ID]`, `resume [ID]`, `toggle-suspend [ID]` send v3 requests. They print `suspended window 608 (and 1 more)` or `resumed window 608`, and on `blocked` print the reason to stderr and exit 1. `melibea thaw-all` runs `Standby::thaw_all` locally (daemon not needed) with a `LiveCompositor` when `NIRI_SOCKET` connects, otherwise without one, prints each line, and exits 0. `status` prints a `suspended:` section read from the ledger. The help text lists the four commands.

- [ ] **Step 1: Write the failing tests**

- `v1_and_v2_results_have_no_reason_key`: serialize an `applied` minimize result at v1 and v2 → bytes equal the existing golden strings.
- `incompatible_version_lists_three_versions`: version 9 → `supported_versions == [1, 2, 3]` and the new message.
- `suspension_requests_require_v3`: `toggle_suspend` at v2 → `invalid_request` with that message, executor never called.
- `v3_subscriber_gets_the_suspended_list_after_the_snapshot`: publish a snapshot, then `publish_suspended([608])` → v3 subscriber reads `Snapshot`, `Suspended(rev 0, [])`, `Suspended(rev 1, [608])`. A v1 subscriber on the same service reads only the snapshot (assert with a read timeout).
- `identical_suspended_list_is_not_rebroadcast`.
- `blocked_outcome_carries_the_reason`: `Blocked(NotAGame)` → `"status":"blocked","reason":"not_a_game: …"` at v3.
- CLI parsing: `toggle-suspend`, `suspend 7`, `resume`, `thaw-all`, and `suspend 7 8` rejected.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --workspace protocol service`
Expected: compile errors and failures.

- [ ] **Step 3: Implement the interfaces above, and write `PROTOCOL-V3.md` in the shape of `PROTOCOL-V2.md`: compatibility, the new requests with JSON examples, `suspended` messages and their revision, the reasons.**

- [ ] **Step 4: Run the checks**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
Expected: all pass, including every existing v1/v2 test unchanged.

- [ ] **Step 5: Commit**

```bash
git add src PROTOCOL-V3.md AGENTS.md
git commit -m "Add protocol v3 with suspend, resume and the suspended list"
```

### Task 12: Daemon wiring, standby after minimizing and recovery

**Files:**
- Modify: `src/main.rs` (executor, controller loop)
- Create: `src/standby/timers.rs`
- Modify: `contrib/systemd/melibea.service`, `README.md`

**Interfaces:**
- Consumes: Tasks 9–11.
- Produces:
```rust
pub struct StandbyTimers { /* minimized since: BTreeMap<u64, (Instant, Duration)>, fired: BTreeSet<u64> */ }
impl StandbyTimers {
    pub fn update(&mut self, now: Instant, minimized: &[(u64, Option<Duration>)]) -> Vec<u64>; // ids that left
    pub fn due(&mut self, now: Instant) -> Vec<u64>;   // each id at most once per minimized period
}
```
The daemon's executor is a struct holding `Arc<Mutex<Standby<SystemdTools, ProcFs>>>`:
- `Suspend`, `Resume` and `ToggleSuspend` open a `LiveCompositor` and map the outcome with `outcome_result`.
- `Restore` and `Close` of a window `is_suspended` in the ledger resume it first; a resume that does not apply returns its result instead of forwarding.
- `Minimize` is forwarded unchanged.

The controller loop shares the same mutex.

- [ ] **Step 1: Write the failing tests**

`StandbyTimers` tests:
- `a_game_is_due_after_its_delay`: minimized at t0 with 5 m → not due at t0 + 299 s, due once at t0 + 300 s, not again at + 400 s.
- `a_window_without_delay_never_fires`.
- `leaving_the_minimized_list_resets_and_is_reported`: `update` returns the id, and a new minimization starts a new period.
- `a_zero_delay_is_due_at_once`.

In `main.rs` tests, the mapping from controller loop events to publications as a pure function `suspended_windows(windows: &[NiriWindow], minimized: &[NiriMinimizedWindowInfo]) -> Vec<SuspendedWindow>`, ordered by id ascending.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --workspace timers suspended_windows`
Expected: compile errors.

- [ ] **Step 3: Implement the wiring**

- The controller loop tracks `is_suspended` from `WindowsChanged`, `WindowOpenedOrChanged`, `WindowClosed` and the minimized list, and calls `publish_suspended` with `suspended_windows(…)` after each such event.
- After the first `WindowsChanged` of each niri connection, it calls `reconcile` and logs every returned line.
- On each `MinimizedWindowsChanged`, it reads `games.toml` and `recognize`s each minimized window to get its delay. Ids that left the list and are in the ledger are resumed.
- On every poll tick, it suspends each `due` id. An outcome other than `Applied` is logged once.
- `melibea.service` gains `ExecStopPost=%h/.local/bin/melibea thaw-all`.
- `README.md` documents `games.toml` (with the example), the four commands, the niri bind `Mod+Shift+P { spawn "melibea" "toggle-suspend"; }` as an example, and the fork's `suspend { }` block.

- [ ] **Step 4: Run the checks**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add src contrib/systemd/melibea.service README.md
git commit -m "Wire standby into the daemon with reconciliation and standby after minimizing"
```

---

## Phase 4 — End to end

### Task 13: Nested end-to-end, install and the author's check

**Files:**
- Modify: `ROADMAP.md` (M12 status and evidence), `README.md` (current state)

- [ ] **Step 1: Nested end-to-end**

Start the fork in a nested niri. Point a Melibea built from this branch at it with `NIRI_SOCKET` set to the nested socket, `MELIBEA_SOCKET` and `XDG_RUNTIME_DIR` set to a temporary directory, and a `games.toml` with `app_id = "^foot$"`, `process = "foot"`. Open `foot` in the nested niri running `sh -c 'while :; do date +%T.%N; done'`.

Expected:
- `melibea toggle-suspend` with `foot` focused prints `suspended window N`. `foot`'s pid moved to `app-melibea-game-<pid>.scope`, which is `frozen`. The window is blurred.
- `melibea toggle-suspend` again resumes it and the clock runs on.
- Suspend again, then `kill -9` the daemon and run `melibea thaw-all`: the unit is `running` and the flag is cleared.
- Suspend, restart the daemon: reconcile keeps the suspension. Stop the nested niri: the next daemon start thaws it.
- `melibea toggle-suspend` on a nested window with no rule prints `blocked: not_a_game: …`.

- [ ] **Step 2: Install, with the author**

With the author's request:
1. Install the fork with its `scripts/install.sh`; it takes effect at the next login.
2. Install Melibea with `scripts/install.sh`.
3. The author copies `contrib/systemd/melibea.service` to `~/.config/systemd/user/` and runs `systemctl --user daemon-reload`.
4. The author adds the bind and a `suspend { }` block to the live niri config, and writes `~/.config/melibea/games.toml` with the Victoria 3 rule.
5. The author logs in again.

- [ ] **Step 3: The author's check**

In the real session:
- Victoria 3 toggled twice: frozen with no CPU, blurred, then resumed intact. Bottles stays usable while it is frozen.
- While Victoria 3 is suspended, the author clicks on it repeatedly; after resuming, none of those clicks has acted in the game. The spike found that without the fork's input exclusion they were queued by Xwayland and replayed on thaw.
- A Steam game toggled the same way, with the Steam client usable meanwhile.
- The bind pressed on Floorp does nothing and prints the reason in `journalctl --user -t niri`.
- `systemctl --user restart melibea.service` while a game is frozen thaws it.

- [ ] **Step 4: Record and commit**

Update the M12 status in `ROADMAP.md` with what was met and what was not. Then:

```bash
git add ROADMAP.md README.md
git commit -m "Record M12 standby's end-to-end and real-session results"
```
