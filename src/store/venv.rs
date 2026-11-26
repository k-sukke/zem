use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{MARKER_SCHEMA, StagedDir, read_marker, write_marker};
use crate::config::Python;
use crate::env::EnvBuilder;
use crate::error::{Result, ZemError};
use crate::fsutil;
use crate::home::keys::sha256_hex;
use crate::lockfile;
use crate::platform;
use crate::process::Runner;

/// Zephyr's base requirements, relative to the zephyr checkout.
pub const REQUIREMENTS_BASE: &str = "scripts/requirements-base.txt";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VenvMarker {
    pub schema: u32,
    pub installer: String,
    pub python: String,
    pub bootstrap: Vec<String>,
    #[serde(default)]
    pub requirements_sha256: Option<String>,
    /// `pip freeze` after the last install, as recorded in `zem.lock`.
    #[serde(default)]
    pub packages: Option<Vec<String>>,
}

/// The frozen packages recorded for an existing venv, whichever installer wrote the marker.
///
/// Unlike [`VenvStore::marker`], this does not require `installer == "uv"`: bootstrap rebuilds
/// non-uv venvs, but venv selection still needs their package metadata to avoid sharing one.
pub fn installed_packages(dir: &Path) -> Option<Vec<String>> {
    read_marker::<VenvMarker>(dir)
        .filter(|m| m.schema == MARKER_SCHEMA)
        .and_then(|m| m.packages)
}

pub fn bootstrap_packages(cfg: &Python) -> Vec<String> {
    let mut pkgs = vec!["west".to_string()];
    if cfg.host_tools_via_pip {
        pkgs.extend(["cmake".to_string(), "ninja".to_string()]);
    }
    pkgs
}

struct StagedRebuild {
    dir: StagedDir,
    venv: PathBuf,
    bootstrap: Vec<String>,
}

pub struct VenvStore<'a> {
    pub dir: &'a Path,
    pub cfg: &'a Python,
    pub env: &'a EnvBuilder,
    pub runner: &'a Runner,
    staged: Option<StagedRebuild>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Reused,
    Created,
}

impl<'a> VenvStore<'a> {
    pub fn new(dir: &'a Path, cfg: &'a Python, env: &'a EnvBuilder, runner: &'a Runner) -> Self {
        Self {
            dir,
            cfg,
            env,
            runner,
            staged: None,
        }
    }

    pub fn python(&self) -> PathBuf {
        platform::venv_python(self.dir)
    }

    pub fn west(&self) -> PathBuf {
        let dir = self
            .staged
            .as_ref()
            .map_or(self.dir, |staged| staged.venv.as_path());
        platform::venv_bin_dir(dir).join(platform::exe_name("west"))
    }

    fn marker(&self) -> Option<VenvMarker> {
        read_marker::<VenvMarker>(self.dir)
            .filter(|m| m.schema == MARKER_SCHEMA && m.installer == "uv" && self.python().is_file())
    }

