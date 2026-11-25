//! Game rules from `games.toml` and the set of processes a game owns.

use std::collections::BTreeSet;
use std::error::Error;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use regex::Regex;
use serde::{Deserialize, Serialize};

use super::process::{ProcessTable, exe_basename};
use crate::config::ConfigPathError;

const CONFIG_DIRECTORY: &str = "melibea";
const GAMES_FILENAME: &str = "games.toml";
const STEAM_WINDOW_PREFIX: &str = "steam_app_";

/// What makes a set of processes one game.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GameIdentity {
    /// Processes whose `argv[0]` names this executable (ASCII-lowercase).
    Process { name: String },
    /// Processes whose environment holds `SteamAppId=<app_id>`.
    SteamApp { app_id: u64 },
}

/// A window recognized as a game.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecognizedGame {
    pub identity: GameIdentity,
    /// Minimized time after which the game is suspended, when the rule sets one.
    pub standby_after: Option<Duration>,
}

#[derive(Clone, Debug)]
struct GameRule {
    app_id: Option<Regex>,
    title: Option<Regex>,
    target: RuleTarget,
    standby_after: Option<Duration>,
}

#[derive(Clone, Debug)]
enum RuleTarget {
    Process(String),
    SteamApp(u64),
    Window,
}

/// Validated `[[game]]` rules in resolution order.
#[derive(Clone, Debug, Default)]
pub struct GameRules {
    rules: Vec<GameRule>,
}

impl GameRules {
    /// Parses and validates `games.toml` text.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid TOML, unknown fields or an invalid rule.
    pub fn parse(input: &str) -> Result<Self, GamesError> {
        let raw: RawGames = toml::from_str(input).map_err(GamesError::Toml)?;
        let rules = raw
            .game
            .into_iter()
            .enumerate()
            .map(|(index, value)| {
                let raw = value
                    .try_into::<RawGame>()
                    .map_err(|error| GamesError::Rule {
                        number: index + 1,
                        message: error.message().to_string(),
                    })?;
                GameRule::try_from_raw(index, raw)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { rules })
    }

    /// Reads rules from `path`; a missing file means no rules.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read or is invalid.
    pub fn load(path: &Path) -> Result<Self, GamesError> {
        let input = match fs::read_to_string(path) {
            Ok(input) => input,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => {
                return Err(GamesError::Read {
                    path: path.to_owned(),
                    source,
                });
            }
        };
        Self::parse(&input).map_err(|error| GamesError::AtPath {
            path: path.to_owned(),
            source: Box::new(error),
        })
    }

    /// Recognizes a window: the first matching rule, else a `steam_app_<N>` app id.
    #[must_use]
    pub fn recognize(&self, app_id: Option<&str>, title: Option<&str>) -> Option<RecognizedGame> {
        let window_steam = app_id.and_then(steam_app_id);
        let steam = |app_id| GameIdentity::SteamApp { app_id };
        if let Some(rule) = self.rules.iter().find(|rule| rule.matches(app_id, title)) {
            let identity = match &rule.target {
                RuleTarget::Process(name) => GameIdentity::Process { name: name.clone() },
                RuleTarget::SteamApp(id) => steam(*id),
                RuleTarget::Window => steam(window_steam?),
            };
            return Some(RecognizedGame {
                identity,
                standby_after: rule.standby_after,
            });
        }
        window_steam.map(|id| RecognizedGame {
            identity: steam(id),
            standby_after: None,
        })
    }
}

impl GameRule {
    fn matches(&self, app_id: Option<&str>, title: Option<&str>) -> bool {
        let ok = |pattern: &Option<Regex>, value: Option<&str>| {
            pattern
                .as_ref()
                .is_none_or(|regex| value.is_some_and(|text| regex.is_match(text)))
        };
        ok(&self.app_id, app_id) && ok(&self.title, title)
    }

