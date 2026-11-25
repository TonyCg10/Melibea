//! A read of the user's processes, their parents and systemd units from `/proc`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

/// What Melibea needs to know about one process.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessInfo {
    /// Process id.
    pub pid: u32,
    /// Parent process id.
    pub ppid: u32,
    /// Owner of `/proc/<pid>`.
    pub uid: u32,
    /// Command name from `stat`.
    pub comm: String,
    /// First `cmdline` argument, when readable and not empty.
    pub argv0: Option<String>,
    /// Control group path (the part after `0::`).
    pub cgroup: String,
    /// `SteamAppId` from the environment, when readable and numeric.
    pub steam_app_id: Option<u64>,
}

/// A snapshot of processes indexed by pid.
#[derive(Clone, Debug, Default)]
pub struct ProcessTable {
    processes: BTreeMap<u32, ProcessInfo>,
}

impl ProcessTable {
    /// Builds a table from already-read processes.
    #[must_use]
    pub fn new(processes: impl IntoIterator<Item = ProcessInfo>) -> Self {
        Self {
            processes: processes.into_iter().map(|p| (p.pid, p)).collect(),
        }
    }

    /// Reads every numeric entry under `proc_root`; processes that vanish
    /// while being read are skipped.
    ///
    /// # Errors
    ///
    /// Returns an error when `proc_root` itself cannot be listed.
    pub fn read(proc_root: &Path) -> io::Result<Self> {
        let mut processes = Vec::new();
        for entry in fs::read_dir(proc_root)? {
            let Ok(entry) = entry else { continue };
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<u32>().ok())
            else {
                continue;
            };
            if let Some(info) = read_process(&entry.path(), pid) {
                processes.push(info);
            }
        }
        Ok(Self::new(processes))
    }

    /// Returns the process with this pid.
    #[must_use]
    pub fn get(&self, pid: u32) -> Option<&ProcessInfo> {
        self.processes.get(&pid)
    }

    /// Iterates over all processes in pid order.
    pub fn iter(&self) -> impl Iterator<Item = &ProcessInfo> {
        self.processes.values()
    }

    /// Returns `roots` that exist in the table plus all their descendants.
    #[must_use]
    pub fn with_descendants(&self, roots: &BTreeSet<u32>) -> BTreeSet<u32> {
        let mut found: BTreeSet<u32> = roots
            .iter()
            .copied()
            .filter(|pid| self.processes.contains_key(pid))
            .collect();
        loop {
            let before = found.len();
            for process in self.processes.values() {
                if found.contains(&process.ppid) {
                    found.insert(process.pid);
                }
            }
            if found.len() == before {
                return found;
            }
        }
    }

    /// Returns the members of `set` whose parent is not a member.
    #[must_use]
    pub fn roots(&self, set: &BTreeSet<u32>) -> BTreeSet<u32> {
        set.iter()
            .copied()
            .filter(|pid| {
                self.processes
                    .get(pid)
                    .is_none_or(|process| !set.contains(&process.ppid))
            })
            .collect()
    }

    /// Returns the pids whose cgroup's last component is `unit`.
    #[must_use]
    pub fn in_unit(&self, unit: &str) -> BTreeSet<u32> {
        self.processes
            .values()
            .filter(|process| process.cgroup.rsplit('/').next() == Some(unit))
            .map(|process| process.pid)
            .collect()
    }
}

fn read_process(dir: &Path, pid: u32) -> Option<ProcessInfo> {
    let uid = fs::metadata(dir).ok()?.uid();
    let stat = fs::read_to_string(dir.join("stat")).ok()?;
    let (comm, parent) = parse_stat(&stat)?;
    let argv0 = fs::read(dir.join("cmdline")).ok().and_then(|bytes| {
        let first = bytes.split(|b| *b == 0).next()?;
        (!first.is_empty()).then(|| String::from_utf8_lossy(first).into_owned())
    });
    let cgroup = fs::read_to_string(dir.join("cgroup"))
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .unwrap_or_default()
        .to_string();
    let steam_app_id = fs::read(dir.join("environ")).ok().and_then(|bytes| {
        bytes.split(|b| *b == 0).find_map(|var| {
            let value = var.strip_prefix(b"SteamAppId=")?;
            std::str::from_utf8(value).ok()?.parse::<u64>().ok()
        })
    });
    Some(ProcessInfo {
        pid,
        ppid: parent,
        uid,
        comm,
        argv0,
        cgroup,
        steam_app_id,
    })
}

/// Parses `pid (comm) state ppid …`, cutting at the first `(` and the last `)`.
fn parse_stat(stat: &str) -> Option<(String, u32)> {
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    let comm = stat.get(open + 1..close)?.to_string();
    let ppid = stat
        .get(close + 1..)?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()?;
    Some((comm, ppid))
}

/// Returns the app unit a cgroup path names, when it sits directly under the
/// user's `app.slice`: `app-*.scope` or `app-*.service`.
#[must_use]
pub fn app_unit(cgroup: &str, uid: u32) -> Option<&str> {
    let prefix = format!("/user.slice/user-{uid}.slice/user@{uid}.service/app.slice/");
    let unit = cgroup.strip_prefix(&prefix)?;
    let name = unit
        .strip_suffix(".scope")
        .or_else(|| unit.strip_suffix(".service"))?;
    (name.starts_with("app-") && !unit.contains('/')).then_some(unit)
}