    /// Create the venv with just enough to run `west` (before a workspace exists).
    ///
    /// When rebuilding over an existing venv, the staged venv is kept aside until
    /// [`ensure_requirements`](Self::ensure_requirements) finishes so `self.dir` is never left
    /// with only bootstrap packages if a later step fails.
    pub fn ensure_bootstrap(
        &mut self,
        uv: &Path,
        pins: Option<(&[String], &Path)>,
    ) -> Result<Outcome> {
        self.staged = None;
        let packages = bootstrap_packages(self.cfg);
        if self.marker().is_some_and(|m| {
            m.bootstrap == packages
                && m.python == self.cfg.version
                && m.packages.is_some()
                && pins.is_none_or(|(locked, _)| m.packages.as_deref() == Some(locked))
        }) && self.west().is_file()
        {
            return Ok(Outcome::Reused);
        }

        let parent = self.dir.parent().ok_or_else(|| ZemError::CorruptState {
            path: self.dir.to_path_buf(),
            message: "venv path has no parent directory".to_string(),
        })?;
        fsutil::create_dir_all(parent)?;
        let staged = StagedDir::new_in(parent)?;
        let staged_venv = staged.path().join("venv");

        let mut cmd = self.command_at(&staged_venv, uv);
        cmd.arg("venv")
            .arg(&staged_venv)
            .args(["--python", &self.cfg.version])
            .args([
                "--python-preference",
                "only-managed",
                "--relocatable",
                "--no-project",
            ]);
        self.runner.run(&mut cmd)?;
        self.pip_install_at(
            &staged_venv,
            uv,
            &packages,
            None,
            pins.map(|(_, constraints)| constraints),
        )?;
        if !platform::venv_bin_dir(&staged_venv)
            .join(platform::exe_name("west"))
            .is_file()
        {
            return Err(ZemError::CorruptState {
                path: staged_venv,
                message: "bootstrap install completed without producing `west`".to_string(),
            });
        }

        if self.dir.exists() {
            self.staged = Some(StagedRebuild {
                dir: staged,
                venv: staged_venv,
                bootstrap: packages,
            });
            return Ok(Outcome::Created);
        }

        let recorded_packages = match pins.filter(|(locked, _)| !locked.is_empty()) {
            Some((locked, _)) => locked.to_vec(),
            None => self.freeze_at(&staged_venv, uv)?,
        };
        write_marker(
            &staged_venv,
            &VenvMarker {
                schema: MARKER_SCHEMA,
                installer: "uv".to_string(),
                python: self.cfg.version.clone(),
                bootstrap: packages,
                requirements_sha256: None,
                packages: Some(recorded_packages),
            },
        )?;
        replace_staged_dir(staged, &staged_venv, self.dir)?;
        Ok(Outcome::Created)
    }

    /// Install Zephyr's base requirements plus `[python].extra_requirements`, constrained to
    /// `pins` when given, and return the resulting frozen package list.
    pub fn ensure_requirements(
        &mut self,
        uv: &Path,
        zephyr_base: &Path,
        pins: Option<(&[String], &Path)>,
    ) -> Result<(Outcome, Vec<String>)> {
        let req = zephyr_base.join(REQUIREMENTS_BASE);
        let text = fs::read_to_string(&req).map_err(ZemError::io(&req))?;
        let digest = requirements_digest(&text, &self.cfg.extra_requirements);

        if let Some(rebuild) = self.staged.take() {
            self.pip_install_at(
                &rebuild.venv,
                uv,
                &self.cfg.extra_requirements,
                Some(&req),
                pins.map(|(_, constraints)| constraints),
            )?;
            let packages = self.freeze_at(&rebuild.venv, uv)?;
            write_marker(
                &rebuild.venv,
                &VenvMarker {
                    schema: MARKER_SCHEMA,
                    installer: "uv".to_string(),
                    python: self.cfg.version.clone(),
                    bootstrap: rebuild.bootstrap,
                    requirements_sha256: Some(digest),
                    packages: Some(packages.clone()),
                },
            )?;
            replace_staged_dir(rebuild.dir, &rebuild.venv, self.dir)?;
            return Ok((Outcome::Created, packages));
        }

        let mut marker = self.marker().ok_or_else(|| ZemError::CorruptState {
            path: super::marker_path(self.dir),
            message: "venv marker disappeared during sync".to_string(),
        })?;
        if marker.requirements_sha256.as_deref() == Some(digest.as_str())
            && let Some(packages) = &marker.packages
            && pins.is_none_or(|(pins, _)| pins == packages.as_slice())
        {
            return Ok((Outcome::Reused, packages.clone()));
        }

        let installed = self
            .pip_install_at(
                self.dir,
                uv,
                &self.cfg.extra_requirements,
                Some(&req),
                pins.map(|(_, constraints)| constraints),
            )
            .and_then(|()| self.freeze_at(self.dir, uv));
        let packages = match installed {
            Ok(packages) => packages,
            Err(err) => {
                let had_sha = marker.requirements_sha256.take().is_some();
                if marker.packages.is_none() {
                    marker.packages = self
                        .freeze_at(self.dir, uv)
                        .ok()
                        .filter(|pkgs| !pkgs.is_empty());
                }
                if had_sha && marker.packages.is_some() {
                    let _ = write_marker(self.dir, &marker);
                }
                return Err(err);
            }
        };
        marker.requirements_sha256 = Some(digest);
        marker.packages = Some(packages.clone());
        write_marker(self.dir, &marker)?;
        Ok((Outcome::Created, packages))
    }

