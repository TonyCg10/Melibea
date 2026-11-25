use std::{
    collections::{BTreeMap, BTreeSet},
    env, fmt, fs,
    io::{self, Write},
    path::PathBuf,
    process::ExitCode,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        mpsc::{self, Receiver, RecvTimeoutError, TryRecvError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use melibea::{
    config::{Config, default_config_path},
    executor::PendingResizes,
    health::HealthReport,
    minimization::{MinimizationEvent, MinimizedRegistry, MinimizedWindow, RegistryChange},
    niri::{NiriAdapter, NiriEvent, NiriWindow},
    niri_ipc::{
        NIRI_SOCKET_ENV, NiriActionClient, NiriEventStream, NiriEventStreamCancellation,
        NiriWindowActionResult, NiriWindowActionStatus,
    },
    protocol::{
        ActionResult, ActionStatus, Message as ProtocolMessage, Operation, Request, ServerMessage,
        SuspendedWindow,
    },
    service::{ActionExecutor, Service, ServiceClient, socket_path},
    standby::{
        controller::{LiveCompositor, ProcFs, Standby, StandbyOutcome, outcome_result},
        detect::LiveX11,
        games::{GameRules, default_games_path},
        ledger::{Ledger, LedgerEntry},
        systemd::SystemdTools,
        timers::StandbyTimers,
    },
    transition::{DecisionOutcome, ExclusionReason, FocusState, FocusTransition, WindowId},
};

macro_rules! outputln {
    () => {
        write_line(io::stdout().lock(), format_args!(""))
    };
    ($($argument:tt)*) => {
        write_line(io::stdout().lock(), format_args!($($argument)*))
    };
}

macro_rules! errorln {
    () => {
        write_line(io::stderr().lock(), format_args!(""))
    };
    ($($argument:tt)*) => {
        write_line(io::stderr().lock(), format_args!($($argument)*))
    };
}

fn write_line(mut writer: impl Write, arguments: fmt::Arguments<'_>) {
    let _ = writer.write_fmt(arguments);
    let _ = writer.write_all(b"\n");
}

fn main() -> ExitCode {
    match parse_args(std::env::args().skip(1)) {
        Ok(Command::Status { path }) => status(path),
        Ok(Command::CheckConfig { path }) => check_config(path),
        Ok(Command::Observe { path }) => observe(path),
        Ok(Command::Run { path }) => run_controller(path),
        Ok(Command::List) => service_list(),
        Ok(Command::Minimize { id }) => service_action(Request::minimize(id.map(|id| id.0))),
        Ok(Command::Restore { id }) => service_action(Request::restore(id.0)),
        Ok(Command::Close { id }) => service_action(Request::Close { window_id: id.0 }),
        Ok(Command::Suspend { id }) => service_standby(Request::Suspend {
            window_id: id.map(|id| id.0),
        }),
        Ok(Command::Resume { id }) => service_standby(Request::Resume {
            window_id: id.map(|id| id.0),
        }),
        Ok(Command::ToggleSuspend { id }) => service_standby(Request::ToggleSuspend {
            window_id: id.map(|id| id.0),
        }),
        Ok(Command::ThawAll) => thaw_all(),
        Ok(Command::Subscribe) => service_subscribe(),
        Ok(Command::Help) => {
            print_help();
            ExitCode::SUCCESS
        }
        Err(message) => {
            errorln!("{message}");
            errorln!();
            print_help();
            ExitCode::from(2)
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Status { path: Option<PathBuf> },
    CheckConfig { path: Option<PathBuf> },
    Observe { path: Option<PathBuf> },
    Run { path: Option<PathBuf> },
    List,
    Minimize { id: Option<WindowId> },
    Restore { id: WindowId },
    Close { id: WindowId },
    Suspend { id: Option<WindowId> },
    Resume { id: Option<WindowId> },
    ToggleSuspend { id: Option<WindowId> },
    ThawAll,
    Subscribe,
    Help,
}

fn parse_args(arguments: impl IntoIterator<Item = String>) -> Result<Command, String> {
    let mut positionals = Vec::new();
    let mut config_path = None;
    let mut arguments = arguments.into_iter();

    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--config" => {
                let path = arguments
                    .next()
                    .ok_or_else(|| "`--config` requires a path".to_owned())?;
                if config_path.replace(PathBuf::from(path)).is_some() {
                    return Err("`--config` may be supplied only once".to_owned());
                }
            }
            "--help" | "-h" => return Ok(Command::Help),
            value => positionals.push(value.to_owned()),
        }
    }

    let Some((command, operands)) = positionals.split_first() else {
        return if config_path.is_some() {
            Err("`--config` requires a command".to_owned())
        } else {
            Ok(Command::Help)
        };
    };

    match command.as_str() {
        "status" => {
            require_no_operands(command, operands)?;
            Ok(Command::Status { path: config_path })
        }
        "check-config" => {
            require_no_operands(command, operands)?;
            Ok(Command::CheckConfig { path: config_path })
        }
        "observe" => {
            require_no_operands(command, operands)?;
            Ok(Command::Observe { path: config_path })
        }
        "run" => {
            require_no_operands(command, operands)?;
            Ok(Command::Run { path: config_path })
        }
        "list" | "minimized" => {
            reject_config(command, config_path.as_ref())?;
            require_no_operands(command, operands)?;
            Ok(Command::List)
        }
        "minimize" => {
            reject_config(command, config_path.as_ref())?;
            Ok(Command::Minimize {
                id: optional_window_id(command, operands)?,
            })
        }
        "suspend" => {
            reject_config(command, config_path.as_ref())?;
            Ok(Command::Suspend {
                id: optional_window_id(command, operands)?,
            })
        }
        "resume" => {
            reject_config(command, config_path.as_ref())?;
            Ok(Command::Resume {
                id: optional_window_id(command, operands)?,
            })
        }
        "toggle-suspend" => {
            reject_config(command, config_path.as_ref())?;
            Ok(Command::ToggleSuspend {
                id: optional_window_id(command, operands)?,
            })
        }
        "thaw-all" => {
            reject_config(command, config_path.as_ref())?;
            require_no_operands(command, operands)?;
            Ok(Command::ThawAll)
        }
        "restore" => {
            reject_config(command, config_path.as_ref())?;
            Ok(Command::Restore {
                id: required_window_id(command, operands)?,
            })
        }
        "close" | "close-window" => {
            reject_config(command, config_path.as_ref())?;
            Ok(Command::Close {
                id: required_window_id(command, operands)?,
            })
        }
        "subscribe" => {
            reject_config(command, config_path.as_ref())?;
            require_no_operands(command, operands)?;
            Ok(Command::Subscribe)
        }
        "help" => {
            reject_config(command, config_path.as_ref())?;
            require_no_operands(command, operands)?;
            Ok(Command::Help)
        }
        value => Err(format!("unknown command: {value}")),
    }
}

fn reject_config(command: &str, path: Option<&PathBuf>) -> Result<(), String> {
    if path.is_some() {
        Err(format!("`{command}` does not accept `--config`"))
    } else {
        Ok(())
    }
}

fn require_no_operands(command: &str, operands: &[String]) -> Result<(), String> {
    if let Some(value) = operands.first() {
        Err(format!("unexpected argument for `{command}`: {value}"))
    } else {
        Ok(())
    }
}

fn optional_window_id(command: &str, operands: &[String]) -> Result<Option<WindowId>, String> {
    match operands {
        [] => Ok(None),
        [id] => parse_window_id(command, id).map(Some),
        _ => Err(format!("`{command}` accepts at most one window id")),
    }
}

fn required_window_id(command: &str, operands: &[String]) -> Result<WindowId, String> {
    match operands {
        [id] => parse_window_id(command, id),
        [] => Err(format!("`{command}` requires a window id")),
        _ => Err(format!("`{command}` requires exactly one window id")),
    }
}

fn parse_window_id(command: &str, value: &str) -> Result<WindowId, String> {
    value
        .parse::<u64>()
        .map(WindowId)
        .map_err(|_| format!("invalid window id for `{command}`: {value}"))
}

type StreamMessage = Result<Option<NiriEvent>, String>;
const RECONNECT_DELAY: Duration = Duration::from_secs(1);
const CONFIG_POLL_INTERVAL: Duration = Duration::from_secs(1);
const HEALTH_READ_TIMEOUT: Duration = Duration::from_secs(2);
const INITIAL_SNAPSHOT_EVENT_LIMIT: usize = 64;

struct LoadedConfig {
    config: Config,
    path: PathBuf,
    source: String,
}

struct ConfigWatcher {
    path: PathBuf,
    last_seen_source: String,
    last_read_error: Option<String>,
    last_poll: Instant,
}

enum ConfigPoll {
    Unchanged,
    Reloaded(Config),
    Rejected(String),
}

impl ConfigWatcher {
    fn new(path: PathBuf, source: String) -> Self {
        Self {
            path,
            last_seen_source: source,
            last_read_error: None,
            last_poll: Instant::now(),
        }
    }

    fn poll_if_due(&mut self) -> ConfigPoll {
        if self.last_poll.elapsed() < CONFIG_POLL_INTERVAL {
            return ConfigPoll::Unchanged;
        }
        self.last_poll = Instant::now();
        self.poll_now()
    }

    fn poll_now(&mut self) -> ConfigPoll {
        let source = match fs::read_to_string(&self.path) {
            Ok(source) => {
                self.last_read_error = None;
                source
            }
            Err(error) => {
                let message = format!(
                    "cannot read configuration update from {}: {error}",
                    self.path.display()
                );
                if self.last_read_error.as_deref() == Some(&message) {
                    return ConfigPoll::Unchanged;
                }
                self.last_read_error = Some(message.clone());
                return ConfigPoll::Rejected(message);
            }
        };

        if source == self.last_seen_source {
            return ConfigPoll::Unchanged;
        }
        self.last_seen_source.clone_from(&source);

        match Config::parse(&source) {
            Ok(config) => ConfigPoll::Reloaded(config),
            Err(error) => ConfigPoll::Rejected(format!(
                "configuration update from {} rejected: {error}",
                self.path.display()
            )),
        }
    }
}

struct EventReader {
    receiver: Receiver<StreamMessage>,
    cancellation: NiriEventStreamCancellation,
    handle: Option<JoinHandle<()>>,
}

impl Drop for EventReader {
    fn drop(&mut self) {
        let _ = self.cancellation.cancel();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn configured(path: Option<PathBuf>) -> Result<LoadedConfig, String> {
    let path = path
        .map_or_else(default_config_path, Ok)
        .map_err(|error| error.to_string())?;
    let (config, source) = Config::load_with_source(&path).map_err(|error| error.to_string())?;
    Ok(LoadedConfig {
        config,
        path,
        source,
    })
}

fn check_config(path: Option<PathBuf>) -> ExitCode {
    match configured(path) {
        Ok(LoadedConfig { config, path, .. }) => {
            outputln!(
                "configuration valid: {} attention rule(s) from {}",
                config.attention_rules().len(),
                path.display()
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            errorln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn status(path: Option<PathBuf>) -> ExitCode {
    match status_inner(path) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            errorln!("melibea health: error");
            errorln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn status_inner(path: Option<PathBuf>) -> Result<(), String> {
    let LoadedConfig { config, path, .. } = configured(path)?;
    let mut stream = NiriEventStream::connect().map_err(|error| error.to_string())?;
    stream
        .set_read_timeout(Some(HEALTH_READ_TIMEOUT))
        .map_err(|error| format!("cannot set niri health timeout: {error}"))?;

    let windows = (0..INITIAL_SNAPSHOT_EVENT_LIMIT)
        .find_map(|_| match stream.read_event() {
            Ok(Some(NiriEvent::WindowsChanged { windows })) => Some(Ok(windows)),
            Ok(Some(_)) => None,
            Ok(None) => Some(Err("niri closed IPC before its window snapshot".to_owned())),
            Err(error) => Some(Err(format!(
                "cannot read niri window snapshot within {} second(s): {error}",
                HEALTH_READ_TIMEOUT.as_secs()
            ))),
        })
        .transpose()?
        .ok_or_else(|| {
            format!(
                "niri sent no window snapshot within {INITIAL_SNAPSHOT_EVENT_LIMIT} initial events"
            )
        })?;
    let report =
        HealthReport::from_windows(&config, &windows).map_err(|error| error.to_string())?;

    outputln!("melibea health: ok");
    outputln!(
        "config={} rules={}",
        path.display(),
        config.attention_rules().len()
    );
    outputln!(
        "windows total={} matched-tiled={} unmatched-tiled={} floating={} matched-floating={}",
        report.total_windows,
        report.matched_tiled_windows,
        report.unmatched_tiled_windows,
        report.floating_windows,
        report.matched_floating_windows
    );
    match report.focused {
        Some(focused) => outputln!(
            "focus window={} app_id={:?} title={:?} policy={}",
            focused.id.0,
            focused.app_id,
            focused.title,
            if focused.is_floating {
                "excluded-floating".to_owned()
            } else {
                focused
                    .matched_rule
                    .map_or_else(|| "none".to_owned(), |index| format!("rule-{index}"))
            }
        ),
        None => outputln!("focus none"),
    }
    for (index, matches) in report.matches_per_rule.iter().enumerate() {
        outputln!("rule={index} tiled-matches={matches}");
    }
    for line in suspended_status_lines() {
        outputln!("{line}");
    }

    Ok(())
}

/// The `suspended:` section of `status`, read straight from the ledger. A
/// missing ledger and an empty one print nothing, so the output is unchanged
/// when nothing is suspended.
fn suspended_status_lines() -> Vec<String> {
    let Ok(path) = Ledger::default_path() else {
        return Vec::new();
    };
    match Ledger::at(path).read() {
        Ok(entries) => suspended_lines(&entries),
        Err(error) => vec![format!("suspended: cannot read the ledger: {error}")],
    }
}

fn suspended_lines(entries: &[LedgerEntry]) -> Vec<String> {
    if entries.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![format!("suspended: {} game(s)", entries.len())];
    lines.extend(entries.iter().map(|entry| {
        format!(
            "suspended game={:?} windows={:?} units={} since={}",
            entry.identity,
            entry.windows,
            entry.units.join(","),
            entry.since_unix
        )
    }));
    lines
}

fn service_list() -> ExitCode {
    let response = service_request(Request::List);
    match response {
        Ok(ServerMessage {
            version,
            message: ProtocolMessage::Snapshot { revision, windows },
        }) => {
            outputln!(
                "protocol={} minimized revision={} count={}",
                version,
                revision,
                windows.len()
            );
            for (index, window) in windows.iter().enumerate() {
                outputln!(
                    "bubble={} window={} app_id={:?} title={:?} icon_name={:?}",
                    index,
                    window.id,
                    window.app_id,
                    window.title,
                    window.icon_name
                );
            }
            ExitCode::SUCCESS
        }
        Ok(message) => protocol_failure(message),
        Err(error) => {
            errorln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn service_action(request: Request) -> ExitCode {
    match service_request(request) {
        Ok(ServerMessage {
            version,
            message: ProtocolMessage::ActionResult(result),
        }) => {
            outputln!(
                "protocol={} operation={} status={} requested={} window={}",
                version,
                operation_name(result.operation),
                action_status_name(result.status),
                result
                    .requested_id
                    .map_or_else(|| "focused".to_owned(), |id| id.to_string()),
                result
                    .window_id
                    .map_or_else(|| "none".to_owned(), |id| id.to_string())
            );
            if matches!(
                result.status,
                ActionStatus::WindowNotFound | ActionStatus::Blocked
            ) {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
        }
        Ok(message) => protocol_failure(message),
        Err(error) => {
            errorln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn service_standby(request: Request) -> ExitCode {
    let response = ServiceClient::from_environment()
        .map_err(|error| error.to_string())
        .and_then(|client| {
            client
                .request_v3(request)
                .map_err(|error| error.to_string())
        });
    match response {
        Ok(ServerMessage {
            message: ProtocolMessage::ActionResult(result),
            ..
        }) => match describe_standby(&result) {
            Ok(line) => {
                outputln!("{line}");
                ExitCode::SUCCESS
            }
            Err(reason) => {
                errorln!("{reason}");
                ExitCode::FAILURE
            }
        },
        Ok(message) => protocol_failure(message),
        Err(error) => {
            errorln!("{error}");
            ExitCode::FAILURE
        }
    }
}

/// The terminal line for a standby result, or the failure text for stderr.
///
/// Note: the protocol result names one window, so the "and N more" of a game
/// with several windows is not shown.
fn describe_standby(result: &ActionResult) -> Result<String, String> {
    let window = result
        .window_id
        .or(result.requested_id)
        .map_or_else(|| "focused".to_owned(), |id| id.to_string());
    match (result.status, result.operation) {
        (ActionStatus::Applied, Operation::Suspend) => Ok(format!("suspended window {window}")),
        (ActionStatus::Applied, Operation::Resume) => Ok(format!("resumed window {window}")),
        (ActionStatus::AlreadyInRequestedState, Operation::Suspend) => {
            Ok(format!("window {window} is already suspended"))
        }
        (ActionStatus::AlreadyInRequestedState, Operation::Resume) => {
            Ok(format!("window {window} is not suspended"))
        }
        (ActionStatus::WindowNotFound, _) => Err(format!("window {window} not found")),
        (ActionStatus::Blocked, _) => Err(result
            .reason
            .clone()
            .unwrap_or_else(|| "blocked".to_owned())),
        (status, operation) => Ok(format!(
            "operation={} status={}",
            operation_name(operation),
            action_status_name(status)
        )),
    }
}

/// Thaws every game locally; the daemon is not needed.
///
/// It never reads `games.toml` (the rules path is a placeholder), and an
/// unresolved ledger path or an unreadable `/proc/self` still thaws every
/// loaded game unit and clears niri's flags; the exit status reports them
/// only after everything that could be done is done.
fn thaw_all() -> ExitCode {
    let mut failed = false;
    let ledger = Ledger::default_path().map_or_else(
        |error| {
            errorln!("{error}");
            failed = true;
            Ledger::unresolved()
        },
        Ledger::at,
    );
    // The uid only guards suspension; thawing never reads it.
    let uid = fs::metadata("/proc/self").map_or_else(
        |error| {
            errorln!("cannot read /proc/self: {error}");
            failed = true;
            u32::MAX
        },
        |metadata| std::os::unix::fs::MetadataExt::uid(&metadata),
    );
    let mut standby = Standby::new(
        SystemdTools::new(),
        ProcFs,
        ledger,
        PathBuf::from("/nonexistent/melibea-thaw-all-reads-no-rules"),
        std::process::id(),
        uid,
        Box::new(LiveX11),
    );
    let lines = match NiriActionClient::connect() {
        Ok(client) => standby.thaw_all(Some(&mut LiveCompositor::new(client))),
        Err(_) => standby.thaw_all(None::<&mut LiveCompositor>),
    };
    for line in lines {
        outputln!("{line}");
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// The standby controller over systemd and `/proc`, with the games path it reads.
fn live_standby() -> Result<(LiveStandby, PathBuf), String> {
    let games_path = default_games_path().map_err(|error| error.to_string())?;
    let ledger = Ledger::at(Ledger::default_path().map_err(|error| error.to_string())?);
    // The owner of /proc/self: the same source the process table reads uids from.
    let uid = fs::metadata("/proc/self")
        .map(|metadata| std::os::unix::fs::MetadataExt::uid(&metadata))
        .map_err(|error| format!("cannot read /proc/self: {error}"))?;
    let standby = Standby::new(
        SystemdTools::new(),
        ProcFs,
        ledger,
        games_path.clone(),
        std::process::id(),
        uid,
        Box::new(LiveX11),
    );
    Ok((standby, games_path))
}

type LiveStandby = Standby<SystemdTools, ProcFs>;

/// The standby controller shared by the service executor and the controller loop.
///
/// Lock discipline (prevents a deadlock): the executor runs on the service's
/// broker thread, and every `Service::publish_*` waits for that thread. So the
/// executor never calls a `Service` method, and the controller loop never holds
/// this mutex while it publishes: it takes the lock, acts, drops it, then publishes.
type SharedStandby = Arc<Mutex<LiveStandby>>;

fn lock_standby(standby: &SharedStandby) -> MutexGuard<'_, LiveStandby> {
    // The controller keeps its state in files, so a panic elsewhere leaves nothing half-done here.
    standby.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Maps a standby outcome for a client: `requested_id` is what the request
/// named (none for the focused window), `window_id` the window acted on.
fn standby_action_result(outcome: &StandbyOutcome, requested_id: Option<u64>) -> ActionResult {
    ActionResult {
        requested_id,
        ..outcome_result(outcome)
    }
}

/// A log line for a standby outcome the daemon started itself.
fn standby_log_line(outcome: &StandbyOutcome) -> String {
    describe_standby(&outcome_result(outcome)).unwrap_or_else(|reason| reason)
}

fn service_subscribe() -> ExitCode {
    let client = match ServiceClient::from_environment() {
        Ok(client) => client,
        Err(error) => {
            errorln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    let mut subscription = match client.subscribe() {
        Ok(subscription) => subscription,
        Err(error) => {
            errorln!("{error}");
            return ExitCode::FAILURE;
        }
    };

    loop {
        match subscription.read() {
            Ok(message) => match serde_json::to_string(&message) {
                Ok(line) => outputln!("{line}"),
                Err(error) => {
                    errorln!("cannot encode Melibea subscription message: {error}");
                    return ExitCode::FAILURE;
                }
            },
            Err(error) => {
                errorln!("{error}");
                return ExitCode::FAILURE;
            }
        }
    }
}

fn service_request(request: Request) -> Result<ServerMessage, String> {
    ServiceClient::from_environment()
        .map_err(|error| error.to_string())?
        .request(request)
        .map_err(|error| error.to_string())
}

fn protocol_failure(message: ServerMessage) -> ExitCode {
    match message.message {
        ProtocolMessage::Error(error) => {
            errorln!("{}: {}", error_code_name(error.code), error.message);
        }
        unexpected => errorln!("unexpected Melibea service response: {unexpected:?}"),
    }
    ExitCode::FAILURE
}

const fn operation_name(operation: Operation) -> &'static str {
    match operation {
        Operation::Minimize => "minimize",
        Operation::Restore => "restore",
        Operation::Close => "close",
        Operation::Suspend => "suspend",
        Operation::Resume => "resume",
    }
}

const fn action_status_name(status: ActionStatus) -> &'static str {
    match status {
        ActionStatus::Applied => "applied",
        ActionStatus::AlreadyInRequestedState => "already-in-requested-state",
        ActionStatus::CloseRequested => "close-requested",
        ActionStatus::WindowNotFound => "window-not-found",
        ActionStatus::Blocked => "blocked",
        ActionStatus::LegacyHandled => "legacy-handled",
    }
}

const fn error_code_name(code: melibea::protocol::ErrorCode) -> &'static str {
    use melibea::protocol::ErrorCode;
    match code {
        ErrorCode::IncompatibleVersion => "incompatible-version",
        ErrorCode::InvalidRequest => "invalid-request",
        ErrorCode::Unavailable => "unavailable",
        ErrorCode::ActionFailed => "action-failed",
    }
}

/// The daemon's protocol action executor. It runs on the service's broker
/// thread, so it must never call a `Service` method (see [`SharedStandby`]).
struct DaemonExecutor {
    standby: SharedStandby,
}

impl ActionExecutor for DaemonExecutor {
    fn execute(&self, request: &Request) -> Result<ActionResult, String> {
        match request {
            Request::Suspend { window_id } => self.standby_action(*window_id, Standby::suspend),
            Request::Resume { window_id } => self.standby_action(*window_id, Standby::resume),
            Request::ToggleSuspend { window_id } => {
                self.standby_action(*window_id, Standby::toggle)
            }
            Request::Restore { window_id, .. } | Request::Close { window_id } => {
                match self.resume_before_forwarding(*window_id)? {
                    Some(refused) => Ok(refused),
                    None => execute_native_action(request),
                }
            }
            Request::Minimize { .. }
            | Request::List
            | Request::Subscribe
            | Request::ListSuspended => execute_native_action(request),
        }
    }
}

impl DaemonExecutor {
    fn standby_action(
        &self,
        window_id: Option<u64>,
        action: impl FnOnce(&mut LiveStandby, &mut LiveCompositor, Option<u64>) -> StandbyOutcome,
    ) -> Result<ActionResult, String> {
        let mut niri = LiveCompositor::connect().map_err(|error| error.to_string())?;
        let outcome = action(&mut lock_standby(&self.standby), &mut niri, window_id);
        Ok(standby_action_result(&outcome, window_id))
    }

    /// Resumes a suspended window before it is restored or closed. Returns the
    /// resume's result when it did not apply, so the action is not forwarded.
    fn resume_before_forwarding(&self, window_id: u64) -> Result<Option<ActionResult>, String> {
        let mut standby = lock_standby(&self.standby);
        if !standby.is_suspended(window_id) {
            return Ok(None);
        }
        let mut niri = LiveCompositor::connect().map_err(|error| error.to_string())?;
        let outcome = standby.resume(&mut niri, Some(window_id));
        Ok((!matches!(outcome, StandbyOutcome::Applied { .. }))
            .then(|| standby_action_result(&outcome, Some(window_id))))
    }
}

fn execute_native_action(request: &Request) -> Result<ActionResult, String> {
    let mut client = NiriActionClient::connect().map_err(|error| error.to_string())?;
    let (operation, result) = match request {
        Request::Minimize {
            window_id,
            transition,
        } => (
            Operation::Minimize,
            client.minimize_window_with_transition(window_id.map(WindowId), transition.clone()),
        ),
        Request::Restore {
            window_id,
            transition,
        } => (
            Operation::Restore,
            client.restore_window_with_transition(WindowId(*window_id), transition.clone()),
        ),
        Request::Close { window_id } => {
            (Operation::Close, client.close_window(WindowId(*window_id)))
        }
        Request::Suspend { .. }
        | Request::Resume { .. }
        | Request::ToggleSuspend { .. }
        | Request::List
        | Request::Subscribe
        | Request::ListSuspended => {
            return Err("request is not a compositor action".to_owned());
        }
    };
    result
        .map_err(|error| error.to_string())
        .map(|result| map_native_action_result(operation, &result))
}

fn map_native_action_result(operation: Operation, result: &NiriWindowActionResult) -> ActionResult {
    let status = match result.status {
        NiriWindowActionStatus::Applied => ActionStatus::Applied,
        NiriWindowActionStatus::AlreadyInRequestedState => ActionStatus::AlreadyInRequestedState,
        NiriWindowActionStatus::CloseRequested => ActionStatus::CloseRequested,
        NiriWindowActionStatus::WindowNotFound => ActionStatus::WindowNotFound,
        NiriWindowActionStatus::Blocked => ActionStatus::Blocked,
        NiriWindowActionStatus::LegacyHandled => ActionStatus::LegacyHandled,
    };
    ActionResult {
        operation,
        requested_id: result.requested_id,
        window_id: result.window_id,
        status,
        reason: None,
    }
}

fn observe(path: Option<PathBuf>) -> ExitCode {
    let LoadedConfig { config, path, .. } = match configured(path) {
        Ok(configured) => configured,
        Err(error) => {
            errorln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    let mut stream = match NiriEventStream::connect() {
        Ok(stream) => stream,
        Err(error) => {
            errorln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    let mut adapter = NiriAdapter::new(config);

    outputln!(
        "observing niri in read-only mode with configuration from {}",
        path.display()
    );

    loop {
        let event = match stream.read_event() {
            Ok(Some(event)) => event,
            Ok(None) => {
                errorln!("niri closed the event stream");
                return ExitCode::FAILURE;
            }
            Err(error) => {
                errorln!("{error}");
                return ExitCode::FAILURE;
            }
        };
        let event_name = event_name(&event);

        match adapter.apply(event) {
            Ok(Some(transition)) => print_transition(event_name, &transition),
            Ok(None) => {}
            Err(error) => {
                errorln!("cannot apply niri event `{event_name}`: {error}");
                return ExitCode::FAILURE;
            }
        }
    }
}

fn run_controller(path: Option<PathBuf>) -> ExitCode {
    match run_controller_inner(path) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            errorln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run_controller_inner(path: Option<PathBuf>) -> Result<(), String> {
    let LoadedConfig {
        mut config,
        path,
        source,
    } = configured(path)?;
    if env::var_os(NIRI_SOCKET_ENV).is_none_or(|value| value.is_empty()) {
        return Err(format!(
            "{NIRI_SOCKET_ENV} is not set; run Melibea inside a niri session"
        ));
    }

    let (standby, games_path) = live_standby()?;
    let standby: SharedStandby = Arc::new(Mutex::new(standby));
    let service_path = socket_path().map_err(|error| error.to_string())?;
    let service = Service::start(
        &service_path,
        DaemonExecutor {
            standby: Arc::clone(&standby),
        },
    )
    .map_err(|error| error.to_string())?;

    outputln!(
        "running focus-responsive geometry with configuration from {}",
        path.display()
    );
    outputln!(
        "serving Melibea protocols v1/v2/v3 on {}",
        service_path.display()
    );
    let mut watcher = ConfigWatcher::new(path, source);

    loop {
        let standby_loop = StandbyLoop::new(Arc::clone(&standby), games_path.clone());
        match run_controller_connection(config.clone(), &mut watcher, &service, standby_loop) {
            Ok(reloaded) => {
                service
                    .set_unavailable("reloading configuration and rebuilding niri state")
                    .map_err(|error| error.to_string())?;
                config = reloaded;
                outputln!("configuration reloaded; rebuilding state from niri snapshot");
            }
            Err(error) => {
                service
                    .set_unavailable(format!("niri connection lost: {error}"))
                    .map_err(|error| error.to_string())?;
                errorln!(
                    "niri connection lost: {error}; rebuilding state in {} second(s)",
                    RECONNECT_DELAY.as_secs()
                );
                thread::sleep(RECONNECT_DELAY);
            }
        }
    }
}

fn run_controller_connection(
    config: Config,
    watcher: &mut ConfigWatcher,
    service: &Service,
    mut standby: StandbyLoop,
) -> Result<Config, String> {
    let stream = NiriEventStream::connect().map_err(|error| error.to_string())?;
    let mut actions = NiriActionClient::connect().map_err(|error| error.to_string())?;
    let reader = spawn_event_reader(stream)?;
    let mut adapter = NiriAdapter::new(config);
    let mut pending = PendingResizes::default();
    let mut minimized = MinimizedRegistry::default();

    outputln!("connected to niri; awaiting authoritative snapshot");

    loop {
        let message = match reader.receiver.recv_timeout(CONFIG_POLL_INTERVAL) {
            Ok(message) => message,
            Err(RecvTimeoutError::Timeout) => {
                standby.suspend_due(Instant::now());
                match watcher.poll_if_due() {
                    ConfigPoll::Unchanged => continue,
                    ConfigPoll::Reloaded(config) => return Ok(config),
                    ConfigPoll::Rejected(error) => {
                        errorln!("{error}; keeping last known-good policy");
                        continue;
                    }
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err("niri event reader stopped unexpectedly".to_owned());
            }
        };
        ingest_stream_message(
            message,
            &mut adapter,
            &mut pending,
            &mut minimized,
            service,
            &mut standby,
        )?;
        drain_stream_messages(
            &reader.receiver,
            &mut adapter,
            &mut pending,
            &mut minimized,
            service,
            &mut standby,
        )?;
        standby.suspend_due(Instant::now());

        match watcher.poll_if_due() {
            ConfigPoll::Unchanged => {}
            ConfigPoll::Reloaded(config) => return Ok(config),
            ConfigPoll::Rejected(error) => {
                errorln!("{error}; keeping last known-good policy");
            }
        }

        while !pending.is_empty() {
            // Coalesce focus changes that arrived before the next mutation.
            drain_stream_messages(
                &reader.receiver,
                &mut adapter,
                &mut pending,
                &mut minimized,
                service,
                &mut standby,
            )?;
            let Some(action) = pending.pop_next() else {
                break;
            };

            actions
                .set_window_width(action.window_id, action.width)
                .map_err(|error| {
                    format!(
                        "generation {} failed for window {}: {error}",
                        action.generation, action.window_id.0
                    )
                })?;
            outputln!(
                "generation={} window={} state={} applied-width={:.1}%",
                action.generation,
                action.window_id.0,
                focus_state_name(action.state),
                action.width.get() * 100.0
            );
        }
    }
}

fn spawn_event_reader(mut stream: NiriEventStream) -> Result<EventReader, String> {
    let cancellation = stream
        .cancellation_handle()
        .map_err(|error| format!("cannot create niri event cancellation handle: {error}"))?;
    let (sender, receiver) = mpsc::channel();
    let handle = thread::spawn(move || {
        loop {
            let message = stream.read_event().map_err(|error| error.to_string());
            let terminal = !matches!(message, Ok(Some(_)));
            if sender.send(message).is_err() || terminal {
                break;
            }
        }
    });
    Ok(EventReader {
        receiver,
        cancellation,
        handle: Some(handle),
    })
}

fn drain_stream_messages(
    receiver: &Receiver<StreamMessage>,
    adapter: &mut NiriAdapter,
    pending: &mut PendingResizes,
    minimized: &mut MinimizedRegistry,
    service: &Service,
    standby: &mut StandbyLoop,
) -> Result<(), String> {
    loop {
        match receiver.try_recv() {
            Ok(message) => {
                ingest_stream_message(message, adapter, pending, minimized, service, standby)?;
            }
            Err(TryRecvError::Empty) => return Ok(()),
            Err(TryRecvError::Disconnected) => {
                return Err("niri event reader stopped unexpectedly".to_owned());
            }
        }
    }
}

fn ingest_stream_message(
    message: StreamMessage,
    adapter: &mut NiriAdapter,
    pending: &mut PendingResizes,
    minimized: &mut MinimizedRegistry,
    service: &Service,
    standby: &mut StandbyLoop,
) -> Result<(), String> {
    let event = match message {
        Ok(Some(event)) => event,
        Ok(None) => return Err("niri closed the event stream".to_owned()),
        Err(error) => return Err(error),
    };
    let name = event_name(&event);
    if let NiriEvent::MinimizedWindowsChanged { windows, .. } = &event {
        let change = minimized
            .apply(MinimizationEvent::Snapshot {
                windows: windows.clone(),
            })
            .map_err(|error| format!("cannot apply niri event `{name}`: {error}"))?;
        service
            .publish_snapshot(minimized.windows())
            .map_err(|error| error.to_string())?;
        if !matches!(change, RegistryChange::Unchanged) {
            outputln!(
                "minimized-revision={} bubbles={}",
                minimized.revision(),
                minimized.windows().len()
            );
        }
    }
    // `observe` takes and drops the standby lock itself; publish only afterwards.
    if standby.observe(&event, Instant::now())
        && let Some(windows) = standby.publication()
    {
        service
            .publish_suspended(&windows)
            .map_err(|error| error.to_string())?;
    }
    let is_snapshot = matches!(&event, NiriEvent::WindowsChanged { .. });
    let closed_window = match &event {
        NiriEvent::WindowClosed { id } => Some(*id),
        _ => None,
    };
    let transition = adapter
        .apply(event)
        .map_err(|error| format!("cannot apply niri event `{name}`: {error}"))?;

    if is_snapshot {
        pending.reset();
    }
    if let Some(window_id) = closed_window {
        pending.forget_window(window_id);
    }
    if let Some(transition) = transition {
        pending.ingest(&transition);
    }
    Ok(())
}

/// The controller loop's side of standby for one niri connection: what is
/// suspended, reconciliation, and standby after minimizing.
///
/// No method here calls the `Service`; each one that takes the standby lock
/// drops it before returning, so the caller may publish afterwards.
struct StandbyLoop {
    standby: SharedStandby,
    games_path: PathBuf,
    timers: StandbyTimers,
    windows: BTreeMap<u64, NiriWindow>,
    minimized: Vec<MinimizedWindow>,
    minimized_suspended: BTreeSet<WindowId>,
    reconciled: bool,
    published: Option<Vec<SuspendedWindow>>,
    rules_error: Option<String>,
}

impl StandbyLoop {
    fn new(standby: SharedStandby, games_path: PathBuf) -> Self {
        Self {
            standby,
            games_path,
            timers: StandbyTimers::default(),
            windows: BTreeMap::new(),
            minimized: Vec::new(),
            minimized_suspended: BTreeSet::new(),
            reconciled: false,
            published: None,
            rules_error: None,
        }
    }

    /// Tracks suspended flags, reconciles after the connection's first
    /// window snapshot, and follows the minimized list. Returns whether the
    /// event can change the suspended list.
    fn observe(&mut self, event: &NiriEvent, now: Instant) -> bool {
        match event {
            NiriEvent::WindowsChanged { windows } => {
                self.windows = windows
                    .iter()
                    .map(|window| (window.id, window.clone()))
                    .collect();
                if !self.reconciled {
                    self.reconciled = true;
                    self.reconcile();
                }
            }
            NiriEvent::WindowOpenedOrChanged { window } => {
                self.windows.insert(window.id, window.clone());
            }
            NiriEvent::WindowClosed { id } => {
                self.windows.remove(&id.0);
            }
            NiriEvent::MinimizedWindowsChanged { windows, suspended } => {
                self.minimized.clone_from(windows);
                self.minimized_suspended.clone_from(suspended);
                self.follow_minimized(now);
            }
            NiriEvent::WindowFocusChanged { .. } | NiriEvent::Ignored { .. } => return false,
        }
        true
    }

    /// The suspended list when it differs from the last one published.
    fn publication(&mut self) -> Option<Vec<SuspendedWindow>> {
        let windows: Vec<NiriWindow> = self.windows.values().cloned().collect();
        let listed = suspended_windows(&windows, &self.minimized, &self.minimized_suspended);
        if self.published.as_ref() == Some(&listed) {
            return None;
        }
        self.published = Some(listed.clone());
        Some(listed)
    }

    fn reconcile(&self) {
        let lines = match LiveCompositor::connect() {
            Ok(mut niri) => lock_standby(&self.standby).reconcile(&mut niri),
            Err(error) => vec![format!("cannot reach niri: {error}")],
        };
        for line in lines {
            outputln!("standby reconcile: {line}");
        }
    }

    /// Restarts the timers from the minimized list and resumes each suspended
    /// window that left it.
    fn follow_minimized(&mut self, now: Instant) {
        let rules = match GameRules::load(&self.games_path) {
            Ok(rules) => {
                self.rules_error = None;
                Some(rules)
            }
            Err(error) => {
                let error = error.to_string();
                if self.rules_error.as_ref() != Some(&error) {
                    errorln!("standby after minimizing is off: {error}");
                    self.rules_error = Some(error);
                }
                None
            }
        };
        let left = self
            .timers
            .update(now, &minimized_delays(rules.as_ref(), &self.minimized));
        if left.is_empty() {
            return;
        }
        let mut standby = lock_standby(&self.standby);
        if !left.iter().any(|id| standby.is_suspended(*id)) {
            return;
        }
        let mut niri = match LiveCompositor::connect() {
            Ok(niri) => niri,
            Err(error) => {
                errorln!("cannot resume windows that left the minimized list: {error}");
                return;
            }
        };
        for id in left {
            // Resuming one window resumes its whole game, so check each again.
            if standby.is_suspended(id) {
                let outcome = standby.resume(&mut niri, Some(id));
                outputln!(
                    "standby: window {id} left the minimized list: {}",
                    standby_log_line(&outcome)
                );
            }
        }
    }

    /// Suspends each minimized game whose delay has elapsed. Each window is
    /// tried once per minimized period, so a refusal is logged once.
    fn suspend_due(&mut self, now: Instant) {
        let due = self.timers.due(now);
        if due.is_empty() {
            return;
        }
        let mut standby = lock_standby(&self.standby);
        let mut niri = match LiveCompositor::connect() {
            Ok(niri) => niri,
            Err(error) => {
                errorln!("cannot suspend minimized windows {due:?}: {error}");
                return;
            }
        };
        for id in due {
            if standby.is_suspended(id) {
                continue;
            }
            let outcome = standby.suspend(&mut niri, Some(id));
            outputln!(
                "standby after minimizing: window {id}: {}",
                standby_log_line(&outcome)
            );
        }
    }
}

/// The delay of each minimized window's game rule; none when the window is
/// no game, its rule names no `standby_after`, or the rules are unusable.
fn minimized_delays(
    rules: Option<&GameRules>,
    minimized: &[MinimizedWindow],
) -> Vec<(u64, Option<Duration>)> {
    minimized
        .iter()
        .map(|window| {
            let delay = rules
                .and_then(|rules| {
                    rules.recognize(window.app_id.as_deref(), window.title.as_deref())
                })
                .and_then(|game| game.standby_after);
            (window.id.0, delay)
        })
        .collect()
}

/// The windows niri reports suspended, visible or minimized, ascending by id.
fn suspended_windows(
    windows: &[NiriWindow],
    minimized: &[MinimizedWindow],
    minimized_suspended: &BTreeSet<WindowId>,
) -> Vec<SuspendedWindow> {
    let mut listed = BTreeMap::new();
    for window in minimized
        .iter()
        .filter(|window| minimized_suspended.contains(&window.id))
    {
        listed.insert(
            window.id.0,
            SuspendedWindow {
                id: window.id.0,
                app_id: window.app_id.clone(),
                title: window.title.clone(),
            },
        );
    }
    // A window in both lists is described by its visible record.
    for window in windows.iter().filter(|window| window.is_suspended) {
        listed.insert(
            window.id,
            SuspendedWindow {
                id: window.id,
                app_id: window.app_id.clone(),
                title: window.title.clone(),
            },
        );
    }
    listed.into_values().collect()
}

const fn event_name(event: &NiriEvent) -> &'static str {
    match event {
        NiriEvent::WindowsChanged { .. } => "WindowsChanged",
        NiriEvent::MinimizedWindowsChanged { .. } => "MinimizedWindowsChanged",
        NiriEvent::WindowOpenedOrChanged { .. } => "WindowOpenedOrChanged",
        NiriEvent::WindowClosed { .. } => "WindowClosed",
        NiriEvent::WindowFocusChanged { .. } => "WindowFocusChanged",
        NiriEvent::Ignored { .. } => "Ignored",
    }
}

fn print_transition(event: &str, transition: &FocusTransition) {
    outputln!(
        "generation={} event={event} focus={}->{}",
        transition.generation,
        format_window_id(transition.previous),
        format_window_id(transition.current)
    );

    for decision in &transition.decisions {
        let state = focus_state_name(decision.state);

        match decision.outcome {
            DecisionOutcome::Resize { rule_index, width } => outputln!(
                "  window={} state={state} rule={rule_index} policy={:.1}% action=would-set-window-width",
                decision.window_id.0,
                width.get() * 100.0
            ),
            DecisionOutcome::Preserve { rule_index } => outputln!(
                "  window={} state={state} rule={rule_index} policy=preserve action=none",
                decision.window_id.0
            ),
            DecisionOutcome::Unmanaged => outputln!(
                "  window={} state={state} rule=none action=none reason=no-match",
                decision.window_id.0
            ),
            DecisionOutcome::Excluded { reason } => outputln!(
                "  window={} state={state} rule=none action=none reason={}",
                decision.window_id.0,
                exclusion_name(reason)
            ),
            DecisionOutcome::UnknownWindow => outputln!(
                "  window={} state={state} rule=unknown action=none reason=missing-metadata",
                decision.window_id.0
            ),
        }
    }
}

const fn focus_state_name(state: FocusState) -> &'static str {
    match state {
        FocusState::Focused => "focused",
        FocusState::Unfocused => "unfocused",
    }
}

fn format_window_id(window_id: Option<WindowId>) -> String {
    window_id.map_or_else(|| "none".to_owned(), |id| id.0.to_string())
}

const fn exclusion_name(reason: ExclusionReason) -> &'static str {
    match reason {
        ExclusionReason::Floating => "floating",
    }
}

fn print_help() {
    outputln!(
        "Usage: melibea [--config PATH] <COMMAND> [WINDOW_ID]\n\nCommands:\n  check-config        Parse and validate configuration without contacting niri\n  observe             Explain live focus decisions without changing niri layout\n  run                 Run geometry policy and the local protocol service\n  status              Check configuration and current niri state without mutation\n  list                List minimized windows through the Melibea v1 service\n  minimize [ID]       Minimize a window, or the focused window when ID is omitted\n  restore ID          Restore one native minimized window\n  close ID            Request that niri close one minimized window\n  suspend [ID]        Suspend a game (freeze its processes), or the focused window's game\n  resume [ID]         Resume a suspended game, or the focused window's game\n  toggle-suspend [ID] Suspend a running game or resume a suspended one\n  thaw-all            Thaw every suspended game and clear the flags, without the daemon\n  subscribe           Print snapshot and incremental events as JSON lines\n  help                Show this help\n\nCompatibility aliases: minimized=list, close-window=close"
    );
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeSet,
        fs,
        io::{self, Write},
        path::PathBuf,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    use super::{
        Command, ConfigPoll, ConfigWatcher, describe_standby, minimized_delays, parse_args,
        standby_action_result, suspended_lines, suspended_windows, write_line,
    };
    use melibea::{
        minimization::MinimizedWindow,
        niri::NiriWindow,
        protocol::{ActionResult, ActionStatus, Operation, SuspendedWindow},
        standby::{
            controller::{BlockReason, StandbyOperation, StandbyOutcome},
            games::{GameIdentity, GameRules},
            ledger::LedgerEntry,
        },
        transition::WindowId,
    };

    struct BrokenWriter;

    impl Write for BrokenWriter {
        fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(ToString::to_string).collect()
    }

    fn temporary_config_path() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "melibea-config-watcher-{}-{nonce}.toml",
            std::process::id()
        ))
    }

    fn config_source(focused: &str) -> String {
        format!(
            r#"
                [[attention]]
                app_id = "^kitty$"
                focused_width = "{focused}"
                unfocused_width = "10%"
            "#
        )
    }

    #[test]
    fn parses_check_config_with_path_before_command() {
        assert_eq!(
            parse_args(args(&["--config", "test.toml", "check-config"])),
            Ok(Command::CheckConfig {
                path: Some(PathBuf::from("test.toml"))
            })
        );
    }

    #[test]
    fn parses_check_config_with_path_after_command() {
        assert_eq!(
            parse_args(args(&["check-config", "--config", "test.toml"])),
            Ok(Command::CheckConfig {
                path: Some(PathBuf::from("test.toml"))
            })
        );
    }

    #[test]
    fn rejects_missing_config_path() {
        assert_eq!(
            parse_args(args(&["check-config", "--config"])),
            Err("`--config` requires a path".to_owned())
        );
    }

    #[test]
    fn accepts_help_subcommand() {
        assert_eq!(parse_args(args(&["help"])), Ok(Command::Help));
    }

    #[test]
    fn parses_status_with_explicit_config() {
        assert_eq!(
            parse_args(args(&["status", "--config", "health.toml"])),
            Ok(Command::Status {
                path: Some(PathBuf::from("health.toml"))
            })
        );
    }

    #[test]
    fn parses_observe_with_explicit_config() {
        assert_eq!(
            parse_args(args(&["observe", "--config", "live.toml"])),
            Ok(Command::Observe {
                path: Some(PathBuf::from("live.toml"))
            })
        );
    }

    #[test]
    fn parses_run_with_explicit_config() {
        assert_eq!(
            parse_args(args(&["--config", "live.toml", "run"])),
            Ok(Command::Run {
                path: Some(PathBuf::from("live.toml"))
            })
        );
    }

    #[test]
    fn parses_minimized_without_configuration() {
        assert_eq!(parse_args(args(&["list"])), Ok(Command::List));
        assert_eq!(parse_args(args(&["minimized"])), Ok(Command::List));
        assert_eq!(
            parse_args(args(&["minimized", "--config", "unused.toml"])),
            Err("`minimized` does not accept `--config`".to_owned())
        );
    }

    #[test]
    fn parses_native_minimization_actions() {
        assert_eq!(
            parse_args(args(&["minimize"])),
            Ok(Command::Minimize { id: None })
        );
        assert_eq!(
            parse_args(args(&["minimize", "42"])),
            Ok(Command::Minimize {
                id: Some(WindowId(42))
            })
        );
        assert_eq!(
            parse_args(args(&["restore", "42"])),
            Ok(Command::Restore { id: WindowId(42) })
        );
        assert_eq!(
            parse_args(args(&["close-window", "42"])),
            Ok(Command::Close { id: WindowId(42) })
        );
        assert_eq!(
            parse_args(args(&["close", "42"])),
            Ok(Command::Close { id: WindowId(42) })
        );
        assert_eq!(parse_args(args(&["subscribe"])), Ok(Command::Subscribe));
    }

    #[test]
    fn rejects_invalid_native_minimization_arguments() {
        assert_eq!(
            parse_args(args(&["restore"])),
            Err("`restore` requires a window id".to_owned())
        );
        assert_eq!(
            parse_args(args(&["minimize", "not-an-id"])),
            Err("invalid window id for `minimize`: not-an-id".to_owned())
        );
        assert_eq!(
            parse_args(args(&["restore", "42", "43"])),
            Err("`restore` requires exactly one window id".to_owned())
        );
        assert_eq!(
            parse_args(args(&["minimize", "--config", "unused.toml"])),
            Err("`minimize` does not accept `--config`".to_owned())
        );
    }

    #[test]
    fn parses_standby_commands() {
        assert_eq!(
            parse_args(args(&["toggle-suspend"])),
            Ok(Command::ToggleSuspend { id: None })
        );
        assert_eq!(
            parse_args(args(&["suspend", "7"])),
            Ok(Command::Suspend {
                id: Some(WindowId(7))
            })
        );
        assert_eq!(
            parse_args(args(&["resume"])),
            Ok(Command::Resume { id: None })
        );
        assert_eq!(parse_args(args(&["thaw-all"])), Ok(Command::ThawAll));
        assert_eq!(
            parse_args(args(&["suspend", "7", "8"])),
            Err("`suspend` accepts at most one window id".to_owned())
        );
        assert_eq!(
            parse_args(args(&["thaw-all", "7"])),
            Err("unexpected argument for `thaw-all`: 7".to_owned())
        );
        assert_eq!(
            parse_args(args(&["resume", "--config", "unused.toml"])),
            Err("`resume` does not accept `--config`".to_owned())
        );
    }

    #[test]
    fn status_lists_suspended_games_and_nothing_when_none() {
        assert!(suspended_lines(&[]).is_empty());
        let entry = LedgerEntry {
            units: vec!["app-melibea-game-1.scope".to_owned()],
            windows: vec![608, 609],
            identity: GameIdentity::SteamApp { app_id: 42 },
            since_unix: 1,
            flagged: true,
        };
        let lines = suspended_lines(&[entry]);
        assert_eq!(lines[0], "suspended: 1 game(s)");
        assert!(lines[1].contains("windows=[608, 609]"), "{lines:?}");
    }

    #[test]
    fn standby_results_are_described_for_the_terminal() {
        let result = |operation, status, reason: Option<&str>| ActionResult {
            operation,
            requested_id: Some(608),
            window_id: Some(608),
            status,
            reason: reason.map(str::to_owned),
        };
        assert_eq!(
            describe_standby(&result(Operation::Suspend, ActionStatus::Applied, None)),
            Ok("suspended window 608".to_owned())
        );
        assert_eq!(
            describe_standby(&result(Operation::Resume, ActionStatus::Applied, None)),
            Ok("resumed window 608".to_owned())
        );
        assert_eq!(
            describe_standby(&result(
                Operation::Suspend,
                ActionStatus::Blocked,
                Some("not_a_game: nope")
            )),
            Err("not_a_game: nope".to_owned())
        );
    }

    fn niri_window(id: u64, is_suspended: bool) -> NiriWindow {
        NiriWindow {
            id,
            title: Some(format!("window {id}")),
            app_id: Some("steam_app_1".to_owned()),
            pid: Some(10),
            is_focused: false,
            is_floating: false,
            is_suspended,
        }
    }

    fn minimized_window(id: u64) -> MinimizedWindow {
        MinimizedWindow {
            id: WindowId(id),
            app_id: Some("steam_app_2".to_owned()),
            title: Some(format!("minimized {id}")),
        }
    }

    #[test]
    fn suspended_windows_merge_visible_and_minimized_flags_by_id() {
        let windows = [
            niri_window(30, true),
            niri_window(10, false),
            niri_window(20, true),
        ];
        let minimized = [
            minimized_window(25),
            minimized_window(5),
            minimized_window(40),
        ];
        let flagged = BTreeSet::from([WindowId(25), WindowId(5)]);

        let listed = suspended_windows(&windows, &minimized, &flagged);

        assert_eq!(
            listed,
            vec![
                SuspendedWindow {
                    id: 5,
                    app_id: Some("steam_app_2".to_owned()),
                    title: Some("minimized 5".to_owned()),
                },
                SuspendedWindow {
                    id: 20,
                    app_id: Some("steam_app_1".to_owned()),
                    title: Some("window 20".to_owned()),
                },
                SuspendedWindow {
                    id: 25,
                    app_id: Some("steam_app_2".to_owned()),
                    title: Some("minimized 25".to_owned()),
                },
                SuspendedWindow {
                    id: 30,
                    app_id: Some("steam_app_1".to_owned()),
                    title: Some("window 30".to_owned()),
                },
            ]
        );
    }

    #[test]
    fn suspended_windows_list_a_window_reported_twice_once() {
        let windows = [niri_window(7, true)];
        let minimized = [minimized_window(7)];
        let flagged = BTreeSet::from([WindowId(7)]);

        let listed = suspended_windows(&windows, &minimized, &flagged);

        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, 7);
    }

    #[test]
    fn suspended_windows_are_empty_when_nothing_is_flagged() {
        let listed = suspended_windows(
            &[niri_window(1, false)],
            &[minimized_window(2)],
            &BTreeSet::new(),
        );
        assert!(listed.is_empty());
    }

    #[test]
    fn standby_results_report_the_requested_id_and_the_performed_operation() {
        let applied = StandbyOutcome::Applied {
            operation: StandbyOperation::Resume,
            window_id: 608,
            windows: vec![608],
        };
        let result = standby_action_result(&applied, None);
        assert_eq!(result.operation, Operation::Resume);
        assert_eq!(result.requested_id, None);
        assert_eq!(result.window_id, Some(608));
        assert_eq!(result.status, ActionStatus::Applied);

        let blocked = StandbyOutcome::Blocked {
            operation: StandbyOperation::Suspend,
            window_id: Some(608),
            reason: BlockReason::NotAGame,
        };
        let result = standby_action_result(&blocked, Some(608));
        assert_eq!(result.operation, Operation::Suspend);
        assert_eq!(result.requested_id, Some(608));
        assert_eq!(result.window_id, Some(608));
        assert_eq!(result.status, ActionStatus::Blocked);
        assert!(
            result
                .reason
                .is_some_and(|reason| reason.starts_with("not_a_game"))
        );
    }

    #[test]
    fn minimized_games_get_the_delay_of_their_rule() {
        let rules = GameRules::parse(
            r#"
                [[game]]
                app_id = "^steam_proton$"
                title = "^Victoria 3$"
                process = "victoria3.exe"
                standby_after = "5m"
            "#,
        )
        .expect("valid rules");
        let game = MinimizedWindow {
            id: WindowId(1),
            app_id: Some("steam_proton".to_owned()),
            title: Some("Victoria 3".to_owned()),
        };
        let steam = MinimizedWindow {
            id: WindowId(2),
            app_id: Some("steam_app_42".to_owned()),
            title: None,
        };
        let other = MinimizedWindow {
            id: WindowId(3),
            app_id: Some("kitty".to_owned()),
            title: None,
        };

        assert_eq!(
            minimized_delays(Some(&rules), &[game.clone(), steam, other]),
            vec![(1, Some(Duration::from_secs(300))), (2, None), (3, None)]
        );
        assert_eq!(minimized_delays(None, &[game]), vec![(1, None)]);
    }

    #[test]
    fn closed_output_pipe_cannot_terminate_controller() {
        write_line(BrokenWriter, format_args!("diagnostic"));
    }

    #[test]
    fn config_watcher_reloads_valid_source_and_deduplicates_rejection() {
        let path = temporary_config_path();
        let initial = config_source("50%");
        fs::write(&path, &initial).expect("write initial config");
        let mut watcher = ConfigWatcher::new(path.clone(), initial);

        fs::write(&path, config_source("60%")).expect("write valid update");
        assert!(matches!(watcher.poll_now(), ConfigPoll::Reloaded(_)));

        fs::write(&path, "invalid = true").expect("write invalid update");
        assert!(matches!(watcher.poll_now(), ConfigPoll::Rejected(_)));
        assert!(matches!(watcher.poll_now(), ConfigPoll::Unchanged));

        fs::write(&path, config_source("70%")).expect("write recovered update");
        assert!(matches!(watcher.poll_now(), ConfigPoll::Reloaded(_)));

        fs::remove_file(path).expect("remove test config");
    }
}
