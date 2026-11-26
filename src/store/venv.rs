//! Python environments, all created by the uv that zem manages.
//!
//! There are two kinds of venv:
//! - the shared west venv under `tools/west`, holding only `west`. It runs `west` before a
//!   Zephyr workspace exists and during workspace updates;
//! - project venvs under `venvs/`, one per workspace, Python version, extra requirements and
//!   build-tool setting. They are built after the workspace, since Zephyr's requirements live
//!   in it.
//!
//! A venv is published by writing its marker last. A directory without a usable marker is not
//! trusted: it is moved to the trash before a new build takes its place. Every step happens
//! under the venv's name lock. Builds happen at their final path rather than in a staging
//! directory and a rename, because the entry-point scripts uv and pip write contain the
//! venv's absolute path.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{gc, lock, read_marker, write_marker};
use crate::config::Python;
use crate::env::EnvBuilder;
use crate::error::{Result, ZemError};
use crate::fsutil;
use crate::home::keys::sha256_hex;
use crate::home::{WestVenvKey, ZemHome};
use crate::lock::FileLock;
use crate::lockfile;
use crate::platform;
use crate::process::Runner;

/// Zephyr's base requirements, relative to the zephyr checkout.
pub const REQUIREMENTS_BASE: &str = "scripts/requirements-base.txt";

/// Layout of a venv's marker. Bumped whenever the meaning of a venv changes, which makes every
/// older venv get rebuilt.
const VENV_SCHEMA: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VenvMarker {
    pub schema: u32,
    pub python: String,
    /// `uv pip freeze` taken after the install.
    pub packages: Vec<String>,
    /// Digest of the inputs a project venv was built from; `None` for the west venv.
    #[serde(default)]
    pub requirements_sha256: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Reused,
    Created,
}

/// The shared west venv for `python`.
pub fn west_dir(home: &ZemHome, python: &str) -> PathBuf {
    home.west_venv_dir(&west_key(python))
}

pub fn west_bin(dir: &Path) -> PathBuf {
    platform::venv_bin_dir(dir).join(platform::exe_name("west"))
}

fn west_key(python: &str) -> WestVenvKey {
    WestVenvKey::new(python.trim(), &["west".to_string()])
}

/// Packages a project venv gets on top of Zephyr's requirements.
pub fn build_packages(cfg: &Python) -> Vec<String> {
    let mut packages = Vec::new();
    if cfg.install_cmake_ninja {
        packages.extend(["cmake".to_string(), "ninja".to_string()]);
    }
    packages.extend(cfg.extra_requirements.iter().cloned());
    packages
}

/// What every build needs. Venvs are always created by the managed uv, never a host tool.
pub struct Builder<'a> {
    pub home: &'a ZemHome,
    pub uv: &'a Path,
    pub env: &'a EnvBuilder,
    pub runner: &'a Runner,
}

impl Builder<'_> {
    fn create(&self, python: &str, dir: &Path) -> Result<()> {
        let mut cmd = self.env.command(self.uv);
        cmd.arg("venv").arg(dir).args([
            "--python",
            python,
            "--python-preference",
            "only-managed",
            "--no-project",
        ]);
        self.runner.run(&mut cmd)
    }

    fn install(
        &self,
        dir: &Path,
        requirements: Option<&Path>,
        constraints: Option<&Path>,
        packages: &[String],
    ) -> Result<()> {
        let mut cmd = self.env.command(self.uv);
        cmd.args(["pip", "install", "--python"])
            .arg(platform::venv_python(dir));
        if let Some(requirements) = requirements {
            cmd.arg("-r").arg(requirements);
        }
        if let Some(constraints) = constraints {
            cmd.arg("-c").arg(constraints);
        }
        cmd.args(packages);
        self.runner.run(&mut cmd)
    }

    fn freeze(&self, dir: &Path) -> Result<Vec<String>> {
        let mut cmd = self.env.command(self.uv);
        cmd.args(["pip", "freeze", "--python"])
            .arg(platform::venv_python(dir));
        Ok(lockfile::parse_freeze(&self.runner.output(&mut cmd)?))
    }

    /// Move whatever is at `dir` to the trash, so a fresh build can take its place.
    fn retire(&self, dir: &Path) -> Result<()> {
        if dir.exists() {
            gc::discard(self.home, dir)
        } else {
            Ok(())
        }
    }

    /// Run `build` to produce a venv at `dir`, which must not exist. If it fails, the partial
    /// directory is discarded, so it is never mistaken for a published venv.
    fn build<T>(&self, dir: &Path, build: impl FnOnce() -> Result<T>) -> Result<T> {
        if let Some(parent) = dir.parent() {
            fsutil::create_dir_all(parent)?;
        }
        let result = build();
        if result.is_err() {
            let _ = gc::discard(self.home, dir);
        }
        result
    }
}

