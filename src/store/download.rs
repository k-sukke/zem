//! HTTP downloads with sha256 verification, shared by the stores that fetch release assets
//! themselves (the Zephyr SDK and uv).

use std::fs::{self, File};
use std::io::{self, BufReader, IsTerminal, Read, Write};
use std::path::Path;

use indicatif::{ProgressBar, ProgressStyle};
use sha2::{Digest, Sha256};

use crate::error::{Result, ZemError};
use crate::fsutil;
use crate::home::keys::hex;
use crate::platform;

pub fn http_get(url: &str) -> Result<ureq::http::Response<ureq::Body>> {
    platform::http_agent()
        .get(url)
        .header("User-Agent", concat!("zem/", env!("CARGO_PKG_VERSION")))
        .call()
        .map_err(|e| download_error(url, e))
}

pub fn download_error(url: &str, e: impl std::fmt::Display) -> ZemError {
    ZemError::Download {
        url: url.to_string(),
        message: e.to_string(),
    }
}

pub fn file_sha256(path: &Path) -> Result<String> {
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

/// Download `url` to `path` unless a copy with the `expected` sha256 is already there. The
/// file only appears at `path` once its checksum matched.
pub fn fetch_verified(url: &str, path: &Path, expected: &str, label: &str) -> Result<()> {
    if path.is_file() && file_sha256(path)? == expected {
        return Ok(());
    }

    let response = http_get(url)?;
    let total = response.body().content_length();
    let partial = path.with_extension("part");
    if let Some(parent) = path.parent() {
        fsutil::create_dir_all(parent)?;
    }
    let mut file = File::create(&partial).map_err(ZemError::io(&partial))?;
    let progress = progress_bar(label, total);
    let mut reader = response.into_body().into_reader();
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf).map_err(|e| download_error(url, e))?;
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
    if actual != expected {
        let _ = fs::remove_file(&partial);
        return Err(ZemError::ChecksumMismatch {
            file: path.to_path_buf(),
            expected: expected.to_string(),
            actual,
        });
    }
    fs::rename(&partial, path).map_err(ZemError::io(path))?;
    Ok(())
}
