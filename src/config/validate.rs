use super::ZemConfig;
use crate::board::{self, ResolvedTarget};
use crate::error::{Result, ZemError};

pub const SUPPORTED_SCHEMA: u32 = 1;
const MIN_ZEPHYR_MAJOR: u32 = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevisionKind {
    Release {
        major: u32,
        minor: u32,
        patch: u32,
        prerelease: Option<String>,
    },
    Commit,
    /// Anything else; treated as a moving branch name.
    Branch,
}

impl RevisionKind {
    pub fn is_immutable(&self) -> bool {
        !matches!(self, RevisionKind::Branch)
    }
}

pub fn classify_revision(rev: &str) -> RevisionKind {
    let rev = rev.trim();
    if rev.len() == 40 && rev.chars().all(|c| c.is_ascii_hexdigit()) {
        return RevisionKind::Commit;
    }
    let body = rev.strip_prefix('v').unwrap_or(rev);
    let (version, prerelease) = match body.split_once('-') {
        Some((v, pre)) => (v, Some(pre.to_string())),
        None => (body, None),
    };
    let parts: Vec<_> = version.split('.').collect();
    if !(2..=3).contains(&parts.len()) {
        return RevisionKind::Branch;
    }
    let nums: Option<Vec<u32>> = parts.iter().map(|p| p.parse().ok()).collect();
    match nums.as_deref() {
        Some([major, minor]) => RevisionKind::Release {
            major: *major,
            minor: *minor,
            patch: 0,
            prerelease,
        },
        Some([major, minor, patch]) => RevisionKind::Release {
            major: *major,
            minor: *minor,
            patch: *patch,
            prerelease,
        },
        _ => RevisionKind::Branch,
    }
}

#[derive(Debug, Clone)]
pub struct Validated {
    pub target: ResolvedTarget,
    pub revision: RevisionKind,
    pub warnings: Vec<String>,
}

pub fn validate(cfg: &ZemConfig) -> Result<Validated> {
    let mut problems = Vec::new();
    let mut warnings = Vec::new();

    if cfg.schema != SUPPORTED_SCHEMA {
        problems.push(format!(
            "schema = {} is not supported by this zem (expected {SUPPORTED_SCHEMA})",
            cfg.schema
        ));
    }

    if !is_name(&cfg.project.name) {
        problems.push(format!(
            "[project].name `{}` must be non-empty and use only letters, digits, `_`, `-` or `.`",
            cfg.project.name
        ));
    }

    let revision = classify_revision(&cfg.zephyr.version);
    match &revision {
        RevisionKind::Release { major, .. } if *major < MIN_ZEPHYR_MAJOR => problems.push(format!(
            "[zephyr].version `{}` is not supported; zem requires Zephyr v{MIN_ZEPHYR_MAJOR}.0 or later",
            cfg.zephyr.version
        )),
        RevisionKind::Branch => warnings.push(format!(
            "[zephyr].version `{}` looks like a branch; sync pins its current commit in zem.lock \
             and only moves to a newer one with `zem sync --update`. Prefer a release tag",
            cfg.zephyr.version
        )),
        _ => {}
    }
    if cfg.zephyr.version.trim().is_empty() {
        problems.push("[zephyr].version must not be empty".to_string());
    }
    if cfg.zephyr.url.trim().is_empty() {
        problems.push("[zephyr].url must not be empty".to_string());
    }

    for name in &cfg.modules.extra {
        if !is_module_name(name) {
            problems.push(format!(
                "[modules].extra entry `{name}` is not a valid west project name"
            ));
        }
    }

    if !is_python_version(&cfg.python.version) {
        problems.push(format!(
            "[python].version `{}` must look like `3.12`",
            cfg.python.version
        ));
    }

    for key in cfg.env.passthrough.iter().chain(cfg.env.set.keys()) {
        if !crate::env::shell::is_portable_name(key) {
            problems.push(format!(
                "[env] variable name {key:?} is invalid (use letters, digits and `_`, not starting with a digit)"
            ));
        }
    }

    if !problems.is_empty() {
        return Err(ZemError::ConfigInvalid { problems });
    }

    let target = board::resolve_static(&cfg.target.board, cfg.target.arch)?;
    if let (RevisionKind::Release { major, minor, .. }, Some(absent)) =
        (&revision, target.soc.absent_through())
        && (*major, *minor) <= absent
    {
        return Err(ZemError::ConfigInvalid {
            problems: vec![format!(
                "{} is not supported by Zephyr {} (first available after v{}.{})",
                target.soc.name(),
                cfg.zephyr.version,
                absent.0,
                absent.1
            )],
        });
    }
    if let Some(original) = &target.normalized_from {
        warnings.push(format!(
            "board `{original}` was expanded to `{}`; consider writing the full name in zem.toml",
            target.board
        ));
    }

    Ok(Validated {
        target,
        revision,
        warnings,
    })
}

