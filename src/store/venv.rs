use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{MARKER_SCHEMA, read_marker, staging_dir, write_marker};
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

/// The python version and bootstrap packages are part of the venv key, so the marker only
/// records what was installed into it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VenvMarker {
    pub schema: u32,
    #[serde(default)]
    pub requirements_sha256: Option<String>,
    /// `uv pip freeze` after the last install, as recorded in `zem.lock`.
    pub packages: Vec<String>,
}

fn read_venv_marker(dir: &Path) -> Option<VenvMarker> {
    read_marker::<VenvMarker>(dir).filter(|m| m.schema == MARKER_SCHEMA)
}

/// The frozen packages recorded for an existing venv, even if its interpreter is gone, so venv
/// selection never shares a venv that belongs to a different package set.
pub fn installed_packages(dir: &Path) -> Option<Vec<String>> {
    read_venv_marker(dir).map(|m| m.packages)
}

pub fn bootstrap_packages(cfg: &Python) -> Vec<String> {
    let mut pkgs = vec!["west".to_string()];
    if cfg.host_tools_via_pip {
        pkgs.extend(["cmake".to_string(), "ninja".to_string()]);
    }
    pkgs
}

pub struct VenvStore<'a> {
    pub dir: &'a Path,
    pub cfg: &'a Python,
    pub env: &'a EnvBuilder,
    pub runner: &'a Runner,
}

pub enum Outcome {
    Reused,
    Created,
}

impl VenvStore<'_> {
    pub fn python(&self) -> PathBuf {
        platform::venv_python(self.dir)
    }

    pub fn west(&self) -> PathBuf {
        platform::venv_exe(self.dir, "west")
    }

    fn marker(&self) -> Option<VenvMarker> {
        read_venv_marker(self.dir).filter(|_| self.python().is_file())
    }

    /// Create the venv with just enough to run `west` (before a workspace exists).
    pub fn ensure_bootstrap(&self, uv: &Path, pins: Option<(&[String], &Path)>) -> Result<Outcome> {
        if self
            .marker()
            .is_some_and(|m| pins.is_none_or(|(locked, _)| m.packages == locked))
            && self.west().is_file()
        {
            return Ok(Outcome::Reused);
        }

        let previous_packages = installed_packages(self.dir);
        let parent = self.dir.parent().ok_or_else(|| ZemError::CorruptState {
            path: self.dir.to_path_buf(),
            message: "venv path has no parent directory".to_string(),
        })?;
        fsutil::create_dir_all(parent)?;
        let staged = staging_dir(parent)?;
        let staged_venv = staged.path().join("venv");

        let mut cmd = self.env.command(uv);
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
            &bootstrap_packages(self.cfg),
            None,
            pins.map(|(_, constraints)| constraints),
        )?;
        if !platform::venv_exe(&staged_venv, "west").is_file() {
            return Err(ZemError::CorruptState {
                path: staged_venv,
                message: "bootstrap install completed without producing `west`".to_string(),
            });
        }

        let recorded_packages = match pins.filter(|(locked, _)| !locked.is_empty()) {
            Some((locked, _)) => locked.to_vec(),
            None => match previous_packages {
                Some(prev) => prev,
                None => self.freeze_at(&staged_venv, uv)?,
            },
        };
        write_marker(
            &staged_venv,
            &VenvMarker {
                schema: MARKER_SCHEMA,
                requirements_sha256: None,
                packages: recorded_packages,
            },
        )?;
        replace_dir(&staged_venv, self.dir, &staged.path().join("previous"))?;
        Ok(Outcome::Created)
    }

    /// Install Zephyr's base requirements plus `[python].extra_requirements`, constrained to
    /// `pins` when given, and return the resulting frozen package list.
    pub fn ensure_requirements(
        &self,
        uv: &Path,
        zephyr_base: &Path,
        pins: Option<(&[String], &Path)>,
    ) -> Result<(Outcome, Vec<String>)> {
        let req = zephyr_base.join(REQUIREMENTS_BASE);
        let text = fs::read_to_string(&req).map_err(ZemError::io(&req))?;
        let digest = requirements_digest(&text, &self.cfg.extra_requirements);
        let mut marker = self.marker().ok_or_else(|| ZemError::CorruptState {
            path: super::marker_path(self.dir),
            message: "venv marker disappeared during sync".to_string(),
        })?;
        if marker.requirements_sha256.as_deref() == Some(digest.as_str())
            && pins.is_none_or(|(pins, _)| pins == marker.packages.as_slice())
        {
            return Ok((Outcome::Reused, marker.packages));
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
                if marker.requirements_sha256.take().is_some() {
                    let _ = write_marker(self.dir, &marker);
                }
                return Err(err);
            }
        };
        marker.requirements_sha256 = Some(digest);
        marker.packages = packages.clone();
        write_marker(self.dir, &marker)?;
        Ok((Outcome::Created, packages))
    }

    fn freeze_at(&self, venv_dir: &Path, uv: &Path) -> Result<Vec<String>> {
        let python = platform::venv_python(venv_dir);
        let mut cmd = self.env.command(uv);
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
        let mut cmd = self.env.command(uv);
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

/// Swap `staged` into `dest`, parking any existing `dest` at `backup` (on the same filesystem)
/// and restoring it if the final rename fails.
fn replace_dir(staged: &Path, dest: &Path, backup: &Path) -> Result<()> {
    let had_existing = match fs::symlink_metadata(dest) {
        Ok(_) => {
            fs::rename(dest, backup).map_err(ZemError::io(dest))?;
            true
        }
        Err(e) if e.kind() == ErrorKind::NotFound => false,
        Err(e) => return Err(ZemError::io(dest)(e)),
    };
    if let Err(err) = fs::rename(staged, dest) {
        if had_existing {
            let _ = fs::rename(backup, dest);
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
    fn installed_packages_checks_schema() {
        let tmp = tempfile::tempdir().unwrap();
        let packages = vec!["west==1.4.0".to_string()];
        let marker = |schema: u32| VenvMarker {
            schema,
            requirements_sha256: Some("abc".to_string()),
            packages: packages.clone(),
        };

        write_marker(tmp.path(), &marker(MARKER_SCHEMA)).unwrap();
        assert_eq!(installed_packages(tmp.path()), Some(packages.clone()));

        write_marker(tmp.path(), &marker(MARKER_SCHEMA + 1)).unwrap();
        assert_eq!(installed_packages(tmp.path()), None);
    }

    #[test]
    fn replace_dir_swaps_and_restores_on_missing_staged() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("venv");
        let staged = tmp.path().join("staged");
        let backup = tmp.path().join("previous");

        fs::create_dir_all(&staged).unwrap();
        fs::write(staged.join("gen"), "v1").unwrap();
        replace_dir(&staged, &dest, &backup).unwrap();
        assert_eq!(fs::read_to_string(dest.join("gen")).unwrap(), "v1");

        // Missing staged directory leaves the existing destination intact.
        assert!(replace_dir(&tmp.path().join("missing"), &dest, &backup).is_err());
        assert_eq!(fs::read_to_string(dest.join("gen")).unwrap(), "v1");

        let staged2 = tmp.path().join("staged2");
        fs::create_dir_all(&staged2).unwrap();
        fs::write(staged2.join("gen"), "v2").unwrap();
        let backup2 = tmp.path().join("previous2");
        replace_dir(&staged2, &dest, &backup2).unwrap();
        assert_eq!(fs::read_to_string(dest.join("gen")).unwrap(), "v2");
    }
}
