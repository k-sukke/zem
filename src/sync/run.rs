use std::collections::BTreeMap;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use console::style;

use super::SyncPlan;
use super::plan::short;
use crate::board::{ResolutionSource, board_yml};
use crate::config::{LoadedConfig, classify_revision};
use crate::env::{EnvBuilder, EnvLocations, zem_env};
use crate::error::{Result, ZemError};
use crate::fsutil;
use crate::home::ZemHome;
use crate::lock::FileLock;
use crate::lockfile::{self, Lockfile, Pins, PythonLock, SdkLock, ZephyrLock};
use crate::platform;
use crate::process::Runner;
use crate::project::{ProjectState, STATE_SCHEMA, StoreRef, TargetState};
use crate::store::registry::{self, ProjectEntry};
use crate::store::sdk::{self, SdkStore};
use crate::store::uv::{UvRelease, UvStore};
use crate::store::venv::{Outcome, VenvStore};
use crate::store::workspace::{WorkspaceStore, resolve_branch};
use crate::store::{self as stores};
use crate::tools;

#[derive(Debug, Clone, Default)]
pub struct SyncOptions {
    pub verbose: bool,
    pub accept_blob_licenses: bool,
    /// Re-run `west update` and blob fetching even if the markers say everything is present,
    /// and re-resolve everything `zem.lock` pins.
    pub update: bool,
    /// Fail instead of updating `zem.lock` when the result would differ from it.
    pub locked: bool,
}

struct Steps {
    total: usize,
    current: usize,
    title: String,
    started: Instant,
}

impl Steps {
    fn begin(&mut self, title: &str) {
        self.current += 1;
        self.title = title.to_string();
        self.started = Instant::now();
        eprintln!(
            "{} {title}",
            style(format!("[{}/{}]", self.current, self.total)).dim()
        );
    }

    fn done(&self, detail: impl AsRef<str>) {
        eprintln!(
            "      {} {} {}",
            style("✓").green(),
            detail.as_ref(),
            style(format!("({:.1}s)", self.started.elapsed().as_secs_f64())).dim()
        );
    }

