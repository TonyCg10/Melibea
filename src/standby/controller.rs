//! The standby controller: suspend, resume, reconcile and thaw-all.
//!
//! It ties the process table, the game rules, systemd, the ledger and niri
//! together under the spec's safety rule. Every refusal returns before any
//! later step runs, and every failure after a change undoes it, so nothing
//! stays frozen or flagged by a suspension that did not complete.

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::games::{GameIdentity, GameRules, game_set};
use super::ledger::{Ledger, LedgerEntry};
use super::process::{ProcessInfo, ProcessTable, app_unit};
use super::systemd::{FreezerState, Systemd, game_unit_name, is_game_unit};
use crate::niri_ipc::{NiriActionClient, NiriActionError, NiriWindowActionStatus};
use crate::protocol::{ActionResult, ActionStatus, Operation};
use crate::transition::WindowId;

/// How long moved pids may take to report their new unit in `/proc`.
pub const MOVE_CONFIRM_TIMEOUT: Duration = Duration::from_secs(2);

const MOVE_POLL_INTERVAL: Duration = Duration::from_millis(50);
/// Discoveries that may still move processes; the next one must find none outside.
const MAX_MOVES: usize = 2;
/// Highest `-<k>` suffix tried when the plain game unit name is taken.
const MAX_UNIT_SUFFIX: u32 = 64;
/// `comm` values that are never part of a game (`comm` is cut at 15 bytes).
const PROTECTED_COMMS: [&str; 3] = ["niri", "xwayland-satell", "Xwayland"];

/// A niri window, visible or minimized, as standby needs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowView {
    pub id: u64,
    pub app_id: Option<String>,
    pub title: Option<String>,
    pub pid: Option<i32>,
    pub is_focused: bool,
    pub is_suspended: bool,
    pub is_minimized: bool,
}

/// What standby needs from the compositor.
pub trait Compositor {
    /// Lists visible and minimized windows.
    ///
    /// # Errors
    /// Returns a description when the compositor cannot answer.
    fn windows(&mut self) -> Result<Vec<WindowView>, String>;
    /// Sets or clears a window's suspended flag.
    ///
    /// # Errors
    /// Returns a description when the compositor refuses or cannot answer.
    fn set_suspended(&mut self, window_id: u64, suspended: bool) -> Result<(), String>;
}

/// A source of process snapshots.
pub trait ProcessSource {
    /// Reads the current processes.
    ///
    /// # Errors
    /// Returns an error when the process list cannot be read.
    fn snapshot(&self) -> io::Result<ProcessTable>;
}

/// The real process source, `/proc`.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProcFs;

impl ProcessSource for ProcFs {
    fn snapshot(&self) -> io::Result<ProcessTable> {
        ProcessTable::read(Path::new("/proc"))
    }
}

/// The real compositor, niri's action socket.
pub struct LiveCompositor(NiriActionClient);

impl LiveCompositor {
    /// Wraps an action client.
    #[must_use]
    pub fn new(client: NiriActionClient) -> Self {
        Self(client)
    }

    /// Connects to niri's socket.
    ///
    /// # Errors
    /// Returns an error when niri cannot be reached.
    pub fn connect() -> Result<Self, NiriActionError> {
        NiriActionClient::connect().map(Self)
    }
}

impl Compositor for LiveCompositor {
    fn windows(&mut self) -> Result<Vec<WindowView>, String> {
        let visible = self.0.windows().map_err(|error| error.to_string())?;
        let minimized = self
            .0
            .minimized_windows()
            .map_err(|error| error.to_string())?;
        let mut views: Vec<WindowView> = visible
            .into_iter()
            .map(|window| WindowView {
                id: window.id,
                app_id: window.app_id,
                title: window.title,
                pid: window.pid,
                is_focused: window.is_focused,
                is_suspended: window.is_suspended,
                is_minimized: false,
            })
            .collect();
        for window in minimized {
            if views.iter().any(|view| view.id == window.id) {
                continue;
            }
            views.push(WindowView {
                id: window.id,
                app_id: window.app_id,
                title: window.title,
                pid: window.pid,
                is_focused: false,
                is_suspended: window.is_suspended,
                is_minimized: true,
            });
        }
        Ok(views)
    }

    fn set_suspended(&mut self, window_id: u64, suspended: bool) -> Result<(), String> {
        let result = self
            .0
            .set_window_suspended(WindowId(window_id), suspended)
            .map_err(|error| error.to_string())?;
        match result.status {
            NiriWindowActionStatus::Applied
            | NiriWindowActionStatus::AlreadyInRequestedState
            | NiriWindowActionStatus::LegacyHandled => Ok(()),
            status => Err(format!(
                "niri answered {status:?} to suspended={suspended} for window {window_id}"
            )),
        }
    }
}

/// Which way a standby request goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StandbyOperation {
    Suspend,
    Resume,
}

/// Why a standby request changed nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockReason {
    NotAGame,
    GameNotRunning,
    ProtectedProcess { pid: u32, comm: String },
    GameEscaped { pids: Vec<u32> },
    InvalidRules(String),
    Systemd(String),
    Ledger(String),
    Compositor(String),
}

impl BlockReason {
    /// The stable machine-readable code.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotAGame => "not_a_game",
            Self::GameNotRunning => "game_not_running",
            Self::ProtectedProcess { .. } => "protected_process",
            Self::GameEscaped { .. } => "game_escaped",
            Self::InvalidRules(_) => "invalid_rules",
            Self::Systemd(_) => "systemd",
            Self::Ledger(_) => "ledger",
            Self::Compositor(_) => "compositor",
        }
    }

    /// A human-readable description.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::NotAGame => "the window is not a recognized game".into(),
            Self::GameNotRunning => "no process of the game is running".into(),
            Self::ProtectedProcess { pid, comm } => {
                format!("process {pid} ({comm}) must not be frozen")
            }
            Self::GameEscaped { pids } => {
                format!("game processes {pids:?} stayed outside the game scope")
            }
            Self::InvalidRules(text) => format!("invalid game rules: {text}"),
            Self::Systemd(text) => format!("systemd: {text}"),
            Self::Ledger(text) => format!("ledger: {text}"),
            Self::Compositor(text) => format!("niri: {text}"),
        }
    }
}

/// Maps a standby outcome onto the protocol's action result.
///
/// The outcome does not keep the id the client sent when it named none, so
/// `requested_id` carries the resolved window (the requested id when the
/// window was not found).
#[must_use]
pub fn outcome_result(outcome: &StandbyOutcome) -> ActionResult {
    let operation_of = |operation: StandbyOperation| match operation {
        StandbyOperation::Suspend => Operation::Suspend,
        StandbyOperation::Resume => Operation::Resume,
    };
    match outcome {
        StandbyOutcome::Applied {
            operation,
            window_id,
            ..
        } => ActionResult {
            operation: operation_of(*operation),
            requested_id: Some(*window_id),
            window_id: Some(*window_id),
            status: ActionStatus::Applied,
            reason: None,
        },
        StandbyOutcome::AlreadyInRequestedState {
            operation,
            window_id,
        } => ActionResult {
            operation: operation_of(*operation),
            requested_id: Some(*window_id),
            window_id: Some(*window_id),
            status: ActionStatus::AlreadyInRequestedState,
            reason: None,
        },
        StandbyOutcome::WindowNotFound {
            operation,
            requested_id,
        } => ActionResult {
            operation: operation_of(*operation),
            requested_id: *requested_id,
            window_id: None,
            status: ActionStatus::WindowNotFound,
            reason: None,
        },
        StandbyOutcome::Blocked {
            operation,
            window_id,
            reason,
        } => ActionResult {
            operation: operation_of(*operation),
            requested_id: *window_id,
            window_id: *window_id,
            status: ActionStatus::Blocked,
            reason: Some(format!("{}: {}", reason.code(), reason.describe())),
        },
    }
}

/// The result of a standby request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StandbyOutcome {
    Applied {
        operation: StandbyOperation,
        window_id: u64,
        windows: Vec<u64>,
    },
    AlreadyInRequestedState {
        operation: StandbyOperation,
        window_id: u64,
    },
    WindowNotFound {
        operation: StandbyOperation,
        requested_id: Option<u64>,
    },
    Blocked {
        operation: StandbyOperation,
        window_id: Option<u64>,
        reason: BlockReason,
    },
}

fn blocked(
    operation: StandbyOperation,
    window_id: Option<u64>,
    reason: BlockReason,
) -> StandbyOutcome {
    StandbyOutcome::Blocked {
        operation,
        window_id,
        reason,
    }
}

