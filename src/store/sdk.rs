//! Minimal Zephyr SDK plus only the toolchains a project needs, downloaded straight from the
//! sdk-ng release assets. `west sdk install` is not used: it relies on the rate-limited GitHub
//! API and always runs `setup.sh`, which registers the SDK in `~/.cmake/packages`.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, BufReader, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};

use indicatif::{ProgressBar, ProgressStyle};
use sha2::{Digest, Sha256};

use crate::config::SdkVersion;
use crate::error::{Result, ZemError};
use crate::fsutil;
use crate::home::ZemHome;
use crate::platform;

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

pub fn missing_toolchains<'a>(sdk_dir: &Path, toolchains: &[&'a str]) -> Vec<&'a str> {
    toolchains
        .iter()
        .copied()
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

pub fn parse_sha256_sum(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let hash = parts.next()?;
            let name = parts.next()?.trim_start_matches('*');
            Some((name.to_string(), hash.to_ascii_lowercase()))
        })
        .collect()
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
            .cache_dir()
            .join("downloads")
            .join(format!("sdk-{}", self.version))
    }

    fn url(&self, asset: &str) -> String {
        format!("{RELEASE_BASE}/v{}/{asset}", self.version)
    }

    pub fn ensure(&self, toolchains: &[&str]) -> Result<SdkOutcome> {
        let dir = self.dir();
        let _lock = super::lock(self.home, &format!("sdk-{}", self.version))?;
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
            let url = self.url("sha256.sum");
            let body = http_get(&url)?
                .into_body()
                .read_to_string()
                .map_err(|e| download_error(&url, e))?;
            fsutil::atomic_write(&path, body.as_bytes())?;
        }
        let text = fs::read_to_string(&path).map_err(ZemError::io(&path))?;
        Ok(parse_sha256_sum(&text))
    }

    /// Download `asset` into the cache unless a verified copy is already there.
    fn fetch(&self, asset: &str, sums: &BTreeMap<String, String>) -> Result<PathBuf> {
        let expected = sums.get(asset).ok_or_else(|| ZemError::SdkAsset {
            version: self.version.to_string(),
            asset: asset.to_string(),
        })?;
        let path = self.downloads_dir().join(asset);
        if path.is_file() && file_sha256(&path)? == *expected {
            return Ok(path);
        }

        let url = self.url(asset);
        let response = http_get(&url)?;
        let total = response.body().content_length();
        let partial = path.with_extension("part");
        fsutil::create_dir_all(&self.downloads_dir())?;
        let mut file = File::create(&partial).map_err(ZemError::io(&partial))?;
        let progress = progress_bar(asset, total);
        let mut reader = response.into_body().into_reader();
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = reader.read(&mut buf).map_err(|e| download_error(&url, e))?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            file.write_all(&buf[..n]).map_err(ZemError::io(&partial))?;
            progress.inc(n as u64);
        }
        progress.finish_and_clear();
        drop(file);

        let actual = hex(&hasher.finalize());
        if actual != *expected {
            let _ = fs::remove_file(&partial);
            return Err(ZemError::ChecksumMismatch {
                file: path,
                expected: expected.clone(),
                actual,
            });
        }
        fs::rename(&partial, &path).map_err(ZemError::io(&path))?;
        Ok(path)
    }
}

fn http_get(url: &str) -> Result<ureq::http::Response<ureq::Body>> {
    platform::http_agent()
        .get(url)
        .header("User-Agent", concat!("zem/", env!("CARGO_PKG_VERSION")))
        .call()
        .map_err(|e| download_error(url, e))
}

fn download_error(url: &str, e: impl std::fmt::Display) -> ZemError {
    ZemError::Download {
        url: url.to_string(),
        message: e.to_string(),
    }
}

fn progress_bar(name: &str, total: Option<u64>) -> ProgressBar {
    if !io::stderr().is_terminal() {
        return ProgressBar::hidden();
    }
    let bar = match total {
        Some(len) => ProgressBar::new(len),
        None => ProgressBar::new_spinner(),
    };
    bar.set_style(
        ProgressStyle::with_template("      {msg} [{bar:30}] {bytes}/{total_bytes}")
            .unwrap_or_else(|_| ProgressStyle::default_bar())
            .progress_chars("=> "),
    );
    bar.set_message(name.to_string());
    bar
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn file_sha256(path: &Path) -> Result<String> {
    let mut file = BufReader::new(File::open(path).map_err(ZemError::io(path))?);
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(ZemError::io(path))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex(&hasher.finalize()))
}

/// Extract into a temp dir next to the destination (same filesystem, so the final rename is
/// atomic) and check that the archive contains `top/`.
fn extract_staged(archive: &Path, parent: &Path, top: &str) -> Result<tempfile::TempDir> {
    let staged = tempfile::Builder::new()
        .prefix(".zem-extract-")
        .tempdir_in(parent)
        .map_err(ZemError::io(parent))?;
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
    fn parses_sha256_sum() {
        let sums = parse_sha256_sum(
            "AB12  zephyr-sdk-0.17.2_macos-aarch64_minimal.tar.xz\n\ncd34 *toolchain_x.tar.xz\n",
        );
        assert_eq!(
            sums["zephyr-sdk-0.17.2_macos-aarch64_minimal.tar.xz"],
            "ab12"
        );
        assert_eq!(sums["toolchain_x.tar.xz"], "cd34");
    }

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
