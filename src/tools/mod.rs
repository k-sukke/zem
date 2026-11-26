use std::fmt;
use std::path::PathBuf;
use std::process::Command;

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

#[derive(Debug, Clone, Copy)]
pub struct ToolSpec {
    pub name: &'static str,
    pub candidates: &'static [&'static str],
    pub version_args: &'static [&'static str],
}

pub const GIT: ToolSpec = ToolSpec {
    name: "git",
    candidates: &["git"],
    version_args: &["--version"],
};
pub const UV: ToolSpec = ToolSpec {
    name: "uv",
    candidates: &["uv"],
    version_args: &["--version"],
};
pub const CMAKE: ToolSpec = ToolSpec {
    name: "cmake",
    candidates: &["cmake"],
    version_args: &["--version"],
};
pub const NINJA: ToolSpec = ToolSpec {
    name: "ninja",
    candidates: &["ninja"],
    version_args: &["--version"],
};
pub const DTC: ToolSpec = ToolSpec {
    name: "dtc",
    candidates: &["dtc"],
    version_args: &["--version"],
};
pub const WEST: ToolSpec = ToolSpec {
    name: "west",
    candidates: &["west"],
    version_args: &["--version"],
};

pub const MIN_CMAKE: Version = Version::new(3, 20, 5);

#[derive(Debug, Clone)]
pub struct Probe {
    pub path: PathBuf,
    pub version: Option<Version>,
}

/// Locate a tool on PATH (honouring PATHEXT on Windows) and read its version.
pub fn probe(spec: &ToolSpec) -> Option<Probe> {
    spec.candidates.iter().find_map(|candidate| {
        let path = which::which(candidate).ok()?;
        let output = Command::new(&path).args(spec.version_args).output().ok()?;
        let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&output.stderr));
        Some(Probe {
            version: Version::parse_first(&text),
            path,
        })
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
        assert!(Version::new(3, 19, 9) < MIN_CMAKE);
    }
}
