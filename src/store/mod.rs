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

/// Prefix of temporary directories that are filled before being renamed into place. `zem cache
/// gc` removes the ones an interrupted run left behind, and only once their lock is free.
pub const STAGING_PREFIX: &str = ".zem-staging-";
/// Lock file held inside a staging directory for as long as its producer uses it.
pub const STAGING_LOCK_FILE: &str = ".zem-staging.lock";

/// A temporary directory under a store root. Dropping it removes everything inside, so a
/// failed build leaves nothing behind; publishing moves the built directory out first.
#[derive(Debug)]
pub struct StagedDir {
    // Dropped first, so the directory is removed while the lock file is still held.
    dir: tempfile::TempDir,
    _lock: FileLock,
}

impl StagedDir {
    pub fn new_in(parent: &Path) -> Result<Self> {
        fsutil::create_dir_all(parent)?;
        let dir = tempfile::Builder::new()
            .prefix(STAGING_PREFIX)
            .tempdir_in(parent)
            .map_err(ZemError::io(parent))?;
        let lock = FileLock::exclusive(&dir.path().join(STAGING_LOCK_FILE), "staging directory")?;
        Ok(Self { dir, _lock: lock })
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
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
