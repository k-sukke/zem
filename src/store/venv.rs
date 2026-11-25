use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{MARKER_SCHEMA, read_marker, write_marker};
use crate::config::{PyInstaller, Python};
use crate::env::EnvBuilder;
use crate::error::{Result, ZemError};
use crate::fsutil;
use crate::home::keys::sha256_hex;
use crate::lockfile;
use crate::platform;
use crate::process::Runner;
use crate::tools::{self, Version};

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

/// The frozen packages of an existing venv, if it was fully installed.
pub fn installed_packages(dir: &Path) -> Option<Vec<String>> {
    read_marker::<VenvMarker>(dir)
        .filter(|m| m.schema == MARKER_SCHEMA)
        .and_then(|m| m.packages)
}

#[derive(Debug, Clone)]
pub enum Installer {
    Uv(PathBuf),
    Venv(PathBuf),
}

impl Installer {
    pub fn name(&self) -> &'static str {
        match self {
            Installer::Uv(_) => "uv",
            Installer::Venv(_) => "venv",
        }
    }
}

fn find_in(env: &EnvBuilder, name: &str) -> Option<PathBuf> {
    let path = env.get("PATH")?;
    let cwd = std::env::current_dir().ok()?;
    which::which_in(name, Some(path), cwd).ok()
}

pub fn resolve_installer(cfg: &Python, env: &EnvBuilder) -> Result<Installer> {
    let uv = || find_in(env, "uv").map(Installer::Uv);
    let venv = || find_python(env, &cfg.version).map(Installer::Venv);
    let found = match cfg.installer {
        PyInstaller::Uv => uv(),
        PyInstaller::Venv => venv(),
        PyInstaller::Auto => uv().or_else(venv),
    };
    found.ok_or_else(|| ZemError::ToolNotFound {
        tool: match cfg.installer {
            PyInstaller::Venv => "python",
            _ => "uv",
        },
        advice: Some(match cfg.installer {
            PyInstaller::Venv => format!(
                "install Python {} or set [python].installer = \"uv\"",
                cfg.version
            ),
            _ => platform::install_hint("uv")
                .map(|h| h.to_string())
                .unwrap_or_else(|| "install uv: https://docs.astral.sh/uv/".to_string()),
        }),
    })
}

/// A host interpreter for the `venv` fallback, preferring an exact `pythonX.Y` match.
fn find_python(env: &EnvBuilder, wanted: &str) -> Option<PathBuf> {
    let exact = format!("python{wanted}");
    std::iter::once(exact.as_str())
        .chain(platform::python_candidates().iter().copied())
        .filter_map(|name| find_in(env, name))
        .find(|path| {
            std::process::Command::new(path)
                .arg("--version")
                .output()
                .ok()
                .and_then(|o| Version::parse_first(&String::from_utf8_lossy(&o.stdout)))
                .is_some_and(|v| v >= tools::MIN_PYTHON)
        })
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
            .filter(|m| m.schema == MARKER_SCHEMA && self.python().is_file())
    }

    /// Create the venv with just enough to run `west` (before a workspace exists).
    pub fn ensure_bootstrap(
        &self,
        installer: &Installer,
        constraints: Option<&Path>,
    ) -> Result<Outcome> {
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
        match installer {
            Installer::Uv(uv) => {
                let mut cmd = self.env.command(uv);
                cmd.arg("venv")
                    .arg(self.dir)
                    .args(["--python", &self.cfg.version])
                    .args(["--python-preference", "only-managed", "--no-project"]);
                self.runner.run(&mut cmd)?;
            }
            Installer::Venv(python) => {
                let mut cmd = self.env.command(python);
                cmd.args(["-m", "venv"]).arg(self.dir);
                self.runner.run(&mut cmd)?;
            }
        }
        self.pip_install(installer, &packages, None, constraints)?;

        write_marker(
            self.dir,
            &VenvMarker {
                schema: MARKER_SCHEMA,
                installer: installer.name().to_string(),
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
        installer: &Installer,
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
            installer,
            &self.cfg.extra_requirements,
            Some(&req),
            pins.map(|(_, constraints)| constraints),
        )?;
        let packages = self.freeze(installer)?;
        marker.requirements_sha256 = Some(digest);
        marker.packages = Some(packages.clone());
        write_marker(self.dir, &marker)?;
        Ok((Outcome::Created, packages))
    }

    fn freeze(&self, installer: &Installer) -> Result<Vec<String>> {
        let python = self.python();
        let mut cmd = match installer {
            Installer::Uv(uv) => {
                let mut cmd = self.env.command(uv);
                cmd.args(["pip", "freeze", "--python"]).arg(&python);
                cmd
            }
            Installer::Venv(_) => {
                let mut cmd = self.env.command(&python);
                cmd.args(["-m", "pip", "freeze", "--disable-pip-version-check"]);
                cmd
            }
        };
        Ok(lockfile::parse_freeze(&self.runner.output(&mut cmd)?))
    }

    fn pip_install(
        &self,
        installer: &Installer,
        packages: &[String],
        requirements: Option<&Path>,
        constraints: Option<&Path>,
    ) -> Result<()> {
        let python = self.python();
        let mut cmd = match installer {
            Installer::Uv(uv) => {
                let mut cmd = self.env.command(uv);
                cmd.args(["pip", "install", "--python"]).arg(&python);
                cmd
            }
            Installer::Venv(_) => {
                let mut cmd = self.env.command(&python);
                cmd.args(["-m", "pip", "install", "--disable-pip-version-check"]);
                cmd
            }
        };
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
    fn missing_uv_is_reported() {
        let env = EnvBuilder::from_vars([("PATH", "")]);
        let cfg = Python {
            installer: PyInstaller::Uv,
            ..Python::default()
        };
        assert!(matches!(
            resolve_installer(&cfg, &env),
            Err(ZemError::ToolNotFound { tool: "uv", .. })
        ));
    }
}
