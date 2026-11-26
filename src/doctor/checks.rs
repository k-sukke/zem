use std::path::Path;

use super::{Check, Report, Status};
use crate::config::{self, LoadedConfig};
use crate::env::{self, shell::posix_quote_if_needed};
use crate::home::ZemHome;
use crate::platform::{self, Os, SerialAccess};
use crate::store::uv::{UvRelease, UvStore};
use crate::tools::{self, MIN_CMAKE, Probe, ToolSpec};
use crate::{serial, session, sync};

const SYNCED_FOLDER_MARKERS: &[&str] = &["mobile documents", "onedrive", "dropbox", "google drive"];

pub fn collect(cwd: &Path) -> Report {
    let project = config::load(cwd);
    let host_tools_via_pip = project
        .as_ref()
        .map(|p| p.config.python.host_tools_via_pip)
        .unwrap_or(true);

    Report {
        sections: vec![
            ("Host", host_checks()),
            ("Required tools", required_tool_checks()),
            ("Build tools", build_tool_checks(host_tools_via_pip)),
            ("Environment", environment_checks()),
            ("Project", project_checks(project)),
        ],
    }
}

fn host_checks() -> Vec<Check> {
    let os = Os::current();
    let mut checks = vec![if os.runs_environments() {
        Check::new(
            Status::Ok,
            "OS",
            format!("{} ({})", os.name(), platform::arch_name()),
        )
    } else {
        Check::new(
            Status::Warn,
            "OS",
            format!(
                "{} ({}): sync/build/flash are not supported yet",
                os.name(),
                platform::arch_name()
            ),
        )
    }];

    checks.push(match ZemHome::resolve() {
        Ok(home) => {
            let path = home.root().display().to_string();
            let lower = path.to_lowercase();
            let state = if home.root().is_dir() {
                "exists"
            } else {
                "will be created"
            };
            if SYNCED_FOLDER_MARKERS.iter().any(|m| lower.contains(m)) {
                Check::new(Status::Warn, "ZEM_HOME", format!("{path} ({state})")).hint(Some(
                    "synced folders break file locks and are slow; set ZEM_HOME to a local path",
                ))
            } else {
                Check::new(Status::Ok, "ZEM_HOME", format!("{path} ({state})"))
            }
        }
        Err(err) => Check::new(Status::Fail, "ZEM_HOME", err.to_string()),
    });
    checks
}

fn describe(probe: &Probe) -> String {
    match probe.version {
        Some(v) => format!("{v} ({})", probe.path.display()),
        None => format!("unknown version ({})", probe.path.display()),
    }
}

fn missing(status: Status, spec: &ToolSpec, detail: &str) -> Check {
    Check::new(status, spec.name, detail).install(platform::install_hint(spec.name))
}

fn required_tool_checks() -> Vec<Check> {
    vec![
        match tools::probe(&tools::GIT) {
            Some(p) => Check::new(Status::Ok, "git", describe(&p)),
            None => missing(Status::Fail, &tools::GIT, "not found (required by west)"),
        },
        uv_check(),
    ]
}

fn uv_check() -> Check {
    let release = match UvRelease::pinned() {
        Ok(release) => release,
        Err(err) => return Check::new(Status::Fail, "uv", err.to_string()),
    };
    let version = release.version.clone();
    let host_uv = tools::probe(&tools::UV);
    let check = match ZemHome::resolve() {
        Ok(home) => {
            let store = UvStore {
                home: &home,
                release,
            };
            if store.installed() {
                Check::new(
                    Status::Ok,
                    "uv",
                    format!("{version} (managed by zem, {})", store.binary().display()),
                )
            } else {
                Check::new(
                    Status::Info,
                    "uv",
                    format!("{version} will be downloaded by `zem sync`"),
                )
            }
        }
        Err(_) => Check::new(
            Status::Info,
            "uv",
            format!("{version} will be downloaded by `zem sync`"),
        ),
    };
    match host_uv {
        Some(p) => check.hint(Some(format!(
            "host uv {} is ignored; zem uses its own pinned copy",
            describe(&p)
        ))),
        None => check,
    }
}

fn build_tool_checks(host_tools_via_pip: bool) -> Vec<Check> {
    let mut checks = Vec::new();
    for (spec, min) in [(tools::CMAKE, Some(MIN_CMAKE)), (tools::NINJA, None)] {
        let check = match (tools::probe(&spec), host_tools_via_pip) {
            (Some(p), true) => Check::new(
                Status::Info,
                spec.name,
                format!(
                    "host {} found; zem uses its own copy in the venv",
                    describe(&p)
                ),
            ),
            (None, true) => Check::new(
                Status::Info,
                spec.name,
                "will be installed into the venv by `zem sync`",
            ),
            (Some(p), false) if min.is_some_and(|m| p.version.is_some_and(|v| v < m)) => missing(
                Status::Fail,
                &spec,
                &format!(
                    "{} is older than {}",
                    describe(&p),
                    min.unwrap_or(MIN_CMAKE)
                ),
            ),
            (Some(p), false) => Check::new(Status::Ok, spec.name, describe(&p)),
            (None, false) => missing(
                Status::Fail,
                &spec,
                "not found (required because [python].host_tools_via_pip = false)",
            ),
        };
        checks.push(check);
    }

    checks.push(match tools::probe(&tools::DTC) {
        Some(p) => Check::new(Status::Ok, "dtc", describe(&p)),
        None => missing(
            Status::Warn,
            &tools::DTC,
            "not found (recommended for devicetree checks)",
        ),
    });

    checks.push(match tools::probe(&tools::WEST) {
        Some(p) => Check::new(
            Status::Info,
            "west",
            format!(
                "global {} is ignored; zem installs west per environment",
                describe(&p)
            ),
        ),
        None => Check::new(
            Status::Ok,
            "west",
            "not needed globally; zem installs it per environment",
        ),
    });
    checks
}

