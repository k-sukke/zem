//! A synced project, ready to run west or other tools in its isolated environment.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::{self, EnvConfig, Isolation, LoadedConfig, ZemConfig};
use crate::env::{EnvBuilder, EnvLocations, zem_env};
use crate::error::{Result, ZemError};
use crate::fsutil;
use crate::home::ZemHome;
use crate::platform;
use crate::project::west_config::{self, WestConfigInputs};
use crate::project::{ProjectDir, ProjectState, STATE_SCHEMA};
use crate::store::sdk;

pub struct Session {
    pub loaded: LoadedConfig,
    pub home: ZemHome,
    pub project: ProjectDir,
    pub state: ProjectState,
}

impl Session {
    /// Never syncs implicitly: a stale or missing environment is an error pointing at `zem sync`.
    pub fn open(cwd: &Path) -> Result<Self> {
        let loaded = config::load(cwd)?;
        let home = ZemHome::resolve()?;
        let project = ProjectDir::new(&loaded.root, &home)?;
        let state = project.read_state()?.ok_or_else(|| ZemError::NotSynced {
            reason: "this project has not been synced yet".to_string(),
        })?;
        check(&state, &loaded, &home)?;
        let session = Session {
            loaded,
            home,
            project,
            state,
        };
        session.write_west_config()?;
        Ok(session)
    }

    pub fn config(&self) -> &ZemConfig {
        &self.loaded.config
    }

    /// The environment for tools zem launches, honouring `[env].isolation`.
    pub fn env(&self) -> EnvBuilder {
        self.env_with(self.config().env.isolation)
    }

    /// The environment applied to the user's own shell (`zem env`, activate scripts). Always
    /// denylist-based: strict mode would unset the user's personal variables.
    pub fn activation_env(&self) -> EnvBuilder {
        self.env_with(Isolation::Denylist)
    }

    fn env_with(&self, isolation: Isolation) -> EnvBuilder {
        let config = EnvConfig {
            isolation,
            ..self.config().env.clone()
        };
        zem_env(
            &EnvLocations {
                home: &self.home,
                venv_dir: &self.state.venv.path,
                zephyr_base: Some(&self.state.zephyr_base),
                sdk_dir: self.state.sdk.as_ref().map(|s| s.path.as_path()),
                west_config: Some(&self.west_config_path()),
            },
            &config,
        )
    }

    pub fn west(&self) -> PathBuf {
        venv_west(&self.state.venv.path)
    }

    pub fn build_dir(&self) -> PathBuf {
        self.config().build.dir.to_path(self.project.root())
    }

    pub fn west_config_path(&self) -> PathBuf {
        self.project.state_dir().join(west_config::WEST_CONFIG_FILE)
    }

    /// Rewritten on every open, so it follows zem.toml and the project's current location.
    /// `west build` reuses an existing CMake cache (board, cmake-args, module list) unless told
    /// otherwise, so a changed config drops the cache to force a reconfigure.
    fn write_west_config(&self) -> Result<()> {
        let text = west_config::render(&WestConfigInputs {
            board: &self.state.target.board,
            modules: self.state.modules.keys().map(String::as_str).collect(),
            build_dir: &self.build_dir(),
            user_cache_dir: &self.home.zephyr_cache_dir(),
            cmake_args: &self.config().build.cmake_args,
        });
        let path = self.west_config_path();
        if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
            fsutil::atomic_write(&path, text.as_bytes())?;
            let cache = self.build_dir().join("CMakeCache.txt");
            if cache.exists() {
                std::fs::remove_file(&cache).map_err(ZemError::io(&cache))?;
                eprintln!(
                    "{} build settings changed; the next `west build` reconfigures {}",
                    console::style("note:").cyan().bold(),
                    self.build_dir().display()
                );
            }
        }
        Ok(())
    }

    /// `west <args>` in the project root with the isolated environment.
    pub fn west_command(&self, env: &EnvBuilder) -> Command {
        let mut cmd = env.command(self.west());
        cmd.current_dir(self.project.root());
        cmd
    }

    /// Resolve `program` against the isolated PATH, so `zem run west` picks the venv's west.
    pub fn resolve_program(&self, env: &EnvBuilder, program: &str) -> Result<PathBuf> {
        let path = env.get("PATH").unwrap_or(OsStr::new(""));
        let cwd = std::env::current_dir().map_err(ZemError::io("."))?;
        which::which_in(program, Some(path), cwd)
            .map_err(|_| ZemError::CommandNotFound(program.to_string()))
    }
}

pub fn venv_west(venv: &Path) -> PathBuf {
    platform::venv_bin_dir(venv).join(platform::exe_name("west"))
}

pub fn check(state: &ProjectState, loaded: &LoadedConfig, home: &ZemHome) -> Result<()> {
    let stale = |reason: String| Err(ZemError::NotSynced { reason });
    if state.schema != STATE_SCHEMA {
        return stale("the project state was written by an incompatible zem version".into());
    }
    if state.config_hash != loaded.content_hash {
        return stale("zem.toml changed since the last sync".into());
    }
    if state.zem_home != home.root() {
        return stale(format!(
            "the project was synced with ZEM_HOME={}, but ZEM_HOME is now {}",
            state.zem_home.display(),
            home.root().display()
        ));
    }
    let Some(sdk) = &state.sdk else {
        return stale("the Zephyr SDK is not installed".into());
    };
    let mut required = vec![
        state.zephyr_base.clone(),
        platform::venv_python(&state.venv.path),
        venv_west(&state.venv.path),
    ];
    required.extend(state.modules.values().cloned());
    required.extend(
        state
            .target
            .toolchains
            .iter()
            .map(|tc| sdk::toolchain_gcc(&sdk.path, tc)),
    );
    match required.iter().find(|p| !p.exists()) {
        Some(missing) => stale(format!("{} is missing", missing.display())),
        None => Ok(()),
    }
}
