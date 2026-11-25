# M12 Game detection without rules Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Pressing the suspend bind on a Steam, Wine or Proton game works with no `games.toml` rule, by asking the X server which host process owns the window and reading that process's environment.

**Architecture:** A small hand-written X11 client (`src/standby/x11.rs`) answers one question, the host pid owning the X11 top-level that matches a niri window's app id and title, through `QueryTree`, `GetProperty` and the X-Resource extension. A pure detector (`src/standby/detect.rs`) turns an owner pid into a `GameIdentity` from `SteamAppId` or `WINESERVERSOCKET`. The controller and the standby timers fall back to it when no rule and no `steam_app_<N>` app id recognize the window.

**Tech Stack:** Rust 2024, std only (Unix sockets), Melibea's existing four dependencies.

**Spec:** [docs/superpowers/specs/2026-11-17-melibea-standby-design.md](../specs/2026-11-17-melibea-standby-design.md), section "Recognizing a game without rules". Previous plan: [2026-11-24-melibea-standby.md](2026-11-24-melibea-standby.md).

## Global Constraints

- Every constraint of the previous plan still binds; the shared copy is `.superpowers/sdd/2026-11-24-melibea-standby/melibea-constraints.md`.
- No new dependency. The X11 client uses `std::os::unix::net::UnixStream` only.
- X11 limits: connect plus whole lookup bounded by `X11_TIMEOUT = 1 s` (socket read and write timeouts, and a deadline across requests); at most `MAX_X11_WINDOWS = 4096` root children examined; a property read asks for at most 4096 bytes (`long_length = 1024`); any reply whose declared length exceeds 1 MiB closes the connection with an error.
- Display: `$DISPLAY` of the form `:N` or `:N.S` only (local Unix socket `/tmp/.X11-unix/XN`); anything else (TCP, missing, abstract-only) means "no X11 lookup". Authorization: MIT-MAGIC-COOKIE-1 for display `N` read from `$XAUTHORITY` (or `$HOME/.Xauthority`) when an entry exists, else empty authorization.
- X-Resource request: major opcode from `QueryExtension("X-Resource")`; minor 4 `XResQueryClientIds` with one spec `{client: window id, mask: 2 (LocalClientPID)}`; the pid is the first `CARD32` of the value whose spec mask is 2.
- Game markers in the owner's `/proc/<pid>/environ`: `SteamAppId=<u64 > 0>` → `SteamApp`; else the variable `WINESERVERSOCKET` present → `Process { name: exe_basename(argv0).to_ascii_lowercase() }`; else not a game. `WINEPREFIX` is never a marker.
- Order of recognition: rule, then `steam_app_<N>`, then detection. A window whose niri pid is not `xwayland-satellite` (by `comm` `xwayland-satell`) is detected from its own niri pid; an `xwayland-satellite` window goes through X11.
- Exactly one distinct owner pid among matching X11 windows, else no recognition (reason stays `not_a_game`, with a `describe()` mentioning the ambiguity).
- Detected games have no `standby_after`.

## Review Focus

1. **Hostile or odd X server replies** (truncated reply, error packet, huge length, property of the wrong type): the lookup returns "unknown" and suspension says `not_a_game`; never a panic or a hang. Test: Task 1 `malformed_replies_are_errors_not_panics`.
2. **No `$DISPLAY` or no X server** (Wayland-only session, Xwayland not started): native windows still detect from their niri pid; X11 windows are just not recognized. Test: Task 2 `without_x11_a_satellite_window_is_not_a_game`.
3. **A browser or chat client** (Floorp, Claude, Siderita, which carry `WINEPREFIX` from the session): never recognized. Test: Task 2 `wineprefix_alone_is_not_a_game`.
4. **Two instances of the same game** with the same title: two owner pids, nothing recognized. Test: Task 1 `two_owners_are_ambiguous`.
5. **A slow X server**: the bind never blocks longer than `X11_TIMEOUT`. Test: Task 1 `a_silent_server_times_out`.