/// Returns the last path component, splitting at `/` or `\`.
#[must_use]
pub fn exe_basename(argv0: &str) -> &str {
    argv0.rsplit(['/', '\\']).next().unwrap_or(argv0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const BOTTLES: &str = "/user.slice/user-1000.slice/user@1000.service/app.slice/app-flatpak-com.usebottles.bottles-1.scope";

    fn fixture(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("melibea-process-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_pid(root: &Path, pid: u32, stat: Option<&str>, environ: &[u8]) {
        let dir = root.join(pid.to_string());
        fs::create_dir_all(&dir).unwrap();
        if let Some(stat) = stat {
            fs::write(dir.join("stat"), stat).unwrap();
        }
        fs::write(
            dir.join("cmdline"),
            b"Z:\\mnt\\JUEGOS\\victoria3.exe\0--flag\0",
        )
        .unwrap();
        fs::write(dir.join("environ"), environ).unwrap();
        fs::write(dir.join("cgroup"), format!("0::{BOTTLES}\n")).unwrap();
    }

    fn fake(pid: u32, parent: u32) -> ProcessInfo {
        ProcessInfo {
            pid,
            ppid: parent,
            uid: 1000,
            comm: String::new(),
            argv0: None,
            cgroup: String::new(),
            steam_app_id: None,
        }
    }

    #[test]
    fn reads_a_wine_process_from_a_fixture_proc() {
        let root = fixture("wine");
        write_pid(
            &root,
            4242,
            Some("4242 (Main Thread) S 4100 4242 4242 0 -1 4194560 0"),
            b"HOME=/home/x\0",
        );
        let table = ProcessTable::read(&root).unwrap();
        let info = table.get(4242).unwrap();
        assert_eq!(info.comm, "Main Thread");
        assert_eq!(info.ppid, 4100);
        assert_eq!(
            info.argv0.as_deref(),
            Some("Z:\\mnt\\JUEGOS\\victoria3.exe")
        );
        assert_eq!(info.steam_app_id, None);
        assert_eq!(info.cgroup, BOTTLES);
    }

    #[test]
    fn reads_the_steam_app_id_from_the_environment() {
        let root = fixture("steam");
        let stat = "7 (game) S 1 7 7 0";
        write_pid(&root, 7, Some(stat), b"A=b\0SteamAppId=1158310\0");
        write_pid(&root, 8, Some("8 (game) S 1 8 8 0"), b"SteamAppId=abc\0");
        let table = ProcessTable::read(&root).unwrap();
        assert_eq!(table.get(7).unwrap().steam_app_id, Some(1_158_310));
        assert_eq!(table.get(8).unwrap().steam_app_id, None);
    }

    #[test]
    fn a_process_that_vanished_is_skipped() {
        let root = fixture("vanished");
        write_pid(&root, 9, None, b"");
        write_pid(&root, 10, Some("10 (ok) S 1 10 10 0"), b"");
        let table = ProcessTable::read(&root).unwrap();
        assert!(table.get(9).is_none());
        assert!(table.get(10).is_some());
    }

    #[test]
    fn descendants_follow_parent_links() {
        let table = ProcessTable::new([fake(1, 0), fake(2, 1), fake(3, 2), fake(4, 0)]);
        let two = BTreeSet::from([2]);
        assert_eq!(table.with_descendants(&two), BTreeSet::from([2, 3]));
        let set = BTreeSet::from([2, 3, 4]);
        assert_eq!(table.roots(&set), BTreeSet::from([2, 4]));
    }

    #[test]
    fn app_unit_accepts_only_app_units_directly_under_app_slice() {
        let base = "/user.slice/user-1000.slice/user@1000.service";
        assert_eq!(
            app_unit(BOTTLES, 1000),
            Some("app-flatpak-com.usebottles.bottles-1.scope")
        );
        assert_eq!(
            app_unit(&format!("{base}/app.slice/app-niri-foot-9.scope"), 1000),
            Some("app-niri-foot-9.scope")
        );
        assert_eq!(
            app_unit(&format!("{base}/session.slice/niri.service"), 1000),
            None
        );
        assert_eq!(
            app_unit(&format!("{base}/app.slice/app-dbus-x.slice/y.scope"), 1000),
            None
        );
        assert_eq!(app_unit(BOTTLES, 1001), None);
        assert_eq!(
            app_unit(&format!("{base}/app.slice/melibea.service"), 1000),
            None
        );
    }

    #[test]
    fn exe_basename_cuts_at_either_separator() {
        assert_eq!(exe_basename("Z:\\a\\b.exe"), "b.exe");
        assert_eq!(exe_basename("/usr/bin/foot"), "foot");
        assert_eq!(exe_basename("foot"), "foot");
    }

    #[test]
    fn in_unit_matches_the_last_cgroup_component() {
        let mut a = fake(1, 0);
        a.cgroup = BOTTLES.to_string();
        let table = ProcessTable::new([a, fake(2, 1)]);
        assert_eq!(
            table.in_unit("app-flatpak-com.usebottles.bottles-1.scope"),
            BTreeSet::from([1])
        );
    }
}
