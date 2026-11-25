//! Moving, freezing and thawing game scopes through `busctl` and `systemctl`.

use std::collections::BTreeSet;
use std::fmt;
use std::io::{self, Read};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// Upper bound for every `busctl`/`systemctl` call.
pub const SYSTEMD_TIMEOUT: Duration = Duration::from_secs(5);

const POLL_INTERVAL: Duration = Duration::from_millis(20);
const STDERR_LIMIT: usize = 4096;
const UNIT_PREFIX: &str = "app-melibea-game-";
const UNIT_SUFFIX: &str = ".scope";

fn is_number(text: &str) -> bool {
    let mut chars = text.chars();
    matches!(chars.next(), Some('1'..='9')) && chars.all(|c| c.is_ascii_digit())
}

/// Whether `name` matches `^app-melibea-game-[1-9][0-9]*(-[1-9][0-9]*)?\.scope$`.
#[must_use]
pub fn is_game_unit(name: &str) -> bool {
    let Some(middle) = name
        .strip_prefix(UNIT_PREFIX)
        .and_then(|rest| rest.strip_suffix(UNIT_SUFFIX))
    else {
        return false;
    };
    match middle.split_once('-') {
        Some((pid, suffix)) => is_number(pid) && is_number(suffix),
        None => is_number(middle),
    }
}

/// Unit name for a game rooted at `root_pid`, with an optional disambiguating suffix.
#[must_use]
pub fn game_unit_name(root_pid: u32, suffix: Option<u32>) -> String {
    match suffix {
        Some(k) => format!("{UNIT_PREFIX}{root_pid}-{k}{UNIT_SUFFIX}"),
        None => format!("{UNIT_PREFIX}{root_pid}{UNIT_SUFFIX}"),
    }
}

/// The freezer state of a unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FreezerState {
    Running,
    Freezing,
    Frozen,
    Thawing,
    NotLoaded,
}

/// Why a systemd call failed.
#[derive(Debug)]
pub enum SystemdError {
    NotAGameUnit(String),
    Spawn(io::Error),
    Timeout { program: String },
    Failed { program: String, stderr: String },
    Unexpected(String),
}

impl fmt::Display for SystemdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAGameUnit(name) => write!(f, "{name} is not a Melibea game unit"),
            Self::Spawn(err) => write!(f, "cannot run systemd tool: {err}"),
            Self::Timeout { program } => write!(f, "{program} timed out"),
            Self::Failed { program, stderr } => write!(f, "{program} failed: {}", stderr.trim()),
            Self::Unexpected(text) => write!(f, "unexpected systemd output: {text}"),
        }
    }
}

impl std::error::Error for SystemdError {}

/// What standby needs from systemd.
pub trait Systemd {
    /// Starts `unit` as a transient scope holding `pids`.
    ///
    /// # Errors
    /// Fails when the unit name is foreign or the call fails.
    fn start_scope(&self, unit: &str, pids: &BTreeSet<u32>) -> Result<(), SystemdError>;
    /// Freezes `unit`.
    ///
    /// # Errors
    /// Fails when the unit name is foreign or the call fails.
    fn freeze(&self, unit: &str) -> Result<(), SystemdError>;
    /// Thaws `unit`.
    ///
    /// # Errors
    /// Fails when the unit name is foreign or the call fails.
    fn thaw(&self, unit: &str) -> Result<(), SystemdError>;
    /// Reads the freezer state of `unit`.
    ///
    /// # Errors
    /// Fails when the unit name is foreign, the call fails or the output is not understood.
    fn freezer_state(&self, unit: &str) -> Result<FreezerState, SystemdError>;
    /// Lists the Melibea game units known to systemd.
    ///
    /// # Errors
    /// Fails when the call fails.
    fn game_units(&self) -> Result<Vec<String>, SystemdError>;
}

/// The real implementation, running `busctl` and `systemctl`.
#[derive(Clone, Debug)]
pub struct SystemdTools {
    busctl: PathBuf,
    systemctl: PathBuf,
    timeout: Duration,
}

impl Default for SystemdTools {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemdTools {
    /// Uses `busctl` and `systemctl` from `PATH` with [`SYSTEMD_TIMEOUT`].
    #[must_use]
    pub fn new() -> Self {
        Self::with_programs("busctl".into(), "systemctl".into(), SYSTEMD_TIMEOUT)
    }

    /// Uses the given programs and timeout.
    #[must_use]
    pub fn with_programs(busctl: PathBuf, systemctl: PathBuf, timeout: Duration) -> Self {
        Self {
            busctl,
            systemctl,
            timeout,
        }
    }

