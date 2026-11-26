pub mod keys;

use std::path::{Path, PathBuf};

pub use keys::{VenvKey, WorkspaceKey};

use crate::error::{Result, ZemError};
use crate::platform;

pub const ZEM_HOME_ENV: &str = "ZEM_HOME";

/// The single global root for everything zem downloads or builds outside a project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZemHome {
    root: PathBuf,
}

impl ZemHome {
    /// Canonical, so paths derived from it match what west and CMake report
    /// (e.g. `/tmp` vs `/private/tmp` on macOS).
    pub fn resolve() -> Result<Self> {
        let root = match std::env::var_os(ZEM_HOME_ENV).filter(|v| !v.is_empty()) {
            Some(value) => {
                let path = PathBuf::from(value);
                if path.is_absolute() {
                    path
                } else {
                    std::env::current_dir()
                        .map_err(ZemError::io("."))?
                        .join(path)
                }
            }
            None => platform::default_zem_home()?,
        };
        Ok(Self::new(platform::canonicalize_lenient(&root)))
    }

    pub fn new(root: PathBuf) -> Self {
        ZemHome { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn workspaces_dir(&self) -> PathBuf {
        self.root.join("workspaces")
    }

    pub fn venvs_dir(&self) -> PathBuf {
        self.root.join("venvs")
    }

    pub fn sdk_dir(&self) -> PathBuf {
        self.root.join("sdk")
    }

    pub fn python_dir(&self) -> PathBuf {
        self.root.join("python")
    }

    pub fn tools_dir(&self) -> PathBuf {
        self.root.join("tools")
    }

    pub fn cache_dir(&self) -> PathBuf {
        self.root.join("cache")
    }

    pub fn git_cache_dir(&self) -> PathBuf {
        self.cache_dir().join("git")
    }

    pub fn uv_cache_dir(&self) -> PathBuf {
        self.cache_dir().join("uv")
    }

    pub fn ccache_dir(&self) -> PathBuf {
        self.cache_dir().join("ccache")
    }

    pub fn zephyr_cache_dir(&self) -> PathBuf {
        self.cache_dir().join("zephyr")
    }

    /// Stores are moved here before deletion, so an interrupted delete never leaves a
    /// half-removed store that still has a valid marker.
    pub fn trash_dir(&self) -> PathBuf {
        self.root.join("trash")
    }

    pub fn locks_dir(&self) -> PathBuf {
        self.root.join("locks")
    }

    pub fn westconfig_path(&self) -> PathBuf {
        self.root.join("westconfig")
    }

    pub fn registry_path(&self) -> PathBuf {
        self.root.join("projects.json")
    }

    pub fn config_path(&self) -> PathBuf {
        self.root.join("config.toml")
    }

    pub fn workspace_dir(&self, key: &WorkspaceKey) -> PathBuf {
        self.workspaces_dir().join(key.dir_name())
    }

    pub fn venv_dir(&self, key: &VenvKey) -> PathBuf {
        self.venvs_dir().join(key.dir_name())
    }

    pub fn sdk_install_dir(&self, version: &str) -> PathBuf {
        self.sdk_dir().join(format!("zephyr-sdk-{version}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_is_rooted() {
        let home = ZemHome::new(PathBuf::from("zh"));
        assert_eq!(
            home.git_cache_dir(),
            Path::new("zh").join("cache").join("git")
        );
        assert_eq!(
            home.sdk_install_dir("0.17.4"),
            Path::new("zh").join("sdk").join("zephyr-sdk-0.17.4")
        );
        let ws = WorkspaceKey::new("https://github.com/zephyrproject-rtos/zephyr", "v4.2.0");
        assert!(home.workspace_dir(&ws).starts_with(home.workspaces_dir()));
    }
}