---

### Task 1: Minimal X11 client

**Files:**
- Create: `src/standby/x11.rs`; Modify: `src/standby/mod.rs`

**Interfaces:**
- Produces:
```rust
pub const X11_TIMEOUT: Duration = Duration::from_secs(1);
pub const MAX_X11_WINDOWS: usize = 4096;
#[derive(Debug, PartialEq, Eq)]
pub enum OwnerLookup { Owner(u32), NoMatch, Ambiguous(Vec<u32>) }
pub fn display_socket(display: &str) -> Option<(PathBuf, u16)>;      // ":0" → (/tmp/.X11-unix/X0, 0)
pub fn find_cookie(xauthority: &[u8], display: u16) -> Option<Vec<u8>>; // MIT-MAGIC-COOKIE-1, family Local or any hostname
pub struct X11Client { /* stream, root, atoms, xres opcode */ }
impl X11Client {
    pub fn connect(socket: &Path, cookie: Option<&[u8]>, timeout: Duration) -> Result<Self, X11Error>;
    pub fn connect_from_env() -> Result<Self, X11Error>;              // DISPLAY, XAUTHORITY/HOME
    pub fn window_owner(&mut self, app_id: &str, title: &str) -> Result<OwnerLookup, X11Error>;
}
pub enum X11Error { NoDisplay, Io(io::Error), Refused(String), Protocol(String), NoXRes, Timeout }
```
`window_owner` matches root children whose `WM_CLASS` instance or class equals `app_id` and whose `_NET_WM_NAME` (`UTF8_STRING`, else `WM_NAME` `STRING`) equals `title`; collects their owner pids via XRes; one distinct pid → `Owner`, none → `NoMatch`, more → `Ambiguous` (sorted).

- [ ] **Step 1: Write the failing tests** against a fake X server: a `UnixListener` in a per-test temp dir on a thread that reads requests and writes canned little-endian replies (helper builders for the setup reply, `InternAtom`, `QueryExtension`, `QueryTree`, `GetProperty`, `XResQueryClientIds`).
  - `display_socket_parses_local_displays`: `":0"` → X0, `":1.0"` → X1, `"host:0"`, `""`, `"wayland-1"` → `None`.
  - `finds_the_cookie_for_the_display`: an Xauthority blob with entries for displays 0 and 1 → the display-1 cookie; no MIT entry → `None`.
  - `setup_refusal_is_reported`: the server answers setup with status 0 and a reason → `Refused(reason)`.
  - `finds_the_owner_of_victoria_3`: root children `0x3400001` (`steam_proton`, `"Victoria 3"`, owner 90994), `0x3400002` (`steam_proton`, `"Default IME"`, 90994), `0x2800003` (`steam_proton`, `"Default IME"`, 91006) → `window_owner("steam_proton", "Victoria 3") == Owner(90994)`.
  - `two_owners_are_ambiguous`: two `"Victoria 3"` windows owned by 1 and 2 → `Ambiguous(vec![1, 2])`.
  - `no_match_is_no_match`, `falls_back_to_wm_name`.
  - `missing_xres_is_reported`: `QueryExtension` present=false → `NoXRes`.
  - `malformed_replies_are_errors_not_panics`: truncated reply, an X error packet (first byte 0), a reply length over 1 MiB → `Err(Protocol(_))`.
  - `a_silent_server_times_out`: the fake accepts and never answers; with a 200 ms timeout → `Err(Timeout | Io)` within 1 s.
- [ ] **Step 2: Run** `cargo test --lib standby::x11` → compile error.
- [ ] **Step 3: Implement.** Little-endian byte order (`'l'`), protocol 11.0. Requests needed: `InternAtom` (16) for `WM_CLASS`, `_NET_WM_NAME`, `UTF8_STRING` (`WM_NAME` and `STRING` are predefined atoms 39 and 31); `QueryExtension` (98); `QueryTree` (15); `GetProperty` (20, delete=false); XRes minor 4. Track the sequence number and skip events (first byte ≥ 2 that is not a reply) until the matching reply.
- [ ] **Step 4: Run** the three checks.
- [ ] **Step 5: Commit** `Ask the X server which process owns a window`.

