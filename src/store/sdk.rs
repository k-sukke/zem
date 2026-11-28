//! Minimal Zephyr SDK plus only the toolchains a project needs, downloaded straight from the
//! sdk-ng release assets. `west sdk install` is not used: it relies on the rate-limited GitHub
//! API and always runs `setup.sh`, which registers the SDK in `~/.cmake/packages`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::SdkVersion;
use crate::error::{Result, ZemError};
use crate::fsutil;
use crate::home::ZemHome;
use crate::platform;
use crate::store::download;
use crate::store::{StagedDir, lock};

pub const RELEASE_BASE: &str = "https://github.com/zephyrproject-rtos/sdk-ng/releases/download";
pub const SDK_VERSION_FILE: &str = "SDK_VERSION";

/// `[sdk].version`, or the `SDK_VERSION` file of the Zephyr checkout for `auto`.
pub fn resolve_version(cfg: &SdkVersion, zephyr_base: &Path) -> Result<String> {
    match cfg {
        SdkVersion::Exact(v) => Ok(v.trim_start_matches('v').to_string()),
        SdkVersion::Auto => {
            let path = zephyr_base.join(SDK_VERSION_FILE);
            let text = fs::read_to_string(&path).map_err(ZemError::io(&path))?;
            Ok(text.trim().trim_start_matches('v').to_string())
        }
    }
}

pub fn toolchain_gcc(sdk_dir: &Path, toolchain: &str) -> PathBuf {
    sdk_dir
        .join(toolchain)
        .join("bin")
        .join(platform::exe_name(&format!("{toolchain}-gcc")))
}

pub fn minimal_installed(sdk_dir: &Path) -> bool {
    sdk_dir.join("sdk_version").is_file()
        && sdk_dir
            .join("cmake")
            .join("Zephyr-sdkConfig.cmake")
            .is_file()
}

pub fn missing_toolchains<'a, S: AsRef<str>>(sdk_dir: &Path, toolchains: &'a [S]) -> Vec<&'a str> {
    toolchains
        .iter()
        .map(AsRef::as_ref)
        .filter(|tc| !toolchain_gcc(sdk_dir, tc).is_file())
        .collect()
}

pub fn minimal_asset(version: &str, host: &str) -> String {
    format!(
        "zephyr-sdk-{version}_{host}_minimal{}",
        platform::sdk_archive_ext()
    )
}

pub fn toolchain_asset(host: &str, toolchain: &str) -> String {
    format!(
        "toolchain_{host}_{toolchain}{}",
        platform::sdk_archive_ext()
    )
}

pub struct SdkStore<'a> {
    pub home: &'a ZemHome,
    pub version: &'a str,
}

pub struct SdkOutcome {
    pub dir: PathBuf,
    pub installed: Vec<String>,
}

