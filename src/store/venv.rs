use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{MARKER_SCHEMA, read_marker, write_marker};
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
        platform::venv_bin_dir(self.dir).join(platform::exe_name("west"))
    }

    fn marker(&self) -> Option<VenvMarker> {
        read_marker::<VenvMarker>(self.dir)
            .filter(|m| m.schema == MARKER_SCHEMA && m.installer == "uv" && self.python().is_file())
    }

    /// Create the venv with just enough to run `west` (before a workspace exists).
    pub fn ensure_bootstrap(&self, uv: &Path, constraints: Option<&Path>) -> Result<Outcome> {
        let packages = bootstrap_packages(self.cfg);
        if self
            .marker()
            .is_some_and(|m| m.bootstrap == packages && m.python == self.cfg.version)
            && self.west().is_file()
        {
            return Ok(Outcome::Reused);
        }

        fsutil::remove_dir_all_if_exists(self.dir)?;
        if let Some(parent) = self.dir.parent() {
            fsutil::create_dir_all(parent)?;
        }
        let mut cmd = self.env.command(uv);
        cmd.arg("venv")
            .arg(self.dir)
            .args(["--python", &self.cfg.version])
            .args(["--python-preference", "only-managed", "--no-project"]);
        self.runner.run(&mut cmd)?;
        self.pip_install(uv, &packages, None, constraints)?;

        write_marker(
            self.dir,
            &VenvMarker {
                schema: MARKER_SCHEMA,
                installer: "uv".to_string(),
                python: self.cfg.version.clone(),
                bootstrap: packages,
                requirements_sha256: None,
                packages: None,
            },
        )?;
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
        let text = std::fs::read_to_string(&req).map_err(ZemError::io(&req))?;
        let digest = requirements_digest(&text, &self.cfg.extra_requirements);
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

        self.pip_install(
            uv,
            &self.cfg.extra_requirements,
            Some(&req),
            pins.map(|(_, constraints)| constraints),
        )?;
        let packages = self.freeze(uv)?;
        marker.requirements_sha256 = Some(digest);
        marker.packages = Some(packages.clone());
        write_marker(self.dir, &marker)?;
        Ok((Outcome::Created, packages))
    }

    fn freeze(&self, uv: &Path) -> Result<Vec<String>> {
        let python = self.python();
        let mut cmd = self.env.command(uv);
        cmd.args(["pip", "freeze", "--python"]).arg(&python);
        Ok(lockfile::parse_freeze(&self.runner.output(&mut cmd)?))
    }

    fn pip_install(
        &self,
        uv: &Path,
        packages: &[String],
        requirements: Option<&Path>,
        constraints: Option<&Path>,
    ) -> Result<()> {
        let python = self.python();
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
}