### Task 2: Detection and wiring

**Files:**
- Create: `src/standby/detect.rs`; Modify: `src/standby/process.rs` (`ProcessInfo` gains `is_wine: bool` from `WINESERVERSOCKET`), `src/standby/controller.rs`, `src/main.rs` (`minimized_delays` only if it must change), `README.md`, `games.example.toml`

**Interfaces:**
- Consumes: Task 1; `ProcessTable`, `ProcessInfo`, `GameIdentity`, `exe_basename`.
- Produces:
```rust
pub fn identity_of(owner: &ProcessInfo) -> Option<GameIdentity>;  // SteamAppId > 0, else is_wine → Process
pub trait WindowOwners { fn owner(&mut self, app_id: &str, title: &str) -> Option<OwnerLookup>; }
pub struct LiveX11;   // connects per call via X11Client::connect_from_env; any error → None
pub fn detect(window: &WindowView, table: &ProcessTable, owners: &mut dyn WindowOwners)
    -> Result<Option<GameIdentity>, String>;   // Err(text) only for Ambiguous, for describe()
```
`detect`: if the window's niri pid process has `comm == "xwayland-satell"`, look up `owners.owner(app_id, title)` (both must be present) and take `Owner(pid)`; otherwise use the niri pid. Then `identity_of(table.get(pid))`.
In the controller, step 2 becomes: rules/`steam_app_<N>` → else `detect` → else `NotAGame` (the describe text names an ambiguity when `detect` returned `Err`). `Standby` gains a `Box<dyn WindowOwners>` constructor parameter (`LiveX11` in the daemon and `thaw-all`, a fake in tests). Step 6 ("same game") counts a window as the same game when it is recognized or detected with the same identity and (for rule and detection) the same app id; pid-in-set still counts. Detection is run once for the target and at most once per other window with the same app id.

- [ ] **Step 1: Write the failing tests**
  - `steam_app_id_in_the_environment_is_a_steam_game`, `a_wine_process_is_a_game_by_its_exe`, `wineprefix_alone_is_not_a_game` (process.rs fixture: `WINEPREFIX` set, no `WINESERVERSOCKET` → `is_wine == false`; `identity_of` → `None`).
  - controller: `suspend_detects_victoria_3_without_rules` (no `games.toml`; window `steam_proton`/`Victoria 3` with the satellite's pid; fake owners → `Owner(4242)` whose process is wine with argv0 `Z:\…\victoria3.exe` → moved and frozen exactly as with the rule).
  - `suspend_detects_a_native_steam_game_by_pid` (window `dotnet`, niri pid = game pid with `SteamAppId=1281930`, no X11 lookup made).
  - `without_x11_a_satellite_window_is_not_a_game` (fake owners returns `None` → `NotAGame`, no calls).
  - `an_ambiguous_x11_owner_is_refused` (`Ambiguous` → `NotAGame`, describe mentions two processes).
  - `a_rule_still_wins_over_detection`.
- [ ] **Step 2: Run** `cargo test --lib standby` → failures.
- [ ] **Step 3: Implement**, then document in README ("most games need no rule; rules for native non-Steam games and `standby_after`") and trim `games.example.toml` to a commented native-game example plus a `standby_after` example.
- [ ] **Step 4: Run** the three checks.
- [ ] **Step 5: Commit** `Recognize Steam, Wine and Proton games without a rule`.

### Task 3: Real-session check (controller with the author)

- [ ] Install Melibea (`scripts/install.sh`), with no `games.toml`. After the author's relogin: `Mod+B` then `S` on Victoria 3 freezes only `victoria3.exe`; on a Steam game it freezes the game's processes; on Floorp it prints `not_a_game`. Record the result in the M12 section of `ROADMAP.md` and commit `Record M12 game detection in the real session`.
