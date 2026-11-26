use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::board::Arch;

pub const DEFAULT_ZEPHYR_URL: &str = "https://github.com/zephyrproject-rtos/zephyr";
pub const DEFAULT_PYTHON_VERSION: &str = "3.12";

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ZemConfig {
    pub schema: u32,
    pub project: Project,
    pub zephyr: Zephyr,
    pub target: Target,
    #[serde(default)]
    pub modules: Modules,
    #[serde(default)]
    pub blobs: Blobs,
    #[serde(default)]
    pub sdk: Sdk,
    #[serde(default)]
    pub python: Python,
    #[serde(default)]
    pub build: Build,
    #[serde(default)]
    pub env: EnvConfig,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Zephyr {
    pub version: String,
    #[serde(default = "default_zephyr_url")]
    pub url: String,
}

fn default_zephyr_url() -> String {
    DEFAULT_ZEPHYR_URL.to_string()
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub board: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arch: Option<Arch>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct Modules {
    pub features: Vec<Feature>,
    pub extra: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Feature {
    Mbedtls,
    Lvgl,
    Littlefs,
    Mcuboot,
    Segger,
}

impl Feature {
    pub const ALL: [Feature; 5] = [
        Feature::Mbedtls,
        Feature::Lvgl,
        Feature::Littlefs,
        Feature::Mcuboot,
        Feature::Segger,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Feature::Mbedtls => "mbedtls",
            Feature::Lvgl => "lvgl",
            Feature::Littlefs => "littlefs",
            Feature::Mcuboot => "mcuboot",
            Feature::Segger => "segger",
        }
    }
}

impl fmt::Display for Feature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Feature {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Feature::ALL
            .into_iter()
            .find(|f| f.as_str() == s)
            .ok_or_else(|| {
                let known: Vec<_> = Feature::ALL.iter().map(|f| f.as_str()).collect();
                format!(
                    "unknown feature `{s}` (expected one of: {})",
                    known.join(", ")
                )
            })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct Blobs {
    pub accept_licenses: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct Sdk {
    pub version: SdkVersion,
}

/// `"auto"` follows the `SDK_VERSION` file of the selected Zephyr revision.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(from = "String", into = "String")]
pub enum SdkVersion {
    #[default]
    Auto,
    Exact(String),
}

impl From<String> for SdkVersion {
    fn from(s: String) -> Self {
        let s = s.trim();
        if s.is_empty() || s.eq_ignore_ascii_case("auto") {
            SdkVersion::Auto
        } else {
            SdkVersion::Exact(s.to_string())
        }
    }
}

impl From<SdkVersion> for String {
    fn from(v: SdkVersion) -> Self {
        match v {
            SdkVersion::Auto => "auto".to_string(),
            SdkVersion::Exact(s) => s,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct Python {
    pub version: String,
    pub host_tools_via_pip: bool,
    pub extra_requirements: Vec<String>,
}

impl Default for Python {
    fn default() -> Self {
        Python {
            version: DEFAULT_PYTHON_VERSION.to_string(),
            host_tools_via_pip: true,
            extra_requirements: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct Build {
    pub dir: RelPath,
    pub cmake_args: Vec<String>,
}

impl Default for Build {
    fn default() -> Self {
        Build {
            dir: RelPath::from_static("build"),
            cmake_args: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct EnvConfig {
    pub isolation: Isolation,
    pub passthrough: Vec<String>,
    pub set: BTreeMap<String, String>,
}

/// How much of the caller's environment reaches the tools zem runs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Isolation {
    /// Remove known conflicting variables (ESP-IDF, other Zephyr setups, Python envs).
    #[default]
    Denylist,
    /// Keep only a small per-OS allowlist, PATH and `[env].passthrough`.
    Strict,
}

/// A relative path stored with `/` separators so the same zem.toml works on every OS.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct RelPath(String);

impl RelPath {
    fn from_static(s: &'static str) -> Self {
        RelPath(s.to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn to_path(&self, base: &Path) -> PathBuf {
        self.0
            .split('/')
            .filter(|c| !c.is_empty() && *c != ".")
            .fold(base.to_path_buf(), |acc, c| acc.join(c))
    }
}

impl Default for RelPath {
    fn default() -> Self {
        RelPath::from_static(".")
    }
}

impl TryFrom<String> for RelPath {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        let normalized = s.trim().replace('\\', "/");
        let has_drive = normalized.as_bytes().get(1) == Some(&b':');
        if normalized.starts_with('/') || has_drive {
            return Err(format!("`{s}` must be a path relative to zem.toml"));
        }
        let trimmed = normalized.trim_end_matches('/');
        Ok(RelPath(if trimmed.is_empty() {
            ".".to_string()
        } else {
            trimmed.to_string()
        }))
    }
}

impl From<RelPath> for String {
    fn from(p: RelPath) -> Self {
        p.0
    }
}

impl fmt::Display for RelPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rel_path_normalizes_separators() {
        let p = RelPath::try_from(r"app\src\".to_string()).unwrap();
        assert_eq!(p.as_str(), "app/src");
        assert_eq!(
            p.to_path(Path::new("root")),
            Path::new("root").join("app").join("src")
        );
    }

    #[test]
    fn rel_path_dot_is_base() {
        let p = RelPath::try_from("./".to_string()).unwrap();
        assert_eq!(p.as_str(), ".");
        assert_eq!(p.to_path(Path::new("root")), Path::new("root"));
    }

    #[test]
    fn rel_path_rejects_absolute() {
        assert!(RelPath::try_from("/abs".to_string()).is_err());
        assert!(RelPath::try_from(r"C:\abs".to_string()).is_err());
    }

    #[test]
    fn sdk_version_auto_and_exact() {
        assert_eq!(SdkVersion::from("auto".to_string()), SdkVersion::Auto);
        assert_eq!(
            SdkVersion::from("0.17.4".to_string()),
            SdkVersion::Exact("0.17.4".into())
        );
    }

    #[test]
    fn feature_from_str() {
        assert_eq!("mbedtls".parse::<Feature>(), Ok(Feature::Mbedtls));
        assert!(
            "openthread"
                .parse::<Feature>()
                .unwrap_err()
                .contains("expected one of")
        );
    }
}
