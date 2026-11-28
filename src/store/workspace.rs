use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{MARKER_SCHEMA, read_marker, write_marker};
use crate::config::classify_revision;
use crate::env::EnvBuilder;
use crate::error::{Result, ZemError};
use crate::fsutil;
use crate::home::WorkspaceKey;
use crate::manifest::{MANIFEST_DIR, WestManifest, render_west_yml};
use crate::platform;
use crate::process::Runner;

/// Workspaces are shared between projects: the revision never changes and modules are only
/// ever added, so a sync for one project cannot break another project's build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceMarker {
    pub schema: u32,
    pub url: String,
    pub revision: String,
    pub modules: BTreeSet<String>,
    #[serde(default)]
    pub blobs_fetched: BTreeSet<String>,
}

pub struct WorkspaceStore<'a> {
    pub dir: &'a Path,
    pub key: &'a WorkspaceKey,
    pub west: &'a Path,
    pub env: &'a EnvBuilder,
    pub runner: &'a Runner,
}

#[derive(Debug, Clone)]
pub struct WorkspaceState {
    pub zephyr_base: PathBuf,
    /// Canonical paths of every project in the workspace, keyed by west project name.
    pub projects: BTreeMap<String, PathBuf>,
    pub added: Vec<String>,
    pub updated: bool,
}

pub fn merged_modules(
    marker: Option<&WorkspaceMarker>,
    requested: &BTreeSet<String>,
) -> BTreeSet<String> {
    marker
        .map(|m| m.modules.union(requested).cloned().collect())
        .unwrap_or_else(|| requested.clone())
}

impl WorkspaceStore<'_> {
    fn west(&self) -> std::process::Command {
        let mut cmd = self.env.command(self.west);
        cmd.current_dir(self.dir);
        cmd
    }

    fn marker(&self) -> Result<Option<WorkspaceMarker>> {
        let Some(marker) =
            read_marker::<WorkspaceMarker>(self.dir).filter(|m| m.schema == MARKER_SCHEMA)
        else {
            return Ok(None);
        };
        if marker.url != self.key.url() || marker.revision != self.key.revision() {
            return Err(ZemError::CorruptState {
                path: super::marker_path(self.dir),
                message: format!(
                    "workspace belongs to {} @ {}, expected {} @ {}",
                    marker.url,
                    marker.revision,
                    self.key.url(),
                    self.key.revision()
                ),
            });
        }
        Ok(Some(marker))
    }

    /// Callers must hold the workspace lock.
    pub fn ensure(
        &self,
        requested: &BTreeSet<String>,
        force_update: bool,
    ) -> Result<WorkspaceState> {
        let previous = self.marker()?;
        let modules = merged_modules(previous.as_ref(), requested);
        let added: Vec<String> = match &previous {
            Some(m) => modules.difference(&m.modules).cloned().collect(),
            None => modules.iter().cloned().collect(),
        };
        let zephyr_dir = self.dir.join("zephyr");
        let needs_update =
            force_update || previous.is_none() || !added.is_empty() || !zephyr_dir.is_dir();

        if needs_update {
            let manifest_dir = self.dir.join(MANIFEST_DIR);
            let text = render_west_yml(&WestManifest {
                zephyr_url: self.key.url(),
                revision: self.key.revision(),
                shallow: matches!(
                    classify_revision(self.key.revision()),
                    crate::config::RevisionKind::Release { .. }
                ),
                modules: &modules,
            });
            fsutil::atomic_write(&manifest_dir.join("west.yml"), text.as_bytes())?;

            if !self.dir.join(".west").join("config").is_file() {
                let mut cmd = self.west();
                cmd.args(["init", "-l"]).arg(&manifest_dir);
                self.runner.run(&mut cmd)?;
            }
            // Named updates are refused for imported projects, so always update everything;
            // projects that are already at their pinned revision are skipped quickly.
            let mut cmd = self.west();
            cmd.args(["update", "--narrow", "-o=--depth=1"]);
            self.runner.run(&mut cmd)?;
        }

        let projects = self.list_projects()?;
        if let Some(missing) = requested.iter().find(|m| !projects.contains_key(*m)) {
            return Err(ZemError::ModuleNotInManifest {
                module: missing.clone(),
                revision: self.key.revision().to_string(),
            });
        }
        let zephyr_base = projects
            .get("zephyr")
            .cloned()
            .unwrap_or_else(|| platform::canonicalize_lenient(&zephyr_dir));

        write_marker(
            self.dir,
            &WorkspaceMarker {
                schema: MARKER_SCHEMA,
                url: self.key.url().to_string(),
                revision: self.key.revision().to_string(),
                modules,
                blobs_fetched: previous.map(|m| m.blobs_fetched).unwrap_or_default(),
            },
        )?;
        Ok(WorkspaceState {
            zephyr_base,
            projects,
            added,
            updated: needs_update,
        })
    }

    fn list_projects(&self) -> Result<BTreeMap<String, PathBuf>> {
        let mut cmd = self.west();
        cmd.args(["list", "-f", "{name} {abspath}"]);
        let out = self.runner.output(&mut cmd)?;
        Ok(parse_west_list(&out))
    }

    /// Checked-out commit of each named project (the manifest repository has none).
    pub fn commits<'n>(
        &self,
        names: impl IntoIterator<Item = &'n str>,
    ) -> Result<BTreeMap<String, String>> {
        let mut cmd = self.west();
        cmd.args(["list", "-f", "{name} {sha}"]).args(names);
        let out = self.runner.output(&mut cmd)?;
        Ok(out
            .lines()
            .filter_map(|line| line.trim().split_once(' '))
            .map(|(name, sha)| (name.to_string(), sha.trim().to_string()))
            .collect())
    }

    /// Fetch binary blobs for `modules` that have not been fetched yet. Callers must hold the
    /// workspace lock.
    pub fn ensure_blobs<S: AsRef<str>>(
        &self,
        modules: &[S],
        accept_licenses: bool,
        force: bool,
    ) -> Result<Vec<String>> {
        let mut marker = self.marker()?.ok_or_else(|| ZemError::CorruptState {
            path: super::marker_path(self.dir),
            message: "workspace marker is missing".to_string(),
        })?;
        let pending: Vec<&str> = modules
            .iter()
            .map(AsRef::as_ref)
            .filter(|m| force || !marker.blobs_fetched.contains(*m))
            .collect();
        for module in &pending {
            let mut cmd = self.west();
            cmd.args(["blobs", "fetch", module]);
            if accept_licenses {
                cmd.arg("--auto-accept");
            }
            self.runner
                .run(&mut cmd)
                .map_err(|e| blob_hint(e, accept_licenses))?;
            marker.blobs_fetched.insert(module.to_string());
            write_marker(self.dir, &marker)?;
        }
        Ok(pending.into_iter().map(str::to_string).collect())
    }
}

