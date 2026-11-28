//! Network end-to-end tests. Run with `cargo test --test e2e -- --ignored --nocapture`.
//! Set `ZEM_E2E_HOME` to reuse a ZEM_HOME between runs (first run downloads ~1 GB).
#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

struct Env {
    home: PathBuf,
    _tmp_home: Option<TempDir>,
}

impl Env {
    fn new() -> Self {
        match std::env::var_os("ZEM_E2E_HOME") {
            Some(home) => {
                // Tests run in parallel and canonicalize the home, so it must exist up front.
                fs::create_dir_all(&home).unwrap();
                Env {
                    home: PathBuf::from(home),
                    _tmp_home: None,
                }
            }
            None => {
                let tmp = tempfile::tempdir().unwrap();
                Env {
                    home: tmp.path().to_path_buf(),
                    _tmp_home: Some(tmp),
                }
            }
        }
    }

    fn zem(&self, project: &Path) -> Command {
        let mut cmd = Command::cargo_bin("zem").unwrap();
        cmd.current_dir(project)
            .env("ZEM_HOME", &self.home)
            .env("NO_COLOR", "1");
        cmd
    }

    fn project(&self, args: &[&str]) -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        self.zem(dir.path())
            .args(["init", "--yes", "--name", "e2e"])
            .args(args)
            .assert()
            .success();
        dir
    }

    fn sync(&self, project: &Path) -> String {
        let out = self.zem(project).arg("sync").output().unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(out.status.success(), "sync failed:\n{stderr}");
        stderr
    }
}

fn cmake_cache_value(build_dir: &Path, key: &str) -> String {
    let text = fs::read_to_string(build_dir.join("CMakeCache.txt")).unwrap();
    text.lines()
        .find_map(|l| {
            l.strip_prefix(key)?
                .split_once('=')
                .map(|(_, v)| v.to_string())
        })
        .unwrap_or_else(|| panic!("{key} not in CMakeCache.txt"))
}

/// A directory with fake ESP-IDF tools that fail loudly if anything picks them up.
fn poisoned_bin() -> TempDir {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    for tool in [
        "xtensa-esp32s3-elf-gcc",
        "riscv32-esp-elf-gcc",
        "cmake",
        "ninja",
        "west",
    ] {
        let path = dir.path().join(tool);
        fs::write(&path, "#!/bin/sh\necho poisoned >&2\nexit 99\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    dir
}

#[test]
#[ignore = "downloads Zephyr and the SDK; run explicitly"]
fn builds_hello_world_isolated_from_host_setup() {
    let env = Env::new();
    let sdk_root = fs::canonicalize(&env.home).unwrap().join("sdk");

    let c3 = env.project(&["--board", "esp32c3_devkitm"]);
    env.sync(c3.path());
    env.zem(c3.path())
        .args(["west", "build"])
        .assert()
        .success();
    let build = c3.path().join("build");
    assert!(build.join("zephyr/zephyr.elf").is_file());
    assert!(cmake_cache_value(&build, "CMAKE_C_COMPILER:").starts_with(sdk_root.to_str().unwrap()));
    let config = fs::read_to_string(build.join("zephyr/.config")).unwrap();
    assert!(config.contains("CONFIG_ZEPHYR_HAL_ESPRESSIF_MODULE=y"));
    assert!(
        !config.contains("CONFIG_ZEPHYR_MBEDTLS_MODULE=y"),
        "modules of other projects leaked into the build"
    );

    let s3 = env.project(&["--board", "esp32s3_devkitc"]);
    env.sync(s3.path());
    let poison = poisoned_bin();
    let path = std::env::join_paths(
        std::iter::once(poison.path().to_path_buf())
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    env.zem(s3.path())
        .env("PATH", path)
        .env("IDF_PATH", "/bogus/esp-idf")
        .env("ZEPHYR_BASE", "/bogus/zephyr")
        .env("VIRTUAL_ENV", "/bogus/venv")
        .args(["west", "build"])
        .assert()
        .success();
    let build = s3.path().join("build");
    let compiler = cmake_cache_value(&build, "CMAKE_C_COMPILER:");
    assert!(
        compiler.starts_with(sdk_root.to_str().unwrap()),
        "{compiler}"
    );
    assert!(
        compiler.contains("xtensa-espressif_esp32s3_zephyr-elf"),
        "{compiler}"
    );
    assert!(build.join("zephyr/zephyr.bin").is_file());

    let activate = s3.path().join(".zem/activate.sh");
    assert!(activate.is_file());
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
#[ignore = "downloads Zephyr; run explicitly"]
fn projects_share_one_workspace_and_modules_are_additive() {
    let env = Env::new();

    let a = env.project(&["--board", "esp32c3_devkitm"]);
    env.sync(a.path());
    let state_a = read_json(&a.path().join(".zem/state.json"));
    let workspace = PathBuf::from(state_a["workspace"]["path"].as_str().unwrap());
    let marker = read_json(&workspace.join(".zem-store.json"));
    assert!(
        marker["blobs_fetched"]
            .as_array()
            .unwrap()
            .contains(&"hal_espressif".into())
    );
    assert!(
        workspace
            .join("modules/hal/espressif/zephyr/blobs")
            .exists()
    );
    assert_eq!(state_a["target"]["source"], "board_yml");
    assert!(
        PathBuf::from(state_a["zephyr_base"].as_str().unwrap())
            .join("VERSION")
            .is_file()
    );

    let started = Instant::now();
    let second = env.sync(a.path());
    assert!(second.contains("up to date"), "{second}");
    assert!(
        started.elapsed().as_secs() < 30,
        "no-op sync took {:?}",
        started.elapsed()
    );

    let b = env.project(&["--board", "esp32s3_devkitc", "--feature", "mbedtls"]);
    let out = env.sync(b.path());
    assert!(out.contains("added mbedtls"), "{out}");
    let state_b = read_json(&b.path().join(".zem/state.json"));
    assert_eq!(state_a["workspace"]["key"], state_b["workspace"]["key"]);
    assert!(state_b["modules"]["mbedtls"].is_string());

    let workspaces: Vec<_> = fs::read_dir(env.home.join("workspaces")).unwrap().collect();
    assert_eq!(workspaces.len(), 1);

    let state_a = read_json(&a.path().join(".zem/state.json"));
    let modules_a: Vec<_> = state_a["modules"].as_object().unwrap().keys().collect();
    assert_eq!(modules_a, ["hal_espressif"]);

    let registry = read_json(&env.home.join("projects.json"));
    assert!(registry["projects"].as_object().unwrap().len() >= 2);
}