    fn try_from_raw(index: usize, raw: RawGame) -> Result<Self, GamesError> {
        let error = |message: String| GamesError::Rule {
            number: index + 1,
            message,
        };
        if raw.app_id.is_none() && raw.title.is_none() {
            return Err(error(
                "at least one of `app_id` or `title` is required".into(),
            ));
        }
        let compile = |field: &str, pattern: Option<String>| {
            pattern
                .map(|value| {
                    Regex::new(&value).map_err(|e| error(format!("invalid `{field}` regex: {e}")))
                })
                .transpose()
        };
        let app_id = compile("app_id", raw.app_id)?;
        let title = compile("title", raw.title)?;
        let target = match (raw.process, raw.steam_app_id) {
            (Some(_), Some(_)) => {
                return Err(error(
                    "`process` and `steam_app_id` are mutually exclusive".into(),
                ));
            }
            (Some(process), None) => {
                if process.is_empty() || process.contains(['/', '\\']) {
                    return Err(error(
                        "`process` must be a non-empty name without `/` or `\\`".into(),
                    ));
                }
                RuleTarget::Process(process.to_ascii_lowercase())
            }
            (None, Some(0)) => return Err(error("`steam_app_id` must be greater than 0".into())),
            (None, Some(id)) => RuleTarget::SteamApp(id),
            (None, None) => RuleTarget::Window,
        };
        let standby_after = raw
            .standby_after
            .map(|text| {
                parse_delay(&text).map_err(|e| error(format!("invalid `standby_after`: {e}")))
            })
            .transpose()?;
        Ok(Self {
            app_id,
            title,
            target,
            standby_after,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGames {
    #[serde(default)]
    game: Vec<toml::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGame {
    app_id: Option<String>,
    title: Option<String>,
    process: Option<String>,
    steam_app_id: Option<u64>,
    standby_after: Option<String>,
}

/// Returns the default `games.toml` path.
///
/// # Errors
///
/// Returns an error when neither `XDG_CONFIG_HOME` nor `HOME` is usable.
pub fn default_games_path() -> Result<PathBuf, ConfigPathError> {
    games_path_from(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )
}

/// Resolves the `games.toml` path from the two environment values; empty
/// values count as unset.
///
/// # Errors
///
/// Returns an error when both values are unset or empty.
pub fn games_path_from(
    xdg_config_home: Option<OsString>,
    home: Option<OsString>,
) -> Result<PathBuf, ConfigPathError> {
    if let Some(base) = xdg_config_home.filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(base)
            .join(CONFIG_DIRECTORY)
            .join(GAMES_FILENAME));
    }
    if let Some(home) = home.filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(home)
            .join(".config")
            .join(CONFIG_DIRECTORY)
            .join(GAMES_FILENAME));
    }
    Err(ConfigPathError)
}

/// Returns `N` for an app id `steam_app_<N>` with `N` a positive decimal.
#[must_use]
pub fn steam_app_id(app_id: &str) -> Option<u64> {
    let digits = app_id.strip_prefix(STEAM_WINDOW_PREFIX)?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok().filter(|id| *id > 0)
}

/// Parses `<u64>s`, `<u64>m` or `<u64>h`.
///
/// # Errors
///
/// Returns a description when the text has another shape or overflows.
pub fn parse_delay(text: &str) -> Result<Duration, String> {
    let invalid = || format!("`{text}` is not a delay like `90s`, `5m` or `1h`");
    let unit = text.chars().last().ok_or_else(invalid)?;
    let multiplier: u64 = match unit {
        's' => 1,
        'm' => 60,
        'h' => 3600,
        _ => return Err(invalid()),
    };
    let digits = &text[..text.len() - 1];
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid());
    }
    let count: u64 = digits.parse().map_err(|_| invalid())?;
    count
        .checked_mul(multiplier)
        .map(Duration::from_secs)
        .ok_or_else(invalid)
}

/// Returns the processes matching `identity` and all their descendants.
#[must_use]
pub fn game_set(table: &ProcessTable, identity: &GameIdentity) -> BTreeSet<u32> {
    let roots: BTreeSet<u32> = table
        .iter()
        .filter(|process| match identity {
            GameIdentity::Process { name } => process
                .argv0
                .as_deref()
                .is_some_and(|argv0| exe_basename(argv0).eq_ignore_ascii_case(name)),
            GameIdentity::SteamApp { app_id } => process.steam_app_id == Some(*app_id),
        })
        .map(|process| process.pid)
        .collect();
    table.with_descendants(&roots)
}

/// A `games.toml` loading or validation error.
#[derive(Debug)]
pub enum GamesError {
    /// The file could not be read.
    Read { path: PathBuf, source: io::Error },
    /// A path adds context to a parsing or validation error.
    AtPath { path: PathBuf, source: Box<Self> },
    /// TOML syntax or shape was invalid.
    Toml(toml::de::Error),
    /// One rule was invalid.
    Rule { number: usize, message: String },
}

