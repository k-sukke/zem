pub mod activate;
mod state;
pub mod west_config;

use std::path::{Path, PathBuf};

pub use state::{ProjectState, STATE_FILE, STATE_SCHEMA, StoreRef, TargetState};

use crate::error::{Result, ZemError};
use crate::home::ZemHome;
use crate::platform;

/// Lightweight per-project state (resolved paths, activation scripts, last log).
/// Everything in it must be small and regenerable offline from ZEM_HOME.
pub const PROJECT_STATE_DIR: &str = ".zem";

#[derive(Debug, Clone)]
pub struct ProjectDir {
    root: PathBuf,
}

impl ProjectDir {
    pub fn new(root: &Path, home: &ZemHome) -> Result<Self> {
        let root = platform::canonicalize(root).map_err(ZemError::io(root))?;
        ensure_not_home(&root)?;
        let state_dir = root.join(PROJECT_STATE_DIR);
        let zem_home = platform::canonicalize_lenient(home.root());
        if zem_home == state_dir {
            return Err(ZemError::ZemHomeCollision { zem_home });
        }
        Ok(ProjectDir { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn state_dir(&self) -> PathBuf {
        self.root.join(PROJECT_STATE_DIR)
    }
}

pub fn ensure_not_home(root: &Path) -> Result<()> {
    let root = platform::canonicalize_lenient(root);
    match platform::home_dir().map(|h| platform::canonicalize_lenient(&h)) {
        Some(home) if home == root => Err(ZemError::ProjectIsHome(root)),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_zem_home_inside_project_state_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let home = ZemHome::new(tmp.path().join(PROJECT_STATE_DIR));
        assert!(matches!(
            ProjectDir::new(tmp.path(), &home),
            Err(ZemError::ZemHomeCollision { .. })
        ));
    }

    #[test]
    fn accepts_separate_zem_home() {
        let tmp = tempfile::tempdir().unwrap();
        let home = ZemHome::new(tmp.path().join(".zem-home"));
        let project = ProjectDir::new(tmp.path(), &home).unwrap();
        assert!(project.state_dir().ends_with(PROJECT_STATE_DIR));
    }

    #[test]
    fn rejects_home_directory() {
        if let Some(home) = platform::home_dir() {
            assert!(matches!(
                ensure_not_home(&home),
                Err(ZemError::ProjectIsHome(_))
            ));
        }
    }
}
