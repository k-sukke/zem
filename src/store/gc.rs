//! Usage accounting and garbage collection for the shared stores under ZEM_HOME.
//!
//! A store is in use when a registered project that still exists references it in its
//! `.zem/state.json` (or, if that is unreadable, in its registry entry).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::registry::{self, Registry};
use super::sdk;
use super::{STAGING_LOCK_FILE, STAGING_PREFIX};
use crate::config::CONFIG_FILE_NAME;
use crate::error::{Result, ZemError};
use crate::fsutil;
use crate::home::ZemHome;
use crate::lock::FileLock;
use crate::project::{PROJECT_STATE_DIR, ProjectState, STATE_FILE};

const SDK_DIR_PREFIX: &str = "zephyr-sdk-";
/// Staging prefix used by older zem versions during SDK extraction.
const LEGACY_EXTRACT_PREFIX: &str = ".zem-extract-";
/// Staging directories younger than this may belong to a sync that is still running.
const LEFTOVER_AGE: Duration = Duration::from_secs(60 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Workspace,
    Venv,
    Sdk,
    Toolchain,
    Leftover,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Workspace => "workspace",
            Kind::Venv => "venv",
            Kind::Sdk => "sdk",
            Kind::Toolchain => "toolchain",
            Kind::Leftover => "leftover",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Store {
    pub kind: Kind,
    pub name: String,
    pub path: PathBuf,
    /// For an SDK, the size without its toolchains (they are listed separately).
    pub size: u64,
    pub users: Vec<PathBuf>,
    /// Store lock guarding writes to this store, if any.
    lock: Option<String>,
}

impl Store {
    pub fn in_use(&self) -> bool {
        !self.users.is_empty()
    }
}

#[derive(Debug, Default)]
pub struct Usage {
    pub stores: Vec<Store>,
    /// Shared directories gc never touches (uv and west venvs, uv-managed Python, caches).
    pub shared: Vec<(String, u64)>,
    pub projects: Vec<PathBuf>,
    /// Registry entries whose project no longer exists.
    pub stale: Vec<String>,
}

impl Usage {
    pub fn total(&self) -> u64 {
        self.stores.iter().map(|s| s.size).sum::<u64>()
            + self.shared.iter().map(|(_, size)| size).sum::<u64>()
    }
}

#[derive(Debug, Default)]
struct References {
    workspaces: BTreeMap<String, Vec<PathBuf>>,
    venvs: BTreeMap<String, Vec<PathBuf>>,
    sdks: BTreeMap<String, Vec<PathBuf>>,
    toolchains: BTreeMap<(String, String), Vec<PathBuf>>,
    /// SDK versions referenced without knowing which toolchains (no readable state.json).
    whole_sdks: BTreeSet<String>,
}

fn users(map: &BTreeMap<String, Vec<PathBuf>>, key: &str) -> Vec<PathBuf> {
    map.get(key).cloned().unwrap_or_default()
}

fn references(home: &ZemHome, registry: &Registry) -> (References, Vec<PathBuf>, Vec<String>) {
    let mut refs = References::default();
    let mut live = Vec::new();
    let mut stale = Vec::new();
    for (key, entry) in &registry.projects {
        let root = PathBuf::from(key);
        if !root.join(CONFIG_FILE_NAME).is_file() {
            stale.push(key.clone());
            continue;
        }
        let state =
            fsutil::read_json::<ProjectState>(&root.join(PROJECT_STATE_DIR).join(STATE_FILE))
                .ok()
                .flatten()
                .filter(|s| s.zem_home == home.root());
        let add = |map: &mut BTreeMap<String, Vec<PathBuf>>, k: &str| {
            map.entry(k.to_string()).or_default().push(root.clone());
        };
        match state {
            Some(state) => {
                add(&mut refs.workspaces, &state.workspace.key);
                add(&mut refs.venvs, &state.venv.key);
                if let Some(sdk) = &state.sdk {
                    add(&mut refs.sdks, &sdk.key);
                    for tc in &state.target.toolchains {
                        refs.toolchains
                            .entry((sdk.key.clone(), tc.clone()))
                            .or_default()
                            .push(root.clone());
                    }
                }
            }
            None => {
                add(&mut refs.workspaces, &entry.workspace);
                add(&mut refs.venvs, &entry.venv);
                if let Some(sdk) = &entry.sdk {
                    add(&mut refs.sdks, sdk);
                    refs.whole_sdks.insert(sdk.clone());
                }
            }
        }
        live.push(root);
    }
    (refs, live, stale)
}

fn subdirs(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut dirs: Vec<_> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
        .collect();
    dirs.sort();
    dirs
}

fn is_old(path: &Path) -> bool {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_some_and(|age| age >= LEFTOVER_AGE)
}

/// Staging directories of `dir` that an interrupted run left behind. Only the old ones are
/// reported; collect still skips any whose lock another process holds.
fn staging_leftovers(dir: &Path, label: &str) -> Vec<Store> {
    subdirs(dir)
        .into_iter()
        .filter(|(name, path)| name.starts_with(STAGING_PREFIX) && is_old(path))
        .map(|(name, path)| leftover(format!("{label}/{name}"), path, None))
        .collect()
}

fn leftover(name: String, path: PathBuf, lock: Option<String>) -> Store {
    Store {
        kind: Kind::Leftover,
        size: fsutil::dir_size(&path),
        name,
        path,
        users: Vec::new(),
        lock,
    }
}

fn scan(home: &ZemHome, refs: &References) -> Vec<Store> {
    let mut stores = Vec::new();
    for (kind, dir, map, prefix) in [
        (
            Kind::Workspace,
            home.workspaces_dir(),
            &refs.workspaces,
            "workspace",
        ),
        (Kind::Venv, home.venvs_dir(), &refs.venvs, "venv"),
    ] {
        for (name, path) in subdirs(&dir) {
            stores.push(Store {
                kind,
                size: fsutil::dir_size(&path),
                users: users(map, &name),
                lock: Some(format!("{prefix}-{name}")),
                name,
                path,
            });
        }
    }

    stores.extend(staging_leftovers(&home.sdk_dir(), "sdk"));
    stores.extend(staging_leftovers(&home.tools_dir().join("uv"), "uv"));
    for (name, path) in subdirs(&home.sdk_dir()) {
        if name.starts_with(STAGING_PREFIX) {
            continue;
        }
        if name.starts_with(LEGACY_EXTRACT_PREFIX) {
            if is_old(&path) {
                stores.push(leftover(format!("sdk/{name}"), path, None));
            }
            continue;
        }
        let Some(version) = name.strip_prefix(SDK_DIR_PREFIX) else {
            continue;
        };
        let lock = Some(format!("sdk-{version}"));
        let mut toolchain_size = 0;
        for (tc, tc_path) in subdirs(&path) {
            if tc.starts_with(STAGING_PREFIX) {
                if is_old(&tc_path) {
                    stores.push(leftover(format!("{name}/{tc}"), tc_path, None));
                }
                continue;
            }
            if tc.starts_with(LEGACY_EXTRACT_PREFIX) {
                if is_old(&tc_path) {
                    stores.push(leftover(format!("{name}/{tc}"), tc_path, lock.clone()));
                }
                continue;
            }
            if !sdk::toolchain_gcc(&path, &tc).is_file() {
                continue;
            }
            let size = fsutil::dir_size(&tc_path);
            toolchain_size += size;
            let users = if refs.whole_sdks.contains(version) {
                users(&refs.sdks, version)
            } else {
                refs.toolchains
                    .get(&(version.to_string(), tc.clone()))
                    .cloned()
                    .unwrap_or_default()
            };
            stores.push(Store {
                kind: Kind::Toolchain,
                name: format!("{version}/{tc}"),
                path: tc_path,
                size,
                users,
                lock: lock.clone(),
            });
        }
        stores.push(Store {
            kind: Kind::Sdk,
            name: version.to_string(),
            size: fsutil::dir_size(&path).saturating_sub(toolchain_size),
            users: users(&refs.sdks, version),
            path,
            lock,
        });
    }

    for (name, path) in subdirs(&home.trash_dir()) {
        stores.push(leftover(format!("trash/{name}"), path, None));
    }
    stores.sort_by(|a, b| (a.kind, &a.name).cmp(&(b.kind, &b.name)));
    stores
}

fn shared(home: &ZemHome) -> Vec<(String, u64)> {
    let mut dirs = vec![
        ("python".to_string(), home.python_dir()),
        ("tools/uv".to_string(), home.tools_dir().join("uv")),
        ("tools/west".to_string(), home.tools_dir().join("west")),
    ];
    dirs.extend(
        subdirs(&home.cache_dir())
            .into_iter()
            .map(|(name, path)| (format!("cache/{name}"), path)),
    );
    dirs.into_iter()
        .map(|(name, path)| (name, fsutil::dir_size(&path)))
        .filter(|(_, size)| *size > 0)
        .collect()
}

pub fn usage(home: &ZemHome) -> Result<Usage> {
    let registry = registry::load(home)?;
    let (refs, projects, stale) = references(home, &registry);
    Ok(Usage {
        stores: scan(home, &refs),
        shared: shared(home),
        projects,
        stale,
    })
}

#[derive(Debug, Default)]
pub struct GcReport {
    pub removed: Vec<Store>,
    /// Unused stores another zem process is currently writing to.
    pub busy: Vec<Store>,
    pub pruned: Vec<String>,
}

impl GcReport {
    pub fn freed(&self) -> u64 {
        self.removed.iter().map(|s| s.size).sum()
    }
}

/// Remove every store no live project references. With `dry_run`, only report.
pub fn collect(home: &ZemHome, dry_run: bool) -> Result<GcReport> {
    // Held throughout, so no sync can register a new reference while we decide.
    let _registry_lock = super::lock(home, "registry")?;
    let mut registry = registry::load(home)?;
    let (refs, _, stale) = references(home, &registry);
    if !dry_run && !stale.is_empty() {
        for key in &stale {
            registry.projects.remove(key);
        }
        fsutil::write_json(&home.registry_path(), &registry)?;
    }

    let mut report = GcReport {
        pruned: stale,
        ..GcReport::default()
    };
    let stores = scan(home, &refs);
    let removed_sdks: BTreeSet<PathBuf> = stores
        .iter()
        .filter(|s| s.kind == Kind::Sdk && !s.in_use())
        .map(|s| s.path.clone())
        .collect();
    for mut store in stores.into_iter().filter(|s| !s.in_use()) {
        let inside_removed_sdk = store.kind != Kind::Sdk
            && store
                .path
                .parent()
                .is_some_and(|p| removed_sdks.contains(p));
        if store.kind == Kind::Sdk {
            store.size = fsutil::dir_size(&store.path);
        } else if inside_removed_sdk {
            continue;
        }
        let guard = match &store.lock {
            Some(name) => match super::try_lock(home, name)? {
                Some(guard) => Some(guard),
                None => {
                    report.busy.push(store);
                    continue;
                }
            },
            None if is_staging(home, &store.path) => {
                match FileLock::try_exclusive(&store.path.join(STAGING_LOCK_FILE))? {
                    Some(guard) => Some(guard),
                    None => {
                        report.busy.push(store);
                        continue;
                    }
                }
            }
            None => None,
        };
        if !dry_run {
            discard(home, &store.path)?;
        }
        drop(guard);
        report.removed.push(store);
    }
    Ok(report)
}

/// A staging directory outside the trash, whose producer may still hold its lock.
fn is_staging(home: &ZemHome, path: &Path) -> bool {
    !path.starts_with(home.trash_dir())
        && path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with(STAGING_PREFIX))
}

