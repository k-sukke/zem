//! The only place where OS-specific behaviour may live.
//!
//! Code outside this module must not use `cfg(unix)`, `cfg(windows)` or `cfg(target_os)`;
//! `scripts/check-platform-boundary.sh` enforces this in CI.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{Result, ZemError};

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as imp;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as imp;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Mac,
    Linux,
    Windows,
    Other,
}

impl Os {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Os::Mac
        } else if cfg!(target_os = "linux") {
            Os::Linux
        } else if cfg!(windows) {
            Os::Windows
        } else {
            Os::Other
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Os::Mac => "macOS",
            Os::Linux => "Linux",
            Os::Windows => "Windows",
            Os::Other => "this OS",
        }
    }

    /// Whether zem can create and run environments (not just parse config) on this OS.
    pub fn runs_environments(self) -> bool {
        matches!(self, Os::Mac | Os::Linux)
    }
}

pub fn arch_name() -> &'static str {
    std::env::consts::ARCH
}

pub fn ensure_supported(feature: &'static str) -> Result<()> {
    let os = Os::current();
    if os.runs_environments() {
        Ok(())
    } else {
        Err(ZemError::Unsupported {
            feature,
            os: os.name(),
        })
    }
}

pub fn home_dir() -> Option<PathBuf> {
    home::home_dir().filter(|p| !p.as_os_str().is_empty())
}

pub fn default_zem_home() -> Result<PathBuf> {
    home_dir()
        .map(|h| h.join(".zem"))
        .ok_or(ZemError::NoHomeDir)
}

pub fn exe_name(base: &str) -> OsString {
    let mut name = OsString::from(base);
    name.push(imp::EXE_SUFFIX);
    name
}

pub fn venv_bin_dir(venv: &Path) -> PathBuf {
    venv.join(imp::VENV_BIN_DIR)
}

/// An executable installed into the venv, e.g. `west`.
pub fn venv_exe(venv: &Path, base: &str) -> PathBuf {
    venv_bin_dir(venv).join(exe_name(base))
}

pub fn venv_python(venv: &Path) -> PathBuf {
    venv_exe(venv, "python")
}

/// Environment variable names are case-insensitive on Windows (`Path` == `PATH`).
pub fn env_key_eq(a: &OsStr, b: &OsStr) -> bool {
    imp::env_key_eq(a, b)
}

/// Variables kept by the strict (allowlist) isolation mode. A trailing `*` matches a prefix.
pub fn strict_env_allowlist() -> &'static [&'static str] {
    imp::STRICT_ENV_ALLOWLIST
}

/// Path rendered for CMake `-D` values, which mis-handle backslashes.
pub fn to_cmake_path(p: &Path) -> String {
    imp::to_cmake_path(p)
}

/// Canonicalize without producing `\\?\` verbatim paths on Windows.
pub fn canonicalize(p: &Path) -> io::Result<PathBuf> {
    dunce::canonicalize(p)
}

/// Canonicalize the longest existing ancestor and re-append the rest, so paths that do not
/// exist yet (e.g. a fresh ZEM_HOME) can still be compared with canonical ones.
pub fn canonicalize_lenient(p: &Path) -> PathBuf {
    let mut existing = p;
    let mut rest = Vec::new();
    loop {
        if let Ok(canon) = canonicalize(existing) {
            return rest.iter().rev().fold(canon, |acc, part| acc.join(part));
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_os_string());
                existing = parent;
            }
            _ => return p.to_path_buf(),
        }
    }
}

/// Host name used in Zephyr SDK release asset names, e.g. `macos-aarch64`.
pub fn sdk_host() -> Option<&'static str> {
    match (Os::current(), arch_name()) {
        (Os::Mac, "aarch64") => Some("macos-aarch64"),
        (Os::Mac, "x86_64") => Some("macos-x86_64"),
        (Os::Linux, "x86_64") => Some("linux-x86_64"),
        (Os::Linux, "aarch64") => Some("linux-aarch64"),
        (Os::Windows, "x86_64") => Some("windows-x86_64"),
        _ => None,
    }
}

