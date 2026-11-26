mod sanitize;
pub mod shell;

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

pub use sanitize::{
    DENY_EXACT, DENY_PREFIXES, is_denied, is_polluting_path_entry, is_python_env_bin,
    is_strict_allowed,
};

use crate::config::{EnvConfig, Isolation};
use crate::home::ZemHome;
use crate::platform;

const PATH: &str = "PATH";

/// The complete environment handed to child processes. Children never inherit zem's own
/// environment directly; [`EnvBuilder::apply`] clears it first.
#[derive(Debug, Clone, Default)]
pub struct EnvBuilder {
    vars: Vec<(OsString, OsString)>,
}

impl EnvBuilder {
    pub fn from_vars<K: Into<OsString>, V: Into<OsString>>(
        vars: impl IntoIterator<Item = (K, V)>,
    ) -> Self {
        let mut env = EnvBuilder::default();
        for (k, v) in vars {
            env.set(k, v);
        }
        env
    }

    pub fn from_current() -> Self {
        Self::from_vars(std::env::vars_os())
    }

    fn position(&self, key: &OsStr) -> Option<usize> {
        self.vars
            .iter()
            .position(|(k, _)| platform::env_key_eq(k, key))
    }

    pub fn get(&self, key: impl AsRef<OsStr>) -> Option<&OsStr> {
        self.position(key.as_ref())
            .map(|i| self.vars[i].1.as_os_str())
    }

    pub fn set(&mut self, key: impl Into<OsString>, value: impl Into<OsString>) {
        let key = key.into();
        let value = value.into();
        match self.position(&key) {
            Some(i) => self.vars[i].1 = value,
            None => self.vars.push((key, value)),
        }
    }

    pub fn remove(&mut self, key: impl AsRef<OsStr>) {
        let key = key.as_ref();
        self.vars.retain(|(k, _)| !platform::env_key_eq(k, key));
    }

    pub fn vars(&self) -> impl Iterator<Item = (&OsStr, &OsStr)> {
        self.vars
            .iter()
            .map(|(k, v)| (k.as_os_str(), v.as_os_str()))
    }

    pub fn path_entries(&self) -> Vec<PathBuf> {
        self.get(PATH)
            .map(|p| std::env::split_paths(p).collect())
            .unwrap_or_default()
    }

    fn set_path_entries(&mut self, entries: Vec<PathBuf>) {
        if let Ok(joined) = std::env::join_paths(entries) {
            self.set(PATH, joined);
        }
    }

    /// Drop variables from other Zephyr/ESP-IDF/Python setups and their PATH entries. In
    /// strict mode, also drop everything outside the platform allowlist.
    pub fn sanitize(&mut self, passthrough: &[String], isolation: Isolation) {
        let foreign_prefixes: Vec<PathBuf> = ["VIRTUAL_ENV", "CONDA_PREFIX"]
            .iter()
            .filter_map(|k| self.get(k).filter(|v| !v.is_empty()).map(PathBuf::from))
            .collect();

        let kept = |k: &OsStr| {
            passthrough
                .iter()
                .any(|p| platform::env_key_eq(k, OsStr::new(p)))
        };
        self.vars.retain(|(k, _)| {
            kept(k)
                || match isolation {
                    Isolation::Denylist => !is_denied(k),
                    Isolation::Strict => {
                        platform::env_key_eq(k, OsStr::new(PATH)) || is_strict_allowed(k)
                    }
                }
        });

        let entries = self
            .path_entries()
            .into_iter()
            .filter(|p| !is_polluting_path_entry(p, &foreign_prefixes) && !is_python_env_bin(p))
            .collect();
        self.set_path_entries(entries);
    }

    pub fn prepend_path(&mut self, dir: &Path) {
        let mut entries = vec![dir.to_path_buf()];
        entries.extend(self.path_entries().into_iter().filter(|p| p != dir));
        self.set_path_entries(entries);
    }

    pub fn apply(&self, cmd: &mut Command) {
        cmd.env_clear();
        cmd.envs(self.vars.iter().map(|(k, v)| (k, v)));
    }

    pub fn command(&self, program: impl AsRef<OsStr>) -> Command {
        let mut cmd = Command::new(program);
        self.apply(&mut cmd);
        cmd
    }
}

/// Locations the isolated environment points at. Fields that are not known yet stay `None`.
pub struct EnvLocations<'a> {
    pub home: &'a ZemHome,
    pub venv_dir: &'a Path,
    pub zephyr_base: Option<&'a Path>,
    pub sdk_dir: Option<&'a Path>,
    /// The project's west configuration. `zem sync` leaves it unset so west manages the shared
    /// workspace with its own `.west/config`.
    pub west_config: Option<&'a Path>,
}

pub fn zem_env(locations: &EnvLocations<'_>, config: &EnvConfig) -> EnvBuilder {
    zem_env_from(EnvBuilder::from_current(), locations, config)
}