    fn run(&self, program: &PathBuf, args: &[String]) -> Result<String, SystemdError> {
        let name = program.display().to_string();
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(SystemdError::Spawn)?;
        let stdout = child.stdout.take().map(read_all);
        let stderr = child.stderr.take().map(read_all);
        let deadline = Instant::now() + self.timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(SystemdError::Timeout { program: name });
                }
                Ok(None) => thread::sleep(POLL_INTERVAL),
                Err(err) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(SystemdError::Spawn(err));
                }
            }
        };
        let out = stdout.map(join_bytes).unwrap_or_default();
        let err = stderr.map(join_bytes).unwrap_or_default();
        if status.success() {
            Ok(String::from_utf8_lossy(&out).into_owned())
        } else {
            let cut = &err[..err.len().min(STDERR_LIMIT)];
            Err(SystemdError::Failed {
                program: name,
                stderr: String::from_utf8_lossy(cut).into_owned(),
            })
        }
    }

    fn ctl(&self, args: &[&str]) -> Result<String, SystemdError> {
        let mut full = vec!["--user".to_owned()];
        full.extend(args.iter().map(|a| (*a).to_owned()));
        self.run(&self.systemctl, &full)
    }
}

fn read_all<R: Read + Send + 'static>(mut reader: R) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = reader.read_to_end(&mut buf);
        buf
    })
}

fn join_bytes(handle: thread::JoinHandle<Vec<u8>>) -> Vec<u8> {
    handle.join().unwrap_or_default()
}

fn check_unit(unit: &str) -> Result<(), SystemdError> {
    if is_game_unit(unit) {
        Ok(())
    } else {
        Err(SystemdError::NotAGameUnit(unit.to_owned()))
    }
}

