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
    let asset = format!("uv-{target}{}", platform::uv_archive_ext());
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

/// Unpack a uv release archive into the version directory. Unix releases are tar.gz files
/// with `uv-<target>/uv`; Windows releases are zip files with `uv.exe` at the top level.
/// The unpacked directory is renamed into place, so a half-extracted uv is never visible.
fn install_archive(home: &ZemHome, archive: &Path, target: &str) -> Result<()> {
    let root = home.tools_dir().join("uv");
    let staged = StagedDir::new_in(&root)?;
    let unpacked = staged.path().join("unpacked");
    unpack(archive, &unpacked).map_err(ZemError::io(archive))?;

    let uv = platform::exe_name("uv");
    let top = [unpacked.join(format!("uv-{target}")), unpacked]
        .into_iter()
        .find(|dir| dir.join(&uv).is_file())
        .ok_or_else(|| ZemError::CorruptState {
            path: archive.to_path_buf(),
            message: format!("archive does not contain `{}`", uv.to_string_lossy()),
        })?;
    let dest = home.uv_dir(UV_VERSION);
    fsutil::remove_dir_all_if_exists(&dest)?;
    fs::rename(&top, &dest).map_err(ZemError::io(&dest))
}

/// Extract a `.tar.gz` or `.zip` archive, chosen by file name.
fn unpack(archive: &Path, dest: &Path) -> std::io::Result<()> {
    let file = BufReader::new(File::open(archive)?);
    if archive.extension().is_some_and(|e| e == "zip") {
        zip::ZipArchive::new(file)?
            .extract(dest)
            .map_err(std::io::Error::other)
    } else {
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
        tar.set_preserve_permissions(true);
        tar.unpack(dest)
    }
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

    const FAKE_UV: &[u8] = b"#!/bin/sh\necho uv 0.13.10\n";

    fn uv_file_name() -> String {
        platform::exe_name("uv").to_string_lossy().into_owned()
    }

    /// A tar.gz laid out like a Unix uv release, with a fake `uv` inside.
    fn fake_release(dir: &Path, target: &str) -> PathBuf {
        let archive = dir.join("uv.tar.gz");
        let gz = flate2::write::GzEncoder::new(
            File::create(&archive).unwrap(),
            flate2::Compression::fast(),
        );
        let mut builder = tar::Builder::new(gz);
        let mut header = tar::Header::new_gnu();
        header.set_size(FAKE_UV.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder
            .append_data(
                &mut header,
                format!("uv-{target}/{}", uv_file_name()),
                FAKE_UV,
            )
            .unwrap();
        builder.into_inner().unwrap().finish().unwrap();
        archive
    }

    /// A zip laid out like a Windows uv release: binaries at the top level.
    fn fake_zip_release(dir: &Path) -> PathBuf {
        use std::io::Write;

        let archive = dir.join("uv.zip");
        let mut zip = zip::ZipWriter::new(File::create(&archive).unwrap());
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for name in [uv_file_name(), "uvx.exe".to_string()] {
            zip.start_file(name, options).unwrap();
            zip.write_all(FAKE_UV).unwrap();
        }
        zip.finish().unwrap();
        archive
    }

    #[test]
    fn installs_a_flat_zip_release() {
        let tmp = tempfile::tempdir().unwrap();
        let home = ZemHome::new(tmp.path().join("zh"));
        let archive = fake_zip_release(tmp.path());
        install_archive(&home, &archive, "x86_64-pc-windows-msvc").unwrap();
        assert_eq!(fs::read(home.uv_bin(UV_VERSION)).unwrap(), FAKE_UV);
        assert!(home.uv_dir(UV_VERSION).join("uvx.exe").is_file());
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
