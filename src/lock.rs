use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;

use crate::error::{Result, ZemError};
use crate::fsutil;

/// Exclusive inter-process lock, released when dropped.
#[derive(Debug)]
pub struct FileLock {
    _file: File,
}

fn open(path: &Path) -> Result<File> {
    if let Some(parent) = path.parent() {
        fsutil::create_dir_all(parent)?;
    }
    OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)
        .map_err(ZemError::io(path))
}

impl FileLock {
    /// `None` if another process holds the lock.
    pub fn try_exclusive(path: &Path) -> Result<Option<Self>> {
        let file = open(path)?;
        match file.try_lock() {
            Ok(()) => Ok(Some(FileLock { _file: file })),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(e)) => Err(ZemError::io(path)(e)),
        }
    }

    pub fn exclusive(path: &Path, what: &str) -> Result<Self> {
        let file = open(path)?;
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                eprintln!("waiting for another zem process to release {what}...");
                file.lock().map_err(ZemError::io(path))?;
            }
            Err(TryLockError::Error(e)) => return Err(ZemError::io(path)(e)),
        }
        Ok(FileLock { _file: file })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_is_released_on_drop() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("locks").join("x.lock");
        let first = FileLock::exclusive(&path, "x").unwrap();
        let probe = File::open(&path).unwrap();
        assert!(matches!(probe.try_lock(), Err(TryLockError::WouldBlock)));
        assert!(FileLock::try_exclusive(&path).unwrap().is_none());
        drop(first);
        assert!(probe.try_lock().is_ok());
        drop(probe);
        assert!(FileLock::try_exclusive(&path).unwrap().is_some());
    }
}
