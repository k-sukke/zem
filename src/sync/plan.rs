use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::PathBuf;

use crate::board::ResolvedTarget;
use crate::config::{LoadedConfig, RevisionKind, SdkVersion, validate};
use crate::error::Result;
use crate::home::{VenvKey, WorkspaceKey, ZemHome};
use crate::lockfile::{self, Lockfile};
use crate::manifest::project_modules;
use crate::project::ProjectDir;
use crate::store::{uv, venv};

/// Everything `zem sync` will create or reuse for a project, resolved without network access.
#[derive(Debug, Clone)]
pub struct SyncPlan {
    pub project: ProjectDir,
    pub zem_home: PathBuf,
    pub target: ResolvedTarget,
    pub modules: BTreeSet<String>,
    pub workspace_key: WorkspaceKey,
    pub workspace_dir: PathBuf,
    /// A branch revision whose commit is not pinned yet; sync resolves it and calls
    /// [`SyncPlan::pin_commit`] so the shared workspace never follows a moving branch.
    pub unresolved_branch: bool,
    pub venv_key: VenvKey,
    pub venv_name: String,
    pub venv_dir: PathBuf,
    pub sdk_version: SdkVersion,
    pub sdk_dir: Option<PathBuf>,
    pub lock: Option<Lockfile>,
    pub warnings: Vec<String>,
}

pub fn plan(loaded: &LoadedConfig, home: &ZemHome) -> Result<SyncPlan> {
    let cfg = &loaded.config;
    let validated = validate(cfg)?;
    let project = ProjectDir::new(&loaded.root, home)?;
    let lock = lockfile::read(&loaded.root)?;

    let branch = matches!(validated.revision, RevisionKind::Branch);
    let locked_commit = lock
        .as_ref()
        .filter(|l| l.zephyr.matches(cfg))
        .map(|l| l.zephyr.commit.as_str());
    let workspace_revision = match locked_commit {
        Some(commit) if branch => commit,
        _ => &cfg.zephyr.version,
    };
    let sdk_dir = match &cfg.sdk.version {
        SdkVersion::Exact(v) => Some(home.sdk_install_dir(v)),
        SdkVersion::Auto => None,
    };

    let locked_packages = lock
        .as_ref()
        .filter(|l| l.zephyr.matches(cfg) && l.python.matches(&cfg.python))
        .map(|l| l.python.packages.clone());
    let (workspace_key, venv_key) = keys(loaded, workspace_revision);
    let mut plan = SyncPlan {
        modules: project_modules(cfg, &validated.target),
        zem_home: home.root().to_path_buf(),
        target: validated.target,
        project,
        workspace_dir: home.workspace_dir(&workspace_key),
        workspace_key,
        unresolved_branch: branch && locked_commit.is_none(),
        venv_name: venv_key.dir_name(),
        venv_dir: home.venv_dir(&venv_key),
        venv_key,
        sdk_version: cfg.sdk.version.clone(),
        sdk_dir,
        lock,
        warnings: validated.warnings,
    };
    plan.select_venv(home, locked_packages.as_deref());
    Ok(plan)
}

fn keys(loaded: &LoadedConfig, revision: &str) -> (WorkspaceKey, VenvKey) {
    let cfg = &loaded.config;
    let workspace = WorkspaceKey::new(&cfg.zephyr.url, revision);
    let venv = VenvKey::new(
        workspace.clone(),
        &cfg.python.version,
        &cfg.python.extra_requirements,
        cfg.python.host_tools_via_pip,
    );
    (workspace, venv)
}

impl SyncPlan {
    /// Key the workspace (and its venv) by the commit a branch currently points to.
    pub fn pin_commit(&mut self, loaded: &LoadedConfig, home: &ZemHome, commit: &str) {
        let (workspace_key, venv_key) = keys(loaded, commit);
        self.workspace_dir = home.workspace_dir(&workspace_key);
        self.workspace_key = workspace_key;
        self.venv_name = venv_key.dir_name();
        self.venv_dir = home.venv_dir(&venv_key);
        self.venv_key = venv_key;
        self.unresolved_branch = false;
    }