    fn command_at(&self, venv_dir: &Path, uv: &Path) -> std::process::Command {
        let mut cmd = self.env.command(uv);
        if venv_dir != self.dir {
            cmd.env("VIRTUAL_ENV", venv_dir);
        }
        cmd
    }

    fn freeze_at(&self, venv_dir: &Path, uv: &Path) -> Result<Vec<String>> {
        let python = platform::venv_python(venv_dir);
        let mut cmd = self.command_at(venv_dir, uv);
        cmd.args(["pip", "freeze", "--python"]).arg(&python);
        Ok(lockfile::parse_freeze(&self.runner.output(&mut cmd)?))
    }

    fn pip_install_at(
        &self,
        venv_dir: &Path,
        uv: &Path,
        packages: &[String],
        requirements: Option<&Path>,
        constraints: Option<&Path>,
    ) -> Result<()> {
        let python = platform::venv_python(venv_dir);
        let mut cmd = self.command_at(venv_dir, uv);
        cmd.args(["pip", "install", "--python"]).arg(&python);
        if let Some(req) = requirements {
            cmd.arg("-r").arg(req);
        }
        if let Some(constraints) = constraints {
            cmd.arg("-c").arg(constraints);
        }
        cmd.args(packages);
        self.runner.run(&mut cmd)
    }
}

/// Swap `staged_venv` (inside `staged`) into `dest`, parking any existing `dest` at
/// `staged.path().join("previous")` (on the same filesystem) and restoring it if the final
/// rename fails. If restoration also fails, `staged` is kept on disk so the backup is not
/// deleted and both errors plus the backup path are reported.
fn replace_staged_dir(staged: StagedDir, staged_venv: &Path, dest: &Path) -> Result<()> {
    replace_staged_dir_with(staged, staged_venv, dest, |from, to| fs::rename(from, to))
}

fn replace_staged_dir_with(
    staged: StagedDir,
    staged_venv: &Path,
    dest: &Path,
    mut rename: impl FnMut(&Path, &Path) -> std::io::Result<()>,
) -> Result<()> {
    let backup = staged.path().join("previous");
    let had_existing = match fs::symlink_metadata(dest) {
        Ok(_) => {
            rename(dest, &backup).map_err(ZemError::io(dest))?;
            true
        }
        Err(e) if e.kind() == ErrorKind::NotFound => false,
        Err(e) => return Err(ZemError::io(dest)(e)),
    };
    if let Err(err) = rename(staged_venv, dest) {
        if had_existing && let Err(restore_err) = rename(&backup, dest) {
            let _kept = staged.keep();
            return Err(ZemError::Io {
                path: dest.to_path_buf(),
                source: std::io::Error::other(format!(
                    "failed to publish staged venv ({err}); failed to restore previous venv from {} ({restore_err})",
                    backup.display()
                )),
            });
        }
        return Err(ZemError::io(dest)(err));
    }
    Ok(())
}

