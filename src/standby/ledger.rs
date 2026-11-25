//! The private ledger of suspended games in the runtime directory.
//!
//! Melibea writes an entry before freezing a game and removes it after
//! thawing, so recovery can thaw whatever a crash left frozen.

use std::error::Error;
use std::ffi::OsString;
use std::fmt;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::games::GameIdentity;

const LEDGER_DIRECTORY: &str = "melibea";
const LEDGER_FILENAME: &str = "standby.json";
const LEDGER_TEMPORARY: &str = "standby.json.tmp";
const LEDGER_VERSION: u64 = 1;

/// One suspended game.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerEntry {
    pub units: Vec<String>,
    pub windows: Vec<u64>,
    pub identity: GameIdentity,
    pub since_unix: u64,
    pub flagged: bool,
}

#[derive(Serialize, Deserialize)]
struct LedgerFile {
    version: u64,
    entries: Vec<LedgerEntry>,
}

#[derive(Debug)]
pub enum LedgerError {
    NoRuntimeDir,
    Io {
        path: PathBuf,
        source: io::Error,
    },
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
    UnsupportedVersion {
        path: PathBuf,
        version: u64,
    },
}

impl fmt::Display for LedgerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoRuntimeDir => write!(formatter, "XDG_RUNTIME_DIR is not set"),
            Self::Io { path, source } => {
                write!(formatter, "ledger `{}`: {source}", path.display())
            }
            Self::Json { path, source } => {
                write!(
                    formatter,
                    "ledger `{}` is not valid: {source}",
                    path.display()
                )
            }
            Self::UnsupportedVersion { path, version } => write!(
                formatter,
                "ledger `{}` has unsupported version {version}",
                path.display()
            ),
        }
    }
}

impl Error for LedgerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Json { source, .. } => Some(source),
            Self::NoRuntimeDir | Self::UnsupportedVersion { .. } => None,
        }
    }
}

/// Resolves the ledger path from the runtime directory; empty counts as unset.
///
/// # Errors
/// Returns [`LedgerError::NoRuntimeDir`] when the value is absent or empty.
pub fn ledger_path_from(runtime_dir: Option<OsString>) -> Result<PathBuf, LedgerError> {
    let runtime_dir = runtime_dir
        .filter(|value| !value.is_empty())
        .ok_or(LedgerError::NoRuntimeDir)?;
    Ok(PathBuf::from(runtime_dir)
        .join(LEDGER_DIRECTORY)
        .join(LEDGER_FILENAME))
}

/// The ledger file.
#[derive(Clone, Debug)]
pub struct Ledger {
    path: PathBuf,
}

impl Ledger {
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// `$XDG_RUNTIME_DIR/melibea/standby.json`.
    ///
    /// # Errors
    /// Returns [`LedgerError::NoRuntimeDir`] when `XDG_RUNTIME_DIR` is unset or empty.
    pub fn default_path() -> Result<PathBuf, LedgerError> {
        ledger_path_from(std::env::var_os("XDG_RUNTIME_DIR"))
    }

    /// Reads the entries; a missing file means none.
    ///
    /// # Errors
    /// Returns an error when the file cannot be read, is not valid JSON, or has an unknown version.
    pub fn read(&self) -> Result<Vec<LedgerEntry>, LedgerError> {
        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => return Err(self.io(&self.path, source)),
        };
        let file: LedgerFile = serde_json::from_str(&text).map_err(|source| LedgerError::Json {
            path: self.path.clone(),
            source,
        })?;
        if file.version != LEDGER_VERSION {
            return Err(LedgerError::UnsupportedVersion {
                path: self.path.clone(),
                version: file.version,
            });
        }
        Ok(file.entries)
    }

    /// Replaces the ledger with `entries` by temporary file and rename.
    ///
    /// # Errors
    /// Returns an error when the directory, temporary file or rename fails.
    pub fn write(&self, entries: &[LedgerEntry]) -> Result<(), LedgerError> {
        let file = LedgerFile {
            version: LEDGER_VERSION,
            entries: entries.to_vec(),
        };
        let body = serde_json::to_vec(&file).map_err(|source| LedgerError::Json {
            path: self.path.clone(),
            source,
        })?;
        if let Some(directory) = self.path.parent().filter(|d| !d.as_os_str().is_empty()) {
            DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(directory)
                .map_err(|source| self.io(directory, source))?;
        }
        let temporary = self.path.with_file_name(LEDGER_TEMPORARY);
        let mut handle = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|source| self.io(&temporary, source))?;
        handle
            .write_all(&body)
            .and_then(|()| handle.sync_all())
            .map_err(|source| self.io(&temporary, source))?;
        drop(handle);
        fs::rename(&temporary, &self.path).map_err(|source| self.io(&self.path, source))
    }

    #[allow(clippy::unused_self)]
    fn io(&self, path: &Path, source: io::Error) -> LedgerError {
        LedgerError::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("melibea-ledger-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn entry(app_id: u64) -> LedgerEntry {
        LedgerEntry {
            units: vec![format!("app-melibea-game-{app_id}.scope")],
            windows: vec![4, 5],
            identity: GameIdentity::SteamApp { app_id },
            since_unix: 1_700_000_000,
            flagged: true,
        }
    }

    #[test]
    fn entries_round_trip() {
        let dir = scratch("round");
        let ledger = Ledger::at(dir.join("melibea").join("standby.json"));
        let entries = vec![
            entry(7),
            LedgerEntry {
                identity: GameIdentity::Process {
                    name: "game.exe".into(),
                },
                flagged: false,
                ..entry(8)
            },
        ];
        ledger.write(&entries).unwrap();
        assert_eq!(ledger.read().unwrap(), entries);
        ledger.write(&[]).unwrap();
        assert_eq!(
            fs::read_to_string(dir.join("melibea").join("standby.json")).unwrap(),
            r#"{"version":1,"entries":[]}"#
        );
        assert!(ledger.read().unwrap().is_empty());
    }

    #[test]
    fn a_missing_file_reads_empty() {
        let ledger = Ledger::at(scratch("missing").join("standby.json"));
        assert!(ledger.read().unwrap().is_empty());
    }

    #[test]
    fn a_corrupt_file_is_an_error() {
        let dir = scratch("corrupt");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("standby.json");
        fs::write(&path, "{not json").unwrap();
        assert!(matches!(
            Ledger::at(&path).read(),
            Err(LedgerError::Json { .. })
        ));
        fs::write(&path, r#"{"version":2,"entries":[]}"#).unwrap();
        assert!(matches!(
            Ledger::at(&path).read(),
            Err(LedgerError::UnsupportedVersion { version: 2, .. })
        ));
    }

    #[test]
    fn written_files_are_private() {
        let dir = scratch("private");
        let directory = dir.join("melibea");
        let ledger = Ledger::at(directory.join("standby.json"));
        ledger.write(&[entry(1)]).unwrap();
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&directory), 0o700);
        assert_eq!(mode(&directory.join("standby.json")), 0o600);
        assert!(!directory.join("standby.json.tmp").exists());
    }

    #[test]
    fn an_unset_runtime_dir_is_an_error() {
        assert!(matches!(
            ledger_path_from(None),
            Err(LedgerError::NoRuntimeDir)
        ));
        assert!(matches!(
            ledger_path_from(Some(OsString::new())),
            Err(LedgerError::NoRuntimeDir)
        ));
        assert_eq!(
            ledger_path_from(Some("/run/user/1000".into())).unwrap(),
            PathBuf::from("/run/user/1000/melibea/standby.json")
        );
    }
}