    /// Pick the shared venv, or a sibling dedicated to `packages` when the shared one already
    /// exists without matching recorded packages, so locked projects never reinstall packages
    /// under another project sharing the same venv key.
    pub fn select_venv(&mut self, home: &ZemHome, packages: Option<&[String]>) {
        self.venv_name = self.venv_key.dir_name();
        self.venv_dir = home.venv_dir(&self.venv_key);
        if let Some(packages) = packages
            && self.venv_dir.exists()
            && venv::installed_packages(&self.venv_dir).as_deref() != Some(packages)
        {
            let digest = lockfile::packages_digest(packages);
            self.venv_name = format!("{}-{}", self.venv_name, &digest[..8]);
            self.venv_dir = home.venvs_dir().join(&self.venv_name);
        }
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        let t = &self.target;
        let modules: Vec<_> = self.modules.iter().map(String::as_str).collect();
        let sdk = match (&self.sdk_version, &self.sdk_dir) {
            (SdkVersion::Exact(v), Some(dir)) => format!("{v} ({})", dir.display()),
            _ => "auto (from zephyr/SDK_VERSION)".to_string(),
        };
        let workspace = if self.unresolved_branch {
            format!(
                "resolved from `{}` during sync",
                self.workspace_key.revision()
            )
        } else {
            self.workspace_dir.display().to_string()
        };
        let lock = match &self.lock {
            Some(lock) => format!("{} ({})", lockfile::LOCK_FILE, short(&lock.zephyr.commit)),
            None => "none (created by sync)".to_string(),
        };
        let rows = [
            ("project", self.project.root().display().to_string()),
            ("board", t.board.clone()),
            (
                "soc / arch",
                format!(
                    "{} / {} (via {})",
                    t.soc.name(),
                    t.arch,
                    t.source.describe()
                ),
            ),
            ("toolchains", t.toolchains.join(", ")),
            ("modules", modules.join(", ")),
            ("ZEM_HOME", self.zem_home.display().to_string()),
            ("workspace", workspace),
            ("venv", self.venv_dir.display().to_string()),
            ("uv", format!("{} (managed by zem)", uv::UV_VERSION)),
            ("sdk", sdk),
            ("lock", lock),
            (
                "project state",
                self.project.state_dir().display().to_string(),
            ),
        ];
        for (label, value) in rows {
            let _ = writeln!(out, "  {label:<14} {value}");
        }
        out
    }
}

pub fn short(sha: &str) -> &str {
    &sha[..sha.len().min(12)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;
    use crate::store::venv::VenvMarker;
    use crate::store::{MARKER_SCHEMA, write_marker};

    #[test]
    fn select_venv_uses_dedicated_when_shared_marker_omits_packages() {
        let tmp = tempfile::tempdir().unwrap();
        let home = ZemHome::new(tmp.path().join("home"));
        let proj = tmp.path().join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(
            proj.join(config::CONFIG_FILE_NAME),
            "schema = 1\n[project]\nname = \"p\"\n[zephyr]\nversion = \"v4.2.0\"\n\
             [target]\nboard = \"esp32c3_devkitm\"\n",
        )
        .unwrap();
        let loaded = config::load(&proj).unwrap();
        let mut plan = plan(&loaded, &home).unwrap();
        let shared_name = plan.venv_key.dir_name();
        let shared_dir = home.venv_dir(&plan.venv_key);
        let locked = vec!["west==1.4.0".to_string()];

        // Absent shared venv can be claimed by a locked project.
        plan.select_venv(&home, Some(&locked));
        assert_eq!(plan.venv_name, shared_name);

        // Existing shared venv whose marker omits `packages` must not be replaced by a locked project.
        std::fs::create_dir_all(&shared_dir).unwrap();
        write_marker(
            &shared_dir,
            &VenvMarker {
                schema: MARKER_SCHEMA,
                installer: "uv".to_string(),
                python: "3.12".to_string(),
                bootstrap: vec!["west".to_string()],
                requirements_sha256: Some("abc".to_string()),
                packages: None,
            },
        )
        .unwrap();
        plan.select_venv(&home, Some(&locked));
        assert_ne!(plan.venv_name, shared_name);
        assert!(plan.venv_name.starts_with(&format!("{shared_name}-")));

        // Matching recorded packages allow reusing the shared venv.
        write_marker(
            &shared_dir,
            &VenvMarker {
                schema: MARKER_SCHEMA,
                installer: "uv".to_string(),
                python: "3.12".to_string(),
                bootstrap: vec!["west".to_string()],
                requirements_sha256: Some("abc".to_string()),
                packages: Some(locked.clone()),
            },
        )
        .unwrap();
        plan.select_venv(&home, Some(&locked));
        assert_eq!(plan.venv_name, shared_name);
    }
}
