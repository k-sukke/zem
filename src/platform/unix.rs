use std::ffi::OsStr;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;

use super::{InstallHint, Os, SerialAccess};

pub const EXE_SUFFIX: &str = "";
pub const SDK_ARCHIVE_EXT: &str = ".tar.xz";
pub const UV_ARCHIVE_EXT: &str = ".tar.gz";
pub const VENV_BIN_DIR: &str = "bin";

pub const STRICT_ENV_ALLOWLIST: &[&str] = &[
    "HOME",
    "USER",
    "LOGNAME",
    "LANG",
    "LC_*",
    "TERM",
    "COLORTERM",
    "TMPDIR",
    "SHELL",
    "SSH_AUTH_SOCK",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "no_proxy",
    "all_proxy",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
];

const SERIAL_GROUPS: &[&str] = &["dialout", "uucp"];

pub fn env_key_eq(a: &OsStr, b: &OsStr) -> bool {
    a == b
}

pub fn run_replacing(mut cmd: Command) -> io::Error {
    cmd.exec()
}

pub fn extract_sdk_archive(archive: &Path, dest: &Path) -> io::Result<()> {
    let file = std::fs::File::open(archive)?;
    let xz = liblzma::read::XzDecoder::new_multi_decoder(io::BufReader::new(file));
    let mut tar = tar::Archive::new(xz);
    tar.set_preserve_permissions(true);
    tar.unpack(dest)
}

pub fn extract_uv_archive(archive: &Path, dest: &Path) -> io::Result<()> {
    let file = std::fs::File::open(archive)?;
    let gz = flate2::read::GzDecoder::new(io::BufReader::new(file));
    let mut tar = tar::Archive::new(gz);
    tar.set_preserve_permissions(true);
    tar.unpack(dest)
}

pub fn tls_config() -> ureq::tls::TlsConfig {
    ureq::tls::TlsConfig::default()
}

pub fn to_cmake_path(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

pub fn serial_port_hint() -> &'static str {
    if cfg!(target_os = "macos") {
        "/dev/cu.usbserial-XXXX"
    } else {
        "/dev/ttyUSB0"
    }
}

pub fn serial_access() -> SerialAccess {
    if !cfg!(target_os = "linux") {
        return SerialAccess::NotApplicable;
    }
    let Ok(out) = Command::new("id").arg("-Gn").output() else {
        return SerialAccess::Unknown;
    };
    let groups = String::from_utf8_lossy(&out.stdout);
    if groups
        .split_whitespace()
        .any(|g| SERIAL_GROUPS.contains(&g))
    {
        SerialAccess::Ok
    } else {
        SerialAccess::MissingGroup {
            candidates: SERIAL_GROUPS,
        }
    }
}

pub fn install_hint(os: Os, tool: &str) -> Option<InstallHint> {
    let hint = |command, alternative| Some(InstallHint::new(command, alternative));
    match (os, tool) {
        (Os::MacOs, "git") => hint("xcode-select --install", Some("brew install git")),
        (Os::MacOs, "cmake") => hint("brew install cmake", None),
        (Os::MacOs, "ninja") => hint("brew install ninja", None),
        (Os::MacOs, "dtc") => hint("brew install dtc", None),
        (Os::Linux, "git") => hint(
            "sudo apt install git",
            Some("your distribution's equivalent"),
        ),
        (Os::Linux, "cmake") => hint("sudo apt install cmake", None),
        (Os::Linux, "ninja") => hint("sudo apt install ninja-build", None),
        (Os::Linux, "dtc") => hint("sudo apt install device-tree-compiler", None),
        _ => None,
    }
}

#[cfg(test)]
pub fn set_dir_modified(path: &Path, time: std::time::SystemTime) -> io::Result<()> {
    std::fs::File::open(path)?.set_modified(time)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_tar_xz_with_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("t.tar.xz");
        let xz = liblzma::write::XzEncoder::new(std::fs::File::create(&archive).unwrap(), 1);
        let mut builder = tar::Builder::new(xz);
        let mut header = tar::Header::new_gnu();
        header.set_size(5);
        header.set_mode(0o755);
        header.set_cksum();
        builder
            .append_data(&mut header, "tc/bin/tc-gcc", &b"hello"[..])
            .unwrap();
        builder.into_inner().unwrap().finish().unwrap();

        let dest = tmp.path().join("out");
        extract_sdk_archive(&archive, &dest).unwrap();
        let gcc = dest.join("tc/bin/tc-gcc");
        assert_eq!(std::fs::read(&gcc).unwrap(), b"hello");
        assert_eq!(
            std::fs::metadata(&gcc).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[test]
    fn extracts_tar_gz_with_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("t.tar.gz");
        let gz = flate2::write::GzEncoder::new(
            std::fs::File::create(&archive).unwrap(),
            flate2::Compression::fast(),
        );
        let mut builder = tar::Builder::new(gz);
        let mut header = tar::Header::new_gnu();
        header.set_size(5);
        header.set_mode(0o755);
        header.set_cksum();
        builder
            .append_data(&mut header, "uv-x/uv", &b"hello"[..])
            .unwrap();
        builder.into_inner().unwrap().finish().unwrap();

        let dest = tmp.path().join("out");
        extract_uv_archive(&archive, &dest).unwrap();
        let uv = dest.join("uv-x/uv");
        assert_eq!(std::fs::read(&uv).unwrap(), b"hello");
        assert_eq!(
            std::fs::metadata(&uv).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }
}
