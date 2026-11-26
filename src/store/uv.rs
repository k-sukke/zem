//! The uv binary zem builds Python environments with.
//!
//! zem does not use a uv from the host `PATH`: that would make the result depend on whatever
//! version the user happens to have. Instead it downloads one pinned release into
//! `ZEM_HOME/tools/uv-<version>`, verifies it against a checksum compiled into zem and reuses
//! it from then on. Nothing outside `ZEM_HOME` is touched.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::{MARKER_SCHEMA, StagedDir, download, read_marker, write_marker};
use crate::error::{Result, ZemError};
use crate::fsutil;
use crate::home::ZemHome;
use crate::platform;

/// The uv release zem uses. To upgrade, change this and every entry of [`CHECKSUMS`]
/// (`uv-<target>.tar.gz.sha256` next to each release asset).
pub const UV_VERSION: &str = "0.13.10";

/// Overrides where releases are downloaded from, for mirrors. The layout must match GitHub's:
/// `<base>/<version>/uv-<target>.tar.gz`. The checksum is verified either way.
pub const BASE_URL_ENV: &str = "ZEM_UV_BASE_URL";

const RELEASE_BASE: &str = "https://github.com/astral-sh/uv/releases/download";

/// sha256 of `uv-<target>.tar.gz` for [`UV_VERSION`].
const CHECKSUMS: &[(&str, &str)] = &[
    (
        "aarch64-apple-darwin",
        "e419bf07533bdbf48795fc793601c834dd79db57a8c49ce8e25cfe3a8639958d",
    ),
    (
        "x86_64-apple-darwin",
        "153e03197588204c04fc4c81cc4bafbfa4280222a385e4ee36a87aeec0ae63b4",
    ),
    (
        "x86_64-unknown-linux-musl",
        "5597970380083a340b7983af05a84824b4db1c62b7059c796935b7fd6dc95fd2",
    ),
    (
        "aarch64-unknown-linux-musl",
        "68174084fe40b6300361c5b2038066120dc62c92920c22a0f31f6fc74b066b57",
    ),
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct UvMarker {
    schema: u32,
    version: String,
    sha256: String,
}

/// What to download: a release, for one platform, and the checksum it must have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UvRelease {
    pub version: String,
    pub target: String,
    pub sha256: String,
    pub base_url: String,
}

impl UvRelease {
    /// The release pinned by this zem for the current host, honouring [`BASE_URL_ENV`].
    pub fn pinned() -> Result<Self> {
        let base = std::env::var(BASE_URL_ENV).ok();
        Self::pinned_for(platform::uv_target(), base.as_deref())
    }

    fn pinned_for(target: Option<&str>, base_url: Option<&str>) -> Result<Self> {
        let unsupported = || ZemError::UvUnsupportedHost {
            os: platform::Os::current().name(),
            arch: platform::arch_name(),
        };
        let target = target.ok_or_else(unsupported)?;
        let sha256 = CHECKSUMS
            .iter()
            .find(|(t, _)| *t == target)
            .map(|(_, sum)| *sum)
            .ok_or_else(unsupported)?;
        let base_url = base_url
            .map(|b| b.trim().trim_end_matches('/'))
            .filter(|b| !b.is_empty())
            .unwrap_or(RELEASE_BASE);
        Ok(UvRelease {
            version: UV_VERSION.to_string(),
            target: target.to_string(),
            sha256: sha256.to_string(),
            base_url: base_url.to_string(),
        })
    }

    fn asset(&self) -> String {
        format!("uv-{}{}", self.target, platform::uv_archive_ext())
    }

    fn url(&self) -> String {
        format!("{}/{}/{}", self.base_url, self.version, self.asset())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UvInstall {
    pub path: PathBuf,
    pub version: String,
    /// Whether this call downloaded it (false when it was already installed).
    pub fetched: bool,
}

pub struct UvStore<'a> {
    pub home: &'a ZemHome,
    pub release: UvRelease,
}

impl UvStore<'_> {
    pub fn dir(&self) -> PathBuf {
        self.home
            .tools_dir()
            .join(format!("uv-{}", self.release.version))
    }

    pub fn binary(&self) -> PathBuf {
        self.dir().join(platform::exe_name("uv"))
    }

