use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use crate::platform;

pub const DENY_PREFIXES: &[&str] = &["IDF_", "ZEPHYR_", "CONDA_", "PIP_", "UV_", "WEST_CONFIG_"];

pub const DENY_EXACT: &[&str] = &[
    "VIRTUAL_ENV",
    "PYTHONPATH",
    "PYTHONHOME",
    "ESP_IDF_VERSION",
    "ESP_ROM_ELF_DIR",
    "OPENOCD_SCRIPTS",
    "ESPRESSIF_TOOLCHAIN_PATH",
    "GNUARMEMB_TOOLCHAIN_PATH",
    "CROSS_COMPILE",
];

pub fn is_denied(key: &OsStr) -> bool {
    let Some(key) = key.to_str() else {
        return false;
    };
    let upper = key.to_ascii_uppercase();
    DENY_EXACT.contains(&upper.as_str()) || DENY_PREFIXES.iter().any(|p| upper.starts_with(p))
}

/// Kept in `[env].isolation = "strict"` mode (besides PATH and `[env].passthrough`).
/// A trailing `*` in the platform table matches a prefix.
pub fn is_strict_allowed(key: &OsStr) -> bool {
    let Some(key) = key.to_str() else {
        return false;
    };
    platform::strict_env_allowlist()
        .iter()
        .any(|entry| match entry.strip_suffix('*') {
            Some(prefix) => key
                .get(..prefix.len())
                .is_some_and(|head| platform::env_key_eq(OsStr::new(head), OsStr::new(prefix))),
            None => platform::env_key_eq(OsStr::new(key), OsStr::new(entry)),
        })
}

/// PATH entries that belong to ESP-IDF, another Zephyr SDK, or a foreign virtualenv/conda env.
pub fn is_polluting_path_entry(entry: &Path, foreign_prefixes: &[PathBuf]) -> bool {
    if foreign_prefixes
        .iter()
        .any(|prefix| entry.starts_with(prefix))
    {
        return true;
    }
    entry.components().any(|c| match c {
        Component::Normal(name) => {
            let name = name.to_string_lossy().to_ascii_lowercase();
            name == ".espressif" || name.starts_with("esp-idf") || name.starts_with("zephyr-sdk-")
        }
        _ => false,
    })
}

/// The `bin`/`Scripts` directory of some other virtualenv or conda environment, recognised by
/// the marker files next to it rather than by name.
pub fn is_python_env_bin(entry: &Path) -> bool {
    entry
        .parent()
        .is_some_and(|env| env.join("pyvenv.cfg").is_file() || env.join("conda-meta").is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_python_environments_by_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let venv = tmp.path().join("venv");
        let conda = tmp.path().join("anaconda3");
        std::fs::create_dir_all(venv.join("bin")).unwrap();
        std::fs::write(venv.join("pyvenv.cfg"), "").unwrap();
        std::fs::create_dir_all(conda.join("conda-meta")).unwrap();
        assert!(is_python_env_bin(&venv.join("bin")));
        assert!(is_python_env_bin(&conda.join("bin")));
        assert!(!is_python_env_bin(tmp.path()));
    }

    #[test]
    fn denies_known_variables() {
        for key in [
            "IDF_PATH",
            "IDF_TOOLS_PATH",
            "ZEPHYR_BASE",
            "VIRTUAL_ENV",
            "CONDA_PREFIX",
            "PIP_INDEX_URL",
            "UV_INDEX_URL",
            "WEST_CONFIG_LOCAL",
        ] {
            assert!(is_denied(OsStr::new(key)), "{key}");
        }
        for key in ["HOME", "PATH", "HTTPS_PROXY", "ZEM_HOME", "IDFX"] {
            assert!(!is_denied(OsStr::new(key)), "{key}");
        }
    }

    #[test]
    fn strict_allowlist_matches_exact_names_and_prefixes() {
        assert!(is_strict_allowed(OsStr::new(
            platform::strict_env_allowlist()[0]
        )));
        assert!(!is_strict_allowed(OsStr::new("EDITOR")));
        assert!(!is_strict_allowed(OsStr::new("IDF_PATH")));
        for prefix in platform::strict_env_allowlist()
            .iter()
            .filter_map(|e| e.strip_suffix('*'))
        {
            assert!(is_strict_allowed(OsStr::new(&format!("{prefix}ALL"))));
            assert!(!is_strict_allowed(OsStr::new(&prefix[..prefix.len() - 1])));
        }
    }

    #[test]
    fn detects_polluting_paths() {
        let none: &[PathBuf] = &[];
        assert!(is_polluting_path_entry(
            Path::new("/u/.espressif/tools/bin"),
            none
        ));
        assert!(is_polluting_path_entry(
            Path::new("/opt/esp-idf-v5.3/tools"),
            none
        ));
        assert!(is_polluting_path_entry(
            Path::new("/u/zephyr-sdk-0.17.0/arm/bin"),
            none
        ));
        assert!(is_polluting_path_entry(
            Path::new("/venv/bin"),
            &[PathBuf::from("/venv")]
        ));
        assert!(!is_polluting_path_entry(Path::new("/usr/local/bin"), none));
        assert!(!is_polluting_path_entry(
            Path::new("/u/.zem/venvs/py3.12-x/bin"),
            none
        ));
    }
}
