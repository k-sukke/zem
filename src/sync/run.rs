use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use console::style;

use super::SyncPlan;
use super::plan::short;
use crate::board::{ResolutionSource, board_yml};
use crate::config::{LoadedConfig, ZemConfig, classify_revision};
use crate::env::{EnvBuilder, EnvLocations, zem_env};
use crate::error::{Result, ZemError};
use crate::fsutil;
use crate::home::ZemHome;
use crate::lockfile::{self, Lockfile, Pins, PythonLock, SdkLock, ZephyrLock};
use crate::platform;
use crate::process::Runner;
use crate::project::{ProjectState, STATE_SCHEMA, StoreRef, TargetState};
use crate::store::registry::{self, ProjectEntry};
use crate::store::sdk::{self, SdkStore};
use crate::store::uv::{self, UV_VERSION};
use crate::store::venv::{self, Builder, Outcome};
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

/// The isolated environment for commands run on behalf of a venv: its `bin` leads `PATH`.
fn sync_env(home: &ZemHome, venv_dir: &Path, cfg: &ZemConfig) -> EnvBuilder {
    zem_env(
        &EnvLocations {
            home,
            venv_dir,
            zephyr_base: None,
            sdk_dir: None,
            west_config: None,
        },
        &cfg.env,
    )
}

/// Check the frozen packages of a new project venv against `zem.lock` before it is published.
fn verify_packages(pins: &Pins, packages: &[String]) -> Result<()> {
    match &pins.packages {
        Some(locked) if locked.as_slice() != packages => {
            let (removed, added) = package_diff(locked, packages);
            pins.verify("python packages", Some(&removed), &added)
        }
        _ => Ok(()),
    }
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

    // Until the project venv exists, commands run with the shared west venv on PATH.
    let west_env = sync_env(home, &plan.west_dir, cfg);
    tools::require_in_env(&west_env, &tools::GIT)?;

    if plan.unresolved_branch
        || (opts.update && !classify_revision(&cfg.zephyr.version).is_immutable())
    {
        let revision = cfg.zephyr.version.trim();
        let commit = resolve_branch(&west_env, &runner, &cfg.zephyr.url, revision)?;
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
    // Held until the end of the sync: the venv must not change under the steps that use it.
    let selected = venv::select_project(home, &plan.venv_name, pins.packages.as_deref())?;

    steps.begin("Python environment");
    let uv = uv::ensure(home)?;
    let west_built = venv::ensure_west(
        &Builder {
            home,
            uv: &uv.path,
            env: &west_env,
            runner: &runner,
        },
        &cfg.python.version,
    )?;
    steps.done(format!(
        "uv {UV_VERSION} {}, west venv {}",
        if uv.fetched { "downloaded" } else { "ready" },
        outcome(west_built, "reused", "created")
    ));

    steps.begin(&format!("Zephyr workspace {}", cfg.zephyr.version));
    let west = venv::west_bin(&plan.west_dir);
    let workspace = WorkspaceStore {
        dir: &plan.workspace_dir,
        key: &plan.workspace_key,
        west: &west,
        env: &west_env,
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
        target.board, target.soc, target.arch
    ));

    steps.begin("Python requirements");
    let env = sync_env(home, &selected.dir, cfg);
    let (created, packages) = venv::ensure_project(
        &Builder {
            home,
            uv: &uv.path,
            env: &env,
            runner: &runner,
        },
        &cfg.python,
        &selected,
        &ws.zephyr_base,
        pins.packages.as_deref().zip(constraints.as_deref()),
        |packages| verify_packages(&pins, packages),
    )?;
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
    // The project venv's west, not the shared one: `west blobs` needs Zephyr's requirements
    // (requests and friends) that only the project venv has.
    let project_west = venv::west_bin(&selected.dir);
    let fetched = WorkspaceStore {
        env: &blob_env,
        west: &project_west,
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
            soc: target.soc.to_string(),
            arch: target.arch,
            toolchains: target.toolchains.clone(),
            source: target.source,
        },
        workspace: StoreRef {
            key: ws_key.clone(),
            path: plan.workspace_dir.clone(),
        },
        zephyr_base: ws.zephyr_base.clone(),
        modules,
        venv: StoreRef {
            key: selected.name.clone(),
            path: selected.dir.clone(),
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
            venv: selected.name.clone(),
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