impl Systemd for SystemdTools {
    fn start_scope(&self, unit: &str, pids: &BTreeSet<u32>) -> Result<(), SystemdError> {
        check_unit(unit)?;
        let mut argv: Vec<String> = [
            "--user",
            "call",
            "org.freedesktop.systemd1",
            "/org/freedesktop/systemd1",
            "org.freedesktop.systemd1.Manager",
            "StartTransientUnit",
            "ssa(sv)a(sa(sv))",
            unit,
            "fail",
            "2",
            "PIDs",
            "au",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        argv.push(pids.len().to_string());
        argv.extend(pids.iter().map(u32::to_string));
        argv.extend(
            ["CollectMode", "s", "inactive-or-failed", "0"]
                .iter()
                .map(|s| (*s).to_owned()),
        );
        self.run(&self.busctl, &argv).map(|_| ())
    }

    fn freeze(&self, unit: &str) -> Result<(), SystemdError> {
        check_unit(unit)?;
        self.ctl(&["freeze", unit]).map(|_| ())
    }

    fn thaw(&self, unit: &str) -> Result<(), SystemdError> {
        check_unit(unit)?;
        self.ctl(&["thaw", unit]).map(|_| ())
    }

    fn freezer_state(&self, unit: &str) -> Result<FreezerState, SystemdError> {
        check_unit(unit)?;
        let out = self.ctl(&["show", "--property=LoadState,FreezerState", unit])?;
        let mut load = None;
        let mut freezer = None;
        for line in out.lines() {
            if let Some(v) = line.strip_prefix("LoadState=") {
                load = Some(v.trim());
            } else if let Some(v) = line.strip_prefix("FreezerState=") {
                freezer = Some(v.trim());
            }
        }
        if load == Some("not-found") {
            return Ok(FreezerState::NotLoaded);
        }
        match freezer {
            Some("running") => Ok(FreezerState::Running),
            Some("freezing") => Ok(FreezerState::Freezing),
            Some("frozen") => Ok(FreezerState::Frozen),
            Some("thawing") => Ok(FreezerState::Thawing),
            _ => Err(SystemdError::Unexpected(out.trim().to_owned())),
        }
    }

    fn game_units(&self) -> Result<Vec<String>, SystemdError> {
        let out = self.ctl(&[
            "list-units",
            "--all",
            "--plain",
            "--no-legend",
            "--full",
            "app-melibea-game-*.scope",
        ])?;
        Ok(out
            .lines()
            .filter_map(|line| line.split_whitespace().next())
            .filter(|name| is_game_unit(name))
            .map(str::to_owned)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    struct Stubs {
        dir: PathBuf,
    }

    impl Stubs {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("melibea-systemd-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self { dir }
        }

        fn script(&self, file: &str, body: &str) -> PathBuf {
            let path = self.dir.join(file);
            fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            path
        }

        fn logging(&self, file: &str, output: &str) -> PathBuf {
            let log = self.dir.join("log");
            self.script(
                file,
                &format!(
                    "echo \"$0 $*\" >> {}\ncat <<'EOF'\n{output}\nEOF",
                    log.display()
                ),
            )
        }

        fn log(&self) -> String {
            fs::read_to_string(self.dir.join("log")).unwrap_or_default()
        }

        fn tools(busctl: PathBuf, systemctl: PathBuf, ms: u64) -> SystemdTools {
            SystemdTools::with_programs(busctl, systemctl, Duration::from_millis(ms))
        }
    }

    impl Drop for Stubs {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    fn missing() -> PathBuf {
        Path::new("/nonexistent").to_path_buf()
    }

    #[test]
    fn unit_pattern_and_names() {
        assert!(is_game_unit("app-melibea-game-4.scope"));
        assert!(is_game_unit("app-melibea-game-4-12.scope"));
        for bad in [
            "app-melibea-game-0.scope",
            "app-melibea-game-4-0.scope",
            "app-melibea-game-04.scope",
            "app-melibea-game-.scope",
            "app-melibea-game-4-.scope",
            "app-melibea-game-4-1-2.scope",
            "app-melibea-game-4.service",
            "xapp-melibea-game-4.scope",
            "app-melibea-game-4.scope\n",
        ] {
            assert!(!is_game_unit(bad), "{bad}");
        }
        assert_eq!(game_unit_name(7, None), "app-melibea-game-7.scope");
        assert_eq!(game_unit_name(7, Some(2)), "app-melibea-game-7-2.scope");
    }

    #[test]
    fn start_scope_sends_the_exact_call() {
        let s = Stubs::new("start");
        let tools = Stubs::tools(s.logging("busctl", ""), missing(), 5000);
        let pids = BTreeSet::from([30, 4]);
        tools
            .start_scope("app-melibea-game-4.scope", &pids)
            .unwrap();
        let log = s.log();
        assert!(
            log.trim_end().ends_with(
                "StartTransientUnit ssa(sv)a(sa(sv)) app-melibea-game-4.scope fail 2 PIDs au 2 4 30 CollectMode s inactive-or-failed 0"
            ),
            "{log}"
        );
    }

    #[test]
    fn every_call_refuses_a_foreign_unit() {
        let s = Stubs::new("foreign");
        let tools = Stubs::tools(s.logging("busctl", ""), s.logging("systemctl", ""), 5000);
        assert!(matches!(
            tools.freeze("app-flatpak-x.scope"),
            Err(SystemdError::NotAGameUnit(_))
        ));
        assert!(matches!(
            tools.thaw("niri.service"),
            Err(SystemdError::NotAGameUnit(_))
        ));
        assert!(matches!(
            tools.start_scope("app-melibea-game-0.scope", &BTreeSet::from([1])),
            Err(SystemdError::NotAGameUnit(_))
        ));
        assert!(matches!(
            tools.freezer_state("niri.service"),
            Err(SystemdError::NotAGameUnit(_))
        ));
        assert_eq!(s.log(), "");
    }

    #[test]
    fn freeze_and_thaw_use_systemctl_user() {
        let s = Stubs::new("freeze");
        let tools = Stubs::tools(missing(), s.logging("systemctl", ""), 5000);
        tools.freeze("app-melibea-game-4.scope").unwrap();
        tools.thaw("app-melibea-game-4.scope").unwrap();
        let log = s.log();
        assert!(log.contains("systemctl --user freeze app-melibea-game-4.scope"));
        assert!(log.contains("systemctl --user thaw app-melibea-game-4.scope"));
    }

    #[test]
    fn freezer_state_reads_both_properties() {
        let s = Stubs::new("state");
        let unit = "app-melibea-game-4.scope";
        let t = Stubs::tools(
            missing(),
            s.logging("systemctl", "LoadState=loaded\nFreezerState=frozen"),
            5000,
        );
        assert_eq!(t.freezer_state(unit).unwrap(), FreezerState::Frozen);
        let t = Stubs::tools(
            missing(),
            s.logging("systemctl2", "LoadState=not-found\nFreezerState=running"),
            5000,
        );
        assert_eq!(t.freezer_state(unit).unwrap(), FreezerState::NotLoaded);
        let t = Stubs::tools(
            missing(),
            s.logging("systemctl3", "LoadState=loaded\nFreezerState=bogus"),
            5000,
        );
        assert!(matches!(
            t.freezer_state(unit),
            Err(SystemdError::Unexpected(_))
        ));
    }

    #[test]
    fn a_failing_tool_reports_its_stderr() {
        let s = Stubs::new("fail");
        let tools = Stubs::tools(
            missing(),
            s.script("systemctl", "echo Access denied >&2\nexit 1"),
            5000,
        );
        match tools.freeze("app-melibea-game-4.scope") {
            Err(SystemdError::Failed { stderr, .. }) => assert!(stderr.contains("Access denied")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_hanging_tool_times_out() {
        let s = Stubs::new("hang");
        let tools = Stubs::tools(missing(), s.script("systemctl", "exec sleep 10"), 200);
        let start = Instant::now();
        let result = tools.freeze("app-melibea-game-4.scope");
        assert!(matches!(result, Err(SystemdError::Timeout { .. })));
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn game_units_lists_only_game_units() {
        let s = Stubs::new("list");
        let out = "app-melibea-game-4.scope loaded active running Game\napp-other-5.scope loaded active running X\napp-melibea-game-9-2.scope loaded inactive dead Game";
        let tools = Stubs::tools(missing(), s.logging("systemctl", out), 5000);
        assert_eq!(
            tools.game_units().unwrap(),
            ["app-melibea-game-4.scope", "app-melibea-game-9-2.scope"]
        );
    }
}