/// Move `path` into the trash first, so an interrupted delete never leaves a half-removed
/// store behind under its original name.
pub fn discard(home: &ZemHome, path: &Path) -> Result<()> {
    let trash = home.trash_dir();
    if path.starts_with(&trash) {
        return fsutil::remove_dir_all_if_exists(path);
    }
    fsutil::create_dir_all(&trash)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let target = trash.join(format!("{name}-{stamp}"));
    match fs::rename(path, &target) {
        Ok(()) => fsutil::remove_dir_all_if_exists(&target),
        Err(_) => {
            let _ = fs::remove_file(super::marker_path(path));
            fs::remove_dir_all(path).map_err(ZemError::io(path))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{Arch, ResolutionSource};
    use crate::project::{STATE_SCHEMA, StoreRef, TargetState};
    use crate::store::registry::ProjectEntry;

    struct Fixture {
        _tmp: tempfile::TempDir,
        home: ZemHome,
        projects: PathBuf,
    }

    fn fixture() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let home = ZemHome::new(tmp.path().join("home"));
        for dir in [
            "workspaces/ws-a",
            "workspaces/ws-old",
            "venvs/py-a",
            "venvs/py-old",
            "sdk/zephyr-sdk-0.17.2/cmake",
            "sdk/zephyr-sdk-0.17.2/riscv64-zephyr-elf/bin",
            "sdk/zephyr-sdk-0.17.2/xtensa-espressif_esp32_zephyr-elf/bin",
            "sdk/zephyr-sdk-0.16.8/arm-zephyr-eabi/bin",
            "cache/uv",
        ] {
            fs::create_dir_all(home.root().join(dir)).unwrap();
        }
        for (sdk, tc) in [
            ("0.17.2", "riscv64-zephyr-elf"),
            ("0.17.2", "xtensa-espressif_esp32_zephyr-elf"),
            ("0.16.8", "arm-zephyr-eabi"),
        ] {
            let gcc = sdk::toolchain_gcc(&home.sdk_install_dir(sdk), tc);
            fs::write(gcc, vec![0u8; 100]).unwrap();
        }
        fs::write(home.root().join("cache/uv/blob"), "x").unwrap();
        let projects = tmp.path().join("projects");
        Fixture {
            _tmp: tmp,
            home,
            projects,
        }
    }

    fn project(f: &Fixture, name: &str, toolchain: &str) -> PathBuf {
        let root = f.projects.join(name);
        fs::create_dir_all(root.join(PROJECT_STATE_DIR)).unwrap();
        fs::write(root.join(CONFIG_FILE_NAME), "").unwrap();
        let store = |key: &str| StoreRef {
            key: key.into(),
            path: PathBuf::new(),
        };
        let state = ProjectState {
            schema: STATE_SCHEMA,
            zem_version: "0".into(),
            config_hash: "h".into(),
            zem_home: f.home.root().to_path_buf(),
            target: TargetState {
                board: "b".into(),
                soc: "esp32c3".into(),
                arch: Arch::Riscv,
                toolchains: vec![toolchain.into()],
                source: ResolutionSource::BoardYml,
            },
            workspace: store("ws-a"),
            zephyr_base: PathBuf::new(),
            modules: BTreeMap::new(),
            venv: store("py-a"),
            sdk: Some(store("0.17.2")),
            synced_at: 0,
        };
        fsutil::write_json(&root.join(PROJECT_STATE_DIR).join(STATE_FILE), &state).unwrap();
        registry::record(
            &f.home,
            &root,
            ProjectEntry {
                workspace: "ws-a".into(),
                venv: "py-a".into(),
                sdk: Some("0.17.2".into()),
                last_sync: 0,
            },
        )
        .unwrap();
        root
    }

    fn names(stores: &[Store]) -> Vec<&str> {
        stores.iter().map(|s| s.name.as_str()).collect()
    }

    #[test]
    fn usage_attributes_stores_to_projects() {
        let f = fixture();
        let a = project(&f, "a", "riscv64-zephyr-elf");
        let usage = usage(&f.home).unwrap();
        let used: Vec<_> = usage.stores.iter().filter(|s| s.in_use()).collect();
        assert_eq!(
            used.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            ["ws-a", "py-a", "0.17.2", "0.17.2/riscv64-zephyr-elf"]
        );
        assert!(used.iter().all(|s| s.users == [a.clone()]));
        assert_eq!(usage.shared, [("cache/uv".to_string(), 1)]);
        assert_eq!(usage.total(), 301);
    }

    #[test]
    fn gc_removes_only_unreferenced_stores() {
        let f = fixture();
        project(&f, "a", "riscv64-zephyr-elf");
        let gone = project(&f, "gone", "riscv64-zephyr-elf");
        fs::remove_dir_all(&gone).unwrap();

        let dry = collect(&f.home, true).unwrap();
        let expected = [
            "ws-old",
            "py-old",
            "0.16.8",
            "0.17.2/xtensa-espressif_esp32_zephyr-elf",
        ];
        assert_eq!(names(&dry.removed), expected);
        assert_eq!(dry.removed[2].size, 100, "whole SDK size is reported");
        assert!(f.home.root().join("workspaces/ws-old").is_dir());

        let report = collect(&f.home, false).unwrap();
        assert_eq!(names(&report.removed), expected);
        assert_eq!(report.pruned, [gone.to_string_lossy()]);
        assert!(!f.home.root().join("workspaces/ws-old").exists());
        assert!(!f.home.sdk_install_dir("0.16.8").exists());
        assert!(f.home.root().join("workspaces/ws-a").is_dir());
        assert!(
            sdk::toolchain_gcc(&f.home.sdk_install_dir("0.17.2"), "riscv64-zephyr-elf").is_file()
        );
        assert_eq!(subdirs(&f.home.trash_dir()), []);
        assert_eq!(registry::load(&f.home).unwrap().projects.len(), 1);
        assert!(collect(&f.home, false).unwrap().removed.is_empty());
    }

    #[test]
    fn gc_skips_stores_in_use_by_another_process() {
        let f = fixture();
        project(&f, "a", "riscv64-zephyr-elf");
        let _held = super::super::lock(&f.home, "workspace-ws-old").unwrap();
        let report = collect(&f.home, false).unwrap();
        assert_eq!(names(&report.busy), ["ws-old"]);
        assert!(f.home.root().join("workspaces/ws-old").is_dir());
    }

    #[test]
    fn unreadable_state_keeps_the_whole_sdk() {
        let f = fixture();
        let a = project(&f, "a", "riscv64-zephyr-elf");
        fs::remove_file(a.join(PROJECT_STATE_DIR).join(STATE_FILE)).unwrap();
        let report = collect(&f.home, true).unwrap();
        assert_eq!(names(&report.removed), ["ws-old", "py-old", "0.16.8"]);
    }
}