    /// Complete and matching the expected release: the marker is written last.
    pub fn installed(&self) -> bool {
        read_marker::<UvMarker>(&self.dir()).is_some_and(|m| {
            m.schema == MARKER_SCHEMA
                && m.version == self.release.version
                && m.sha256 == self.release.sha256
        }) && self.binary().is_file()
    }

    fn install(&self, fetched: bool) -> UvInstall {
        UvInstall {
            path: self.binary(),
            version: self.release.version.clone(),
            fetched,
        }
    }

    /// Make sure the pinned uv is installed, downloading it if needed.
    pub fn ensure(&self) -> Result<UvInstall> {
        if self.installed() {
            return Ok(self.install(false));
        }
        let _lock = super::lock(self.home, &format!("uv-{}", self.release.version))?;
        if self.installed() {
            return Ok(self.install(false));
        }

        let tools = self.home.tools_dir();
        fsutil::create_dir_all(&tools)?;
        let asset = self.release.asset();
        let archive = self
            .home
            .cache_dir()
            .join("downloads")
            .join(format!("uv-{}", self.release.version))
            .join(&asset);
        download::fetch_verified(&self.release.url(), &archive, &self.release.sha256, &asset)?;

        // Unpack next to the destination so the final rename is atomic.
        let staged = StagedDir::new_in(&tools)?;
        platform::extract_uv_archive(&archive, staged.path()).map_err(ZemError::io(&archive))?;
        let top = staged.path().join(format!("uv-{}", self.release.target));
        if !top.join(platform::exe_name("uv")).is_file() {
            return Err(ZemError::CorruptState {
                path: archive,
                message: format!("archive does not contain uv-{}/uv", self.release.target),
            });
        }

        let dir = self.dir();
        fsutil::remove_dir_all_if_exists(&dir)?;
        fs::rename(&top, &dir).map_err(ZemError::io(&dir))?;
        write_marker(
            &dir,
            &UvMarker {
                schema: MARKER_SCHEMA,
                version: self.release.version.clone(),
                sha256: self.release.sha256.clone(),
            },
        )?;
        let _ = fs::remove_file(&archive);
        Ok(self.install(true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_supported_target_has_a_checksum() {
        for target in [
            "aarch64-apple-darwin",
            "x86_64-apple-darwin",
            "x86_64-unknown-linux-musl",
            "aarch64-unknown-linux-musl",
        ] {
            let release = UvRelease::pinned_for(Some(target), None).unwrap();
            assert_eq!(release.sha256.len(), 64, "{target}");
            assert!(release.sha256.chars().all(|c| c.is_ascii_hexdigit()));
        }
        if let Some(host) = platform::uv_target() {
            assert!(UvRelease::pinned_for(Some(host), None).is_ok(), "{host}");
        }
    }

    #[test]
    fn unknown_targets_are_unsupported() {
        for target in [None, Some("riscv64-unknown-linux-musl")] {
            assert!(matches!(
                UvRelease::pinned_for(target, None),
                Err(ZemError::UvUnsupportedHost { .. })
            ));
        }
    }

    #[test]
    fn url_follows_github_release_layout() {
        let release = UvRelease::pinned_for(Some("aarch64-apple-darwin"), None).unwrap();
        assert_eq!(
            release.url(),
            format!(
                "https://github.com/astral-sh/uv/releases/download/{UV_VERSION}/uv-aarch64-apple-darwin{}",
                platform::uv_archive_ext()
            )
        );
    }

    #[test]
    fn base_url_override_is_normalised() {
        let release =
            UvRelease::pinned_for(Some("x86_64-apple-darwin"), Some(" http://mirror/uv/ "))
                .unwrap();
        assert_eq!(release.base_url, "http://mirror/uv");
        assert!(release.url().starts_with("http://mirror/uv/"));

        let blank = UvRelease::pinned_for(Some("x86_64-apple-darwin"), Some("  ")).unwrap();
        assert_eq!(blank.base_url, RELEASE_BASE);
    }

    #[test]
    fn nothing_is_installed_in_an_empty_home() {
        let tmp = tempfile::tempdir().unwrap();
        let home = ZemHome::new(tmp.path().to_path_buf());
        let store = UvStore {
            home: &home,
            release: UvRelease::pinned_for(Some("aarch64-apple-darwin"), None).unwrap(),
        };
        assert!(!store.installed());
        assert!(store.binary().starts_with(home.tools_dir()));
    }
}
