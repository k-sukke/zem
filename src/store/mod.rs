//! Shared, content-addressed stores under ZEM_HOME. Each store directory carries a marker
//! file describing what is installed; a store without a valid marker is treated as absent.

pub mod download;
pub mod gc;
pub mod registry;
pub mod sdk;
pub mod uv;
pub mod venv;
pub mod workspace;

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::{Result, ZemError};
use crate::fsutil;
use crate::home::ZemHome;
use crate::lock::FileLock;

pub const MARKER_FILE: &str = ".zem-store.json";
pub const MARKER_SCHEMA: u32 = 1;
/// Prefix of the temporary directories archives are unpacked into before being renamed into
/// place; `zem cache gc` removes the ones an interrupted sync left behind.
pub const EXTRACT_PREFIX: &str = ".zem-extract-";
/// Lock file held inside a staging directory for as long as its producer is using it.
pub const STAGING_LOCK_FILE: &str = ".zem-staging.lock";

/// A temporary staging directory under a store root, guarded by an exclusive lock file so
/// `zem cache gc` never deletes an in-use staging directory regardless of its age.
#[derive(Debug)]
pub struct StagedDir {
    // Dropped first so the directory is removed while the lock fd is still held.
    dir: tempfile::TempDir,
    _lock: FileLock,
}

impl StagedDir {
    pub fn new_in(parent: &Path) -> Result<Self> {
        let dir = tempfile::Builder::new()
            .prefix(EXTRACT_PREFIX)
            .tempdir_in(parent)
            .map_err(ZemError::io(parent))?;
        let lock_path = dir.path().join(STAGING_LOCK_FILE);
        let lock = FileLock::exclusive(&lock_path, "staging directory")?;
        Ok(Self { dir, _lock: lock })
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Prevent automatic deletion on drop and return the staging directory path.
    pub fn keep(self) -> PathBuf {
        self.dir.keep()
    }
}

pub fn marker_path(dir: &Path) -> PathBuf {
    dir.join(MARKER_FILE)
}

/// A missing, corrupt or outdated marker all mean "rebuild".
pub fn read_marker<T: DeserializeOwned>(dir: &Path) -> Option<T> {
    fsutil::read_json(&marker_path(dir)).ok().flatten()
}

pub fn write_marker<T: Serialize>(dir: &Path, marker: &T) -> Result<()> {
    fsutil::write_json(&marker_path(dir), marker)
}

fn lock_path(home: &ZemHome, name: &str) -> PathBuf {
    home.locks_dir().join(format!("{name}.lock"))
}

pub fn lock(home: &ZemHome, name: &str) -> Result<FileLock> {
    let what = name.replace('-', " ");
    FileLock::exclusive(&lock_path(home, name), &what)
}

/// `None` while another zem process is using the store.
pub fn try_lock(home: &ZemHome, name: &str) -> Result<Option<FileLock>> {
    FileLock::try_exclusive(&lock_path(home, name))
}
