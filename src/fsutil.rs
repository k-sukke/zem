use std::fs;
use std::io::{ErrorKind, Write};
use std::path::Path;
use std::thread;
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::{Result, ZemError};

const PERSIST_ATTEMPTS: u32 = 5;

pub fn create_dir_all(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(ZemError::io(path))
}

pub fn remove_dir_all_if_exists(path: &Path) -> Result<()> {
    match fs::remove_dir_all(path) {
        Err(e) if e.kind() != ErrorKind::NotFound => Err(ZemError::io(path)(e)),
        _ => Ok(()),
    }
}

/// Write via a temp file in the same directory and rename it into place, so readers never
/// observe a partially written file.
pub fn atomic_write(path: &Path, contents: &[u8]) -> Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    create_dir_all(dir)?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir).map_err(ZemError::io(dir))?;
    tmp.write_all(contents).map_err(ZemError::io(path))?;

    // Renaming over a file that another process holds open fails transiently on Windows.
    let mut attempt = 0;
    loop {
        match tmp.persist(path) {
            Ok(_) => return Ok(()),
            Err(e) if attempt + 1 < PERSIST_ATTEMPTS => {
                tmp = e.file;
                thread::sleep(Duration::from_millis(50 << attempt));
                attempt += 1;
            }
            Err(e) => return Err(ZemError::io(path)(e.error)),
        }
    }
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| ZemError::CorruptState {
                path: path.to_path_buf(),
                message: e.to_string(),
            }),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(ZemError::io(path)(e)),
    }
}

pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|e| ZemError::CorruptState {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;
    bytes.push(b'\n');
    atomic_write(path, &bytes)
}

/// Total size of the files under `path`, not following symlinks. Unreadable entries count as 0.
pub fn dir_size(path: &Path) -> u64 {
    let mut total = 0;
    let mut pending = vec![path.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            match entry.file_type() {
                Ok(t) if t.is_dir() => pending.push(entry.path()),
                Ok(_) => total += entry.metadata().map(|m| m.len()).unwrap_or(0),
                Err(_) => {}
            }
        }
    }
    total
}

/// Create an empty file if it does not exist yet.
pub fn touch(path: &Path) -> Result<()> {
    if path.is_file() {
        return Ok(());
    }
    atomic_write(path, b"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_round_trip_and_missing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested").join("state.json");
        assert_eq!(read_json::<Vec<u32>>(&path).unwrap(), None);
        write_json(&path, &vec![1u32, 2]).unwrap();
        assert_eq!(read_json::<Vec<u32>>(&path).unwrap(), Some(vec![1, 2]));
    }

    #[test]
    fn corrupt_json_is_reported() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("bad.json");
        fs::write(&path, "{").unwrap();
        assert!(matches!(
            read_json::<Vec<u32>>(&path),
            Err(ZemError::CorruptState { .. })
        ));
    }

    #[test]
    fn atomic_write_replaces_existing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("f");
        atomic_write(&path, b"one").unwrap();
        atomic_write(&path, b"two").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
    }
}