/// A complete venv: current schema, interpreter present.
fn published(dir: &Path, python: &str) -> Option<VenvMarker> {
    read_marker::<VenvMarker>(dir).filter(|m| {
        m.schema == VENV_SCHEMA && m.python == python && platform::venv_python(dir).is_file()
    })
}

/// The packages of a venv that has a current marker, complete or not.
fn marker_packages(dir: &Path) -> Option<Vec<String>> {
    read_marker::<VenvMarker>(dir)
        .filter(|m| m.schema == VENV_SCHEMA)
        .map(|m| m.packages)
}

/// Make sure the shared west venv for `python` exists.
pub fn ensure_west(builder: &Builder, python: &str) -> Result<Outcome> {
    let python = python.trim();
    let key = west_key(python);
    let dir = builder.home.west_venv_dir(&key);
    let _lock = lock(builder.home, &format!("west-{}", key.dir_name()))?;
    if published(&dir, python).is_some() && west_bin(&dir).is_file() {
        return Ok(Outcome::Reused);
    }
    builder.retire(&dir)?;
    builder.build(&dir, || {
        builder.create(python, &dir)?;
        builder.install(&dir, None, None, &["west".to_string()])?;
        let packages = builder.freeze(&dir)?;
        write_marker(
            &dir,
            &VenvMarker {
                schema: VENV_SCHEMA,
                python: python.to_string(),
                packages,
                requirements_sha256: None,
            },
        )
    })?;
    Ok(Outcome::Created)
}

/// The project venv a sync uses, and the locks that keep it stable until the sync ends.
pub struct Selected {
    pub name: String,
    pub dir: PathBuf,
    _locks: Vec<FileLock>,
}

/// Choose the project venv named `plain` (its dedicated name), or a sibling of it when a
/// locked project needs different packages than the one already published under `plain`.
/// The plain venv is locked first, and a sibling is always locked after its plain venv.
pub fn select_project(home: &ZemHome, plain: &str, pinned: Option<&[String]>) -> Result<Selected> {
    let mut locks = vec![lock(home, &format!("venv-{plain}"))?];
    let name = match pinned {
        Some(pins) => match marker_packages(&home.venvs_dir().join(plain)) {
            Some(current) if current != pins => {
                let sibling = format!("{plain}-{}", &lockfile::packages_digest(pins)[..8]);
                locks.push(lock(home, &format!("venv-{sibling}"))?);
                sibling
            }
            _ => plain.to_string(),
        },
        None => plain.to_string(),
    };
    Ok(Selected {
        dir: home.venvs_dir().join(&name),
        name,
        _locks: locks,
    })
}

/// Make sure the selected project venv holds Zephyr's requirements, the build packages and
/// `[python].extra_requirements`. `pinned` is the frozen package set from `zem.lock` with the
/// constraints file that enforces it. `verify` sees the frozen packages before the venv is
/// published, so a venv that fails verification is never reused.
pub fn ensure_project(
    builder: &Builder,
    cfg: &Python,
    selected: &Selected,
    zephyr_base: &Path,
    pinned: Option<(&[String], &Path)>,
    verify: impl FnOnce(&[String]) -> Result<()>,
) -> Result<(Outcome, Vec<String>)> {
    let req = zephyr_base.join(REQUIREMENTS_BASE);
    let text = std::fs::read_to_string(&req).map_err(ZemError::io(&req))?;
    let python = cfg.version.trim();
    let packages = build_packages(cfg);
    let digest = requirements_digest(&text, &packages);
    let dir = &selected.dir;

    if let Some(marker) = published(dir, python)
        && marker.requirements_sha256.as_deref() == Some(digest.as_str())
        && pinned.is_none_or(|(pins, _)| pins == marker.packages.as_slice())
    {
        return Ok((Outcome::Reused, marker.packages));
    }

    builder.retire(dir)?;
    let frozen = builder.build(dir, || {
        builder.create(python, dir)?;
        builder.install(
            dir,
            Some(&req),
            pinned.map(|(_, constraints)| constraints),
            &packages,
        )?;
        let frozen = builder.freeze(dir)?;
        verify(&frozen)?;
        write_marker(
            dir,
            &VenvMarker {
                schema: VENV_SCHEMA,
                python: python.to_string(),
                packages: frozen.clone(),
                requirements_sha256: Some(digest.clone()),
            },
        )?;
        Ok(frozen)
    })?;
    Ok((Outcome::Created, frozen))
}

