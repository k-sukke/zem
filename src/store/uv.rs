//! The uv release that zem uses for every Python operation. It is pinned, downloaded from the
//! astral-sh release assets, checked against the published SHA-256 and kept under
//! `ZEM_HOME/tools/uv/<version>`. The host's uv is never used.

use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Path, PathBuf};

use crate::error::{Result, ZemError};
use crate::fsutil;
use crate::home::ZemHome;
use crate::platform;
use crate::store::{StagedDir, download, lock};

pub const UV_VERSION: &str = "0.13.10";
const RELEASE_BASE: &str = "https://github.com/astral-sh/uv/releases/download";

/// A usable uv binary, and whether this call downloaded it.
#[derive(Debug, Clone)]
pub struct Uv {
    pub path: PathBuf,
    pub fetched: bool,
}

/// Make sure the pinned uv is installed and return its path.
pub fn ensure(home: &ZemHome) -> Result<Uv> {
    let path = home.uv_bin(UV_VERSION);
    if path.is_file() {
        return Ok(Uv {
            path,
            fetched: false,
        });
    }
    let target = platform::uv_target().ok_or_else(|| {
        unsupported(&format!(
            "for {} {}",
            platform::Os::current().name(),
            platform::arch_name()
        ))
    })?;

    let _lock = lock(home, &format!("uv-{UV_VERSION}"))?;
    if path.is_file() {
        return Ok(Uv {
            path,
            fetched: false,
        });
    }
    let asset = format!("uv-{target}.tar.gz");
    let url = format!("{RELEASE_BASE}/{UV_VERSION}/{asset}");
    let checksum = download::get_text(&format!("{url}.sha256"))?;
    let expected = download::sha256_of_line(&checksum).ok_or_else(|| unsupported(&asset))?;
    let archive = home
        .downloads_dir()
        .join(format!("uv-{UV_VERSION}"))
        .join(&asset);
    download::fetch_verified(&url, &expected, &archive)?;
    install_archive(home, &archive, target)?;
    let _ = fs::remove_file(&archive);
    Ok(Uv {
        path,
        fetched: true,
    })
}

/// Unpack a uv release archive (`uv-<target>/uv`, `uv-<target>/uvx`) into the version directory.
/// The unpacked directory is renamed into place, so a half-extracted uv is never visible.
fn install_archive(home: &ZemHome, archive: &Path, target: &str) -> Result<()> {
    let root = home.tools_dir().join("uv");
    let staged = StagedDir::new_in(&root)?;
    let file = File::open(archive).map_err(ZemError::io(archive))?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(BufReader::new(file)));
    tar.set_preserve_permissions(true);
    tar.unpack(staged.path()).map_err(ZemError::io(archive))?;

    let top = staged.path().join(format!("uv-{target}"));
    if !top.join(platform::exe_name("uv")).is_file() {
        return Err(ZemError::CorruptState {
            path: archive.to_path_buf(),
            message: format!("archive does not contain `uv-{target}/uv`"),
        });
    }
    let dest = home.uv_dir(UV_VERSION);
    fsutil::remove_dir_all_if_exists(&dest)?;
    fs::rename(&top, &dest).map_err(ZemError::io(&dest))
}

fn unsupported(asset: &str) -> ZemError {
    ZemError::ToolAsset {
        tool: "uv",
        version: UV_VERSION.to_string(),
        asset: asset.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tar.gz laid out like a uv release, with a fake `uv` inside.
    fn fake_release(dir: &Path, target: &str) -> PathBuf {
        let archive = dir.join("uv.tar.gz");
        let gz = flate2::write::GzEncoder::new(
            File::create(&archive).unwrap(),
            flate2::Compression::fast(),
        );
        let mut builder = tar::Builder::new(gz);
        let body = b"#!/bin/sh\necho uv 0.13.10\n";
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder
            .append_data(
                &mut header,
                format!("uv-{target}/{}", platform::exe_name("uv").to_string_lossy()),
                &body[..],
            )
            .unwrap();
        builder.into_inner().unwrap().finish().unwrap();
        archive
    }

    #[test]
    fn installs_the_unpacked_release_under_its_version() {
        let tmp = tempfile::tempdir().unwrap();
        let home = ZemHome::new(tmp.path().join("zh"));
        let archive = fake_release(tmp.path(), "test-target");
        install_archive(&home, &archive, "test-target").unwrap();
        assert!(home.uv_bin(UV_VERSION).is_file());
        assert!(
            fs::read_dir(home.tools_dir().join("uv"))
                .unwrap()
                .flatten()
                .all(|e| !e.file_name().to_string_lossy().starts_with(".zem-staging-"))
        );
    }

    #[test]
    fn archive_without_uv_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let home = ZemHome::new(tmp.path().join("zh"));
        let archive = fake_release(tmp.path(), "test-target");
        assert!(matches!(
            install_archive(&home, &archive, "other-target"),
            Err(ZemError::CorruptState { .. })
        ));
        assert!(!home.uv_bin(UV_VERSION).exists());
    }

    #[test]
    fn existing_binary_is_reused_without_network() {
        let tmp = tempfile::tempdir().unwrap();
        let home = ZemHome::new(tmp.path().join("zh"));
        let bin = home.uv_bin(UV_VERSION);
        fs::create_dir_all(bin.parent().unwrap()).unwrap();
        fs::write(&bin, "").unwrap();
        let uv = ensure(&home).unwrap();
        assert_eq!(uv.path, bin);
        assert!(!uv.fetched);
    }
}