pub fn zem_env_from(mut env: EnvBuilder, loc: &EnvLocations<'_>, config: &EnvConfig) -> EnvBuilder {
    let home = loc.home;
    env.sanitize(&config.passthrough, config.isolation);
    env.prepend_path(&platform::venv_bin_dir(loc.venv_dir));

    env.set("ZEM_ACTIVE", "1");
    env.set("ZEM_HOME", home.root());
    env.set("VIRTUAL_ENV", loc.venv_dir);
    env.set("WEST_CONFIG_GLOBAL", home.westconfig_path());
    env.set("WEST_CONFIG_SYSTEM", home.westconfig_path());
    env.set("PYTHONNOUSERSITE", "1");
    env.set("UV_CACHE_DIR", home.uv_cache_dir());
    env.set("UV_PYTHON_INSTALL_DIR", home.python_dir());
    env.set("CCACHE_DIR", home.ccache_dir());
    if let Some(base) = loc.zephyr_base {
        env.set("ZEPHYR_BASE", base);
    }
    if let Some(sdk) = loc.sdk_dir {
        env.set("ZEPHYR_SDK_INSTALL_DIR", sdk);
        env.set("ZEPHYR_TOOLCHAIN_VARIANT", "zephyr");
    }
    if let Some(west_config) = loc.west_config {
        env.set("WEST_CONFIG_LOCAL", west_config);
    }
    for (k, v) in &config.set {
        env.set(k, v);
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    fn polluted() -> EnvBuilder {
        let path = std::env::join_paths([
            PathBuf::from("/home/me/.espressif/tools/xtensa/bin"),
            PathBuf::from("/opt/esp-idf/tools"),
            PathBuf::from("/old/venv/bin"),
            PathBuf::from("/usr/bin"),
            PathBuf::from("/home/me/zephyr-sdk-0.16.8/arm-zephyr-eabi/bin"),
        ])
        .unwrap();
        EnvBuilder::from_vars([
            ("PATH", path),
            ("IDF_PATH", "/opt/esp-idf".into()),
            ("ZEPHYR_BASE", "/other/zephyr".into()),
            ("VIRTUAL_ENV", "/old/venv".into()),
            ("PIP_INDEX_URL", "https://mirror".into()),
            ("HTTPS_PROXY", "http://proxy".into()),
            ("HOME", "/home/me".into()),
        ])
    }

    #[test]
    fn sanitize_removes_foreign_setups() {
        let mut env = polluted();
        env.sanitize(&[], Isolation::Denylist);
        for gone in ["IDF_PATH", "ZEPHYR_BASE", "VIRTUAL_ENV", "PIP_INDEX_URL"] {
            assert!(env.get(gone).is_none(), "{gone} should be removed");
        }
        assert!(env.get("HOME").is_some());
        assert!(env.get("HTTPS_PROXY").is_some());
        assert_eq!(env.path_entries(), [PathBuf::from("/usr/bin")]);
    }

    #[test]
    fn passthrough_keeps_variables() {
        let mut env = polluted();
        env.sanitize(&["PIP_INDEX_URL".to_string()], Isolation::Denylist);
        assert!(env.get("PIP_INDEX_URL").is_some());
    }

    #[test]
    fn strict_keeps_only_allowlist_path_and_passthrough() {
        let mut env = polluted();
        env.set("EDITOR", "vim");
        env.set("MY_TOKEN", "t");
        env.sanitize(&["MY_TOKEN".to_string()], Isolation::Strict);
        for (key, _) in env.vars() {
            assert!(
                key == "PATH" || key == "MY_TOKEN" || is_strict_allowed(key),
                "{key:?}"
            );
        }
        assert!(env.get("MY_TOKEN").is_some() && env.get("HTTPS_PROXY").is_some());
        assert!(env.get("EDITOR").is_none() && env.get("IDF_PATH").is_none());
        assert_eq!(env.path_entries(), [PathBuf::from("/usr/bin")]);
    }

    #[test]
    fn zem_env_points_at_zem_locations() {
        let home = ZemHome::new(PathBuf::from("/zh"));
        let venv = PathBuf::from("/zh/venvs/py3.12-x");
        let base = PathBuf::from("/zh/workspaces/w/zephyr");
        let mut config = EnvConfig::default();
        config.set.insert("FOO".into(), "bar".into());
        let env = zem_env_from(
            polluted(),
            &EnvLocations {
                home: &home,
                venv_dir: &venv,
                zephyr_base: Some(&base),
                sdk_dir: None,
                west_config: None,
            },
            &config,
        );
        assert_eq!(env.path_entries()[0], platform::venv_bin_dir(&venv));
        assert!(env.get("WEST_CONFIG_LOCAL").is_none());
        assert_eq!(env.get("ZEPHYR_BASE"), Some(base.as_os_str()));
        assert_eq!(env.get("VIRTUAL_ENV"), Some(venv.as_os_str()));
        assert_eq!(env.get("FOO"), Some(OsStr::new("bar")));
        assert!(env.get("ZEPHYR_SDK_INSTALL_DIR").is_none());
        assert!(env.get("IDF_PATH").is_none());
    }

    #[test]
    fn set_replaces_existing_key() {
        let mut env = EnvBuilder::from_vars([("A", "1")]);
        env.set("A", "2");
        assert_eq!(env.vars().count(), 1);
        assert_eq!(env.get("A"), Some(OsStr::new("2")));
    }
}
