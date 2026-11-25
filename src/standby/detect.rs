//! Recognizing a game without a rule: find the process that owns the window
//! and read what its environment says about it.

use super::controller::WindowView;
use super::games::GameIdentity;
use super::process::{ProcessInfo, ProcessTable, exe_basename};
use super::x11::{OwnerLookup, X11Client};

/// `comm` of `xwayland-satellite`, cut at 15 bytes by the kernel.
const SATELLITE_COMM: &str = "xwayland-satell";

/// What a process's environment says about it: a Steam game, a Wine or Proton
/// process, or nothing. `WINEPREFIX` is no marker: the session exports it to
/// unrelated programs.
#[must_use]
pub fn identity_of(owner: &ProcessInfo) -> Option<GameIdentity> {
    if let Some(app_id) = owner.steam_app_id.filter(|id| *id > 0) {
        return Some(GameIdentity::SteamApp { app_id });
    }
    if owner.is_wine {
        let argv0 = owner.argv0.as_deref()?;
        return Some(GameIdentity::Process {
            name: exe_basename(argv0).to_ascii_lowercase(),
        });
    }
    None
}

/// Who owns the X11 window with this app id and title.
pub trait WindowOwners {
    /// `None` when X11 cannot be asked (no display, no server, a bad reply).
    fn owner(&mut self, app_id: &str, title: &str) -> Option<OwnerLookup>;
}

/// The real X server, connected to afresh for every lookup.
#[derive(Clone, Copy, Debug, Default)]
pub struct LiveX11;

impl WindowOwners for LiveX11 {
    fn owner(&mut self, app_id: &str, title: &str) -> Option<OwnerLookup> {
        X11Client::connect_from_env()
            .and_then(|mut client| client.window_owner(app_id, title))
            .ok()
    }
}

/// Finds the game `window` belongs to from its owner's environment.
///
/// A window of `xwayland-satellite` is looked up through X11; any other
/// window is owned by the process niri reports.
///
/// # Errors
///
/// Returns a description when several processes own matching X11 windows.
pub fn detect(
    window: &WindowView,
    table: &ProcessTable,
    owners: &mut dyn WindowOwners,
) -> Result<Option<GameIdentity>, String> {
    let Some(niri_pid) = window.pid.and_then(|pid| u32::try_from(pid).ok()) else {
        return Ok(None);
    };
    let Some(niri_process) = table.get(niri_pid) else {
        return Ok(None);
    };
    let owner_pid = if niri_process.comm == SATELLITE_COMM {
        let (Some(app_id), Some(title)) = (window.app_id.as_deref(), window.title.as_deref())
        else {
            return Ok(None);
        };
        match owners.owner(app_id, title) {
            Some(OwnerLookup::Owner(pid)) => pid,
            Some(OwnerLookup::Ambiguous(pids)) => {
                return Err(format!(
                    "{} processes own windows named like this one",
                    pids.len()
                ));
            }
            Some(OwnerLookup::NoMatch) | None => return Ok(None),
        }
    } else {
        niri_pid
    };
    Ok(table.get(owner_pid).and_then(identity_of))
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
            Some(GameIdentity::SteamApp { app_id: 1_281_930 })
        );
        game.steam_app_id = Some(0);
        assert_eq!(identity_of(&game), None);
    }

    #[test]
    fn a_wine_process_is_a_game_by_its_exe() {
        let mut game = process(1, "Main Thread", Some("Z:\\mnt\\JUEGOS\\Victoria3.EXE"));
        game.is_wine = true;
        assert_eq!(
            identity_of(&game),
            Some(GameIdentity::Process {
                name: "victoria3.exe".into()
            })
        );
        game.steam_app_id = Some(529_340);
        assert_eq!(
            identity_of(&game),
            Some(GameIdentity::SteamApp { app_id: 529_340 })
        );
    }

    #[test]
    fn wineprefix_alone_is_not_a_game() {
        // A browser carrying WINEPREFIX has `is_wine == false`.
        assert_eq!(identity_of(&process(1, "floorp", Some("floorp"))), None);
    }

    struct Answer(Option<OwnerLookup>, usize);

    impl WindowOwners for Answer {
        fn owner(&mut self, _: &str, _: &str) -> Option<OwnerLookup> {
            self.1 += 1;
            self.0.take()
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
        ProcessTable::new([process(20, "xwayland-satell", None), wine])
    }

    #[test]
    fn a_satellite_window_without_app_id_or_title_is_not_looked_up() {
        let mut owners = Answer(Some(OwnerLookup::Owner(40)), 0);
        let none = window(20, None, Some("t"));
        assert_eq!(detect(&none, &table(), &mut owners), Ok(None));
        let none = window(20, Some("a"), None);
        assert_eq!(detect(&none, &table(), &mut owners), Ok(None));
        assert_eq!(owners.1, 0);
    }

    #[test]
    fn an_owner_missing_from_the_table_is_not_a_game() {
        let mut owners = Answer(Some(OwnerLookup::Owner(99)), 0);
        let view = window(20, Some("a"), Some("t"));
        assert_eq!(detect(&view, &table(), &mut owners), Ok(None));
    }

    #[test]
    fn a_native_window_never_asks_x11() {
        let mut owners = Answer(Some(OwnerLookup::Owner(20)), 0);
        let view = window(40, Some("a"), Some("t"));
        assert!(matches!(
            detect(&view, &table(), &mut owners),
            Ok(Some(GameIdentity::Process { .. }))
        ));
        assert_eq!(owners.1, 0);
    }
}
