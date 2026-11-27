use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::{InstallHint, Os, SerialAccess};

pub const EXE_SUFFIX: &str = ".exe";
pub const SDK_ARCHIVE_EXT: &str = ".7z";
pub const UV_ARCHIVE_EXT: &str = ".zip";
pub const VENV_BIN_DIR: &str = "Scripts";

pub const STRICT_ENV_ALLOWLIST: &[&str] = &[
    "SYSTEMROOT",
    "SYSTEMDRIVE",
    "WINDIR",
    "COMSPEC",
    "PATHEXT",
    "TEMP",
    "TMP",
    "USERPROFILE",
    "USERNAME",
    "HOMEDRIVE",
    "HOMEPATH",
    "APPDATA",
    "LOCALAPPDATA",
    "PROGRAMDATA",
    "PROGRAMFILES",
    "PROGRAMFILES(X86)",
    "NUMBER_OF_PROCESSORS",
    "PROCESSOR_ARCHITECTURE",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "ALL_PROXY",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
];

pub fn env_key_eq(a: &OsStr, b: &OsStr) -> bool {
    match (a.to_str(), b.to_str()) {
        (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
        _ => a == b,
    }
}

/// The console delivers Ctrl-C to every process attached to it. The child decides what it
/// means (west and serial monitors handle it themselves); zem only waits for it to exit, so
/// it must not die first and leave the child running detached from the shell.
pub fn run_replacing(mut cmd: Command) -> io::Error {
    // Fails only if a handler is already installed, which also keeps zem alive.
    let _ = ctrlc::set_handler(|| {});
    match cmd.status() {
        Ok(status) => std::process::exit(status.code().unwrap_or(1)),
        Err(e) => e,
    }
}

pub fn extract_sdk_archive(archive: &Path, dest: &Path) -> io::Result<()> {
    sevenz_rust2::decompress_file(archive, dest).map_err(io::Error::other)
}

pub fn tls_config() -> ureq::tls::TlsConfig {
    ureq::tls::TlsConfig::builder()
        .provider(ureq::tls::TlsProvider::NativeTls)
        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
        .build()
}

pub fn to_cmake_path(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

pub fn user_shell() -> Option<PathBuf> {
    None
}

pub fn fallback_shell() -> PathBuf {
    which::which("pwsh.exe").unwrap_or_else(|_| PathBuf::from("powershell.exe"))
}

const FILESYSTEM_KEY: &str = r"HKLM\SYSTEM\CurrentControlSet\Control\FileSystem";

pub fn long_paths_enabled() -> Option<bool> {
    let out = Command::new("reg")
        .args(["query", FILESYSTEM_KEY, "/v", "LongPathsEnabled"])
        .output()
        .ok()?;
    // A missing value means the default, which is disabled.
    Some(parse_reg_dword(&String::from_utf8_lossy(&out.stdout)).is_some_and(|v| v != 0))
}

/// The value of a `REG_DWORD` line in `reg query` output, e.g. `    X    REG_DWORD    0x1`.
fn parse_reg_dword(output: &str) -> Option<u32> {
    output.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let _name = parts.next()?;
        (parts.next()? == "REG_DWORD").then_some(())?;
        u32::from_str_radix(parts.next()?.trim_start_matches("0x"), 16).ok()
    })
}

pub fn serial_port_hint() -> &'static str {
    "COM3"
}

pub fn serial_access() -> SerialAccess {
    SerialAccess::NotApplicable
}

pub fn install_hint(_os: Os, tool: &str) -> Option<InstallHint> {
    let hint = |command, alternative| Some(InstallHint::new(command, alternative));
    match tool {
        "git" => hint("winget install --id Git.Git", None),
        "cmake" => hint("winget install --id Kitware.CMake", None),
        "ninja" => hint("winget install --id Ninja-build.Ninja", None),
        "dtc" => hint("choco install dtc-msys2", None),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_keys_are_case_insensitive() {
        assert!(env_key_eq(OsStr::new("Path"), OsStr::new("PATH")));
    }

    #[test]
    fn extracts_7z() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let gcc = src.join("tc").join("bin").join("tc-gcc.exe");
        std::fs::create_dir_all(gcc.parent().unwrap()).unwrap();
        std::fs::write(&gcc, b"hello").unwrap();
        let archive = tmp.path().join("t.7z");
        sevenz_rust2::compress_to_path(&src, &archive).unwrap();

        let dest = tmp.path().join("out");
        extract_sdk_archive(&archive, &dest).unwrap();
        assert_eq!(
            std::fs::read(dest.join("tc").join("bin").join("tc-gcc.exe")).unwrap(),
            b"hello"
        );
    }

    #[test]
    fn parses_reg_query_dword() {
        let out = "\r\nHKEY_LOCAL_MACHINE\\SYSTEM\\CurrentControlSet\\Control\\FileSystem\r\n    LongPathsEnabled    REG_DWORD    0x1\r\n\r\n";
        assert_eq!(parse_reg_dword(out), Some(1));
        assert_eq!(parse_reg_dword("ERROR: not found"), None);
    }

    #[test]
    fn cmake_paths_use_forward_slashes() {
        assert_eq!(
            to_cmake_path(Path::new(r"C:\Users\me\.zem")),
            "C:/Users/me/.zem"
        );
    }
}