/// The commit `branch` currently points to on the remote.
pub fn resolve_branch(
    env: &EnvBuilder,
    runner: &Runner,
    url: &str,
    branch: &str,
) -> Result<String> {
    let mut cmd = env.command("git");
    cmd.args(["ls-remote", "--", url, &format!("refs/heads/{branch}")]);
    parse_ls_remote(&runner.output(&mut cmd)?).ok_or_else(|| ZemError::ResolveRevision {
        url: url.to_string(),
        revision: branch.to_string(),
    })
}

fn parse_ls_remote(out: &str) -> Option<String> {
    let sha = out.split_whitespace().next()?;
    (sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit())).then(|| sha.to_string())
}

fn blob_hint(err: ZemError, accepted: bool) -> ZemError {
    match err {
        ZemError::CommandFailed {
            command,
            status,
            tail,
            advice,
        } if !accepted => ZemError::CommandFailed {
            command,
            status,
            tail,
            advice: Some(format!(
                "if the blobs require accepting a license, review it and set [blobs].accept_licenses = true \
                 or pass --accept-blob-licenses{}",
                advice.map(|a| format!("; {a}")).unwrap_or_default()
            )),
        },
        other => other,
    }
}

pub fn parse_west_list(out: &str) -> BTreeMap<String, PathBuf> {
    out.lines()
        .filter_map(|line| line.trim().split_once(' '))
        .map(|(name, path)| {
            let path = PathBuf::from(path.trim());
            (
                name.to_string(),
                platform::canonicalize(&path).unwrap_or(path),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn modules_are_only_added() {
        let marker = WorkspaceMarker {
            schema: MARKER_SCHEMA,
            url: "u".into(),
            revision: "v4.2.0".into(),
            modules: set(&["hal_espressif", "mbedtls"]),
            blobs_fetched: BTreeSet::new(),
        };
        assert_eq!(
            merged_modules(Some(&marker), &set(&["hal_espressif", "lvgl"])),
            set(&["hal_espressif", "lvgl", "mbedtls"])
        );
        assert_eq!(
            merged_modules(None, &set(&["hal_espressif"])),
            set(&["hal_espressif"])
        );
    }

    #[test]
    fn parses_ls_remote() {
        let sha = "02f236e1206013b780c522f4edc47badbaeb2a62";
        assert_eq!(
            parse_ls_remote(&format!("{sha}\trefs/heads/main\n")).as_deref(),
            Some(sha)
        );
        assert_eq!(parse_ls_remote(""), None);
    }

    #[test]
    fn parses_west_list_with_spaces_in_paths() {
        let out = "manifest /ws/manifest\nzephyr /ws/zephyr\nhal_espressif /my ws/modules/hal/espressif\n";
        let projects = parse_west_list(out);
        assert_eq!(projects.len(), 3);
        assert_eq!(
            projects["hal_espressif"],
            PathBuf::from("/my ws/modules/hal/espressif")
        );
    }
}
