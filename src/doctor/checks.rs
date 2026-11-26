use std::path::Path;

use super::{Check, Report, Status};
use crate::config::{self, LoadedConfig};
use crate::home::ZemHome;
use crate::platform::{self, Os, SerialAccess};
use crate::store::uv::UV_VERSION;
use crate::tools::{self, MIN_CMAKE, Probe, ToolSpec};
use crate::{serial, session, sync};

const CONFLICTING_VARS: &[&str] = &[
    "IDF_PATH",
    "IDF_TOOLS_PATH",
    "IDF_PYTHON_ENV_PATH",
    "ZEPHYR_BASE",
    "ZEPHYR_SDK_INSTALL_DIR",
    "ZEPHYR_TOOLCHAIN_VARIANT",
    "ESPRESSIF_TOOLCHAIN_PATH",
    "VIRTUAL_ENV",
    "CONDA_PREFIX",
    "PYTHONPATH",
    "PYTHONHOME",
];

const SYNCED_FOLDER_MARKERS: &[&str] = &["mobile documents", "onedrive", "dropbox", "google drive"];

pub fn collect(cwd: &Path) -> Report {
    let project = config::load(cwd);
    let install_cmake_ninja = project
        .as_ref()
        .map(|p| p.config.python.install_cmake_ninja)
        .unwrap_or(true);
    let home = ZemHome::resolve().ok();

    Report {
        sections: vec![
            ("Host", host_checks()),
            ("Required tools", required_tool_checks(home.as_ref())),
            ("Build tools", build_tool_checks(install_cmake_ninja)),
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

fn required_tool_checks(home: Option<&ZemHome>) -> Vec<Check> {
    let git = match tools::probe(&tools::GIT) {
        Some(p) => Check::new(Status::Ok, "git", describe(&p)),
        None => missing(Status::Fail, &tools::GIT, "not found (required by west)"),
    };
    // zem downloads the uv it uses; a uv on the host is never used.
    let uv = match home.map(|h| h.uv_bin(UV_VERSION)).filter(|b| b.is_file()) {
        Some(bin) => Check::new(
            Status::Ok,
            "uv",
            format!("{UV_VERSION} (managed by zem, {})", bin.display()),
        ),
        None => Check::new(
            Status::Info,
            "uv",
            format!("{UV_VERSION} will be downloaded by `zem sync`"),
        ),
    };
    vec![git, uv]
}

fn build_tool_checks(install_cmake_ninja: bool) -> Vec<Check> {
    let mut checks = Vec::new();
    if install_cmake_ninja {
        checks.push(Check::new(
            Status::Info,
            "cmake, ninja",
            "installed into the project venv by `zem sync`",
        ));
    } else {
        for (spec, min) in [(tools::CMAKE, Some(MIN_CMAKE)), (tools::NINJA, None)] {
            let check = match tools::probe(&spec) {
                Some(p) if min.is_some_and(|m| p.version.is_some_and(|v| v < m)) => missing(
                    Status::Fail,
                    &spec,
                    &format!("{} is older than {}", describe(&p), MIN_CMAKE),
                ),
                Some(p) => Check::new(Status::Ok, spec.name, describe(&p)),
                None => missing(
                    Status::Fail,
                    &spec,
                    "not found (required because [python].install_cmake_ninja = false)",
                ),
            };
            checks.push(check);
        }
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
                "global {} is ignored; zem runs west from its own venv",
                describe(&p)
            ),
        ),
        None => Check::new(
            Status::Ok,
            "west",
            "not needed globally; zem runs west from its own venv",
        ),
    });
    checks
}

fn environment_checks() -> Vec<Check> {
    let mut checks = Vec::new();
    let present: Vec<&str> = CONFLICTING_VARS
        .iter()
        .copied()
        .filter(|name| std::env::var_os(name).is_some_and(|v| !v.is_empty()))
        .collect();
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

fn shell_quote(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
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
                shell_quote(&plan.project.root().display().to_string())
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