fn is_name(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

fn is_module_name(s: &str) -> bool {
    is_name(s) && s.starts_with(|c: char| c.is_ascii_alphanumeric())
}

fn is_python_version(s: &str) -> bool {
    let parts: Vec<_> = s.split('.').collect();
    (2..=3).contains(&parts.len())
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::config::parse;

    fn config(board: &str, version: &str) -> ZemConfig {
        let text = format!(
            "schema = 1\n[project]\nname = \"demo\"\n[zephyr]\nversion = \"{version}\"\n[target]\nboard = \"{board}\"\n"
        );
        parse(Path::new("zem.toml"), &text).unwrap()
    }

    #[test]
    fn classifies_revisions() {
        assert_eq!(
            classify_revision("v4.2.0"),
            RevisionKind::Release {
                major: 4,
                minor: 2,
                patch: 0,
                prerelease: None
            }
        );
        assert_eq!(
            classify_revision("v4.3.0-rc1"),
            RevisionKind::Release {
                major: 4,
                minor: 3,
                patch: 0,
                prerelease: Some("rc1".into())
            }
        );
        assert_eq!(
            classify_revision("4.1"),
            RevisionKind::Release {
                major: 4,
                minor: 1,
                patch: 0,
                prerelease: None
            }
        );
        assert_eq!(classify_revision(&"a".repeat(40)), RevisionKind::Commit);
        assert_eq!(classify_revision("main"), RevisionKind::Branch);
        assert_eq!(classify_revision("v4-branch"), RevisionKind::Branch);
    }

    #[test]
    fn rejects_env_names_shells_cannot_set() {
        let mut cfg = config("esp32c3_devkitm", "v4.2.0");
        cfg.env.set.insert("X\necho INJECTED".into(), "1".into());
        cfg.env.passthrough.push("A;B".into());
        cfg.env.passthrough.push("MY_TOKEN".into());
        match validate(&cfg) {
            Err(ZemError::ConfigInvalid { problems }) => {
                assert_eq!(problems.len(), 2, "{problems:?}");
                assert!(problems[0].contains(r#""A;B""#), "{problems:?}");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn accepts_v4_release() {
        let v = validate(&config("esp32c3_devkitm", "v4.2.0")).unwrap();
        assert!(v.warnings.is_empty());
        assert!(v.revision.is_immutable());
    }

    #[test]
    fn rejects_v3() {
        let err = validate(&config("esp32c3_devkitm", "v3.7.0")).unwrap_err();
        assert!(err.to_string().contains("v4.0 or later"), "{err}");
    }

    #[test]
    fn warns_on_branch_and_normalization() {
        let v = validate(&config("esp32s3_devkitc", "main")).unwrap();
        assert_eq!(v.warnings.len(), 2, "{:?}", v.warnings);
        assert_eq!(v.target.board, "esp32s3_devkitc/esp32s3/procpu");
    }

    #[test]
    fn rejects_soc_missing_from_release() {
        let err = validate(&config("esp32h2_devkitm", "v4.2.0")).unwrap_err();
        assert!(
            err.to_string().contains("esp32h2 is not supported"),
            "{err}"
        );
        assert!(validate(&config("esp32h2_devkitm", "v4.3.0")).is_ok());
        assert!(validate(&config("esp32h2_devkitm", "main")).is_ok());
    }

    #[test]
    fn reports_unsupported_board() {
        let err = validate(&config("nrf52840dk/nrf52840", "v4.2.0")).unwrap_err();
        assert!(matches!(err, ZemError::Board(_)));
    }

    #[test]
    fn collects_multiple_problems() {
        let mut cfg = config("esp32c3_devkitm", "v4.2.0");
        cfg.project.name = "bad name".into();
        cfg.python.version = "three".into();
        match validate(&cfg).unwrap_err() {
            ZemError::ConfigInvalid { problems } => assert_eq!(problems.len(), 2),
            other => panic!("unexpected: {other:?}"),
        }
    }
}