    fn fail(&self) {
        eprintln!(
            "      {} {} failed {}\n",
            style("✗").red().bold(),
            style(&self.title).red(),
            style(format!(
                "(step {}/{}, {:.1}s)",
                self.current,
                self.total,
                self.started.elapsed().as_secs_f64()
            ))
            .dim()
        );
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn outcome(o: Outcome, reused: &str, created: &str) -> String {
    match o {
        Outcome::Reused => reused.to_string(),
        Outcome::Created => created.to_string(),
    }
}

fn sync_env(home: &ZemHome, plan: &SyncPlan, cfg: &crate::config::ZemConfig) -> EnvBuilder {
    zem_env(
        &EnvLocations {
            home,
            venv_dir: &plan.venv_dir,
            zephyr_base: None,
            sdk_dir: None,
            west_config: None,
        },
        &cfg.env,
    )
}

/// Pins only in `locked` and pins only in `actual`, for a readable mismatch message.
fn package_diff(locked: &[String], actual: &[String]) -> (String, String) {
    let only = |a: &[String], b: &[String]| {
        let pins: Vec<_> = a.iter().filter(|p| !b.contains(p)).cloned().collect();
        if pins.is_empty() {
            "nothing".to_string()
        } else {
            pins.join(", ")
        }
    };
    (only(locked, actual), only(actual, locked))
}

/// Decide between the shared venv and a package-dedicated sibling while holding the shared
/// venv lock, keeping that lock when the shared venv is chosen and switching to the sibling's
/// lock otherwise so no concurrent rebuild can race the decision.
fn lock_and_select_venv(
    home: &ZemHome,
    plan: &mut SyncPlan,
    packages: Option<&[String]>,
) -> Result<FileLock> {
    lock_and_select_venv_with(home, plan, packages, stores::lock)
}

fn lock_and_select_venv_with(
    home: &ZemHome,
    plan: &mut SyncPlan,
    packages: Option<&[String]>,
    acquire: impl Fn(&ZemHome, &str) -> Result<FileLock>,
) -> Result<FileLock> {
    let shared_name = plan.venv_key.dir_name();
    let shared_lock = acquire(home, &format!("venv-{shared_name}"))?;
    plan.select_venv(home, packages);
    if plan.venv_name == shared_name {
        Ok(shared_lock)
    } else {
        drop(shared_lock);
        acquire(home, &format!("venv-{}", plan.venv_name))
    }
}

pub fn run(
    loaded: &LoadedConfig,
    home: &ZemHome,
    plan: &SyncPlan,
    opts: &SyncOptions,
) -> Result<ProjectState> {
    let mut steps = Steps {
        total: 7,
        current: 0,
        title: String::new(),
        started: Instant::now(),
    };
    let result = run_steps(loaded, home, plan, opts, &mut steps);
    if result.is_err() && steps.current > 0 {
        steps.fail();
    }
    result
}

fn run_steps(
    loaded: &LoadedConfig,
    home: &ZemHome,
    plan: &SyncPlan,
    opts: &SyncOptions,
    steps: &mut Steps,
) -> Result<ProjectState> {
    platform::ensure_supported("zem sync")?;
    let cfg = &loaded.config;
    let mut plan = plan.clone();
    let pins = if opts.update {
        Pins::default()
    } else {
        Pins::new(plan.lock.as_ref(), cfg, opts.locked)?
    };
    let project = plan.project.clone();
    project.ensure_state_dir()?;
    let runner = Runner::new(Some(project.log_path("sync")), opts.verbose)?;

    for dir in [
        home.workspaces_dir(),
        home.venvs_dir(),
        home.locks_dir(),
        home.cache_dir(),
    ] {
        fsutil::create_dir_all(&dir)?;
    }
    fsutil::touch(&home.westconfig_path())?;

    // west runs git for every workspace update, and branch revisions need `git ls-remote`:
    // fail with an install hint before anything is downloaded.
    tools::require_in_env(&sync_env(home, &plan, cfg), &tools::GIT)?;

    if plan.unresolved_branch
        || (opts.update && !classify_revision(&cfg.zephyr.version).is_immutable())
    {
        let revision = cfg.zephyr.version.trim();
        let commit = resolve_branch(
            &sync_env(home, &plan, cfg),
            &runner,
            &cfg.zephyr.url,
            revision,
        )?;
        eprintln!(
            "{} `{revision}` is at {}",
            style("pinning").dim(),
            short(&commit)
        );
        plan.pin_commit(loaded, home, &commit);
    }
    let constraints = match &pins.packages {
        Some(packages) => {
            let path = project.state_dir().join("constraints.txt");
            fsutil::atomic_write(&path, format!("{}\n", packages.join("\n")).as_bytes())?;
            Some(path)
        }
        None => None,
    };

    let ws_key = plan.workspace_key.dir_name();
    let _ws_lock = stores::lock(home, &format!("workspace-{ws_key}"))?;
    let _venv_lock = lock_and_select_venv(home, &mut plan, pins.packages.as_deref())?;
    let venv_key = plan.venv_name.clone();
    let locked_packages = pins.packages.as_deref().zip(constraints.as_deref());

    let env = sync_env(home, &plan, cfg);

    steps.begin("Python environment");
    let uv = UvStore {
        home,
        release: UvRelease::pinned()?,
    }
    .ensure()?;
    if uv.fetched {
        eprintln!("      {} downloaded uv {}", style("·").dim(), uv.version);
    }
    let mut venv = VenvStore::new(&plan.venv_dir, &cfg.python, &env, &runner);
    let created = venv.ensure_bootstrap(&uv.path, locked_packages)?;
    steps.done(outcome(
        created,
        &format!("reused {venv_key}"),
        &format!(
            "created {venv_key} with uv {} (Python {})",
            uv.version, cfg.python.version
        ),
    ));

    steps.begin(&format!("Zephyr workspace {}", cfg.zephyr.version));
    let west = venv.west();
    let workspace = WorkspaceStore {
        dir: &plan.workspace_dir,
        key: &plan.workspace_key,
        west: &west,
        env: &env,
        runner: &runner,
    };
    let ws = workspace.ensure(&plan.modules, opts.update)?;
    let commits = workspace
        .commits(std::iter::once("zephyr").chain(plan.modules.iter().map(String::as_str)))?;
    let commit_of = |name: &str| {
        commits
            .get(name)
            .cloned()
            .ok_or_else(|| ZemError::NotSynced {
                reason: format!("west reported no commit for `{name}`"),
            })
    };
    let zephyr_commit = commit_of("zephyr")?;
    pins.verify("zephyr commit", pins.commit.as_deref(), &zephyr_commit)?;
    let mut module_commits = BTreeMap::new();
    for module in &plan.modules {
        let sha = commit_of(module)?;
        pins.verify(
            &format!("module {module}"),
            pins.modules.get(module).map(String::as_str),
            &sha,
        )?;
        module_commits.insert(module.clone(), sha);
    }
    steps.done(match (ws.updated, ws.added.is_empty()) {
        (false, _) => format!("up to date ({ws_key})"),
        (true, true) => format!("updated {ws_key}"),
        (true, false) => format!("{ws_key}: added {}", ws.added.join(", ")),
    });

    steps.begin("Board");
    board_yml::verify(&ws.zephyr_base, &plan.target, &cfg.zephyr.version)?;
    let mut target = plan.target.clone();
    target.source = ResolutionSource::BoardYml;
    steps.done(format!(
        "{} ({} / {})",
        target.board,
        target.soc.name(),
        target.arch
    ));

    steps.begin("Python requirements");
    let (created, packages) =
        venv.ensure_requirements(&uv.path, &ws.zephyr_base, locked_packages)?;
    if let Some(locked) = &pins.packages
        && *locked != packages
    {
        let (removed, added) = package_diff(locked, &packages);
        pins.verify("python packages", Some(&removed), &added)?;
    }
    steps.done(outcome(
        created,
        &format!("up to date ({} packages)", packages.len()),
        &format!("installed {} packages", packages.len()),
    ));

    let sdk_version = sdk::resolve_version(&cfg.sdk.version, &ws.zephyr_base)?;
    pins.verify("SDK version", pins.sdk.as_deref(), &sdk_version)?;
    steps.begin(&format!("Zephyr SDK {sdk_version}"));
    let sdk_out = SdkStore {
        home,
        version: &sdk_version,
    }
    .ensure(&target.toolchains)?;
    steps.done(if sdk_out.installed.is_empty() {
        format!("up to date ({})", target.toolchains.join(", "))
    } else {
        format!("installed {}", sdk_out.installed.join(", "))
    });

    steps.begin("Binary blobs");
    let mut blob_env = env.clone();
    blob_env.set("ZEPHYR_BASE", &ws.zephyr_base);
    let fetched = WorkspaceStore {
        env: &blob_env,
        ..workspace
    }
    .ensure_blobs(
        &target.blob_modules,
        cfg.blobs.accept_licenses || opts.accept_blob_licenses,
        opts.update,
    )?;
    steps.done(if fetched.is_empty() {
        "up to date".to_string()
    } else {
        format!("fetched {}", fetched.join(", "))
    });

    steps.begin("Project state");
    let modules: BTreeMap<String, _> = plan
        .modules
        .iter()
        .map(|m| {
            ws.projects
                .get(m)
                .map(|p| (m.clone(), p.clone()))
                .ok_or_else(|| ZemError::ModuleNotInManifest {
                    module: m.clone(),
                    revision: cfg.zephyr.version.clone(),
                })
        })
        .collect::<Result<_>>()?;
    let synced_at = now();
    let state = ProjectState {
        schema: STATE_SCHEMA,
        zem_version: env!("CARGO_PKG_VERSION").to_string(),
        config_hash: loaded.content_hash.clone(),
        zem_home: home.root().to_path_buf(),
        target: TargetState {
            board: target.board.clone(),
            soc: target.soc.name().to_string(),
            arch: target.arch,
            toolchains: target.toolchains.iter().map(|s| s.to_string()).collect(),
            source: target.source,
        },
        workspace: StoreRef {
            key: ws_key.clone(),
            path: plan.workspace_dir.clone(),
        },
        zephyr_base: ws.zephyr_base.clone(),
        modules,
        venv: StoreRef {
            key: venv_key.clone(),
            path: plan.venv_dir.clone(),
        },
        sdk: Some(StoreRef {
            key: sdk_version.clone(),
            path: sdk_out.dir.clone(),
        }),
        synced_at,
    };
    let lock = Lockfile {
        version: lockfile::LOCK_VERSION,
        zephyr: ZephyrLock {
            url: cfg.zephyr.url.clone(),
            revision: cfg.zephyr.version.trim().to_string(),
            commit: zephyr_commit,
        },
        modules: module_commits,
        sdk: SdkLock {
            version: sdk_version.clone(),
        },
        python: PythonLock::new(&cfg.python, packages),
    };
    let lock_note = if opts.locked || plan.lock.as_ref() == Some(&lock) {
        "zem.lock unchanged"
    } else {
        lockfile::write(project.root(), &lock)?;
        if plan.lock.is_some() {
            "updated zem.lock"
        } else {
            "created zem.lock"
        }
    };
    registry::record(
        home,
        project.root(),
        ProjectEntry {
            workspace: ws_key,
            venv: venv_key,
            sdk: Some(sdk_version),
            last_sync: synced_at,
        },
    )?;
    project.write_state(&state)?;
    if let Ok(exe) = std::env::current_exe() {
        project.write_activate_scripts(&exe)?;
    }
    steps.done(format!("{lock_note}, {}", project.state_path().display()));

    Ok(state)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};
    use std::thread;

    use super::*;
    use crate::config;
    use crate::store::venv::VenvMarker;
    use crate::store::{MARKER_SCHEMA, write_marker};
    use crate::sync;

    fn test_plan(project_root: &std::path::Path, home: &ZemHome) -> SyncPlan {
        std::fs::write(
            project_root.join(config::CONFIG_FILE_NAME),
            "schema = 1\n[project]\nname = \"p\"\n[zephyr]\nversion = \"v4.2.0\"\n\
             [target]\nboard = \"esp32c3_devkitm\"\n",
        )
        .unwrap();
        let loaded = config::load(project_root).unwrap();
        sync::plan(&loaded, home).unwrap()
    }

    #[test]
    fn lock_and_select_venv_keeps_shared_lock_when_selected() {
        let tmp = tempfile::tempdir().unwrap();
        let home = ZemHome::new(tmp.path().join("home"));
        let proj = tmp.path().join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        let mut plan = test_plan(&proj, &home);
        let shared = plan.venv_key.dir_name();

        let guard = lock_and_select_venv(&home, &mut plan, None).unwrap();
        assert_eq!(plan.venv_name, shared);
        assert!(
            stores::try_lock(&home, &format!("venv-{shared}"))
                .unwrap()
                .is_none()
        );
        drop(guard);
        assert!(
            stores::try_lock(&home, &format!("venv-{shared}"))
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn lock_and_select_venv_observes_state_written_under_shared_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let home = Arc::new(ZemHome::new(tmp.path().join("home")));
        let proj = tmp.path().join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        let mut plan = test_plan(&proj, &home);
        let shared_name = plan.venv_key.dir_name();
        let shared_dir = home.venv_dir(&plan.venv_key);

        // Pre-lock the shared venv before the locked project calls `lock_and_select_venv`.
        let held = stores::lock(&home, &format!("venv-{shared_name}")).unwrap();
        let contending = Arc::new(Barrier::new(2));
        let contending_bg = Arc::clone(&contending);
        let home_bg = Arc::clone(&home);
        let expected_shared_lock = format!("venv-{shared_name}");
        let locked_pkgs = vec!["west==1.4.0".to_string()];
        let handle = thread::spawn(move || {
            let guard = lock_and_select_venv_with(
                &home_bg,
                &mut plan,
                Some(&locked_pkgs),
                move |home, name| {
                    if name == expected_shared_lock {
                        // Prove we are actually contending on the held shared lock before
                        // signalling the main thread to populate the marker and release it.
                        assert!(stores::try_lock(home, name)?.is_none());
                        contending_bg.wait();
                    }
                    stores::lock(home, name)
                },
            )
            .unwrap();
            (plan, guard)
        });

        // Wait until the background thread has observed lock contention on the shared lock,
        // then populate a different package set in the shared venv and release the lock.
        contending.wait();
        std::fs::create_dir_all(&shared_dir).unwrap();
        write_marker(
            &shared_dir,
            &VenvMarker {
                schema: MARKER_SCHEMA,
                installer: "uv".to_string(),
                python: "3.12".to_string(),
                bootstrap: vec!["west".to_string()],
                requirements_sha256: Some("abc".to_string()),
                packages: Some(vec!["west==1.5.0".to_string()]),
            },
        )
        .unwrap();
        drop(held);

        let (selected_plan, dedicated_guard) = handle.join().unwrap();
        assert_ne!(selected_plan.venv_name, shared_name);
        assert!(
            selected_plan
                .venv_name
                .starts_with(&format!("{shared_name}-"))
        );
        // Shared lock was released; dedicated lock is held.
        assert!(
            stores::try_lock(&home, &format!("venv-{shared_name}"))
                .unwrap()
                .is_some()
        );
        assert!(
            stores::try_lock(&home, &format!("venv-{}", selected_plan.venv_name))
                .unwrap()
                .is_none()
        );
        drop(dedicated_guard);
    }
}