impl fmt::Display for GamesError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => {
                write!(formatter, "could not read `{}`: {source}", path.display())
            }
            Self::AtPath { path, source } => {
                write!(
                    formatter,
                    "invalid games file `{}`: {source}",
                    path.display()
                )
            }
            Self::Toml(error) => write!(formatter, "invalid TOML: {error}"),
            Self::Rule { number, message } => write!(formatter, "rule {number}: {message}"),
        }
    }
}

impl Error for GamesError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::AtPath { source, .. } => Some(source),
            Self::Toml(error) => Some(error),
            Self::Rule { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::standby::process::{ProcessInfo, ProcessTable};
    use std::collections::BTreeSet;
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::time::Duration;

    fn rules(text: &str) -> GameRules {
        GameRules::parse(text).expect("valid rules")
    }

    fn proc(id: u32, parent: u32, argv0: Option<&str>, steam: Option<u64>) -> ProcessInfo {
        ProcessInfo {
            pid: id,
            ppid: parent,
            uid: 1000,
            comm: "x".to_string(),
            argv0: argv0.map(str::to_string),
            cgroup: String::new(),
            steam_app_id: steam,
        }
    }

    fn steam(app_id: u64) -> GameIdentity {
        GameIdentity::SteamApp { app_id }
    }

    #[test]
    fn a_steam_window_is_a_game_without_rules() {
        let none = GameRules::default();
        let game = none
            .recognize(Some("steam_app_1158310"), Some("t"))
            .unwrap();
        assert_eq!(game.identity, steam(1_158_310));
        assert_eq!(game.standby_after, None);
        assert!(none.recognize(Some("steam_app_12x"), None).is_none());
        assert!(none.recognize(Some("steam_proton"), None).is_none());
        assert!(none.recognize(None, None).is_none());
    }

    #[test]
    fn the_first_matching_rule_wins() {
        let r = rules(
            "[[game]]\ntitle='^Victoria 3$'\nprocess='a.exe'\n\
             [[game]]\ntitle='^Victoria 3$'\nprocess='b.exe'\n",
        );
        let game = r
            .recognize(Some("steam_proton"), Some("Victoria 3"))
            .unwrap();
        assert_eq!(
            game.identity,
            GameIdentity::Process {
                name: "a.exe".into()
            }
        );
    }

    #[test]
    fn a_rule_names_the_process_case_insensitively() {
        let r = rules("[[game]]\ntitle='^V$'\nprocess='Victoria3.EXE'\nstandby_after='5m'\n");
        let game = r.recognize(None, Some("V")).unwrap();
        assert_eq!(game.standby_after, Some(Duration::from_secs(300)));
        let identity = game.identity;
        assert_eq!(
            identity,
            GameIdentity::Process {
                name: "victoria3.exe".into()
            }
        );
        let table = ProcessTable::new([
            proc(1, 0, Some("start.exe"), None),
            proc(2, 1, Some("Z:\\x\\VICTORIA3.exe"), None),
            proc(3, 2, None, None),
            proc(4, 0, Some("start.exe"), None),
        ]);
        let mut start = proc(5, 0, Some("start.exe"), None);
        start.comm = "victoria3.exe".into();
        let table2 = ProcessTable::new(table.iter().cloned().chain([start]));
        assert_eq!(game_set(&table2, &identity), BTreeSet::from([2, 3]));
    }

    #[test]
    fn a_rule_without_process_needs_a_steam_window() {
        let r = rules("[[game]]\ntitle='^X$'\nstandby_after='90s'\n");
        let game = r.recognize(Some("steam_app_5"), Some("X")).unwrap();
        assert_eq!(game.identity, steam(5));
        assert_eq!(game.standby_after, Some(Duration::from_secs(90)));
        assert!(r.recognize(Some("foo"), Some("X")).is_none());
    }

    #[test]
    fn a_rule_names_a_native_steam_game() {
        let r = rules("[[game]]\napp_id='^dotnet$'\ntitle='^Terraria'\nsteam_app_id=1281930\n");
        let game = r
            .recognize(Some("dotnet"), Some("Terraria: ¡Cava y calla!"))
            .unwrap();
        assert_eq!(game.identity, steam(1_281_930));
    }

    #[test]
    fn steam_game_set_follows_the_environment() {
        let table = ProcessTable::new([
            proc(1, 0, Some("steam"), None),
            proc(2, 1, Some("game"), Some(5)),
            proc(3, 2, Some("helper"), None),
            proc(4, 1, Some("other"), Some(6)),
        ]);
        assert_eq!(game_set(&table, &steam(5)), BTreeSet::from([2, 3]));
    }

    #[test]
    fn rules_are_validated() {
        let ok = "[[game]]\ntitle='a'\nprocess='a.exe'\n";
        for bad in [
            "[[game]]\nprocess='a.exe'\n",
            "[[game]]\ntitle='('\n",
            "[[game]]\ntitle='a'\nprocess=''\n",
            "[[game]]\ntitle='a'\nprocess='a/b'\n",
            "[[game]]\ntitle='a'\nprocess='a\\b'\n",
            "[[game]]\ntitle='a'\nprocess='a'\nsteam_app_id=5\n",
            "[[game]]\ntitle='a'\nsteam_app_id=0\n",
            "[[game]]\ntitle='a'\nstandby_after='5x'\n",
            "[[game]]\ntitle='a'\nbogus=1\n",
        ] {
            let error = GameRules::parse(&format!("{ok}{bad}")).unwrap_err();
            assert!(error.to_string().contains("rule 2"), "{bad}: {error}");
        }
    }

    #[test]
    fn delays_parse() {
        assert_eq!(parse_delay("0s"), Ok(Duration::ZERO));
        assert_eq!(parse_delay("90s"), Ok(Duration::from_secs(90)));
        assert_eq!(parse_delay("5m"), Ok(Duration::from_secs(300)));
        assert_eq!(parse_delay("1h"), Ok(Duration::from_secs(3600)));
        for bad in ["5", "m", "-1s", "", "5x", "1.5m", "99999999999999999999h"] {
            assert!(parse_delay(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_missing_file_means_no_rules() {
        let dir = std::env::temp_dir().join(format!("melibea-games-{}", std::process::id()));
        let loaded = GameRules::load(&dir.join("absent.toml")).unwrap();
        assert!(loaded.recognize(Some("steam_proton"), Some("t")).is_none());
        assert!(loaded.recognize(Some("steam_app_1"), None).is_some());
    }

    #[test]
    fn a_present_file_loads_and_an_invalid_one_names_its_path() {
        let dir = std::env::temp_dir().join(format!("melibea-games-load-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let good = dir.join("games.toml");
        std::fs::write(&good, "[[game]]\ntitle='^X$'\nprocess='x.exe'\n").unwrap();
        assert!(
            GameRules::load(&good)
                .unwrap()
                .recognize(None, Some("X"))
                .is_some()
        );
        let bad = dir.join("bad.toml");
        std::fs::write(&bad, "[[game]]\n").unwrap();
        assert!(
            GameRules::load(&bad)
                .unwrap_err()
                .to_string()
                .contains("bad.toml")
        );
    }

    #[test]
    fn the_example_file_parses() {
        let r = rules(include_str!("../../games.example.toml"));
        let victoria = r
            .recognize(Some("steam_proton"), Some("Victoria 3"))
            .unwrap();
        assert_eq!(
            victoria.identity,
            GameIdentity::Process {
                name: "victoria3.exe".into()
            }
        );
        assert_eq!(victoria.standby_after, Some(Duration::from_secs(300)));
        let tml = r.recognize(Some("dotnet"), Some("Terraria")).unwrap();
        assert_eq!(tml.identity, steam(1_281_930));
    }

    #[test]
    fn the_path_follows_xdg_then_home() {
        let os = |s: &str| Some(OsString::from(s));
        assert_eq!(
            games_path_from(os("/x"), os("/h")),
            Ok(PathBuf::from("/x/melibea/games.toml"))
        );
        assert_eq!(
            games_path_from(os(""), os("/h")),
            Ok(PathBuf::from("/h/.config/melibea/games.toml"))
        );
        assert_eq!(games_path_from(None, os("")), Err(ConfigPathError));
    }

    #[test]
    fn steam_app_ids_are_decimal() {
        assert_eq!(steam_app_id("steam_app_7"), Some(7));
        assert_eq!(steam_app_id("steam_app_"), None);
        assert_eq!(steam_app_id("steam_app_+7"), None);
        assert_eq!(steam_app_id("steam_app_0"), None);
    }
}
