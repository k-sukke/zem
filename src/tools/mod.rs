use std::fmt;
use std::path::PathBuf;
use std::process::Command;

use crate::env::EnvBuilder;
use crate::error::{Result, ZemError};
use crate::platform;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Version {
            major,
            minor,
            patch,
        }
    }

    /// First `N.N[.N]` found in free-form `--version` output.
    pub fn parse_first(text: &str) -> Option<Version> {
        text.split(|c: char| !(c.is_ascii_digit() || c == '.'))
            .map(|token| token.trim_matches('.'))
            .find_map(|token| {
                let mut nums = token.split('.').map(|p| p.parse::<u32>().ok());
                let major = nums.next()??;
                let minor = nums.next()??;
                let patch = nums.next().flatten().unwrap_or(0);
                Some(Version::new(major, minor, patch))
            })
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[derive(Debug, Clone)]
pub struct Probe {
    pub path: PathBuf,
    pub version: Option<Version>,
}

/// Find `tool` on the `PATH` of `env`, the environment zem runs its child processes with.
/// Fails with an install hint when it is not found.
pub fn require_in_env(env: &EnvBuilder, tool: &'static str) -> Result<PathBuf> {
    env.get("PATH")
        .and_then(|path| {
            let cwd = std::env::current_dir().ok()?;
            which::which_in(tool, Some(path), cwd).ok()
        })
        .ok_or_else(|| ZemError::ToolNotFound {
            tool,
            advice: platform::install_hint(tool).map(|h| h.to_string()),
        })
}

/// Locate a tool on PATH (honouring PATHEXT on Windows) and read its `--version`.
pub fn probe(tool: &str) -> Option<Probe> {
    let path = which::which(tool).ok()?;
    let output = Command::new(&path).arg("--version").output().ok()?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    Some(Probe {
        version: Version::parse_first(&text),
        path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_world_version_strings() {
        let cases = [
            ("git version 2.39.5 (Apple Git-154)", Version::new(2, 39, 5)),
            (
                "cmake version 3.31.6\n\nCMake suite maintained by Kitware",
                Version::new(3, 31, 6),
            ),
            ("1.12.1\n", Version::new(1, 12, 1)),
            ("Version: DTC 1.7.2", Version::new(1, 7, 2)),
            ("uv 0.9.5 (Homebrew 2025-10-21)", Version::new(0, 9, 5)),
            ("Python 3.13.2", Version::new(3, 13, 2)),
            ("West version: v1.4.0", Version::new(1, 4, 0)),
            ("Python 3.12", Version::new(3, 12, 0)),
        ];
        for (text, expected) in cases {
            assert_eq!(Version::parse_first(text), Some(expected), "{text:?}");
        }
    }

    #[test]
    fn rejects_text_without_versions() {
        assert_eq!(Version::parse_first("no version here 42"), None);
    }

    #[test]
    fn version_ordering() {
        assert!(Version::new(3, 20, 5) > Version::new(3, 20, 0));
        assert!(Version::new(3, 19, 9) < Version::new(3, 20, 5));
    }
}