pub fn sdk_archive_ext() -> &'static str {
    imp::SDK_ARCHIVE_EXT
}

/// Target triple in uv release asset names, e.g. `aarch64-apple-darwin`. Linux uses the
/// statically linked musl builds, which run on any distribution.
pub fn uv_target() -> Option<&'static str> {
    match (Os::current(), arch_name()) {
        (Os::Mac, "aarch64") => Some("aarch64-apple-darwin"),
        (Os::Mac, "x86_64") => Some("x86_64-apple-darwin"),
        (Os::Linux, "x86_64") => Some("x86_64-unknown-linux-musl"),
        (Os::Linux, "aarch64") => Some("aarch64-unknown-linux-musl"),
        _ => None,
    }
}

pub fn uv_archive_ext() -> &'static str {
    imp::UV_ARCHIVE_EXT
}

/// Unpack a uv release archive (`uv_archive_ext`) into `dest`.
pub fn extract_uv_archive(archive: &Path, dest: &Path) -> io::Result<()> {
    imp::extract_uv_archive(archive, dest)
}

/// Unpack an SDK release archive (`sdk_archive_ext`) into `dest`.
pub fn extract_sdk_archive(archive: &Path, dest: &Path) -> io::Result<()> {
    imp::extract_sdk_archive(archive, dest)
}

/// TLS backend for downloads: rustls with bundled roots on Unix, the OS (SChannel and the
/// Windows certificate store) on Windows.
pub fn http_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .tls_config(imp::tls_config())
        .build()
        .into()
}

/// Hand the terminal over to `cmd`: `exec` on Unix, so signals and the exit code pass through
/// unchanged; spawn, wait and exit with the child's code on Windows. Returns only on failure.
pub fn run_replacing(cmd: Command) -> ZemError {
    let command = crate::process::display_command(&cmd);
    ZemError::Spawn {
        command,
        source: imp::run_replacing(cmd),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SerialAccess {
    Ok,
    MissingGroup { candidates: &'static [&'static str] },
    Unknown,
    NotApplicable,
}

pub fn serial_access() -> SerialAccess {
    imp::serial_access()
}

/// How to install a host tool: a command `zem doctor --fix` can print as-is, plus an
/// optional alternative for humans.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstallHint {
    pub command: &'static str,
    pub alternative: Option<&'static str>,
}

impl InstallHint {
    pub const fn new(command: &'static str, alternative: Option<&'static str>) -> Self {
        InstallHint {
            command,
            alternative,
        }
    }
}

impl std::fmt::Display for InstallHint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.alternative {
            Some(alt) => write!(f, "{}  (or: {alt})", self.command),
            None => f.write_str(self.command),
        }
    }
}

pub fn install_hint(tool: &str) -> Option<InstallHint> {
    imp::install_hint(Os::current(), tool)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn venv_python_lives_in_bin_dir() {
        let venv = Path::new("venv");
        assert_eq!(venv_python(venv).parent().unwrap(), venv_bin_dir(venv));
    }

    #[test]
    fn exe_name_keeps_base() {
        assert!(exe_name("west").to_string_lossy().starts_with("west"));
    }

    #[test]
    fn env_key_eq_is_reflexive() {
        assert!(env_key_eq(OsStr::new("PATH"), OsStr::new("PATH")));
        assert!(!env_key_eq(OsStr::new("PATH"), OsStr::new("HOME")));
    }

    #[test]
    fn canonicalize_lenient_handles_missing_tail() {
        let tmp = std::env::temp_dir();
        let missing = tmp.join("zem-does-not-exist").join("child");
        let got = canonicalize_lenient(&missing);
        assert_eq!(
            got,
            canonicalize(&tmp)
                .unwrap()
                .join("zem-does-not-exist")
                .join("child")
        );
    }
}