impl SdkStore<'_> {
    pub fn dir(&self) -> PathBuf {
        self.home.sdk_install_dir(self.version)
    }

    fn downloads_dir(&self) -> PathBuf {
        self.home
            .downloads_dir()
            .join(format!("sdk-{}", self.version))
    }

    fn url(&self, asset: &str) -> String {
        format!("{RELEASE_BASE}/v{}/{asset}", self.version)
    }

    pub fn ensure<S: AsRef<str>>(&self, toolchains: &[S]) -> Result<SdkOutcome> {
        let dir = self.dir();
        let _lock = lock(self.home, &format!("sdk-{}", self.version))?;
        let missing = missing_toolchains(&dir, toolchains);
        let need_minimal = !minimal_installed(&dir);
        if missing.is_empty() && !need_minimal {
            return Ok(SdkOutcome {
                dir,
                installed: Vec::new(),
            });
        }

        let host = platform::sdk_host().ok_or_else(|| ZemError::SdkAsset {
            version: self.version.to_string(),
            asset: format!(
                "for {} {}",
                platform::Os::current().name(),
                platform::arch_name()
            ),
        })?;
        let sums = self.checksums()?;
        let sdk_root = self.home.sdk_dir();
        fsutil::create_dir_all(&sdk_root)?;
        let mut installed = Vec::new();

        if need_minimal {
            let asset = minimal_asset(self.version, host);
            let archive = self.fetch(&asset, &sums)?;
            let top = format!("zephyr-sdk-{}", self.version);
            let staged = extract_staged(&archive, &sdk_root, &top)?;
            // A partial SDK dir can only hold toolchains installed by an earlier run; keep them.
            if dir.is_dir() {
                for entry in fs::read_dir(staged.path().join(&top)).map_err(ZemError::io(&dir))? {
                    let entry = entry.map_err(ZemError::io(&dir))?;
                    let dest = dir.join(entry.file_name());
                    if !dest.exists() {
                        fs::rename(entry.path(), &dest).map_err(ZemError::io(&dest))?;
                    }
                }
            } else {
                fs::rename(staged.path().join(&top), &dir).map_err(ZemError::io(&dir))?;
            }
            let _ = fs::remove_file(&archive);
            installed.push("minimal".to_string());
        }

        for toolchain in missing {
            let asset = toolchain_asset(host, toolchain);
            let archive = self.fetch(&asset, &sums)?;
            let staged = extract_staged(&archive, &dir, toolchain)?;
            let dest = dir.join(toolchain);
            fsutil::remove_dir_all_if_exists(&dest)?;
            fs::rename(staged.path().join(toolchain), &dest).map_err(ZemError::io(&dest))?;
            let _ = fs::remove_file(&archive);
            installed.push(toolchain.to_string());
        }

        Ok(SdkOutcome { dir, installed })
    }

    fn checksums(&self) -> Result<BTreeMap<String, String>> {
        let path = self.downloads_dir().join("sha256.sum");
        if !path.is_file() {
            let body = download::get_text(&self.url("sha256.sum"))?;
            fsutil::atomic_write(&path, body.as_bytes())?;
        }
        let text = fs::read_to_string(&path).map_err(ZemError::io(&path))?;
        Ok(download::parse_sha256_sum(&text))
    }

    /// Download `asset` into the cache unless a verified copy is already there.
    fn fetch(&self, asset: &str, sums: &BTreeMap<String, String>) -> Result<PathBuf> {
        let expected = sums.get(asset).ok_or_else(|| ZemError::SdkAsset {
            version: self.version.to_string(),
            asset: asset.to_string(),
        })?;
        let path = self.downloads_dir().join(asset);
        download::fetch_verified(&self.url(asset), expected, &path)?;
        Ok(path)
    }
}

/// Extract into a staging directory next to the destination (same filesystem, so the final
/// rename is atomic) and check that the archive contains `top/`.
fn extract_staged(archive: &Path, parent: &Path, top: &str) -> Result<StagedDir> {
    let staged = StagedDir::new_in(parent)?;
    platform::extract_sdk_archive(archive, staged.path()).map_err(ZemError::io(archive))?;
    if !staged.path().join(top).is_dir() {
        return Err(ZemError::CorruptState {
            path: archive.to_path_buf(),
            message: format!("archive does not contain `{top}/`"),
        });
    }
    Ok(staged)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_names_follow_sdk_ng() {
        let ext = platform::sdk_archive_ext();
        assert_eq!(
            minimal_asset("0.17.2", "linux-x86_64"),
            format!("zephyr-sdk-0.17.2_linux-x86_64_minimal{ext}")
        );
        assert_eq!(
            toolchain_asset("macos-aarch64", "riscv64-zephyr-elf"),
            format!("toolchain_macos-aarch64_riscv64-zephyr-elf{ext}")
        );
    }

    #[test]
    fn version_from_config_or_zephyr() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join(SDK_VERSION_FILE), "0.17.2\n").unwrap();
        assert_eq!(
            resolve_version(&SdkVersion::Auto, tmp.path()).unwrap(),
            "0.17.2"
        );
        assert_eq!(
            resolve_version(&SdkVersion::Exact("v0.17.4".into()), tmp.path()).unwrap(),
            "0.17.4"
        );
    }

    #[test]
    fn detects_missing_toolchains() {
        let tmp = tempfile::tempdir().unwrap();
        let gcc = toolchain_gcc(tmp.path(), "riscv64-zephyr-elf");
        fs::create_dir_all(gcc.parent().unwrap()).unwrap();
        fs::write(&gcc, "").unwrap();
        assert_eq!(
            missing_toolchains(
                tmp.path(),
                &["riscv64-zephyr-elf", "xtensa-espressif_esp32_zephyr-elf"]
            ),
            ["xtensa-espressif_esp32_zephyr-elf"]
        );
    }
}