pub fn requirements_digest(requirements: &str, packages: &[String]) -> String {
    let mut input = String::from("zem-requirements-v2\0");
    input.push_str(requirements);
    for package in packages {
        input.push('\0');
        input.push_str(package);
    }
    sha256_hex(input.as_bytes())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn publish(dir: &Path, schema: u32, python: &str, packages: &[&str]) {
        fs::create_dir_all(platform::venv_bin_dir(dir)).unwrap();
        fs::write(platform::venv_python(dir), "").unwrap();
        write_marker(
            dir,
            &VenvMarker {
                schema,
                python: python.to_string(),
                packages: packages.iter().map(|p| p.to_string()).collect(),
                requirements_sha256: None,
            },
        )
        .unwrap();
    }

    fn builder<'a>(home: &'a ZemHome, env: &'a EnvBuilder, runner: &'a Runner) -> Builder<'a> {
        Builder {
            home,
            uv: Path::new("/nonexistent/uv"),
            env,
            runner,
        }
    }

    #[test]
    fn build_packages_add_build_tools_then_extras() {
        let mut cfg = Python {
            extra_requirements: vec!["pytest".to_string()],
            ..Python::default()
        };
        assert_eq!(build_packages(&cfg), ["cmake", "ninja", "pytest"]);
        cfg.install_cmake_ninja = false;
        assert_eq!(build_packages(&cfg), ["pytest"]);
    }

    #[test]
    fn requirements_digest_depends_on_packages() {
        let a = requirements_digest("PyYAML\n", &[]);
        let b = requirements_digest("PyYAML\n", &["pytest".to_string()]);
        let c = requirements_digest("PyYAML>=6\n", &[]);
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_eq!(a, requirements_digest("PyYAML\n", &[]));
    }

    #[test]
    fn only_complete_current_venvs_are_published() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("py");
        publish(&dir, VENV_SCHEMA, "3.12", &[]);
        fs::remove_file(platform::venv_python(&dir)).unwrap();
        assert!(published(&dir, "3.12").is_none(), "interpreter missing");

        publish(&dir, VENV_SCHEMA, "3.12", &[]);
        assert!(published(&dir, "3.11").is_none(), "other Python");
        assert!(published(&dir, "3.12").is_some());

        publish(&dir, 1, "3.12", &[]);
        assert!(published(&dir, "3.12").is_none(), "older schema");
    }

    #[test]
    fn unpinned_and_unknown_venvs_use_the_plain_name() {
        let tmp = tempfile::tempdir().unwrap();
        let home = ZemHome::new(tmp.path().join("zh"));
        let plain = "py3.12-abc";
        publish(
            &home.venvs_dir().join(plain),
            VENV_SCHEMA,
            "3.12",
            &["a==1"],
        );

        let other = ["a==2".to_string()];
        assert_eq!(select_project(&home, plain, None).unwrap().name, plain);
        let fresh = select_project(&home, "py3.12-new", Some(&other)).unwrap();
        assert_eq!(fresh.name, "py3.12-new");
        drop(fresh);
        let same = ["a==1".to_string()];
        assert_eq!(
            select_project(&home, plain, Some(&same)).unwrap().name,
            plain
        );
    }

    #[test]
    fn locked_projects_get_a_sibling_when_packages_differ() {
        let tmp = tempfile::tempdir().unwrap();
        let home = ZemHome::new(tmp.path().join("zh"));
        let plain = "py3.12-abc";
        publish(
            &home.venvs_dir().join(plain),
            VENV_SCHEMA,
            "3.12",
            &["a==1"],
        );

        let other = ["a==2".to_string()];
        let selected = select_project(&home, plain, Some(&other)).unwrap();
        assert_eq!(
            selected.name,
            format!("{plain}-{}", &lockfile::packages_digest(&other)[..8])
        );
        assert_eq!(selected.dir, home.venvs_dir().join(&selected.name));
    }

    #[test]
    fn failed_build_leaves_no_venv_behind() {
        let tmp = tempfile::tempdir().unwrap();
        let home = ZemHome::new(tmp.path().join("zh"));
        let env = EnvBuilder::from_vars([("PATH", "")]);
        let runner = Runner::new(None, false).unwrap();
        let builder = builder(&home, &env, &runner);
        let dir = home.venvs_dir().join("py-broken");

        let result: Result<()> = builder.build(&dir, || {
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("partial"), "").unwrap();
            Err(ZemError::CorruptState {
                path: dir.clone(),
                message: "install failed".to_string(),
            })
        });
        assert!(result.is_err());
        assert!(!dir.exists());
    }
}
