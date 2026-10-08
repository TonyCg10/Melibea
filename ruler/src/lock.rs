//! One ruler at a time: a second one would capture the first one's guides.
//!
//! `flock` rather than a pid file, because the kernel drops the lock when the process dies, so
//! a crashed ruler never leaves the next one locked out.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;

pub fn acquire() -> Result<Option<File>, String> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").ok_or("XDG_RUNTIME_DIR is not set")?;
    acquire_in(Path::new(&dir))
}

pub fn acquire_in(dir: &Path) -> Result<Option<File>, String> {
    let path = dir.join("melibea-ruler.lock");
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(e)) => Err(format!("cannot lock {}: {e}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_ruler_is_turned_away_until_the_first_is_gone() {
        let dir = std::env::temp_dir().join(format!("melibea-ruler-lock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let first = acquire_in(&dir).unwrap();
        assert!(first.is_some());
        assert!(acquire_in(&dir).unwrap().is_none());

        drop(first);
        assert!(acquire_in(&dir).unwrap().is_some());

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
