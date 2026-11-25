use std::fs;
use std::path::{Path, PathBuf};

use miette::{NamedSource, SourceSpan};

use super::ZemConfig;
use crate::error::{ConfigParseError, Result, ZemError};
use crate::home::keys::sha256_hex;
use crate::platform;

pub const CONFIG_FILE_NAME: &str = "zem.toml";

#[derive(Debug, Clone)]
pub struct LoadedConfig {
    pub root: PathBuf,
    pub path: PathBuf,
    pub config: ZemConfig,
    /// sha256 of the raw file; compared with `.zem/state.json` to detect stale syncs.
    pub content_hash: String,
}

/// Walk up from `start` looking for zem.toml, never past the home directory: a zem.toml in
/// `~` itself is an error because the project's `.zem/` would collide with the global one.
pub fn find_project_root(start: &Path) -> Result<Option<PathBuf>> {
    let start = platform::canonicalize(start).map_err(ZemError::io(start))?;
    let home = platform::home_dir().map(|h| platform::canonicalize_lenient(&h));

    for dir in start.ancestors() {
        if home.as_deref() == Some(dir) {
            if dir.join(CONFIG_FILE_NAME).is_file() {
                return Err(ZemError::ProjectIsHome(dir.to_path_buf()));
            }
            return Ok(None);
        }
        if dir.join(CONFIG_FILE_NAME).is_file() {
            return Ok(Some(dir.to_path_buf()));
        }
    }
    Ok(None)
}

pub fn load(start: &Path) -> Result<LoadedConfig> {
    let root = find_project_root(start)?.ok_or_else(|| ZemError::ConfigNotFound {
        start: start.to_path_buf(),
    })?;
    let path = root.join(CONFIG_FILE_NAME);
    let text = fs::read_to_string(&path).map_err(ZemError::io(&path))?;
    let config = parse(&path, &text)?;
    Ok(LoadedConfig {
        root,
        content_hash: sha256_hex(text.as_bytes()),
        path,
        config,
    })
}

pub fn parse(path: &Path, text: &str) -> Result<ZemConfig> {
    toml::from_str(text).map_err(|e| {
        ZemError::ConfigParse(Box::new(ConfigParseError {
            path: path.to_path_buf(),
            message: e.message().to_string(),
            span: e.span().map(|r| SourceSpan::from((r.start, r.len()))),
            src: NamedSource::new(path.display().to_string(), text.to_string()),
        }))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"
schema = 1
[project]
name = "demo"
[zephyr]
version = "v4.2.0"
[target]
board = "esp32c3_devkitm"
"#;

    #[test]
    fn minimal_config_gets_defaults() {
        let cfg = parse(Path::new("zem.toml"), MINIMAL).unwrap();
        assert_eq!(cfg.zephyr.url, super::super::DEFAULT_ZEPHYR_URL);
        assert_eq!(cfg.python.version, "3.12");
        assert!(cfg.python.host_tools_via_pip);
        assert_eq!(cfg.build.dir.as_str(), "build");
    }

    #[test]
    fn unknown_keys_are_rejected_with_span() {
        let text = format!("{MINIMAL}\n[build]\ndirr = \"out\"\n");
        let err = parse(Path::new("zem.toml"), &text).unwrap_err();
        match err {
            ZemError::ConfigParse(e) => {
                assert!(e.message.contains("dirr"), "{}", e.message);
                assert!(e.span.is_some());
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn finds_root_from_subdirectory() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join(CONFIG_FILE_NAME), MINIMAL).unwrap();
        let sub = tmp.path().join("src").join("deep");
        fs::create_dir_all(&sub).unwrap();
        let loaded = load(&sub).unwrap();
        assert_eq!(loaded.root, platform::canonicalize(tmp.path()).unwrap());
        assert_eq!(loaded.content_hash.len(), 64);
    }
}
