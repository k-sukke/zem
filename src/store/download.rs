//! Downloads whose content is pinned by a SHA-256 checksum. Shared by the Zephyr SDK and uv
//! stores: a file only appears at its final path once its checksum matched.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, BufReader, IsTerminal, Read, Write};
use std::path::Path;

use indicatif::{ProgressBar, ProgressStyle};
use sha2::{Digest, Sha256};

use crate::error::{Result, ZemError};
use crate::fsutil;
use crate::platform;

pub fn get_text(url: &str) -> Result<String> {
    http_get(url)?
        .into_body()
        .read_to_string()
        .map_err(|e| download_error(url, e))
}

/// The hash of a single-file checksum (`<hash>  <name>` or `<hash>`), lowercased.
pub fn sha256_of_line(text: &str) -> Option<String> {
    let hash = text.split_whitespace().next()?.to_ascii_lowercase();
    let valid = hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit());
    valid.then_some(hash)
}

/// Parse a `sha256sum`-style listing into `name -> hash`.
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

/// Download `url` to `path` unless `path` already holds content with `expected` as its
/// SHA-256. The download goes to a `.part` file first and is renamed only after it verified.
pub fn fetch_verified(url: &str, expected: &str, path: &Path) -> Result<()> {
    if path.is_file() && file_sha256(path)? == expected {
        return Ok(());
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let parent = path.parent().unwrap_or(Path::new("."));
    fsutil::create_dir_all(parent)?;

    let response = http_get(url)?;
    let total = response.body().content_length();
    let partial = parent.join(format!("{name}.part"));
    let mut file = File::create(&partial).map_err(ZemError::io(&partial))?;
    let progress = progress_bar(&name, total);
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
    fs::rename(&partial, path).map_err(ZemError::io(path))
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

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "e419bf07533bdbf48795fc793601c834dd79db57a8c49ce8e25cfe3a8639958d";

    #[test]
    fn single_checksum_line_forms() {
        assert_eq!(
            sha256_of_line(&format!("{HASH}  uv-x.tar.gz\n")).as_deref(),
            Some(HASH)
        );
        assert_eq!(sha256_of_line(&HASH.to_uppercase()).as_deref(), Some(HASH));
        assert_eq!(sha256_of_line("not a hash"), None);
        assert_eq!(sha256_of_line(""), None);
    }

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
}