/// The window a request names, or the focused one.
fn target(windows: &[WindowView], window_id: Option<u64>) -> Option<&WindowView> {
    match window_id {
        Some(id) => windows.iter().find(|window| window.id == id),
        None => windows.iter().find(|window| window.is_focused),
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// Suspends and resumes games.
pub struct Standby<S: Systemd, P: ProcessSource> {
    systemd: S,
    processes: P,
    ledger: Ledger,
    games_path: PathBuf,
    own_pid: u32,
    uid: u32,
}

impl<S: Systemd, P: ProcessSource> Standby<S, P> {
    /// `own_pid` is Melibea's pid and `uid` the user's; both are protected by the safety rule.
    pub fn new(
        systemd: S,
        processes: P,
        ledger: Ledger,
        games_path: PathBuf,
        own_pid: u32,
        uid: u32,
    ) -> Self {
        Self {
            systemd,
            processes,
            ledger,
            games_path,
            own_pid,
            uid,
        }
    }

    /// Suspends the named window's game, or the focused window's.
    pub fn suspend(
        &mut self,
        niri: &mut impl Compositor,
        window_id: Option<u64>,
    ) -> StandbyOutcome {
        self.try_suspend(niri, window_id)
            .unwrap_or_else(|outcome| outcome)
    }

    /// Resumes the named window's game, or the focused window's.
    pub fn resume(&mut self, niri: &mut impl Compositor, window_id: Option<u64>) -> StandbyOutcome {
        self.try_resume(niri, window_id)
            .unwrap_or_else(|outcome| outcome)
    }

    /// Resumes when the target has a ledger entry, else suspends.
    pub fn toggle(&mut self, niri: &mut impl Compositor, window_id: Option<u64>) -> StandbyOutcome {
        let target_id = window_id.or_else(|| {
            niri.windows()
                .ok()
                .and_then(|windows| target(&windows, None).map(|window| window.id))
        });
        if target_id.is_some_and(|id| self.is_suspended(id)) {
            self.resume(niri, window_id)
        } else {
            self.suspend(niri, window_id)
        }
    }

    /// Whether the ledger records `window_id` as suspended; an unreadable ledger says no.
    #[must_use]
    pub fn is_suspended(&self, window_id: u64) -> bool {
        self.ledger.read().is_ok_and(|entries| {
            entries
                .iter()
                .any(|entry| entry.windows.contains(&window_id))
        })
    }

    // ---- the safety rule -------------------------------------------------

    /// The one place that decides whether a process belongs to the user.
    fn owned_by_user(&self, process: &ProcessInfo) -> bool {
        process.uid == self.uid
    }

    /// Safety rule 3: a process that must never be frozen.
    fn is_protected(&self, process: &ProcessInfo) -> bool {
        !self.owned_by_user(process)
            || process.pid == self.own_pid
            || PROTECTED_COMMS.contains(&process.comm.as_str())
            || app_unit(&process.cgroup, self.uid).is_none()
    }

    fn snapshot(&self) -> Result<ProcessTable, BlockReason> {
        self.processes
            .snapshot()
            .map_err(|error| BlockReason::Systemd(format!("cannot read processes: {error}")))
    }

    fn check_members(&self, table: &ProcessTable, set: &BTreeSet<u32>) -> Result<(), BlockReason> {
        for process in set.iter().filter_map(|pid| table.get(*pid)) {
            if self.is_protected(process) {
                return Err(BlockReason::ProtectedProcess {
                    pid: process.pid,
                    comm: process.comm.clone(),
                });
            }
        }
        Ok(())
    }

    /// Members whose unit is not a Melibea game unit.
    fn stragglers(&self, table: &ProcessTable, set: &BTreeSet<u32>) -> BTreeSet<u32> {
        set.iter()
            .copied()
            .filter(|pid| {
                table
                    .get(*pid)
                    .and_then(|process| app_unit(&process.cgroup, self.uid))
                    .is_none_or(|unit| !is_game_unit(unit))
            })
            .collect()
    }

    /// Steps 3 and 4: discover the game, check it, and move stragglers into a
    /// game scope, at most twice; the last discovery must find none outside.
    fn separate(
        &self,
        identity: &GameIdentity,
    ) -> Result<(ProcessTable, BTreeSet<u32>), BlockReason> {
        let mut moves = 0;
        loop {
            let table = self.snapshot()?;
            let set = game_set(&table, identity);
            if set.is_empty() {
                return Err(BlockReason::GameNotRunning);
            }
            self.check_members(&table, &set)?;
            let stragglers = self.stragglers(&table, &set);
            if stragglers.is_empty() {
                return Ok((table, set));
            }
            if moves == MAX_MOVES {
                return Err(BlockReason::GameEscaped {
                    pids: stragglers.into_iter().collect(),
                });
            }
            moves += 1;
            let unit = self.free_unit_name(&table, &stragglers)?;
            self.systemd
                .start_scope(&unit, &stragglers)
                .map_err(|error| BlockReason::Systemd(error.to_string()))?;
            self.confirm_move(&unit, &stragglers)?;
        }
    }

    fn free_unit_name(
        &self,
        table: &ProcessTable,
        stragglers: &BTreeSet<u32>,
    ) -> Result<String, BlockReason> {
        let root = table
            .roots(stragglers)
            .into_iter()
            .min()
            .ok_or_else(|| BlockReason::Systemd("no root process to name the scope".into()))?;
        for suffix in std::iter::once(None).chain((1..=MAX_UNIT_SUFFIX).map(Some)) {
            let name = game_unit_name(root, suffix);
            let state = self
                .systemd
                .freezer_state(&name)
                .map_err(|error| BlockReason::Systemd(error.to_string()))?;
            if state == FreezerState::NotLoaded {
                return Ok(name);
            }
        }
        Err(BlockReason::Systemd(format!(
            "no free scope name for process {root}"
        )))
    }

    /// Polls `/proc` until every surviving straggler reports `unit`.
    fn confirm_move(&self, unit: &str, stragglers: &BTreeSet<u32>) -> Result<(), BlockReason> {
        let deadline = Instant::now() + MOVE_CONFIRM_TIMEOUT;
        loop {
            let table = self.snapshot()?;
            let moved = stragglers
                .iter()
                .filter_map(|pid| table.get(*pid))
                .all(|process| app_unit(&process.cgroup, self.uid) == Some(unit));
            if moved {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(BlockReason::Systemd(format!(
                    "processes did not move into {unit} within {} s",
                    MOVE_CONFIRM_TIMEOUT.as_secs()
                )));
            }
            thread::sleep(MOVE_POLL_INTERVAL);
        }
    }

    /// Step 5: the set's units, each holding only game processes.
    fn target_units(
        &self,
        table: &ProcessTable,
        set: &BTreeSet<u32>,
    ) -> Result<Vec<String>, BlockReason> {
        let units: BTreeSet<String> = set
            .iter()
            .filter_map(|pid| table.get(*pid))
            .filter_map(|process| app_unit(&process.cgroup, self.uid))
            .map(str::to_owned)
            .collect();
        let slice = format!(
            "/user.slice/user-{uid}.slice/user@{uid}.service/app.slice/",
            uid = self.uid
        );
        for unit in &units {
            if !is_game_unit(unit) {
                return Err(BlockReason::Systemd(format!("{unit} is not a game unit")));
            }
            let path = format!("{slice}{unit}");
            let inside = |cgroup: &str| {
                cgroup == path
                    || cgroup
                        .strip_prefix(path.as_str())
                        .is_some_and(|rest| rest.starts_with('/'))
            };
            if let Some(intruder) = table
                .iter()
                .find(|process| inside(&process.cgroup) && !set.contains(&process.pid))
            {
                return Err(BlockReason::ProtectedProcess {
                    pid: intruder.pid,
                    comm: intruder.comm.clone(),
                });
            }
        }
        Ok(units.into_iter().collect())
    }

    // ---- suspend ---------------------------------------------------------

    fn try_suspend(
        &mut self,
        niri: &mut impl Compositor,
        window_id: Option<u64>,
    ) -> Result<StandbyOutcome, StandbyOutcome> {
        let op = StandbyOperation::Suspend;
        // Step 1: the target, and a ledger proven writable.
        let windows = niri
            .windows()
            .map_err(|error| blocked(op, window_id, BlockReason::Compositor(error)))?;
        let target =
            target(&windows, window_id)
                .cloned()
                .ok_or(StandbyOutcome::WindowNotFound {
                    operation: op,
                    requested_id: window_id,
                })?;
        let id = target.id;
        let block = |reason| blocked(op, Some(id), reason);
        let ledger_error =
            |error: super::ledger::LedgerError| block(BlockReason::Ledger(error.to_string()));
        let entries = self.ledger.read().map_err(ledger_error)?;
        self.ledger.write(&entries).map_err(ledger_error)?;
        if entries.iter().any(|entry| entry.windows.contains(&id)) {
            return Err(StandbyOutcome::AlreadyInRequestedState {
                operation: op,
                window_id: id,
            });
        }
        // Step 2: recognize.
        let rules = GameRules::load(&self.games_path)
            .map_err(|error| block(BlockReason::InvalidRules(error.to_string())))?;
        let game = rules
            .recognize(target.app_id.as_deref(), target.title.as_deref())
            .ok_or_else(|| block(BlockReason::NotAGame))?;
        // Steps 3–5: discover, check, separate, check the units.
        let (table, set) = self.separate(&game.identity).map_err(block)?;
        let units = self.target_units(&table, &set).map_err(block)?;
        // Step 6: the game's windows.
        let game_windows: Vec<u64> = windows
            .iter()
            .filter(|window| {
                let same_game = rules
                    .recognize(window.app_id.as_deref(), window.title.as_deref())
                    .is_some_and(|other| other.identity == game.identity);
                let in_set = window
                    .pid
                    .and_then(|pid| u32::try_from(pid).ok())
                    .is_some_and(|pid| set.contains(&pid));
                same_game || in_set
            })
            .map(|window| window.id)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        // Step 7: record before changing anything visible or frozen.
        let mut recorded = entries.clone();
        recorded.push(LedgerEntry {
            units: units.clone(),
            windows: game_windows.clone(),
            identity: game.identity,
            since_unix: now_unix(),
            flagged: true,
        });
        self.ledger.write(&recorded).map_err(ledger_error)?;
        // Step 8: flag the windows.
        for (index, window) in game_windows.iter().enumerate() {
            if let Err(error) = niri.set_suspended(*window, true) {
                // The failing window too: a lost reply may have set it.
                let unflag = Self::unflag(niri, &game_windows[..=index]);
                let note = self.restore_ledger(&entries);
                return Err(block(BlockReason::Compositor(format!(
                    "{error}{unflag}{note}"
                ))));
            }
        }
        // Step 9: freeze.
        for (index, unit) in units.iter().enumerate() {
            if let Err(error) = self.systemd.freeze(unit) {
                // The failing unit too: a timed-out freeze may still complete.
                let stuck = self.thaw_each(&units[..=index]);
                let note = if stuck.is_empty() {
                    let unflag = Self::unflag(niri, &game_windows);
                    format!("{unflag}{}", self.restore_ledger(&entries))
                } else {
                    // Something may still be frozen: keep the entry so recovery
                    // knows the units, and keep the flags so the window takes
                    // no input before its client can answer.
                    format!("; still frozen: {}", stuck.join("; "))
                };
                return Err(block(BlockReason::Systemd(format!("{error}{note}"))));
            }
        }
        Ok(StandbyOutcome::Applied {
            operation: op,
            window_id: id,
            windows: game_windows,
        })
    }

    /// Clears flags; returns a note naming the ones that stay set.
    fn unflag(niri: &mut impl Compositor, windows: &[u64]) -> String {
        let failures: Vec<String> = windows
            .iter()
            .filter_map(|window| {
                niri.set_suspended(*window, false)
                    .err()
                    .map(|error| format!("window {window}: {error}"))
            })
            .collect();
        if failures.is_empty() {
            String::new()
        } else {
            format!("; flags not cleared: {}", failures.join("; "))
        }
    }

    /// Thaws each unit; returns the failures.
    fn thaw_each(&self, units: &[String]) -> Vec<String> {
        units
            .iter()
            .filter_map(|unit| self.thaw_confirmed(unit).err())
            .collect()
    }

    /// Writes `entries` back; returns a note when that fails.
    fn restore_ledger(&self, entries: &[LedgerEntry]) -> String {
        match self.ledger.write(entries) {
            Ok(()) => String::new(),
            Err(error) => format!("; ledger not restored: {error}"),
        }
    }

    // ---- resume ----------------------------------------------------------

    /// Thaws `unit` and confirms it running or gone.
    fn thaw_confirmed(&self, unit: &str) -> Result<(), String> {
        let thawed = self.systemd.thaw(unit);
        let state = self
            .systemd
            .freezer_state(unit)
            .map_err(|error| format!("{unit}: {error}"))?;
        match (state, thawed) {
            (FreezerState::Running | FreezerState::NotLoaded, _) => Ok(()),
            (_, Err(error)) => Err(format!("cannot thaw {unit}: {error}")),
            (state, Ok(())) => Err(format!("{unit} is {state:?} after thawing")),
        }
    }

    fn try_resume(
        &mut self,
        niri: &mut impl Compositor,
        window_id: Option<u64>,
    ) -> Result<StandbyOutcome, StandbyOutcome> {
        let op = StandbyOperation::Resume;
        let windows = niri
            .windows()
            .map_err(|error| blocked(op, window_id, BlockReason::Compositor(error)))?;
        let target_id = match window_id {
            Some(id) => id,
            None => target(&windows, None).map(|window| window.id).ok_or(
                StandbyOutcome::WindowNotFound {
                    operation: op,
                    requested_id: None,
                },
            )?,
        };
        let block = |reason| blocked(op, Some(target_id), reason);
        let mut entries = self
            .ledger
            .read()
            .map_err(|error| block(BlockReason::Ledger(error.to_string())))?;
        let Some(index) = entries
            .iter()
            .position(|entry| entry.windows.contains(&target_id))
        else {
            return Err(if windows.iter().any(|window| window.id == target_id) {
                StandbyOutcome::AlreadyInRequestedState {
                    operation: op,
                    window_id: target_id,
                }
            } else {
                StandbyOutcome::WindowNotFound {
                    operation: op,
                    requested_id: window_id,
                }
            });
        };
        let entry = entries[index].clone();
        let failures = self.thaw_each(&entry.units);
        if !failures.is_empty() {
            return Err(block(BlockReason::Systemd(failures.join("; "))));
        }
        let mut errors = Vec::new();
        let mut still_flagged = Vec::new();
        for window in &entry.windows {
            if windows.iter().any(|view| view.id == *window)
                && let Err(error) = niri.set_suspended(*window, false)
            {
                errors.push(format!("window {window}: {error}"));
                still_flagged.push(*window);
            }
        }
        if still_flagged.is_empty() {
            entries.remove(index);
        } else {
            // Keep the entry for the windows still flagged, so a retried
            // resume finds it; thawing again is harmless.
            entries[index].windows = still_flagged;
        }
        self.ledger
            .write(&entries)
            .map_err(|error| block(BlockReason::Ledger(error.to_string())))?;
        if !errors.is_empty() {
            return Err(block(BlockReason::Compositor(errors.join("; "))));
        }
        Ok(StandbyOutcome::Applied {
            operation: op,
            window_id: target_id,
            windows: entry.windows,
        })
    }

    // ---- recovery --------------------------------------------------------

    /// Brings the ledger, niri's flags and systemd's freezer back into agreement.
    pub fn reconcile(&mut self, niri: &mut impl Compositor) -> Vec<String> {
        let entries = match self.ledger.read() {
            Ok(entries) => entries,
            Err(error) => {
                let mut lines = vec![format!("ledger unreadable, thawing everything: {error}")];
                lines.extend(self.thaw_all(Some(niri)));
                return lines;
            }
        };
        let mut lines = Vec::new();
        let windows = match niri.windows() {
            Ok(windows) => Some(windows),
            Err(error) => {
                lines.push(format!("cannot list niri windows: {error}"));
                None
            }
        };
        let mut kept = Vec::new();
        let mut changed = false;
        for entry in entries {
            let Some(windows) = &windows else {
                kept.push(entry);
                continue;
            };
            let names = entry.units.join(", ");
            if self.units_gone(&entry) {
                // Its windows' flags are cleared below, with every flag no kept entry owns.
                lines.push(format!("dropped the entry for {names}: its units are gone"));
                changed = true;
            } else if !entry
                .windows
                .iter()
                .any(|id| windows.iter().any(|v| v.id == *id && v.is_suspended))
            {
                let failures = self.thaw_each(&entry.units);
                if failures.is_empty() {
                    lines.push(format!("thawed {names}: no suspended window is left"));
                    changed = true;
                } else {
                    lines.push(format!("cannot thaw {names}: {}", failures.join("; ")));
                    kept.push(entry);
                }
            } else {
                kept.push(entry);
            }
        }
        if let Some(windows) = &windows {
            for window in windows.iter().filter(|v| v.is_suspended) {
                if !kept.iter().any(|entry| entry.windows.contains(&window.id)) {
                    lines.push(flag_line(niri, window.id));
                }
            }
        }
        let named: BTreeSet<&String> = kept.iter().flat_map(|entry| &entry.units).collect();
        match self.systemd.game_units() {
            Ok(units) => {
                for unit in units.iter().filter(|unit| !named.contains(unit)) {
                    if matches!(
                        self.systemd.freezer_state(unit),
                        Ok(FreezerState::Frozen | FreezerState::Freezing)
                    ) {
                        lines.push(match self.systemd.thaw(unit) {
                            Ok(()) => format!("thawed {unit}: no entry names it"),
                            Err(error) => format!("cannot thaw {unit}: {error}"),
                        });
                    }
                }
            }
            Err(error) => lines.push(format!("cannot list game units: {error}")),
        }
        if changed && let Err(error) = self.ledger.write(&kept) {
            lines.push(format!("cannot write the ledger: {error}"));
        }
        lines
    }

    /// Whether every unit of `entry` is gone; an unanswered query keeps it.
    fn units_gone(&self, entry: &LedgerEntry) -> bool {
        entry.units.iter().all(|unit| {
            matches!(
                self.systemd.freezer_state(unit),
                Ok(FreezerState::NotLoaded)
            )
        })
    }

    /// Thaws every ledger unit and every listed game unit, clears niri's flags
    /// when given niri, and empties the ledger.
    pub fn thaw_all<C: Compositor>(&mut self, niri: Option<&mut C>) -> Vec<String> {
        let mut lines = Vec::new();
        let mut units = BTreeSet::new();
        match self.ledger.read() {
            Ok(entries) => units.extend(entries.into_iter().flat_map(|entry| entry.units)),
            Err(error) => lines.push(format!("ledger unreadable: {error}")),
        }
        match self.systemd.game_units() {
            Ok(listed) => units.extend(listed),
            Err(error) => lines.push(format!("cannot list game units: {error}")),
        }
        for unit in &units {
            lines.push(match self.systemd.thaw(unit) {
                Ok(()) => format!("thawed {unit}"),
                Err(error) => format!("cannot thaw {unit}: {error}"),
            });
        }
        if let Some(niri) = niri {
            match niri.windows() {
                Ok(windows) => {
                    for window in windows.iter().filter(|window| window.is_suspended) {
                        lines.push(flag_line(niri, window.id));
                    }
                }
                Err(error) => lines.push(format!("cannot list niri windows: {error}")),
            }
        }
        lines.push(match self.ledger.write(&[]) {
            Ok(()) => "emptied the ledger".to_owned(),
            Err(error) => format!("cannot empty the ledger: {error}"),
        });
        lines
    }
}

/// Clears one window's flag and describes the result.
fn flag_line(niri: &mut impl Compositor, window_id: u64) -> String {
    match niri.set_suspended(window_id, false) {
        Ok(()) => format!("cleared the suspended flag of window {window_id}"),
        Err(error) => format!("cannot clear the suspended flag of window {window_id}: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::io;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::rc::Rc;

    use super::*;
    use crate::standby::games::GameIdentity;
    use crate::standby::ledger::{Ledger, LedgerEntry};
    use crate::standby::process::{ProcessInfo, ProcessTable};
    use crate::standby::systemd::{FreezerState, Systemd, SystemdError};

    const UID: u32 = 1000;
    const OWN_PID: u32 = 9999;
    const APP: &str = "/user.slice/user-1000.slice/user@1000.service/app.slice/";
    const NIRI_SERVICE: &str =
        "/user.slice/user-1000.slice/user@1000.service/session.slice/niri.service";
    const BOTTLES: &str = "app-flatpak-com.usebottles.bottles-1.scope";
    const STEAM_CLIENT: &str = "app-flatpak-com.valvesoftware.Steam-2.scope";
    const STEAM_LAUNCHER: &str = "app-flatpak-com.valvesoftware.Steam-3.scope";
    const STEAM_CONTAINER: &str = "app-flatpak-com.valvesoftware.Steam-4.scope";
    const GAME_UNIT: &str = "app-melibea-game-4242.scope";
    const VICTORIA_RULES: &str =
        "[[game]]\ntitle = \"^Victoria 3$\"\nprocess = \"victoria3.exe\"\n";
    const VICTORIA_EXE: &str = "Z:\\mnt\\JUEGOS\\victoria3.exe";

    #[derive(Default)]
    struct World {
        processes: BTreeMap<u32, ProcessInfo>,
        states: BTreeMap<String, FreezerState>,
        calls: Vec<String>,
        /// A failing freeze leaves its unit in this state (a timed-out freeze may complete).
        fail_freeze: Option<FreezerState>,
        fail_thaw: bool,
        /// `set_suspended(window, suspended)` calls that fail.
        fail_flag: BTreeSet<(u64, bool)>,
        /// One game child forked into the Bottles unit after each move.
        forks: Vec<u32>,
        /// A foreign process that appears in this unit when it is started.
        intruder_on: Option<String>,
        windows: Vec<WindowView>,
    }

    type Shared = Rc<RefCell<World>>;

    struct FakeSystemd(Shared);
    struct FakeProcesses(Shared);
    struct FakeCompositor(Shared);

    fn failed(text: &str) -> SystemdError {
        SystemdError::Failed {
            program: "systemctl".into(),
            stderr: text.into(),
        }
    }

    impl Systemd for FakeSystemd {
        fn start_scope(&self, unit: &str, pids: &BTreeSet<u32>) -> Result<(), SystemdError> {
            let mut world = self.0.borrow_mut();
            world.calls.push(format!("start_scope {unit} {pids:?}"));
            for pid in pids {
                if let Some(process) = world.processes.get_mut(pid) {
                    process.cgroup = in_app(unit);
                }
            }
            world.states.insert(unit.to_owned(), FreezerState::Running);
            if world.intruder_on.as_deref() == Some(unit) {
                world.processes.insert(
                    6000,
                    process(6000, 1, "intruder", Some("intruder"), &in_app(unit)),
                );
            }
            if !world.forks.is_empty() {
                let pid = world.forks.remove(0);
                world.processes.insert(
                    pid,
                    process(
                        pid,
                        4100,
                        "Main Thread",
                        Some(VICTORIA_EXE),
                        &in_app(BOTTLES),
                    ),
                );
            }
            Ok(())
        }

        fn freeze(&self, unit: &str) -> Result<(), SystemdError> {
            let mut world = self.0.borrow_mut();
            world.calls.push(format!("freeze {unit}"));
            if let Some(state) = world.fail_freeze {
                world.states.insert(unit.to_owned(), state);
                return Err(failed("Access denied"));
            }
            world.states.insert(unit.to_owned(), FreezerState::Frozen);
            Ok(())
        }

        fn thaw(&self, unit: &str) -> Result<(), SystemdError> {
            let mut world = self.0.borrow_mut();
            world.calls.push(format!("thaw {unit}"));
            if world.fail_thaw {
                return Err(failed("timed out"));
            }
            match world.states.get(unit) {
                None | Some(FreezerState::NotLoaded) => Err(failed("Unit not loaded")),
                Some(_) => {
                    world.states.insert(unit.to_owned(), FreezerState::Running);
                    Ok(())
                }
            }
        }

        fn freezer_state(&self, unit: &str) -> Result<FreezerState, SystemdError> {
            let mut world = self.0.borrow_mut();
            world.calls.push(format!("freezer_state {unit}"));
            Ok(world
                .states
                .get(unit)
                .copied()
                .unwrap_or(FreezerState::NotLoaded))
        }

        fn game_units(&self) -> Result<Vec<String>, SystemdError> {
            let mut world = self.0.borrow_mut();
            world.calls.push("game_units".into());
            Ok(world
                .states
                .iter()
                .filter(|(_, state)| **state != FreezerState::NotLoaded)
                .map(|(unit, _)| unit.clone())
                .collect())
        }
    }

    impl ProcessSource for FakeProcesses {
        fn snapshot(&self) -> io::Result<ProcessTable> {
            Ok(ProcessTable::new(
                self.0.borrow().processes.values().cloned(),
            ))
        }
    }

    impl Compositor for FakeCompositor {
        fn windows(&mut self) -> Result<Vec<WindowView>, String> {
            Ok(self.0.borrow().windows.clone())
        }

        fn set_suspended(&mut self, window_id: u64, suspended: bool) -> Result<(), String> {
            let mut world = self.0.borrow_mut();
            world
                .calls
                .push(format!("set_suspended {window_id} {suspended}"));
            if world.fail_flag.contains(&(window_id, suspended)) {
                return Err(format!("niri refused window {window_id}"));
            }
            let window = world
                .windows
                .iter_mut()
                .find(|window| window.id == window_id)
                .ok_or_else(|| format!("window {window_id} not found"))?;
            window.is_suspended = suspended;
            Ok(())
        }
    }

    fn in_app(unit: &str) -> String {
        format!("{APP}{unit}")
    }

    fn process(
        pid: u32,
        parent: u32,
        comm: &str,
        argv0: Option<&str>,
        cgroup: &str,
    ) -> ProcessInfo {
        ProcessInfo {
            pid,
            ppid: parent,
            uid: UID,
            comm: comm.into(),
            argv0: argv0.map(Into::into),
            cgroup: cgroup.into(),
            steam_app_id: None,
        }
    }

    fn steam(pid: u32, parent: u32, comm: &str, unit: &str, app_id: Option<u64>) -> ProcessInfo {
        ProcessInfo {
            steam_app_id: app_id,
            ..process(pid, parent, comm, Some(comm), &in_app(unit))
        }
    }

    fn window(id: u64, app_id: &str, title: &str, pid: i32, focused: bool) -> WindowView {
        WindowView {
            id,
            app_id: Some(app_id.into()),
            title: Some(title.into()),
            pid: Some(pid),
            is_focused: focused,
            is_suspended: false,
            is_minimized: false,
        }
    }

    /// The spec's session: Bottles running Victoria 3, the Steam client, the
    /// satellite in `niri.service`, and Victoria's window with the satellite's pid.
    fn victoria_world() -> World {
        let bottles = in_app(BOTTLES);
        let processes = [
            process(1900, 1, "niri", Some("niri"), NIRI_SERVICE),
            process(
                2000,
                1900,
                "xwayland-satell",
                Some("xwayland-satellite"),
                NIRI_SERVICE,
            ),
            process(3000, 1, "steam", Some("steam"), &in_app(STEAM_CLIENT)),
            process(4000, 1, "bwrap", Some("bwrap"), &bottles),
            process(4100, 4000, "bottles", Some("bottles"), &bottles),
            process(4200, 4100, "wineserver", Some("wineserver"), &bottles),
            process(4242, 4100, "Main Thread", Some(VICTORIA_EXE), &bottles),
            process(
                5500,
                1,
                "floorp",
                Some("floorp"),
                &in_app("app-floorp-1.scope"),
            ),
            process(
                OWN_PID,
                1,
                "melibea",
                Some("melibea"),
                &in_app("melibea.service"),
            ),
        ];
        World {
            processes: processes.into_iter().map(|p| (p.pid, p)).collect(),
            windows: vec![
                window(608, "steam_proton", "Victoria 3", 2000, true),
                window(700, "floorp", "Floorp", 5500, false),
            ],
            ..World::default()
        }
    }

    /// The tModLoader layout the spike found: `SteamAppId=5` in three Steam units.
    fn steam_world() -> World {
        let processes = [
            steam(3000, 1, "steam", STEAM_CLIENT, None),
            steam(
                3100,
                3000,
                "steam-runtime-launch-client",
                STEAM_CLIENT,
                Some(5),
            ),
            steam(3200, 1, "reaper", STEAM_LAUNCHER, Some(5)),
            steam(3300, 1, "bwrap", STEAM_CONTAINER, None),
            steam(3350, 3300, "xdg-dbus-proxy", STEAM_CONTAINER, None),
            steam(3400, 3300, "dotnet", STEAM_CONTAINER, Some(5)),
            steam(3401, 3400, "dotnet-child", STEAM_CONTAINER, None),
        ];
        World {
            processes: processes.into_iter().map(|p| (p.pid, p)).collect(),
            windows: vec![window(800, "steam_app_5", "tModLoader", 3400, true)],
            ..World::default()
        }
    }

    struct Fixture {
        dir: PathBuf,
        world: Shared,
        standby: Standby<FakeSystemd, FakeProcesses>,
    }

    impl Fixture {
        fn new(name: &str, rules: Option<&str>, world: World) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("melibea-controller-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            if let Some(rules) = rules {
                fs::write(dir.join("games.toml"), rules).unwrap();
            }
            let world = Rc::new(RefCell::new(world));
            let standby = Standby::new(
                FakeSystemd(world.clone()),
                FakeProcesses(world.clone()),
                Ledger::at(dir.join("run").join("standby.json")),
                dir.join("games.toml"),
                OWN_PID,
                UID,
            );
            Self {
                dir,
                world,
                standby,
            }
        }

        fn victoria(name: &str) -> Self {
            Self::new(name, Some(VICTORIA_RULES), victoria_world())
        }

        fn niri(&self) -> FakeCompositor {
            FakeCompositor(self.world.clone())
        }

        fn calls(&self) -> Vec<String> {
            self.world.borrow().calls.clone()
        }

        /// Calls that change something: no `freezer_state` or `game_units` queries.
        fn changes(&self) -> Vec<String> {
            self.calls()
                .into_iter()
                .filter(|call| !call.starts_with("freezer_state") && call != "game_units")
                .collect()
        }

        fn clear_calls(&self) {
            self.world.borrow_mut().calls.clear();
        }

        fn entries(&self) -> Vec<LedgerEntry> {
            self.standby.ledger.read().unwrap()
        }

        fn cgroup(&self, pid: u32) -> String {
            self.world.borrow().processes[&pid].cgroup.clone()
        }

        fn state(&self, unit: &str) -> Option<FreezerState> {
            self.world.borrow().states.get(unit).copied()
        }

        fn suspended_windows(&self) -> Vec<u64> {
            self.world
                .borrow()
                .windows
                .iter()
                .filter(|window| window.is_suspended)
                .map(|window| window.id)
                .collect()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::set_permissions(&self.dir, fs::Permissions::from_mode(0o755));
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    fn blocked_code(outcome: &StandbyOutcome) -> &'static str {
        match outcome {
            StandbyOutcome::Blocked { reason, .. } => reason.code(),
            other => panic!("expected a block, got {other:?}"),
        }
    }

    fn victoria_entry(units: &[&str]) -> LedgerEntry {
        LedgerEntry {
            units: units.iter().map(|unit| (*unit).to_owned()).collect(),
            windows: vec![608],
            identity: GameIdentity::Process {
                name: "victoria3.exe".into(),
            },
            since_unix: 1,
            flagged: true,
        }
    }

    #[test]
    fn suspend_moves_only_the_bottles_game_then_flags_then_freezes() {
        let f = Fixture::victoria("bottles");
        let mut f = f;
        let mut niri = f.niri();
        let outcome = f.standby.suspend(&mut niri, None);
        assert_eq!(
            outcome,
            StandbyOutcome::Applied {
                operation: StandbyOperation::Suspend,
                window_id: 608,
                windows: vec![608],
            }
        );
        assert_eq!(
            f.changes(),
            [
                "start_scope app-melibea-game-4242.scope {4242}",
                "set_suspended 608 true",
                "freeze app-melibea-game-4242.scope",
            ]
        );
        assert_eq!(f.cgroup(4242), in_app(GAME_UNIT));
        assert_eq!(f.cgroup(4200), in_app(BOTTLES));
        assert_eq!(f.cgroup(4100), in_app(BOTTLES));
        let entries = f.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].windows, [608]);
        assert_eq!(entries[0].units, [GAME_UNIT]);
        assert!(entries[0].flagged);
        assert_eq!(
            entries[0].identity,
            GameIdentity::Process {
                name: "victoria3.exe".into()
            }
        );
        assert!(f.standby.is_suspended(608));
        assert!(!f.standby.is_suspended(700));
    }

    #[test]
    fn suspend_moves_a_steam_game_by_app_id() {
        let mut f = Fixture::new("steam", None, steam_world());
        let mut niri = f.niri();
        let outcome = f.standby.suspend(&mut niri, None);
        assert!(
            matches!(outcome, StandbyOutcome::Applied { window_id: 800, .. }),
            "{outcome:?}"
        );
        let unit = "app-melibea-game-3100.scope";
        assert_eq!(
            f.changes(),
            [
                format!("start_scope {unit} {{3100, 3200, 3400, 3401}}"),
                "set_suspended 800 true".to_owned(),
                format!("freeze {unit}"),
            ]
        );
        for pid in [3100, 3200, 3400, 3401] {
            assert_eq!(f.cgroup(pid), in_app(unit), "{pid}");
        }
        assert_eq!(f.cgroup(3000), in_app(STEAM_CLIENT));
        assert_eq!(f.cgroup(3300), in_app(STEAM_CONTAINER));
        assert_eq!(f.cgroup(3350), in_app(STEAM_CONTAINER));
        assert_eq!(
            f.entries()[0].identity,
            GameIdentity::SteamApp { app_id: 5 }
        );
    }

    #[test]
    fn refuses_a_window_that_is_not_a_game() {
        let mut f = Fixture::victoria("not-a-game");
        {
            let mut world = f.world.borrow_mut();
            world.windows[0].is_focused = false;
            world.windows[1].is_focused = true;
        }
        let mut niri = f.niri();
        let outcome = f.standby.suspend(&mut niri, None);
        assert_eq!(blocked_code(&outcome), "not_a_game");
        assert!(matches!(
            outcome,
            StandbyOutcome::Blocked {
                window_id: Some(700),
                ..
            }
        ));
        assert!(f.calls().is_empty(), "{:?}", f.calls());
        assert!(f.entries().is_empty());
    }

    #[test]
    fn refuses_a_game_set_with_a_protected_process() {
        let mut f = Fixture::victoria("protected");
        f.world
            .borrow_mut()
            .processes
            .get_mut(&4242)
            .unwrap()
            .cgroup = NIRI_SERVICE.into();
        let mut niri = f.niri();
        let outcome = f.standby.suspend(&mut niri, None);
        match &outcome {
            StandbyOutcome::Blocked {
                reason: BlockReason::ProtectedProcess { pid, comm },
                ..
            } => {
                assert_eq!(*pid, 4242);
                assert_eq!(comm, "Main Thread");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(blocked_code(&outcome), "protected_process");
        assert!(f.calls().is_empty(), "{:?}", f.calls());
        assert!(f.entries().is_empty());
    }

    #[test]
    fn refuses_processes_of_another_user_melibea_and_the_compositor() {
        for (name, change) in [
            ("uid", 0_u8),
            ("own-pid", 1),
            ("comm-niri", 2),
            ("comm-xwayland", 3),
        ] {
            let mut f = Fixture::victoria(&format!("protect-{name}"));
            {
                let mut world = f.world.borrow_mut();
                let victoria = world.processes.get_mut(&4242).unwrap();
                match change {
                    0 => victoria.uid = 0,
                    1 => victoria.pid = OWN_PID,
                    2 => victoria.comm = "niri".into(),
                    _ => victoria.comm = "Xwayland".into(),
                }
                if change == 1 {
                    let moved = world.processes.remove(&4242).unwrap();
                    world.processes.insert(OWN_PID, moved);
                }
            }
            let mut niri = f.niri();
            let outcome = f.standby.suspend(&mut niri, None);
            assert_eq!(blocked_code(&outcome), "protected_process", "{name}");
            assert!(f.calls().is_empty(), "{name}: {:?}", f.calls());
        }
    }

    #[test]
    fn refuses_a_game_that_is_not_running() {
        let mut f = Fixture::victoria("not-running");
        f.world.borrow_mut().processes.remove(&4242);
        let mut niri = f.niri();
        let outcome = f.standby.suspend(&mut niri, None);
        assert_eq!(blocked_code(&outcome), "game_not_running");
        assert!(f.changes().is_empty());
        assert!(f.entries().is_empty());
    }

    #[test]
    fn refuses_invalid_rules_and_reports_a_missing_window() {
        let mut f = Fixture::new("invalid-rules", Some("[[game]\n"), victoria_world());
        let mut niri = f.niri();
        assert_eq!(
            blocked_code(&f.standby.suspend(&mut niri, None)),
            "invalid_rules"
        );
        assert_eq!(
            f.standby.suspend(&mut niri, Some(1234)),
            StandbyOutcome::WindowNotFound {
                operation: StandbyOperation::Suspend,
                requested_id: Some(1234),
            }
        );
        assert!(f.calls().is_empty());
    }

    #[test]
    fn reuses_a_game_unit_without_moving() {
        let mut f = Fixture::victoria("reuse");
        {
            let mut world = f.world.borrow_mut();
            world.processes.get_mut(&4242).unwrap().cgroup = in_app(GAME_UNIT);
            world.states.insert(GAME_UNIT.into(), FreezerState::Running);
        }
        let mut niri = f.niri();
        let outcome = f.standby.suspend(&mut niri, None);
        assert!(
            matches!(outcome, StandbyOutcome::Applied { .. }),
            "{outcome:?}"
        );
        assert_eq!(
            f.changes(),
            [
                "set_suspended 608 true",
                "freeze app-melibea-game-4242.scope"
            ]
        );
    }

    #[test]
    fn scope_name_skips_a_loaded_unit_with_the_same_pid() {
        let mut f = Fixture::victoria("skip-name");
        {
            let mut world = f.world.borrow_mut();
            world.states.insert(GAME_UNIT.into(), FreezerState::Running);
            world.processes.insert(
                6000,
                process(6000, 1, "other", Some("other"), &in_app(GAME_UNIT)),
            );
        }
        let mut niri = f.niri();
        let outcome = f.standby.suspend(&mut niri, None);
        assert!(
            matches!(outcome, StandbyOutcome::Applied { .. }),
            "{outcome:?}"
        );
        assert_eq!(
            f.changes()[0],
            "start_scope app-melibea-game-4242-1.scope {4242}"
        );
        assert_eq!(f.entries()[0].units, ["app-melibea-game-4242-1.scope"]);
    }

    #[test]
    fn a_foreign_process_in_the_game_unit_refuses() {
        let mut f = Fixture::victoria("foreign-in-unit");
        {
            let mut world = f.world.borrow_mut();
            world.processes.get_mut(&4242).unwrap().cgroup = in_app(GAME_UNIT);
            world.states.insert(GAME_UNIT.into(), FreezerState::Running);
            world.processes.insert(
                6000,
                process(6000, 1, "intruder", Some("intruder"), &in_app(GAME_UNIT)),
            );
        }
        let mut niri = f.niri();
        let outcome = f.standby.suspend(&mut niri, None);
        assert!(matches!(
            outcome,
            StandbyOutcome::Blocked {
                reason: BlockReason::ProtectedProcess { pid: 6000, .. },
                ..
            }
        ));
        assert!(f.changes().is_empty());
        assert!(f.entries().is_empty());
    }

    #[test]
    fn escaped_processes_refuse_after_the_second_discovery() {
        let mut f = Fixture::victoria("escaped");
        f.world.borrow_mut().forks = vec![5000, 5001];
        let mut niri = f.niri();
        let outcome = f.standby.suspend(&mut niri, None);
        match &outcome {
            StandbyOutcome::Blocked {
                reason: BlockReason::GameEscaped { pids },
                ..
            } => assert_eq!(pids, &[5001]),
            other => panic!("{other:?}"),
        }
        assert_eq!(blocked_code(&outcome), "game_escaped");
        let changes = f.changes();
        assert_eq!(
            changes,
            [
                "start_scope app-melibea-game-4242.scope {4242}",
                "start_scope app-melibea-game-5000.scope {5000}",
            ]
        );
        assert!(f.entries().is_empty());
        assert!(f.suspended_windows().is_empty());
    }

    #[test]
    fn freeze_failure_rolls_back_flags_and_ledger() {
        let mut f = Fixture::victoria("freeze-fails");
        f.world.borrow_mut().fail_freeze = Some(FreezerState::Running);
        let mut niri = f.niri();
        let outcome = f.standby.suspend(&mut niri, None);
        assert_eq!(blocked_code(&outcome), "systemd");
        let changes = f.changes();
        let freeze = changes
            .iter()
            .position(|c| c == "freeze app-melibea-game-4242.scope")
            .unwrap();
        let unflag = changes
            .iter()
            .position(|c| c == "set_suspended 608 false")
            .unwrap();
        assert!(freeze < unflag, "{changes:?}");
        assert!(changes.contains(&"thaw app-melibea-game-4242.scope".to_owned()));
        assert!(f.entries().is_empty());
        assert!(f.suspended_windows().is_empty());
        assert_ne!(f.state(GAME_UNIT), Some(FreezerState::Frozen));
    }

    #[test]
    fn unwritable_ledger_refuses_before_any_change() {
        if fs::metadata("/proc/self").is_ok_and(|m| std::os::unix::fs::MetadataExt::uid(&m) == 0) {
            return; // root ignores directory permissions
        }
        let mut f = Fixture::victoria("ro-ledger");
        let readonly = f.dir.join("ro");
        fs::create_dir_all(&readonly).unwrap();
        fs::set_permissions(&readonly, fs::Permissions::from_mode(0o555)).unwrap();
        f.standby.ledger = Ledger::at(readonly.join("standby.json"));
        let mut niri = f.niri();
        let outcome = f.standby.suspend(&mut niri, None);
        fs::set_permissions(&readonly, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(blocked_code(&outcome), "ledger");
        assert!(f.calls().is_empty(), "{:?}", f.calls());
    }

    #[test]
    fn the_windows_of_one_game_change_together() {
        let mut world = steam_world();
        world.windows.push(window(
            801,
            "steam_app_5",
            "tModLoader launcher",
            3401,
            false,
        ));
        let mut f = Fixture::new("two-windows", None, world);
        let mut niri = f.niri();
        let outcome = f.standby.suspend(&mut niri, None);
        assert!(
            matches!(&outcome, StandbyOutcome::Applied { windows, .. } if windows == &[800, 801]),
            "{outcome:?}"
        );
        assert_eq!(f.suspended_windows(), [800, 801]);
        let outcome = f.standby.resume(&mut niri, Some(801));
        assert!(
            matches!(&outcome, StandbyOutcome::Applied { windows, .. } if windows == &[800, 801]),
            "{outcome:?}"
        );
        assert!(f.suspended_windows().is_empty());
        assert!(f.entries().is_empty());
    }

    #[test]
    fn resume_thaws_before_clearing_flags() {
        let mut f = Fixture::victoria("resume-order");
        let mut niri = f.niri();
        f.standby.suspend(&mut niri, None);
        f.clear_calls();
        let outcome = f.standby.resume(&mut niri, None);
        assert_eq!(
            outcome,
            StandbyOutcome::Applied {
                operation: StandbyOperation::Resume,
                window_id: 608,
                windows: vec![608],
            }
        );
        assert_eq!(
            f.calls(),
            [
                "thaw app-melibea-game-4242.scope",
                "freezer_state app-melibea-game-4242.scope",
                "set_suspended 608 false",
            ]
        );
        assert!(f.entries().is_empty());
        assert_eq!(f.state(GAME_UNIT), Some(FreezerState::Running));
    }

    #[test]
    fn resume_without_an_entry_is_already_in_the_requested_state() {
        let mut f = Fixture::victoria("resume-none");
        let mut niri = f.niri();
        assert_eq!(
            f.standby.resume(&mut niri, None),
            StandbyOutcome::AlreadyInRequestedState {
                operation: StandbyOperation::Resume,
                window_id: 608,
            }
        );
        assert_eq!(
            f.standby.resume(&mut niri, Some(1234)),
            StandbyOutcome::WindowNotFound {
                operation: StandbyOperation::Resume,
                requested_id: Some(1234),
            }
        );
        assert!(f.calls().is_empty());
    }

    #[test]
    fn suspending_twice_is_already_in_the_requested_state() {
        let mut f = Fixture::victoria("twice");
        let mut niri = f.niri();
        f.standby.suspend(&mut niri, None);
        f.clear_calls();
        assert_eq!(
            f.standby.suspend(&mut niri, None),
            StandbyOutcome::AlreadyInRequestedState {
                operation: StandbyOperation::Suspend,
                window_id: 608,
            }
        );
        assert!(f.calls().is_empty());
    }

    #[test]
    fn resume_ignores_the_rules_file() {
        let mut f = Fixture::victoria("resume-rules");
        let mut niri = f.niri();
        f.standby.suspend(&mut niri, None);
        fs::write(f.dir.join("games.toml"), "not = [valid").unwrap();
        let outcome = f.standby.resume(&mut niri, None);
        assert!(
            matches!(outcome, StandbyOutcome::Applied { .. }),
            "{outcome:?}"
        );
    }

    #[test]
    fn resume_of_a_vanished_game_clears_flags() {
        let mut f = Fixture::victoria("resume-vanished");
        let mut niri = f.niri();
        f.standby.suspend(&mut niri, None);
        f.world.borrow_mut().states.clear();
        let outcome = f.standby.resume(&mut niri, None);
        assert!(
            matches!(outcome, StandbyOutcome::Applied { .. }),
            "{outcome:?}"
        );
        assert!(f.suspended_windows().is_empty());
        assert!(f.entries().is_empty());
    }

    #[test]
    fn resume_keeps_the_entry_when_a_unit_stays_frozen() {
        let mut f = Fixture::victoria("resume-stuck");
        let mut niri = f.niri();
        f.standby.suspend(&mut niri, None);
        f.world.borrow_mut().fail_thaw = true;
        let outcome = f.standby.resume(&mut niri, None);
        assert_eq!(blocked_code(&outcome), "systemd");
        assert_eq!(f.entries().len(), 1);
        assert_eq!(f.suspended_windows(), [608]);
    }

    #[test]
    fn resume_keeps_the_entry_until_every_flag_is_cleared() {
        let mut world = steam_world();
        world.windows.push(window(
            801,
            "steam_app_5",
            "tModLoader launcher",
            3401,
            false,
        ));
        let mut f = Fixture::new("resume-flag-fails", None, world);
        let mut niri = f.niri();
        f.standby.suspend(&mut niri, None);
        f.world.borrow_mut().fail_flag.insert((801, false));
        let outcome = f.standby.resume(&mut niri, Some(801));
        assert_eq!(blocked_code(&outcome), "compositor");
        assert_eq!(
            f.state("app-melibea-game-3100.scope"),
            Some(FreezerState::Running)
        );
        assert_eq!(f.suspended_windows(), [801]);
        let entries = f.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].windows, [801]);
        assert!(f.standby.is_suspended(801));
        assert!(!f.standby.is_suspended(800));
        // niri answers again: a retried resume clears the flag and the entry.
        f.world.borrow_mut().fail_flag.clear();
        let outcome = f.standby.resume(&mut niri, Some(801));
        assert!(
            matches!(&outcome, StandbyOutcome::Applied { windows, .. } if windows == &[801]),
            "{outcome:?}"
        );
        assert!(f.suspended_windows().is_empty());
        assert!(f.entries().is_empty());
    }

    #[test]
    fn flag_failure_rolls_back_flags_and_ledger() {
        let mut world = steam_world();
        world.windows.push(window(
            801,
            "steam_app_5",
            "tModLoader launcher",
            3401,
            false,
        ));
        let mut f = Fixture::new("flag-fails", None, world);
        f.world.borrow_mut().fail_flag.insert((801, true));
        let mut niri = f.niri();
        let outcome = f.standby.suspend(&mut niri, None);
        assert_eq!(blocked_code(&outcome), "compositor");
        let changes = f.changes();
        assert!(
            changes.contains(&"set_suspended 800 false".to_owned()),
            "{changes:?}"
        );
        assert!(
            !changes.iter().any(|c| c.starts_with("freeze")),
            "{changes:?}"
        );
        assert!(f.suspended_windows().is_empty());
        assert!(f.entries().is_empty());
    }

    #[test]
    fn a_failed_unflag_in_a_rollback_is_reported() {
        let mut f = Fixture::victoria("unflag-fails");
        {
            let mut world = f.world.borrow_mut();
            world.fail_freeze = Some(FreezerState::Running);
            world.fail_flag.insert((608, false));
        }
        let mut niri = f.niri();
        let outcome = f.standby.suspend(&mut niri, None);
        match &outcome {
            StandbyOutcome::Blocked {
                reason: BlockReason::Systemd(text),
                ..
            } => assert!(text.contains("flags not cleared: window 608"), "{text}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(f.suspended_windows(), [608]);
        assert!(f.entries().is_empty());
    }

    #[test]
    fn a_failed_rollback_thaw_keeps_the_entry_and_the_flags() {
        let mut f = Fixture::victoria("rollback-stuck");
        {
            let mut world = f.world.borrow_mut();
            world.fail_freeze = Some(FreezerState::Frozen);
            world.fail_thaw = true;
        }
        let mut niri = f.niri();
        let outcome = f.standby.suspend(&mut niri, None);
        match &outcome {
            StandbyOutcome::Blocked {
                reason: BlockReason::Systemd(text),
                ..
            } => assert!(text.contains("still frozen"), "{text}"),
            other => panic!("{other:?}"),
        }
        assert!(!f.changes().contains(&"set_suspended 608 false".to_owned()));
        assert_eq!(f.suspended_windows(), [608]);
        assert_eq!(f.entries().len(), 1);
        assert_eq!(f.entries()[0].units, [GAME_UNIT]);
        // Once systemd answers, an explicit resume releases it.
        f.world.borrow_mut().fail_thaw = false;
        let outcome = f.standby.resume(&mut niri, None);
        assert!(
            matches!(outcome, StandbyOutcome::Applied { .. }),
            "{outcome:?}"
        );
        assert_eq!(f.state(GAME_UNIT), Some(FreezerState::Running));
        assert!(f.suspended_windows().is_empty());
    }

    #[test]
    fn an_intruder_after_the_second_move_refuses() {
        let mut f = Fixture::victoria("intruder-second-move");
        {
            let mut world = f.world.borrow_mut();
            world.forks = vec![5000];
            world.intruder_on = Some("app-melibea-game-5000.scope".into());
        }
        let mut niri = f.niri();
        let outcome = f.standby.suspend(&mut niri, None);
        assert!(
            matches!(
                outcome,
                StandbyOutcome::Blocked {
                    reason: BlockReason::ProtectedProcess { pid: 6000, .. },
                    ..
                }
            ),
            "{outcome:?}"
        );
        assert_eq!(
            f.changes(),
            [
                "start_scope app-melibea-game-4242.scope {4242}",
                "start_scope app-melibea-game-5000.scope {5000}",
            ]
        );
        assert!(f.entries().is_empty());
        assert!(f.suspended_windows().is_empty());
    }

    #[test]
    fn toggle_suspends_then_resumes() {
        let mut f = Fixture::victoria("toggle");
        let mut niri = f.niri();
        assert!(matches!(
            f.standby.toggle(&mut niri, None),
            StandbyOutcome::Applied {
                operation: StandbyOperation::Suspend,
                ..
            }
        ));
        assert_eq!(f.state(GAME_UNIT), Some(FreezerState::Frozen));
        assert!(matches!(
            f.standby.toggle(&mut niri, None),
            StandbyOutcome::Applied {
                operation: StandbyOperation::Resume,
                ..
            }
        ));
        assert_eq!(f.state(GAME_UNIT), Some(FreezerState::Running));
        assert!(f.entries().is_empty());
        assert!(f.suspended_windows().is_empty());
    }

    #[test]
    fn reconcile_thaws_an_entry_without_a_suspended_window() {
        let mut f = Fixture::victoria("reconcile-thaw");
        let mut niri = f.niri();
        f.standby.suspend(&mut niri, None);
        for window in &mut f.world.borrow_mut().windows {
            window.is_suspended = false;
        }
        f.clear_calls();
        let lines = f.standby.reconcile(&mut niri);
        assert!(!lines.is_empty());
        assert!(
            f.changes()
                .contains(&"thaw app-melibea-game-4242.scope".to_owned())
        );
        assert_eq!(f.state(GAME_UNIT), Some(FreezerState::Running));
        assert!(f.entries().is_empty());
    }

    #[test]
    fn reconcile_keeps_a_live_suspension() {
        let mut f = Fixture::victoria("reconcile-keep");
        let mut niri = f.niri();
        f.standby.suspend(&mut niri, None);
        f.clear_calls();
        let lines = f.standby.reconcile(&mut niri);
        assert!(lines.is_empty(), "{lines:?}");
        assert!(f.changes().is_empty());
        assert_eq!(f.entries().len(), 1);
        assert_eq!(f.state(GAME_UNIT), Some(FreezerState::Frozen));
    }

    #[test]
    fn reconcile_clears_a_flag_without_an_entry() {
        let mut f = Fixture::victoria("reconcile-flag");
        f.world.borrow_mut().windows[0].is_suspended = true;
        let mut niri = f.niri();
        let lines = f.standby.reconcile(&mut niri);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert_eq!(f.changes(), ["set_suspended 608 false"]);
        assert!(f.suspended_windows().is_empty());
    }

    #[test]
    fn reconcile_drops_entry_whose_units_are_gone() {
        let mut f = Fixture::victoria("reconcile-gone");
        f.standby
            .ledger
            .write(&[victoria_entry(&[GAME_UNIT])])
            .unwrap();
        f.world.borrow_mut().windows[0].is_suspended = true;
        let mut niri = f.niri();
        let lines = f.standby.reconcile(&mut niri);
        assert!(!lines.is_empty());
        assert_eq!(f.changes(), ["set_suspended 608 false"]);
        assert!(f.entries().is_empty());
        assert!(f.suspended_windows().is_empty());
    }

    #[test]
    fn reconcile_thaws_a_frozen_unit_no_entry_names() {
        let mut f = Fixture::victoria("reconcile-orphan");
        f.world
            .borrow_mut()
            .states
            .insert(GAME_UNIT.into(), FreezerState::Frozen);
        let mut niri = f.niri();
        let lines = f.standby.reconcile(&mut niri);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert_eq!(f.changes(), ["thaw app-melibea-game-4242.scope"]);
        assert_eq!(f.state(GAME_UNIT), Some(FreezerState::Running));
    }

    #[test]
    fn reconcile_with_a_corrupt_ledger_thaws_everything() {
        let mut f = Fixture::victoria("reconcile-corrupt");
        fs::create_dir_all(f.dir.join("run")).unwrap();
        fs::write(f.dir.join("run").join("standby.json"), "{oops").unwrap();
        {
            let mut world = f.world.borrow_mut();
            world.states.insert(GAME_UNIT.into(), FreezerState::Frozen);
            world.windows[0].is_suspended = true;
        }
        let mut niri = f.niri();
        f.standby.reconcile(&mut niri);
        assert_eq!(f.state(GAME_UNIT), Some(FreezerState::Running));
        assert!(f.suspended_windows().is_empty());
        assert!(f.entries().is_empty());
    }

    #[test]
    fn thaw_all_releases_listed_game_units_with_a_corrupt_ledger() {
        let mut f = Fixture::victoria("thaw-all");
        fs::create_dir_all(f.dir.join("run")).unwrap();
        fs::write(f.dir.join("run").join("standby.json"), "{oops").unwrap();
        {
            let mut world = f.world.borrow_mut();
            world.states.insert(GAME_UNIT.into(), FreezerState::Frozen);
            world.windows[0].is_suspended = true;
        }
        let mut niri = f.niri();
        let lines = f.standby.thaw_all(Some(&mut niri));
        assert!(lines.len() >= 4, "{lines:?}");
        assert!(
            f.changes()
                .contains(&"thaw app-melibea-game-4242.scope".to_owned())
        );
        assert_eq!(f.state(GAME_UNIT), Some(FreezerState::Running));
        assert!(f.suspended_windows().is_empty());
        assert!(f.entries().is_empty());
    }

    #[test]
    fn thaw_all_without_niri_still_thaws_ledger_units() {
        let mut f = Fixture::victoria("thaw-all-no-niri");
        f.standby
            .ledger
            .write(&[victoria_entry(&[GAME_UNIT])])
            .unwrap();
        f.world
            .borrow_mut()
            .states
            .insert(GAME_UNIT.into(), FreezerState::Frozen);
        f.standby.thaw_all::<FakeCompositor>(None);
        assert_eq!(f.state(GAME_UNIT), Some(FreezerState::Running));
        assert!(f.entries().is_empty());
    }

    #[test]
    fn block_reasons_have_stable_codes() {
        let reasons = [
            (BlockReason::NotAGame, "not_a_game"),
            (BlockReason::GameNotRunning, "game_not_running"),
            (
                BlockReason::ProtectedProcess {
                    pid: 1,
                    comm: "niri".into(),
                },
                "protected_process",
            ),
            (BlockReason::GameEscaped { pids: vec![2] }, "game_escaped"),
            (BlockReason::InvalidRules("x".into()), "invalid_rules"),
            (BlockReason::Systemd("x".into()), "systemd"),
            (BlockReason::Ledger("x".into()), "ledger"),
            (BlockReason::Compositor("x".into()), "compositor"),
        ];
        for (reason, code) in reasons {
            assert_eq!(reason.code(), code);
            assert!(!reason.describe().is_empty());
        }
    }

    #[test]
    fn blocked_outcome_carries_the_reason() {
        use crate::protocol::{
            ActionStatus, Message, Operation, PROTOCOL_VERSION_V3, ServerMessage,
        };

        let blocked = outcome_result(&StandbyOutcome::Blocked {
            operation: StandbyOperation::Suspend,
            window_id: Some(7),
            reason: BlockReason::NotAGame,
        });
        assert_eq!(blocked.operation, Operation::Suspend);
        assert_eq!(blocked.status, ActionStatus::Blocked);
        let wire = serde_json::to_string(&ServerMessage::for_version(
            PROTOCOL_VERSION_V3,
            Message::ActionResult(blocked),
        ))
        .expect("serialize");
        assert!(
            wire.contains(
                r#""status":"blocked","reason":"not_a_game: the window is not a recognized game""#
            ),
            "{wire}"
        );

        let missing = outcome_result(&StandbyOutcome::WindowNotFound {
            operation: StandbyOperation::Resume,
            requested_id: Some(9),
        });
        assert_eq!(missing.operation, Operation::Resume);
        assert_eq!(missing.status, ActionStatus::WindowNotFound);
        assert_eq!(missing.requested_id, Some(9));
        assert_eq!(missing.window_id, None);
        assert_eq!(missing.reason, None);

        let applied = outcome_result(&StandbyOutcome::Applied {
            operation: StandbyOperation::Suspend,
            window_id: 7,
            windows: vec![7, 8],
        });
        assert_eq!(applied.status, ActionStatus::Applied);
        assert_eq!(applied.window_id, Some(7));
        let already = outcome_result(&StandbyOutcome::AlreadyInRequestedState {
            operation: StandbyOperation::Resume,
            window_id: 7,
        });
        assert_eq!(already.status, ActionStatus::AlreadyInRequestedState);
    }
}