pub fn requirements_digest(requirements: &str, extras: &[String]) -> String {
    let mut input = String::from("zem-requirements-v1\0");
    input.push_str(requirements);
    for extra in extras {
        input.push('\0');
        input.push_str(extra);
    }
    sha256_hex(input.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_includes_host_tools_only_when_enabled() {
        let mut cfg = Python::default();
        assert_eq!(bootstrap_packages(&cfg), ["west", "cmake", "ninja"]);
        cfg.host_tools_via_pip = false;
        assert_eq!(bootstrap_packages(&cfg), ["west"]);
    }

    #[test]
    fn requirements_digest_depends_on_extras() {
        let a = requirements_digest("PyYAML\n", &[]);
        let b = requirements_digest("PyYAML\n", &["pytest".to_string()]);
        let c = requirements_digest("PyYAML>=6\n", &[]);
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_eq!(a, requirements_digest("PyYAML\n", &[]));
    }

    #[test]
    fn installed_packages_ignores_installer_but_checks_schema() {
        let tmp = tempfile::tempdir().unwrap();
        let packages = vec!["west==1.4.0".to_string()];
        let marker = |schema: u32, installer: &str| VenvMarker {
            schema,
            installer: installer.to_string(),
            python: "3.12".to_string(),
            bootstrap: vec!["west".to_string()],
            requirements_sha256: Some("abc".to_string()),
            packages: Some(packages.clone()),
        };

        write_marker(tmp.path(), &marker(MARKER_SCHEMA, "venv")).unwrap();
        assert_eq!(installed_packages(tmp.path()), Some(packages.clone()));

        write_marker(tmp.path(), &marker(MARKER_SCHEMA, "uv")).unwrap();
        assert_eq!(installed_packages(tmp.path()), Some(packages.clone()));

        write_marker(tmp.path(), &marker(MARKER_SCHEMA + 1, "uv")).unwrap();
        assert_eq!(installed_packages(tmp.path()), None);
    }

    #[test]
    fn replace_staged_dir_swaps_and_restores_on_missing_staged() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("venv");

        let staged = StagedDir::new_in(tmp.path()).unwrap();
        let staged_root = staged.path().to_path_buf();
        let staged_venv = staged_root.join("venv");
        fs::create_dir_all(&staged_venv).unwrap();
        fs::write(staged_venv.join("gen"), "v1").unwrap();
        replace_staged_dir(staged, &staged_venv, &dest).unwrap();
        assert_eq!(fs::read_to_string(dest.join("gen")).unwrap(), "v1");
        assert!(
            !staged_root.exists(),
            "staging dir should be cleaned up on success"
        );

        // Missing staged directory leaves the existing destination intact and cleans up staging.
        let staged_missing = StagedDir::new_in(tmp.path()).unwrap();
        let missing_root = staged_missing.path().to_path_buf();
        let missing_venv = missing_root.join("missing");
        assert!(replace_staged_dir(staged_missing, &missing_venv, &dest).is_err());
        assert_eq!(fs::read_to_string(dest.join("gen")).unwrap(), "v1");
        assert!(
            !missing_root.exists(),
            "staging dir should be cleaned up after successful restore"
        );

        let staged2 = StagedDir::new_in(tmp.path()).unwrap();
        let staged2_root = staged2.path().to_path_buf();
        let staged2_venv = staged2_root.join("venv");
        fs::create_dir_all(&staged2_venv).unwrap();
        fs::write(staged2_venv.join("gen"), "v2").unwrap();
        replace_staged_dir(staged2, &staged2_venv, &dest).unwrap();
        assert_eq!(fs::read_to_string(dest.join("gen")).unwrap(), "v2");
        assert!(!staged2_root.exists());
    }

    #[test]
    fn replace_staged_dir_preserves_backup_when_restore_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("venv");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("gen"), "v1").unwrap();

        let staged = StagedDir::new_in(tmp.path()).unwrap();
        let staged_root = staged.path().to_path_buf();
        let staged_venv = staged_root.join("venv");
        let backup = staged_root.join("previous");
        fs::create_dir_all(&staged_venv).unwrap();
        fs::write(staged_venv.join("gen"), "v2").unwrap();

        let mut call = 0usize;
        let err = replace_staged_dir_with(staged, &staged_venv, &dest, |from, to| {
            call += 1;
            match call {
                1 => fs::rename(from, to),
                2 => Err(std::io::Error::new(
                    ErrorKind::PermissionDenied,
                    "simulated publish failure",
                )),
                3 => Err(std::io::Error::new(
                    ErrorKind::PermissionDenied,
                    "simulated restore failure",
                )),
                _ => unreachable!(),
            }
        })
        .unwrap_err();

        // Compare the source's `Display` text: `{:?}` would escape Windows path backslashes.
        let msg = match &err {
            ZemError::Io { path, source } => {
                assert_eq!(path, &dest);
                source.to_string()
            }
            other => panic!("expected an I/O error, got {other:?}"),
        };
        assert!(
            msg.contains(&backup.display().to_string()),
            "error should include backup path: {msg}"
        );
        assert!(
            msg.contains("simulated publish failure"),
            "error should include publish error: {msg}"
        );
        assert!(
            msg.contains("simulated restore failure"),
            "error should include restore error: {msg}"
        );
        assert!(
            backup.is_dir(),
            "backup must not be deleted when restore fails"
        );
        assert_eq!(fs::read_to_string(backup.join("gen")).unwrap(), "v1");
    }
}
