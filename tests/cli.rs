use std::fs;
use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

struct Sandbox {
    project: TempDir,
    zem_home: TempDir,
}

impl Sandbox {
    fn new() -> Self {
        Sandbox {
            project: tempfile::tempdir().unwrap(),
            zem_home: tempfile::tempdir().unwrap(),
        }
    }

    fn zem(&self) -> Command {
        let mut cmd = Command::cargo_bin("zem").unwrap();
        cmd.current_dir(self.project.path())
            .env("ZEM_HOME", self.zem_home.path())
            .env("NO_COLOR", "1");
        cmd
    }

    fn path(&self) -> &Path {
        self.project.path()
    }

    fn init(&self, board: &str) {
        self.zem()
            .args(["init", "--yes", "--name", "demo", "--board", board])
            .assert()
            .success();
    }
}

#[test]
fn init_writes_config_and_gitignore() {
    let sb = Sandbox::new();
    sb.zem()
        .args([
            "init",
            "--yes",
            "--name",
            "blinky",
            "--board",
            "esp32s3_devkitc",
            "--feature",
            "mbedtls",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "esp32s3 / xtensa → xtensa-espressif_esp32s3_zephyr-elf",
        ));

    let config = fs::read_to_string(sb.path().join("zem.toml")).unwrap();
    assert!(
        config.contains(r#"board = "esp32s3_devkitc/esp32s3/procpu""#),
        "{config}"
    );
    assert!(config.contains(r#"features = ["mbedtls"]"#), "{config}");
    assert_eq!(
        fs::read_to_string(sb.path().join(".gitignore")).unwrap(),
        "build/\n"
    );
}

#[test]
fn init_refuses_to_overwrite_without_force() {
    let sb = Sandbox::new();
    sb.init("esp32c3_devkitm");
    sb.zem()
        .args(["init", "--yes", "--board", "esp32c3_devkitm"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already exists"));
    sb.zem()
        .args(["init", "--yes", "--board", "esp32c6_devkitc", "--force"])
        .assert()
        .success();
    assert!(
        fs::read_to_string(sb.path().join("zem.toml"))
            .unwrap()
            .contains("esp32c6_devkitc")
    );
}

#[test]
fn init_requires_board_when_not_interactive() {
    let sb = Sandbox::new();
    sb.zem()
        .args(["init", "--yes"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--board"));
    assert!(!sb.path().join("zem.toml").exists());
}

#[test]
fn init_rejects_non_esp_boards_and_old_zephyr() {
    let sb = Sandbox::new();
    sb.zem()
        .args(["init", "--yes", "--board", "nrf52840dk/nrf52840"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not an ESP32-series SoC"));
    sb.zem()
        .args([
            "init",
            "--yes",
            "--board",
            "esp32c3_devkitm",
            "--zephyr-version",
            "v3.7.0",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("v4.0 or later"));
    assert!(!sb.path().join("zem.toml").exists());
}

#[test]
fn sync_dry_run_shows_shared_locations() {
    let sb = Sandbox::new();
    sb.init("esp32c3_devkitm");
    sb.zem()
        .args(["sync", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("riscv64-zephyr-elf"))
        .stdout(predicate::str::contains("hal_espressif"))
        .stdout(predicate::str::contains("zephyr-v4.2.0-"));
    assert!(
        fs::read_dir(sb.zem_home.path()).unwrap().next().is_none(),
        "dry run must not touch ZEM_HOME"
    );
    assert!(
        !sb.path().join(".zem").exists(),
        "dry run must not create .zem/"
    );
}

#[test]
fn projects_with_same_zephyr_share_a_workspace() {
    let a = Sandbox::new();
    let b = Sandbox::new();
    a.init("esp32c3_devkitm");
    b.zem()
        .args([
            "init",
            "--yes",
            "--board",
            "esp32s3_devkitc",
            "--feature",
            "mbedtls",
        ])
        .assert()
        .success();

    let workspace_line = |sb: &Sandbox| {
        let out = sb
            .zem()
            .args(["sync", "--dry-run"])
            .env("ZEM_HOME", "/shared/zem")
            .output()
            .unwrap();
        String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .find(|l| l.trim_start().starts_with("workspace"))
            .unwrap()
            .to_string()
    };
    assert_eq!(workspace_line(&a), workspace_line(&b));
}

#[test]
fn sync_finds_config_from_subdirectory() {
    let sb = Sandbox::new();
    sb.init("esp32c3_devkitm");
    let sub = sb.path().join("src");
    fs::create_dir_all(&sub).unwrap();
    sb.zem()
        .current_dir(&sub)
        .args(["sync", "--dry-run"])
        .assert()
        .success();
}

#[test]
fn sync_rejects_zem_home_in_project_state_dir() {
    let sb = Sandbox::new();
    sb.init("esp32c3_devkitm");
    sb.zem()
        .env("ZEM_HOME", sb.path().join(".zem"))
        .args(["sync", "--dry-run"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("collides"));
}

#[test]
fn sync_reports_config_errors_with_location() {
    let sb = Sandbox::new();
    sb.init("esp32c3_devkitm");
    let path = sb.path().join("zem.toml");
    let text = fs::read_to_string(&path)
        .unwrap()
        .replace("[build]\n", "[build]\ndirr = \"x\"\n");
    fs::write(&path, text).unwrap();
    sb.zem()
        .args(["sync", "--dry-run"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("dirr"));
}

#[cfg(unix)]
#[test]
fn sync_ignores_host_uv_and_fails_cleanly_when_it_cannot_download() {
    use std::os::unix::fs::PermissionsExt;

    let sb = Sandbox::new();
    sb.init("esp32c3_devkitm");

    // A uv on PATH must never be used: zem manages its own pinned copy. git is required, so
    // the PATH keeps the real one.
    let bin = tempfile::tempdir().unwrap();
    let git = which::which("git").expect("git is required to run the tests");
    std::os::unix::fs::symlink(git, bin.path().join("git")).unwrap();
    let ran = bin.path().join("host-uv-ran");
    let fake_uv = bin.path().join("uv");
    fs::write(
        &fake_uv,
        // `: >` is a shell builtin; the child PATH holds only git and uv, so no `touch`.
        format!("#!/bin/sh\n: > '{}'\nexit 0\n", ran.display()),
    )
    .unwrap();
    fs::set_permissions(&fake_uv, fs::Permissions::from_mode(0o755)).unwrap();

    // Point the download at a port nothing listens on, so no network is needed.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let mirror = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);

    sb.zem()
        .env("PATH", bin.path())
        .env("ZEM_UV_BASE_URL", &mirror)
        .arg("sync")
        .assert()
        .failure()
        .stderr(predicate::str::contains("download of"))
        .stderr(predicate::str::contains(&mirror));
    assert!(!ran.exists(), "the uv on PATH was executed");

    let ignore = fs::read_to_string(sb.path().join(".zem").join(".gitignore")).unwrap();
    assert!(ignore.lines().any(|l| l == "*"));
    assert!(
        fs::read_dir(sb.zem_home.path().join("venvs"))
            .unwrap()
            .next()
            .is_none()
    );
    assert!(
        !sb.zem_home
            .path()
            .join("tools")
            .read_dir()
            .is_ok_and(|mut d| d.any(|e| e.is_ok_and(|e| e.file_name() == "uv-0.13.10"))),
        "a failed download must not leave an installed uv"
    );
}

#[cfg(unix)]
#[test]
fn sync_without_git_fails_before_downloading() {
    let sb = Sandbox::new();
    sb.init("esp32c3_devkitm");
    let empty_path = tempfile::tempdir().unwrap();
    sb.zem()
        .env("PATH", empty_path.path())
        .arg("sync")
        .assert()
        .failure()
        .stderr(predicate::str::contains("git was not found"));
    assert!(
        fs::read_dir(sb.zem_home.path().join("tools"))
            .map(|mut d| d.next().is_none())
            .unwrap_or(true),
        "nothing may be downloaded before git is found"
    );
}

#[test]
fn sync_dry_run_shows_the_managed_uv() {
    let sb = Sandbox::new();
    sb.init("esp32c3_devkitm");
    sb.zem()
        .args(["sync", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("managed by zem"));
}

#[test]
fn cache_commands_work_on_an_empty_home() {
    let sb = Sandbox::new();
    sb.zem()
        .args(["cache", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("0 project(s) registered"));
    fs::create_dir_all(sb.zem_home.path().join("venvs/py3.12-orphan")).unwrap();
    sb.zem()
        .args(["cache", "gc", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("would remove venv py3.12-orphan"));
    assert!(sb.zem_home.path().join("venvs/py3.12-orphan").is_dir());
    sb.zem().args(["cache", "gc"]).assert().success();
    assert!(!sb.zem_home.path().join("venvs/py3.12-orphan").exists());
    sb.zem()
        .args(["cache", "gc"])
        .assert()
        .success()
        .stdout(predicate::str::contains("nothing to clean up"));
}

#[cfg(unix)]
#[test]
fn locked_sync_requires_a_lockfile() {
    let sb = Sandbox::new();
    sb.init("esp32c3_devkitm");
    sb.zem()
        .args(["sync", "--locked"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("zem.lock not found"));
    sb.zem()
        .args(["sync", "--locked", "--update"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot be used with"));
}

#[test]
fn branch_revisions_use_the_locked_commit() {
    let sb = Sandbox::new();
    sb.zem()
        .args([
            "init",
            "--yes",
            "--name",
            "demo",
            "--board",
            "esp32c3_devkitm",
            "--zephyr-version",
            "main",
        ])
        .assert()
        .success();
    sb.zem()
        .args(["sync", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("resolved from `main` during sync"))
        .stdout(predicate::str::contains("none (created by sync)"));

    let commit = "02f236e1206013b780c522f4edc47badbaeb2a62";
    fs::write(
        sb.path().join("zem.lock"),
        format!(
            r#"version = 1
[zephyr]
url = "https://github.com/zephyrproject-rtos/zephyr"
revision = "main"
commit = "{commit}"
[sdk]
version = "0.17.2"
[python]
version = "3.12"
host-tools-via-pip = true
packages = []
"#
        ),
    )
    .unwrap();
    sb.zem()
        .args(["sync", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "workspaces{}zephyr-02f236e120601",
            std::path::MAIN_SEPARATOR
        )))
        .stdout(predicate::str::contains("zem.lock (02f236e12060)"));

    fs::write(sb.path().join("zem.lock"), "version = 2\n").unwrap();
    sb.zem()
        .args(["sync", "--dry-run"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("zem.lock is invalid"));
}

/// A project whose `.zem/state.json` points at a fake ZEM_HOME, so build/run/env can be tested
/// without downloading anything. `west` is a script that prints its arguments.
#[cfg(unix)]
mod synced {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    pub fn setup(sb: &Sandbox) {
        setup_with(sb, "");
    }

    /// `extra_toml` is appended to the generated zem.toml before it is hashed into the state.
    pub fn setup_with(sb: &Sandbox, extra_toml: &str) {
        sb.init("esp32c3_devkitm");
        if !extra_toml.is_empty() {
            let path = sb.path().join("zem.toml");
            let text = fs::read_to_string(&path).unwrap();
            fs::write(&path, format!("{text}\n{extra_toml}")).unwrap();
        }
        write_state(sb);
    }

    /// Fake stores in ZEM_HOME and a `.zem/state.json` matching the current zem.toml.
    pub fn write_state(sb: &Sandbox) {
        let home = fs::canonicalize(sb.zem_home.path()).unwrap();
        let ws = home.join("workspaces/zephyr-v4.2.0-test");
        let venv = home.join("venvs/py3.12-test");
        let sdk = home.join("sdk/zephyr-sdk-0.17.2");
        for dir in [
            ws.join("zephyr"),
            ws.join("modules/hal/espressif"),
            ws.join("modules/crypto/mbedtls"),
            venv.join("bin"),
            sdk.join("riscv64-zephyr-elf/bin"),
        ] {
            fs::create_dir_all(dir).unwrap();
        }
        let python = venv.join("bin/python");
        fs::write(&python, "#!/bin/sh\necho \"python $*\"\n").unwrap();
        fs::set_permissions(&python, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(
            sdk.join("riscv64-zephyr-elf/bin/riscv64-zephyr-elf-gcc"),
            "",
        )
        .unwrap();
        let west = venv.join("bin/west");
        fs::write(
            &west,
            "#!/bin/sh\necho \"west $*\"\necho \"sdk=$ZEPHYR_SDK_INSTALL_DIR base=$ZEPHYR_BASE idf=${IDF_PATH:-unset}\"\necho \"cwd=$(pwd -P)\"\necho \"westcfg=$WEST_CONFIG_LOCAL\"\n",
        )
        .unwrap();
        fs::set_permissions(&west, fs::Permissions::from_mode(0o755)).unwrap();

        let config = fs::read(sb.path().join("zem.toml")).unwrap();
        let state = serde_json::json!({
            "schema": 1,
            "zem_version": "test",
            "config_hash": zem::home::keys::sha256_hex(&config),
            "zem_home": home,
            "target": {
                "board": "esp32c3_devkitm",
                "soc": "esp32c3",
                "arch": "riscv",
                "toolchains": ["riscv64-zephyr-elf"],
                "source": "board_yml"
            },
            "workspace": { "key": "zephyr-v4.2.0-test", "path": ws },
            "zephyr_base": ws.join("zephyr"),
            "modules": { "hal_espressif": ws.join("modules/hal/espressif") },
            "venv": { "key": "py3.12-test", "path": venv },
            "sdk": { "key": "0.17.2", "path": sdk },
            "synced_at": 0
        });
        fs::create_dir_all(sb.path().join(".zem")).unwrap();
        fs::write(
            sb.path().join(".zem/state.json"),
            serde_json::to_vec_pretty(&state).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn west_build_uses_project_west_config_and_isolated_env() {
        let sb = Sandbox::new();
        let path = sb.path().join("zem.toml");
        sb.init("esp32c3_devkitm");
        let text = fs::read_to_string(&path).unwrap().replace(
            "dir = \"build\"\n",
            "dir = \"build\"\ncmake_args = [\"-DEXTRA=1\"]\n",
        );
        fs::write(&path, text).unwrap();
        write_state(&sb);
        let out = sb
            .zem()
            .env("IDF_PATH", "/bogus/esp-idf")
            .env("WEST_CONFIG_LOCAL", "/bogus/config")
            .args(["west", "build", "-p"])
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        let stdout = String::from_utf8(out.stdout).unwrap();
        assert!(stdout.contains("west build -p\n"), "{stdout}");
        assert!(stdout.contains("sdk/zephyr-sdk-0.17.2 base="), "{stdout}");
        assert!(stdout.contains("idf=unset"), "{stdout}");

        let root = fs::canonicalize(sb.path()).unwrap();
        let west_config = root.join(".zem/west-config");
        assert!(
            stdout.contains(&format!("westcfg={}\n", west_config.display())),
            "{stdout}"
        );
        let text = fs::read_to_string(&west_config).unwrap();
        assert!(text.contains("board = esp32c3_devkitm\n"), "{text}");
        assert!(
            text.contains("project-filter = -.*,+zephyr,+hal_espressif\n"),
            "{text}"
        );
        assert!(
            text.contains(&format!("dir-fmt = {}\n", root.join("build").display())),
            "{text}"
        );
        assert!(
            text.contains("-DCMAKE_FIND_USE_PACKAGE_REGISTRY=OFF -DEXTRA=1\n"),
            "{text}"
        );
    }

    #[test]
    fn changed_build_settings_drop_the_cmake_cache() {
        let sb = Sandbox::new();
        let path = sb.path().join("zem.toml");
        sb.init("esp32c3_devkitm");
        write_state(&sb);
        let cache = sb.path().join("build/CMakeCache.txt");
        let build = || sb.zem().args(["west", "build"]).output().unwrap();

        assert!(build().status.success());
        fs::create_dir_all(cache.parent().unwrap()).unwrap();
        fs::write(&cache, "").unwrap();
        assert!(build().status.success());
        assert!(cache.exists(), "unchanged settings must keep the build");

        let text = fs::read_to_string(&path).unwrap().replace(
            "dir = \"build\"\n",
            "dir = \"build\"\ncmake_args = [\"-DEXTRA=1\"]\n",
        );
        fs::write(&path, text).unwrap();
        write_state(&sb);
        let out = build();
        assert!(out.status.success(), "{out:?}");
        assert!(
            !cache.exists(),
            "changed cmake-args must force a reconfigure"
        );
        assert!(String::from_utf8_lossy(&out.stderr).contains("reconfigures"));
    }

    #[test]
    fn run_uses_isolated_environment() {
        let sb = Sandbox::new();
        setup(&sb);
        let out = sb
            .zem()
            .env("IDF_PATH", "/bogus/esp-idf")
            .env("VIRTUAL_ENV", "/bogus/venv")
            .args(["run", "env"])
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        let env = String::from_utf8(out.stdout).unwrap();
        let var = |k: &str| {
            env.lines()
                .find_map(|l| l.strip_prefix(&format!("{k}=")))
                .map(str::to_string)
        };
        assert_eq!(var("IDF_PATH"), None);
        assert_eq!(var("ZEM_ACTIVE").as_deref(), Some("1"));
        assert!(var("VIRTUAL_ENV").unwrap().ends_with("venvs/py3.12-test"));
        assert!(
            var("PATH")
                .unwrap()
                .starts_with(&var("VIRTUAL_ENV").unwrap())
        );
        assert_eq!(var("ZEPHYR_TOOLCHAIN_VARIANT").as_deref(), Some("zephyr"));
    }

    #[test]
    fn strict_isolation_keeps_only_allowlisted_variables() {
        let sb = Sandbox::new();
        setup_with(
            &sb,
            "[env]\nisolation = \"strict\"\npassthrough = [\"MY_TOKEN\"]\n",
        );
        let out = sb
            .zem()
            .env("EDITOR", "vim")
            .env("MY_TOKEN", "t")
            .env("LC_ALL", "C")
            .args(["run", "env"])
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        let env = String::from_utf8(out.stdout).unwrap();
        let has = |k: &str| env.lines().any(|l| l.starts_with(&format!("{k}=")));
        assert!(!has("EDITOR") && !has("NO_COLOR"), "{env}");
        assert!(has("MY_TOKEN") && has("LC_ALL") && has("PATH") && has("ZEPHYR_BASE"));

        sb.zem()
            .env("EDITOR", "vim")
            .args(["env", "--shell", "bash"])
            .assert()
            .success()
            .stdout(predicate::str::contains("EDITOR").not());
    }

    #[test]
    fn shell_refuses_to_nest() {
        let sb = Sandbox::new();
        setup(&sb);
        sb.zem()
            .env("ZEM_SHELL", "/some/project")
            .arg("shell")
            .assert()
            .failure()
            .stderr(predicate::str::contains("already inside a zem shell"));
    }

    #[test]
    fn west_passes_arguments_through_from_project_root() {
        let sb = Sandbox::new();
        setup(&sb);
        let sub = sb.path().join("src");
        fs::create_dir_all(&sub).unwrap();
        sb.zem()
            .current_dir(&sub)
            .args(["west", "espressif", "monitor", "-d", "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains(
                "west espressif monitor -d --help\n",
            ))
            .stdout(predicate::str::contains(format!(
                "cwd={}\n",
                fs::canonicalize(sb.path()).unwrap().display()
            )));
    }

    #[test]
    fn run_forwards_exit_code() {
        let sb = Sandbox::new();
        setup(&sb);
        sb.zem()
            .args(["run", "sh", "-c", "exit 7"])
            .assert()
            .code(7);
    }

    #[test]
    fn env_prints_unset_and_exports() {
        let sb = Sandbox::new();
        setup(&sb);
        sb.zem()
            .env("IDF_PATH", "/bogus/esp-idf")
            .args(["env", "--shell", "bash"])
            .assert()
            .success()
            .stdout(predicate::str::contains("unset IDF_PATH\n"))
            .stdout(predicate::str::contains("export ZEPHYR_BASE='"));
        sb.zem()
            .args(["env", "--shell", "json"])
            .assert()
            .success()
            .stdout(predicate::str::contains("\"ZEM_ACTIVE\": \"1\""));
    }

    #[test]
    fn stale_config_requires_sync() {
        let sb = Sandbox::new();
        setup(&sb);
        let path = sb.path().join("zem.toml");
        let text = fs::read_to_string(&path).unwrap() + "\n# edited\n";
        fs::write(&path, text).unwrap();
        sb.zem()
            .args(["west", "build"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("zem.toml changed"))
            .stderr(predicate::str::contains("zem sync"));
    }
}

#[cfg(unix)]
#[test]
fn west_without_sync_points_to_sync() {
    let sb = Sandbox::new();
    sb.init("esp32c3_devkitm");
    sb.zem()
        .args(["west", "build"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not been synced"));
}

#[cfg(windows)]
#[test]
fn environment_commands_are_rejected_on_windows() {
    let sb = Sandbox::new();
    sb.init("esp32c3_devkitm");
    for cmd in [&["sync"][..], &["west", "build"]] {
        sb.zem()
            .args(cmd)
            .assert()
            .failure()
            .stderr(predicate::str::contains("not supported on Windows yet"));
    }
}

#[test]
fn init_scaffolds_app_only_when_missing() {
    let sb = Sandbox::new();
    sb.init("esp32c3_devkitm");
    assert!(sb.path().join("src/main.c").is_file());
    assert!(
        fs::read_to_string(sb.path().join("CMakeLists.txt"))
            .unwrap()
            .contains("project(demo)")
    );

    let other = Sandbox::new();
    fs::write(other.path().join("CMakeLists.txt"), "# mine\n").unwrap();
    other.init("esp32c3_devkitm");
    assert_eq!(
        fs::read_to_string(other.path().join("CMakeLists.txt")).unwrap(),
        "# mine\n"
    );
    assert!(!other.path().join("src/main.c").exists());
}

#[test]
fn doctor_runs_without_project() {
    let sb = Sandbox::new();
    sb.zem()
        .arg("doctor")
        .assert()
        .stdout(predicate::str::contains("ZEM_HOME"))
        .stdout(predicate::str::contains("zem init"));
}

#[cfg(unix)]
mod venv_rebuild {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    use zem::config::{self, Python};
    use zem::env::EnvBuilder;
    use zem::home::ZemHome;
    use zem::process::Runner;
    use zem::store::venv::{
        REQUIREMENTS_BASE, VenvMarker, VenvStore, bootstrap_packages, installed_packages,
    };
    use zem::store::{MARKER_SCHEMA, read_marker, write_marker};
    use zem::sync;

    fn write_exec(path: &Path, body: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// Fake `uv` implementing `venv`, `pip install` and `pip freeze`, controlled via env vars:
    /// - `FAKE_UV_FAIL_STEP`: `venv`, `install` or `freeze` to fail that subcommand.
    /// - `FAKE_UV_FREEZE`: newline-separated freeze output.
    /// - `FAKE_UV_LOG`: file recording every invocation.
    fn fake_uv(dir: &Path) -> PathBuf {
        let path = dir.join("uv");
        write_exec(
            &path,
            r#"#!/bin/sh
set -eu
if [ -n "${FAKE_UV_LOG:-}" ]; then
    printf '%s\n' "$*" >> "$FAKE_UV_LOG"
fi
sub="$1"
shift
case "$sub" in
    venv)
        if [ "${FAKE_UV_FAIL_STEP:-}" = "venv" ]; then
            echo "simulated uv venv failure" >&2
            exit 1
        fi
        target="$1"
        mkdir -p "$target/bin"
        printf '#!/bin/sh\nexit 0\n' > "$target/bin/python"
        chmod +x "$target/bin/python"
        ;;
    pip)
        op="$1"
        shift
        case "$op" in
            install)
                if [ "${FAKE_UV_FAIL_STEP:-}" = "install" ]; then
                    echo "simulated uv pip install failure" >&2
                    exit 1
                fi
                py=""
                while [ $# -gt 0 ]; do
                    if [ "$1" = "--python" ]; then
                        py="$2"
                        shift 2
                    else
                        shift
                    fi
                done
                bin_dir="$(dirname "$py")"
                for exe in west cmake ninja; do
                    printf '#!/bin/sh\nexit 0\n' > "$bin_dir/$exe"
                    chmod +x "$bin_dir/$exe"
                done
                ;;
            freeze)
                if [ "${FAKE_UV_FAIL_STEP:-}" = "freeze" ]; then
                    echo "simulated uv pip freeze failure" >&2
                    exit 1
                fi
                printf '%s\n' "${FAKE_UV_FREEZE:-west==1.5.0}"
                ;;
        esac
        ;;
esac
"#,
        );
        path
    }

    fn seed_working_venv(dir: &Path, installer: &str, packages: &[String]) {
        write_exec(&dir.join("bin/python"), "#!/bin/sh\necho old-python\n");
        write_exec(&dir.join("bin/west"), "#!/bin/sh\necho old-west\n");
        fs::write(dir.join("keep.txt"), "working").unwrap();
        write_marker(
            dir,
            &VenvMarker {
                schema: MARKER_SCHEMA,
                installer: installer.to_string(),
                python: "3.12".to_string(),
                bootstrap: vec!["west".to_string()],
                requirements_sha256: Some("old-sha".to_string()),
                packages: Some(packages.to_vec()),
            },
        )
        .unwrap();
    }

    #[test]
    fn failed_bootstrap_preserves_existing_environment_for_all_installers() {
        let tmp = tempfile::tempdir().unwrap();
        let uv = fake_uv(tmp.path());
        let runner = Runner::new(None, false).unwrap();
        let cfg = Python::default(); // host_tools_via_pip = true -> triggers rebuild over ["west"]

        for installer in ["venv", "uv"] {
            for fail_step in ["venv", "install"] {
                let venvs = tmp.path().join(format!("venvs-{installer}-{fail_step}"));
                let dir = venvs.join("py3.12-shared");
                let old_pkgs = vec!["PyYAML==6.0.1".to_string(), "west==1.4.0".to_string()];
                seed_working_venv(&dir, installer, &old_pkgs);

                let env = EnvBuilder::from_vars([
                    ("PATH", "/usr/bin:/bin"),
                    ("FAKE_UV_FAIL_STEP", fail_step),
                ]);
                let store = VenvStore {
                    dir: &dir,
                    cfg: &cfg,
                    env: &env,
                    runner: &runner,
                };
                assert!(store.ensure_bootstrap(&uv, None).is_err());

                assert_eq!(fs::read_to_string(dir.join("keep.txt")).unwrap(), "working");
                assert!(
                    fs::read_to_string(dir.join("bin/west"))
                        .unwrap()
                        .contains("old-west")
                );
                let marker = read_marker::<VenvMarker>(&dir).unwrap();
                assert_eq!(marker.installer, installer);
                assert_eq!(marker.packages, Some(old_pkgs));

                let entries: Vec<_> = fs::read_dir(&venvs)
                    .unwrap()
                    .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                    .collect();
                assert_eq!(entries, ["py3.12-shared"], "staging dir must be cleaned up");
            }
        }
    }

    #[test]
    fn rebuild_and_fresh_bootstrap_never_expose_unowned_shared_venv() {
        let tmp = tempfile::tempdir().unwrap();
        let home = ZemHome::new(tmp.path().join("home"));
        let proj = tmp.path().join("proj");
        fs::create_dir_all(&proj).unwrap();
        fs::write(
            proj.join(config::CONFIG_FILE_NAME),
            "schema = 1\n[project]\nname = \"p\"\n[zephyr]\nversion = \"v4.2.0\"\n\
             [target]\nboard = \"esp32c3_devkitm\"\n",
        )
        .unwrap();
        let loaded = config::load(&proj).unwrap();
        let mut plan = sync::plan(&loaded, &home).unwrap();
        let shared_dir = home.venv_dir(&plan.venv_key);
        let shared_name = plan.venv_key.dir_name();

        let uv = fake_uv(tmp.path());
        let log = tmp.path().join("uv.log");
        let runner = Runner::new(None, false).unwrap();
        let cfg = Python::default();

        // 1. Fresh unlocked bootstrap records bootstrap freeze immediately, so a locked project
        // with a different package set never treats an interrupted shared venv as unowned.
        let env = EnvBuilder::from_vars([
            ("PATH", "/usr/bin:/bin"),
            ("FAKE_UV_FREEZE", "cmake==4.0.0\nninja==1.12.1\nwest==1.5.0"),
            ("FAKE_UV_LOG", log.to_str().unwrap()),
        ]);
        let store = VenvStore {
            dir: &shared_dir,
            cfg: &cfg,
            env: &env,
            runner: &runner,
        };
        store.ensure_bootstrap(&uv, None).unwrap();
        assert!(
            fs::read_to_string(&log).unwrap().contains("--relocatable"),
            "staged venv must be relocatable"
        );
        assert!(installed_packages(&shared_dir).is_some());

        let other_locked = vec!["PyYAML==6.0.2".to_string(), "west==1.4.0".to_string()];
        plan.select_venv(&home, Some(&other_locked));
        assert_ne!(plan.venv_name, shared_name);

        // 2. Rebuild of an existing venv preserves its previous package record across bootstrap
        // until `ensure_requirements` replaces it (and keeps it if requirements install fails).
        let owner_pkgs = vec!["PyYAML==6.0.1".to_string(), "west==1.4.0".to_string()];
        seed_working_venv(&shared_dir, "venv", &owner_pkgs);
        store.ensure_bootstrap(&uv, None).unwrap();
        assert!(!shared_dir.join("keep.txt").exists(), "venv was replaced");
        assert_eq!(installed_packages(&shared_dir), Some(owner_pkgs.clone()));
        assert_eq!(
            read_marker::<VenvMarker>(&shared_dir)
                .unwrap()
                .requirements_sha256,
            None
        );
        plan.select_venv(&home, Some(&other_locked));
        assert_ne!(plan.venv_name, shared_name);

        let zephyr_base = tmp.path().join("zephyr");
        let req_path = zephyr_base.join(REQUIREMENTS_BASE);
        fs::create_dir_all(req_path.parent().unwrap()).unwrap();
        fs::write(&req_path, "PyYAML>=6.0\n").unwrap();

        // Failing `ensure_requirements` still keeps `owner_pkgs` recorded.
        let fail_env =
            EnvBuilder::from_vars([("PATH", "/usr/bin:/bin"), ("FAKE_UV_FAIL_STEP", "install")]);
        let failing_store = VenvStore {
            env: &fail_env,
            ..store
        };
        assert!(
            failing_store
                .ensure_requirements(&uv, &zephyr_base, None)
                .is_err()
        );
        assert_eq!(installed_packages(&shared_dir), Some(owner_pkgs));

        // Succeeding `ensure_requirements` replaces the package record with the new freeze.
        let new_env = EnvBuilder::from_vars([
            ("PATH", "/usr/bin:/bin"),
            ("FAKE_UV_FREEZE", "PyYAML==6.0.2\nwest==1.5.0"),
        ]);
        let new_store = VenvStore {
            env: &new_env,
            ..store
        };
        let (_, final_pkgs) = new_store
            .ensure_requirements(&uv, &zephyr_base, None)
            .unwrap();
        assert_eq!(
            final_pkgs,
            vec!["PyYAML==6.0.2".to_string(), "west==1.5.0".to_string()]
        );
        assert_eq!(installed_packages(&shared_dir), Some(final_pkgs));
        assert_eq!(
            bootstrap_packages(&cfg),
            read_marker::<VenvMarker>(&shared_dir).unwrap().bootstrap
        );
    }
}
