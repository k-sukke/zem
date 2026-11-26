use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::fsutil;
use crate::home::ZemHome;

pub const REGISTRY_SCHEMA: u32 = 1;

/// Which projects use which stores; consulted by `zem gc` to decide what is unused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registry {
    pub schema: u32,
    #[serde(default)]
    pub projects: BTreeMap<String, ProjectEntry>,
}

impl Default for Registry {
    fn default() -> Self {
        Registry {
            schema: REGISTRY_SCHEMA,
            projects: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectEntry {
    pub workspace: String,
    pub venv: String,
    pub sdk: String,
    pub last_sync: u64,
}

pub fn load(home: &ZemHome) -> Result<Registry> {
    Ok(fsutil::read_json(&home.registry_path())?.unwrap_or_default())
}

pub fn record(home: &ZemHome, project_root: &Path, entry: ProjectEntry) -> Result<()> {
    let _lock = super::lock(home, "registry")?;
    let path: PathBuf = home.registry_path();
    // A corrupt registry only loses usage tracking; never let it block a sync.
    let mut registry = fsutil::read_json::<Registry>(&path)
        .ok()
        .flatten()
        .unwrap_or_default();
    registry
        .projects
        .insert(project_root.to_string_lossy().into_owned(), entry);
    fsutil::write_json(&path, &registry)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(ws: &str) -> ProjectEntry {
        ProjectEntry {
            workspace: ws.into(),
            venv: "py3.12-x".into(),
            sdk: "0.17.2".into(),
            last_sync: 1,
        }
    }

    #[test]
    fn records_and_replaces_projects() {
        let tmp = tempfile::tempdir().unwrap();
        let home = ZemHome::new(tmp.path().to_path_buf());
        record(&home, Path::new("/a"), entry("w1")).unwrap();
        record(&home, Path::new("/b"), entry("w1")).unwrap();
        record(&home, Path::new("/a"), entry("w2")).unwrap();
        let reg = load(&home).unwrap();
        assert_eq!(reg.projects.len(), 2);
        assert_eq!(reg.projects["/a"].workspace, "w2");
    }
}
