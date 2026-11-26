//! Content-addressed keys for shared components under ZEM_HOME.
//!
//! Directory names must stay identical across OSes and zem releases, otherwise existing
//! caches are silently orphaned. Changing the hashing scheme requires bumping the domain tag.

use sha2::{Digest, Sha256};

const HASH_LEN: usize = 8;

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn short_hash(domain: &str, parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(domain.as_bytes());
    for part in parts {
        hasher.update([0u8]);
        hasher.update(part.as_bytes());
    }
    let mut digest = hex(&hasher.finalize());
    digest.truncate(HASH_LEN);
    digest
}

fn slug(s: &str, max_len: usize) -> String {
    let slug: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .take(max_len)
        .collect();
    let slug = slug.trim_matches(|c| c == '-' || c == '.');
    if slug.is_empty() {
        "x".to_string()
    } else {
        slug.to_string()
    }
}

pub fn normalize_git_url(url: &str) -> String {
    let url = url.trim().trim_end_matches('/');
    url.strip_suffix(".git").unwrap_or(url).to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorkspaceKey {
    url: String,
    revision: String,
}

impl WorkspaceKey {
    pub fn new(url: &str, revision: &str) -> Self {
        WorkspaceKey {
            url: normalize_git_url(url),
            revision: revision.trim().to_string(),
        }
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn revision(&self) -> &str {
        &self.revision
    }

    pub fn dir_name(&self) -> String {
        format!(
            "zephyr-{}-{}",
            slug(&self.revision, 16),
            short_hash("zem-workspace-v1", &[&self.url, &self.revision])
        )
    }
}

/// Project venv: one per Zephyr workspace and Python setup. Locked projects with a different
/// package set get a sibling directory (see `store::venv::select_project`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VenvKey {
    workspace: WorkspaceKey,
    python: String,
    extra_requirements: Vec<String>,
    install_cmake_ninja: bool,
}

impl VenvKey {
    pub fn new(
        workspace: WorkspaceKey,
        python: &str,
        extra_requirements: &[String],
        install_cmake_ninja: bool,
    ) -> Self {
        let mut extra: Vec<String> = extra_requirements
            .iter()
            .map(|r| r.trim().to_string())
            .collect();
        extra.sort();
        extra.dedup();
        VenvKey {
            workspace,
            python: python.trim().to_string(),
            extra_requirements: extra,
            install_cmake_ninja,
        }
    }

    pub fn dir_name(&self) -> String {
        let extra = self.extra_requirements.join("\n");
        let build_tools = if self.install_cmake_ninja { "1" } else { "0" };
        format!(
            "py{}-{}",
            slug(&self.python, 8),
            short_hash(
                "zem-venv-v1",
                &[
                    &self.workspace.url,
                    &self.workspace.revision,
                    &self.python,
                    &extra,
                    build_tools
                ],
            )
        )
    }
}

/// The shared venv that runs west before a workspace exists. It depends only on the Python
/// version and the bootstrap packages, so every project with the same pair reuses it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WestVenvKey {
    python: String,
    packages: Vec<String>,
}

impl WestVenvKey {
    pub fn new(python: &str, packages: &[String]) -> Self {
        let mut packages: Vec<String> = packages.iter().map(|p| p.trim().to_string()).collect();
        packages.sort();
        packages.dedup();
        WestVenvKey {
            python: python.trim().to_string(),
            packages,
        }
    }

    pub fn dir_name(&self) -> String {
        format!(
            "west-py{}-{}",
            slug(&self.python, 8),
            short_hash(
                "zem-west-venv-v1",
                &[&self.python, &self.packages.join("\n")]
            )
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const URL: &str = "https://github.com/zephyrproject-rtos/zephyr";

    #[test]
    fn workspace_key_is_stable() {
        let key = WorkspaceKey::new(URL, "v4.2.0");
        insta::assert_snapshot!(key.dir_name(), @"zephyr-v4.2.0-84fcfa2f");
    }

    #[test]
    fn workspace_key_ignores_url_cosmetics() {
        let a = WorkspaceKey::new(URL, "v4.2.0");
        let b = WorkspaceKey::new(&format!(" {URL}.git/ "), " v4.2.0 ");
        assert_eq!(a.dir_name(), b.dir_name());
    }

    #[test]
    fn workspace_key_depends_on_revision_and_url() {
        let a = WorkspaceKey::new(URL, "v4.2.0");
        assert_ne!(a.dir_name(), WorkspaceKey::new(URL, "v4.1.0").dir_name());
        assert_ne!(
            a.dir_name(),
            WorkspaceKey::new("https://example.com/fork/zephyr", "v4.2.0").dir_name()
        );
    }

    #[test]
    fn sha_revisions_are_shortened() {
        let key = WorkspaceKey::new(URL, &"0123456789abcdef".repeat(2));
        assert!(key.dir_name().starts_with("zephyr-0123456789abcdef-"));
        assert!(key.dir_name().len() <= 40);
    }

    #[test]
    fn venv_key_is_order_insensitive() {
        let ws = WorkspaceKey::new(URL, "v4.2.0");
        let a = VenvKey::new(ws.clone(), "3.12", &["b".into(), "a".into()], true);
        let b = VenvKey::new(
            ws.clone(),
            "3.12",
            &["a".into(), "b".into(), "a".into()],
            true,
        );
        assert_eq!(a.dir_name(), b.dir_name());
        assert_ne!(
            a.dir_name(),
            VenvKey::new(ws, "3.12", &["a".into(), "b".into()], false).dir_name()
        );
        assert!(a.dir_name().starts_with("py3.12-"));
    }
}
