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
use crate::store::venv;

/// Everything `zem sync` will create or reuse for a project, resolved without network access.
#[derive(Debug, Clone)]
pub struct SyncPlan {
    pub project: ProjectDir,
    pub zem_home: PathBuf,
    /// The board as written in zem.toml.
    pub board: String,
    /// `None` until sync resolves the board from the Zephyr tree.
    pub target: Option<ResolvedTarget>,
    /// Modules known before the workspace exists. Sync adds the HAL of a target it resolves
    /// from the tree.
    pub modules: BTreeSet<String>,
    pub workspace_key: WorkspaceKey,
    pub workspace_dir: PathBuf,
    /// A branch revision whose commit is not pinned yet; sync resolves it and calls
    /// [`SyncPlan::pin_commit`] so the shared workspace never follows a moving branch.
    pub unresolved_branch: bool,
    pub venv_key: VenvKey,
    pub venv_name: String,
    /// Where `venv_name` lives. The venv sync uses can be a sibling of this one.
    pub venv_dir: PathBuf,
    /// The shared west venv that runs west before and during the workspace update.
    pub west_dir: PathBuf,
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

    let (workspace_key, venv_key) = keys(loaded, workspace_revision);
    let plan = SyncPlan {
        board: cfg.target.board.trim().to_string(),
        modules: project_modules(cfg, validated.target.as_ref()),
        zem_home: home.root().to_path_buf(),
        target: validated.target,
        project,
        workspace_dir: home.workspace_dir(&workspace_key),
        workspace_key,
        unresolved_branch: branch && locked_commit.is_none(),
        west_dir: venv::west_dir(home, &cfg.python.version),
        venv_name: venv_key.dir_name(),
        venv_dir: home.venv_dir(&venv_key),
        venv_key,
        sdk_version: cfg.sdk.version.clone(),
        sdk_dir,
        lock,
        warnings: validated.warnings,
    };
    Ok(plan)
}

fn keys(loaded: &LoadedConfig, revision: &str) -> (WorkspaceKey, VenvKey) {
    let cfg = &loaded.config;
    let workspace = WorkspaceKey::new(&cfg.zephyr.url, revision);
    let venv = VenvKey::new(
        workspace.clone(),
        &cfg.python.version,
        &cfg.python.extra_requirements,
        cfg.python.install_cmake_ninja,
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

    pub fn render(&self) -> String {
        let mut out = String::new();
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
            (
                "board",
                self.target
                    .as_ref()
                    .map_or(self.board.clone(), |t| t.board.clone()),
            ),
            (
                "soc / arch",
                match &self.target {
                    Some(t) => format!("{} / {} (via {})", t.soc, t.arch, t.source.describe()),
                    None => format!(
                        "resolved from the Zephyr tree during sync (supported SoCs: {})",
                        crate::vendor::supported_socs()
                    ),
                },
            ),
            (
                "toolchains",
                self.target.as_ref().map_or_else(
                    || "after the board is resolved".to_string(),
                    |t| t.toolchains.join(", "),
                ),
            ),
            (
                "modules",
                match &self.target {
                    Some(_) => modules.join(", "),
                    None => modules
                        .iter()
                        .copied()
                        .chain(["the board's HAL (added during sync)"])
                        .collect::<Vec<_>>()
                        .join(", "),
                },
            ),
            ("ZEM_HOME", self.zem_home.display().to_string()),
            ("workspace", workspace),
            ("west venv", self.west_dir.display().to_string()),
            ("venv", self.venv_dir.display().to_string()),
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
