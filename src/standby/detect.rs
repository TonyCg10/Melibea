//! Recognizing a game without a rule: find the process that owns the window
//! and read what its environment says about it.

use super::controller::WindowView;
use super::games::GameIdentity;
use super::process::{ProcessInfo, ProcessTable, exe_basename};
use super::x11::{OwnerLookup, X11Client};

/// `comm` of `xwayland-satellite`, cut at 15 bytes by the kernel.
const SATELLITE_COMM: &str = "xwayland-satell";

/// Executables Wine runs for itself in every prefix; a window they own is
/// never a game (compared ignoring ASCII case).
const WINE_SYSTEM_EXES: [&str; 12] = [
    "explorer.exe",
    "services.exe",
    "winedevice.exe",
    "plugplay.exe",
    "svchost.exe",
    "rpcss.exe",
    "conhost.exe",
    "start.exe",
    "wineboot.exe",
    "winecfg.exe",
    "rundll32.exe",
    "tabtip.exe",
];

/// What a process's environment says about it: a Steam game, a Wine or Proton
/// process, or nothing. `WINEPREFIX` is no marker: the session exports it to
/// unrelated programs.
///
/// # Errors
///
/// Returns why the process is not a game.
pub fn identity_of(owner: &ProcessInfo) -> Result<GameIdentity, String> {
    if let Some(app_id) = owner.steam_app_id.filter(|id| *id > 0) {
        return Ok(GameIdentity::SteamApp { app_id });
    }
    if owner.is_wine {
        let Some(argv0) = owner.argv0.as_deref() else {
            return Err(format!(
                "Wine process {} ({}) has no argv[0]",
                owner.pid, owner.comm
            ));
        };
        let name = exe_basename(argv0).to_ascii_lowercase();
        if WINE_SYSTEM_EXES.contains(&name.as_str()) {
            return Err(format!(
                "process {} runs Wine's own {name}, not a game",
                owner.pid
            ));
        }
        return Ok(GameIdentity::Process { name });
    }
    Err(format!(
        "process {} ({}) has neither SteamAppId nor WINESERVERSOCKET",
        owner.pid, owner.comm
    ))
}

/// Who owns the X11 window with this app id and title.
pub trait WindowOwners {
    /// # Errors
    ///
    /// Returns the error text when X11 cannot be asked (no display, no
    /// server, a bad reply).
    fn owner(&mut self, app_id: &str, title: &str) -> Result<OwnerLookup, String>;
}

/// The real X server, connected to afresh for every lookup.
#[derive(Clone, Copy, Debug, Default)]
pub struct LiveX11;

impl WindowOwners for LiveX11 {
    fn owner(&mut self, app_id: &str, title: &str) -> Result<OwnerLookup, String> {
        X11Client::connect_from_env()
            .and_then(|mut client| client.window_owner(app_id, title))
            .map_err(|error| error.to_string())
    }
}