fn environment_checks() -> Vec<Check> {
    let mut checks = Vec::new();
    let mut present: Vec<String> = std::env::vars_os()
        .filter(|(k, v)| !v.is_empty() && env::is_denied(k))
        .map(|(k, _)| k.to_string_lossy().into_owned())
        .collect();
    present.sort();
    checks.push(if present.is_empty() {
        Check::new(Status::Ok, "variables", "no conflicting variables set")
    } else {
        Check::new(
            Status::Warn,
            "variables",
            format!("{} set", present.join(", ")),
        )
        .hint(Some(
            "zem removes these when it runs west/cmake; no action needed unless you rely on them",
        ))
    });

    match platform::serial_access() {
        SerialAccess::Ok => checks.push(Check::new(
            Status::Ok,
            "serial access",
            "user is in a serial group",
        )),
        SerialAccess::MissingGroup { candidates } => checks.push(
            Check::new(
                Status::Warn,
                "serial access",
                format!("user is not in {}", candidates.join(" / ")),
            )
            .hint(Some(format!(
                "sudo usermod -aG {} $USER, then log out and back in",
                candidates[0]
            )))
            .fix(format!("sudo usermod -aG {} $USER", candidates[0])),
        ),
        SerialAccess::Unknown => checks.push(Check::new(
            Status::Info,
            "serial access",
            "could not determine",
        )),
        SerialAccess::NotApplicable => {}
    }

    let ports = serial::usb_ports();
    checks.push(if ports.is_empty() {
        Check::new(
            Status::Info,
            "serial ports",
            "no USB serial device connected",
        )
    } else {
        let names: Vec<_> = ports.iter().map(|p| p.describe()).collect();
        Check::new(Status::Ok, "serial ports", names.join(", "))
    });
    checks
}

fn sync_check(plan: &sync::SyncPlan, loaded: &LoadedConfig, home: &ZemHome) -> Check {
    let result = plan.project.read_state().and_then(|state| match state {
        Some(state) => session::check(&state, loaded, home).map(|()| Some(state)),
        None => Ok(None),
    });
    match result {
        Ok(Some(state)) => Check::new(
            Status::Ok,
            "sync",
            format!(
                "ready ({} / SDK {})",
                state.workspace.key,
                state.sdk.map(|s| s.key).unwrap_or_default()
            ),
        ),
        Ok(None) => Check::new(Status::Info, "sync", "not synced yet").hint(Some("run `zem sync`")),
        Err(err) => Check::new(Status::Warn, "sync", err.to_string())
            .hint(Some("run `zem sync`"))
            .fix(format!(
                "zem --project-dir {} sync",
                posix_quote_if_needed(&plan.project.root().display().to_string())
            )),
    }
}

fn project_checks(project: crate::Result<LoadedConfig>) -> Vec<Check> {
    let loaded = match project {
        Ok(loaded) => loaded,
        Err(crate::ZemError::ConfigNotFound { .. }) => {
            return vec![
                Check::new(Status::Info, "zem.toml", "not found")
                    .hint(Some("run `zem init` in your project directory")),
            ];
        }
        Err(err) => return vec![Check::new(Status::Fail, "zem.toml", err.to_string())],
    };

    let mut checks = vec![Check::new(
        Status::Ok,
        "zem.toml",
        loaded.path.display().to_string(),
    )];
    let home = match ZemHome::resolve() {
        Ok(home) => home,
        Err(err) => {
            checks.push(Check::new(Status::Fail, "plan", err.to_string()));
            return checks;
        }
    };
    match sync::plan(&loaded, &home) {
        Ok(plan) => {
            let t = &plan.target;
            checks.push(Check::new(Status::Ok, "board", t.board.clone()));
            checks.push(Check::new(
                Status::Ok,
                "target",
                format!(
                    "{} / {} → {}",
                    t.soc.name(),
                    t.arch,
                    t.toolchains.join(", ")
                ),
            ));
            for warning in &plan.warnings {
                checks.push(Check::new(Status::Warn, "config", warning.clone()));
            }
            checks.push(sync_check(&plan, &loaded, &home));
        }
        Err(err) => checks.push(Check::new(Status::Fail, "config", err.to_string())),
    }
    checks
}