/// Finds the game `window` belongs to from its owner's environment.
///
/// A window of `xwayland-satellite` is looked up through X11; any other
/// window is owned by the process niri reports. `Ok(None)` means nothing
/// could be attempted: niri reports no usable pid for the window.
///
/// # Errors
///
/// Returns why the window is not recognized: the owner is not a game, is
/// not running, the X11 lookup failed or found no window, or several
/// processes own matching X11 windows.
pub fn detect(
    window: &WindowView,
    table: &ProcessTable,
    owners: &mut dyn WindowOwners,
) -> Result<Option<GameIdentity>, String> {
    let Some(niri_pid) = window.pid.and_then(|pid| u32::try_from(pid).ok()) else {
        return Ok(None);
    };
    let not_running = |pid: u32| format!("process {pid} is not running");
    let niri_process = table.get(niri_pid).ok_or_else(|| not_running(niri_pid))?;
    let owner_pid = if niri_process.comm == SATELLITE_COMM {
        let (Some(app_id), Some(title)) = (window.app_id.as_deref(), window.title.as_deref())
        else {
            return Err("the X11 window has no app id or no title".into());
        };
        match owners.owner(app_id, title) {
            Ok(OwnerLookup::Owner(pid)) => pid,
            Ok(OwnerLookup::Ambiguous(pids)) => {
                return Err(format!(
                    "{} processes own X11 windows with class {app_id:?} and title {title:?}",
                    pids.len()
                ));
            }
            Ok(OwnerLookup::NoMatch) => {
                return Err(format!(
                    "no X11 window with class {app_id:?} and title {title:?}"
                ));
            }
            Err(error) => return Err(format!("X11 lookup failed: {error}")),
        }
    } else {
        niri_pid
    };
    let owner = table.get(owner_pid).ok_or_else(|| not_running(owner_pid))?;
    identity_of(owner).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(pid: u32, comm: &str, argv0: Option<&str>) -> ProcessInfo {
        ProcessInfo {
            pid,
            ppid: 1,
            uid: 1000,
            comm: comm.into(),
            argv0: argv0.map(Into::into),
            cgroup: String::new(),
            steam_app_id: None,
            is_wine: false,
        }
    }

    #[test]
    fn steam_app_id_in_the_environment_is_a_steam_game() {
        let mut game = process(1, "dotnet", Some("dotnet"));
        game.steam_app_id = Some(1_281_930);
        assert_eq!(
            identity_of(&game),
            Ok(GameIdentity::SteamApp { app_id: 1_281_930 })
        );
        game.steam_app_id = Some(0);
        assert!(identity_of(&game).is_err());
    }

    #[test]
    fn a_wine_process_is_a_game_by_its_exe() {
        let mut game = process(1, "Main Thread", Some("Z:\\mnt\\JUEGOS\\Victoria3.EXE"));
        game.is_wine = true;
        assert_eq!(
            identity_of(&game),
            Ok(GameIdentity::Process {
                name: "victoria3.exe".into()
            })
        );
        game.steam_app_id = Some(529_340);
        assert_eq!(
            identity_of(&game),
            Ok(GameIdentity::SteamApp { app_id: 529_340 })
        );
    }

    #[test]
    fn wineprefix_alone_is_not_a_game() {
        // A browser carrying WINEPREFIX has `is_wine == false`.
        assert_eq!(
            identity_of(&process(7, "floorp", Some("floorp"))),
            Err("process 7 (floorp) has neither SteamAppId nor WINESERVERSOCKET".into())
        );
    }

    #[test]
    fn wine_system_executables_are_not_games() {
        for argv0 in [
            "C:\\windows\\system32\\explorer.exe",
            "C:\\windows\\system32\\SERVICES.EXE",
            "winedevice.exe",
            "C:\\windows\\system32\\plugplay.exe",
            "C:\\windows\\system32\\svchost.exe",
            "C:\\windows\\system32\\rpcss.exe",
            "C:\\windows\\system32\\conhost.exe",
            "C:\\windows\\command\\start.exe",
            "C:\\windows\\system32\\WineBoot.exe",
            "/usr/lib/wine/x86_64-windows/winecfg.exe",
            "C:\\windows\\system32\\rundll32.exe",
            "C:\\Program Files\\Common Files\\microsoft shared\\ink\\TabTip.exe",
        ] {
            let mut system = process(3, "explorer.exe", Some(argv0));
            system.is_wine = true;
            let reason = identity_of(&system).expect_err(argv0);
            assert!(reason.contains("Wine's own"), "{argv0}: {reason}");
        }
        let mut game = process(3, "Main Thread", Some("C:\\games\\explorers.exe"));
        game.is_wine = true;
        assert!(identity_of(&game).is_ok());
    }

    struct Answer(Option<Result<OwnerLookup, String>>, usize);

    impl WindowOwners for Answer {
        fn owner(&mut self, _: &str, _: &str) -> Result<OwnerLookup, String> {
            self.1 += 1;
            self.0.take().unwrap_or(Ok(OwnerLookup::NoMatch))
        }
    }

    fn window(pid: i32, app_id: Option<&str>, title: Option<&str>) -> WindowView {
        WindowView {
            id: 1,
            app_id: app_id.map(Into::into),
            title: title.map(Into::into),
            pid: Some(pid),
            is_focused: true,
            is_suspended: false,
            is_minimized: false,
        }
    }

    fn table() -> ProcessTable {
        let mut wine = process(40, "Main Thread", Some("C:\\g.exe"));
        wine.is_wine = true;
        ProcessTable::new([
            process(20, "xwayland-satell", None),
            wine,
            process(50, "floorp", Some("floorp")),
        ])
    }

    fn reason(answer: Result<OwnerLookup, String>) -> String {
        let mut owners = Answer(Some(answer), 0);
        let view = window(20, Some("a"), Some("t"));
        detect(&view, &table(), &mut owners).expect_err("not recognized")
    }

    #[test]
    fn a_satellite_window_without_app_id_or_title_is_not_looked_up() {
        let mut owners = Answer(Some(Ok(OwnerLookup::Owner(40))), 0);
        let none = window(20, None, Some("t"));
        assert!(detect(&none, &table(), &mut owners).is_err());
        let none = window(20, Some("a"), None);
        assert!(detect(&none, &table(), &mut owners).is_err());
        assert_eq!(owners.1, 0);
    }

    #[test]
    fn a_window_without_a_usable_pid_is_not_attempted() {
        let mut owners = Answer(None, 0);
        let mut view = window(20, Some("a"), Some("t"));
        view.pid = None;
        assert_eq!(detect(&view, &table(), &mut owners), Ok(None));
        view.pid = Some(-1);
        assert_eq!(detect(&view, &table(), &mut owners), Ok(None));
        assert_eq!(owners.1, 0);
    }

    #[test]
    fn every_non_recognition_says_why() {
        assert_eq!(
            reason(Err("no local X11 display".into())),
            "X11 lookup failed: no local X11 display"
        );
        assert_eq!(
            reason(Ok(OwnerLookup::NoMatch)),
            r#"no X11 window with class "a" and title "t""#
        );
        assert_eq!(
            reason(Ok(OwnerLookup::Ambiguous(vec![1, 2]))),
            r#"2 processes own X11 windows with class "a" and title "t""#
        );
        assert_eq!(
            reason(Ok(OwnerLookup::Owner(99))),
            "process 99 is not running"
        );
        assert_eq!(
            reason(Ok(OwnerLookup::Owner(50))),
            "process 50 (floorp) has neither SteamAppId nor WINESERVERSOCKET"
        );
        let mut owners = Answer(None, 0);
        assert_eq!(
            detect(&window(77, Some("a"), Some("t")), &table(), &mut owners),
            Err("process 77 is not running".into())
        );
    }

    #[test]
    fn a_native_window_never_asks_x11() {
        let mut owners = Answer(Some(Ok(OwnerLookup::Owner(20))), 0);
        let view = window(40, Some("a"), Some("t"));
        assert!(matches!(
            detect(&view, &table(), &mut owners),
            Ok(Some(GameIdentity::Process { .. }))
        ));
        assert_eq!(owners.1, 0);
    }
}
