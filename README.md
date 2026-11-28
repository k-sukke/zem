# zem — Zephyr Environment Manager

A CLI that sets up an isolated Zephyr RTOS build and flash environment from a single declarative
`zem.toml` — just say which board and which Zephyr version you want. Built for the ESP32 series.

```sh
zem init --name blinky --board esp32_devkitc
zem sync          # prepares the west workspace, Python, SDK and blobs
zem west build
zem west flash
zem west espressif monitor
```

> [!WARNING]
> **This project is experimental.**
> - Commands, the `zem.toml` / `zem.lock` formats and the `~/.zem` layout may change without notice.
> - It has been tested on real hardware **only with an ESP32 (ESP32-D0WD-V3 / `esp32_devkitc`) on macOS (Apple Silicon)**.
>   ESP32-S3 / C3 / C6 have only been verified to build. Other SoCs and boards are untested.
> - On Linux only the automated tests have been run (no hardware or end-to-end testing yet).
> - Windows (x86_64) support is new: it builds and passes the automated tests, but has not been tested end to end
>   (sync, build and flash) on a real machine yet.
> - The only Zephyr version tested is v4.2.0.
>
> Please report problems via Issues.

## Why

Getting started with Zephyr on an ESP32 involves a lot of setup that has nothing to do with what you actually want to build.

- **Too many steps**: installing west, `west init` / `west update`, a Python venv and `requirements.txt`,
  fetching the Zephyr SDK and toolchains, `west blobs fetch`, and environment variables such as `ZEPHYR_BASE` —
  all in the right order, every time.
- **Conflicts with the host**: if ESP-IDF, PlatformIO, another Zephyr checkout, conda or pyenv is already installed,
  `IDF_PATH` or a different toolchain/Python on `PATH` easily leaks in, leading to "it only breaks on my machine".
- **Heavy**: a workspace per project means downloading hundreds of MB to several GB (Zephyr plus `hal_espressif`) every time.
- **Hard to reproduce**: there is no standard way to pin the same Zephyr, modules, SDK and Python packages
  across teammates and CI.

zem tries to solve these with the following principles:

- **Declarative**: the only file in your project is `zem.toml`. Running `zem sync` repeatedly converges to the same state (idempotent).
- **Isolated**: west / cmake / Python launched by zem do not see variables or `PATH` entries from a host ESP-IDF,
  another Zephyr or other Python environments. zem never modifies your shell configuration or `~/.cmake`.
- **Shared**: Zephyr workspaces, venvs and SDKs live once in `~/.zem` and are shared between projects with the same setup.
  Even when shared, each project's build only uses its own modules.
- **Reproducible**: `zem.lock` records the Zephyr and module commits, the SDK and the Python packages;
  `zem sync --locked` reproduces them exactly.

## Requirements

- macOS, Linux or Windows 10/11 (x86_64)
- git (on Windows: [Git for Windows](https://gitforwindows.org/), with `git config --global core.longpaths true`)
- Rust 1.89+ to build zem

zem downloads the uv it uses (pinned, checked against its SHA-256) and lets uv provide Python, so neither a host uv nor a host Python is needed.
cmake and ninja are installed into the project venv by default (`[python].install_cmake_ninja`). `dtc` is recommended.
Run `zem doctor` to see what is missing.

On Windows, also enable long paths (Zephyr module and build paths exceed 260 characters) from an administrator
PowerShell, then sign out and back in:

```powershell
New-ItemProperty -Path HKLM:\SYSTEM\CurrentControlSet\Control\FileSystem -Name LongPathsEnabled -Value 1 -PropertyType DWORD -Force
```

`zem doctor` warns when either setting is off.

## Installation

### Homebrew

```sh
brew install k-sukke/tap/zem
```

### Prebuilt binary

Prebuilt binaries for macOS (Apple Silicon / Intel), Linux (x86_64 / aarch64, statically linked) and Windows (x86_64)
are attached to each [release](https://github.com/k-sukke/zem/releases). For example, on Apple Silicon:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/k-sukke/zem/releases/latest/download/zem-installer.sh | sh
```

On Windows (PowerShell):

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/k-sukke/zem/releases/latest/download/zem-installer.ps1 | iex"
```

Or download an archive manually (cargo-dist publishes `.tar.xz`):

```sh
curl -fsSL https://github.com/k-sukke/zem/releases/latest/download/zem-aarch64-apple-darwin.tar.xz | tar xJ
install -m 755 zem-aarch64-apple-darwin/zem ~/.local/bin/zem   # or any directory on your PATH
```

Replace the target with `x86_64-apple-darwin`, `x86_64-unknown-linux-musl` or `aarch64-unknown-linux-musl` as needed.
Each archive has a `.sha256` file next to it. If you download through a browser on macOS, clear the quarantine flag with
`xattr -d com.apple.quarantine zem` (the binaries are not notarized).

### From source

```sh
cargo install --git https://github.com/k-sukke/zem
```

## Usage

### 1. Create a project

```sh
mkdir blinky && cd blinky
zem init --name blinky --board esp32_devkitc
```

This writes `zem.toml`. The application (`CMakeLists.txt`, `prj.conf`, ...) lives in the same directory; if there is
no `CMakeLists.txt`, zem scaffolds a hello world app that prints the board name.
Omitted arguments are asked for interactively.

```toml
schema = 1

[project]
name = "blinky"

[zephyr]
version = "v4.2.0"          # a release tag or commit SHA

[target]
board = "esp32_devkitc"     # qualified names (e.g. esp32s3_devkitc/esp32s3/procpu) also work

[modules]
features = []               # mbedtls, lvgl, littlefs, mcuboot, segger
extra = []                  # any project name from Zephyr's west.yml

[blobs]
accept_licenses = false     # accept Espressif binary blob licenses non-interactively

[python]
version = "3.12"

[build]
dir = "build"
# cmake_args = ["-DEXTRA_CONF_FILE=debug.conf"]
```

### 2. Prepare the environment

```sh
zem sync --dry-run   # only show what would be created or reused
zem sync
```

Sync runs seven steps. When nothing has changed, subsequent runs finish in under a second.

1. Python environment (download the pinned uv, create the shared west venv)
2. Zephyr workspace (`west init` / `west update`, shallow-fetching only the required modules)
3. Board check (against Zephyr's `board.yml` / `soc.yml`)
4. Zephyr's Python requirements (the project venv, built once the workspace exists)
5. Zephyr SDK (the version that Zephyr requires, with only the needed toolchains)
6. Binary blobs (`west blobs fetch`)
7. Project state (`.zem/state.json`, `zem.lock`)

If blob licenses need to be accepted, review them and run `zem sync --accept-blob-licenses`,
or set `[blobs].accept_licenses = true`. On failure, zem shows which step failed and where the log is.
Use `-v` / `--verbose` to see west and uv output directly.

### 3. Build, flash, monitor

```sh
zem west build                  # west build (-p for a pristine build)
zem west build -t menuconfig
zem west build -- -DFOO=1       # arguments after -- go to CMake
zem west flash                  # esptool auto-detects the port
zem west flash --esp-device /dev/cu.usbserial-10 --esp-baud-rate 460800
zem west espressif monitor -d   # serial monitor with backtrace decoding; exit with Ctrl+]
```

`zem west <args...>` runs the environment's west in the project root, from any subdirectory, and passes the
arguments through unchanged. zem does not wrap or translate west commands; instead it gives west a project-specific
configuration (`.zem/west-config`, through `WEST_CONFIG_LOCAL`) with:

- `build.board` — the board from `zem.toml`
- `manifest.project-filter` — only this project's modules are active in the shared workspace, so they are the only
  ones Zephyr builds with
- `build.dir-fmt` — `[build].dir`, so `west flash` and `west espressif monitor` find the build too
- `build.cmake-args` — `[build].cmake_args`, plus a Zephyr cache directory inside `~/.zem`

The same configuration applies to plain `west` inside `zem shell` or after `eval "$(zem env)"`. Run `west build`
there from the project root, since west treats the current directory as the application. The file is regenerated
from `zem.toml`, so set things there rather than with `west config`. To pin the port for every flash:

```toml
[env.set]
ESPTOOL_PORT = "/dev/cu.usbserial-10"
```

Use `zem sync`, not `west update`, to update the shared workspace.

### 4. Run other commands in the isolated environment

```sh
zem west boards                 # west in the project root
zem run python -V               # run any command in the isolated environment
zem shell                       # isolated subshell (prompt gets a (zem:<name>) prefix)
eval "$(zem env)"               # apply the environment to the current shell (fish: zem env | source)
. .zem/activate.sh              # same as above (generated by sync)
```

On Windows, `zem shell` starts PowerShell (`pwsh` if installed), and the environment is applied with:

```powershell
. .zem\activate.ps1   # PowerShell (generated by sync)
.zem\activate.bat     # cmd.exe
```

Windows blocks PowerShell scripts by default (the `Restricted` execution policy), so `activate.ps1` fails until local
scripts are allowed. To allow them for the current PowerShell session only:

```powershell
Set-ExecutionPolicy -Scope Process RemoteSigned
```

Use `-Scope CurrentUser` to keep the setting. A policy set by Group Policy takes precedence and cannot be changed this
way. `zem shell` and `activate.bat` do not need any of this.

Serial ports are `COM` names there, e.g. `zem west flash --esp-device COM3`.

## Commands

| Command | Description |
|---|---|
| `zem init` | Create `zem.toml` (and a hello world app) |
| `zem sync [--dry-run] [--locked] [--update] [-v]` | Prepare the environment |
| `zem west <args...>` | Run west in the project root (`build`, `flash`, `espressif monitor`, ...) |
| `zem run <cmd...>` | Run any command in the isolated environment |
| `zem shell` | Isolated subshell |
| `zem env [--shell bash\|zsh\|fish\|powershell\|cmd\|json]` | Print shell commands that apply the environment |
| `zem cache list` / `zem cache gc [--dry-run]` | Show `~/.zem` usage and remove unused entries |
| `zem doctor [--fix]` | Check host tools, configuration and project state (`--fix` prints fix commands; it does not run them) |

If `zem.toml` changed since the last `zem sync`, `west` / `run` / `shell` / `env` stop and ask you to
run `zem sync`. They never sync automatically.

## Reproducibility: `zem.lock`

`zem sync` writes `zem.lock` next to `zem.toml`. It contains the commit SHAs of Zephyr and its modules, the SDK version,
and the list of installed Python packages. **Commit `zem.lock`.**

- Plain `zem sync`: follows the lock if present; if the result differs, warns and updates the lock.
- `zem sync --locked`: fails without touching the lock if it is missing, stale or does not match (for CI).
- `zem sync --update`: ignores the lock and fetches the latest.

If `[zephyr].version` is a branch name (e.g. `main`), zem pins the commit at sync time and
only moves to a newer commit with `--update`.

## `~/.zem` and cache management

Everything zem downloads or generates outside your project goes into `~/.zem` (override with the `ZEM_HOME` environment variable).
The project's `.zem/` holds only lightweight files and is git-ignored.

```
~/.zem/
├── tools/        # the pinned uv and the shared west venv
├── workspaces/   # Zephyr + modules (per Zephyr revision, shared across projects)
├── venvs/        # project Python environments (built once, never modified)
├── sdk/          # Zephyr SDK (only the toolchains you need)
├── python/       # Python interpreters managed by uv
└── cache/        # downloads, uv / git / ccache caches
```

```sh
zem cache list        # what uses how much space, and which projects use it
zem cache gc          # remove anything no project uses
```

Deleting `~/.zem` removes everything zem has created.

## Isolation

By default, commands launched by zem do not receive:

- ESP-IDF variables (`IDF_PATH`, ...), other Zephyr variables (`ZEPHYR_*`), Python environment variables
  (`VIRTUAL_ENV`, `CONDA_*`, `PYTHONPATH`, `PIP_*`, `UV_*`, ...)
- `PATH` entries for `.espressif`, `esp-idf`, `zephyr-sdk-*`, and other venv / conda `bin` directories

For stricter isolation, there is a mode that passes only allowlisted variables:

```toml
[env]
isolation = "strict"       # pass only PATH and a minimal set (HOME, LANG, proxy settings, ...)
passthrough = ["MY_TOKEN"] # variables to pass through explicitly
set = { FOO = "bar" }      # extra variables to set
```

`zem env` and the activate scripts apply to your own shell, so they keep your personal variables even in strict mode.

## Supported boards

Official Zephyr boards for ESP32 / ESP32-S2 / ESP32-S3 / ESP32-C2 / ESP32-C3 / ESP32-C6 (`esp32_devkitc`,
`esp32s3_devkitc`, `esp32c3_devkitm`, ...) are targeted. ESP32-H2 requires a Zephyr newer than 4.2.
The final decision on a board follows the fetched Zephyr's `board.yml`.

As noted above, only the ESP32 (`esp32_devkitc`) has been tested on real hardware.

## Development

```sh
cargo test                          # unit and CLI tests (no network needed)
cargo test --test e2e -- --ignored  # E2E that actually fetches and builds Zephyr (several GB, several minutes)
```

To release, bump `version` in `Cargo.toml`, commit, then push a matching tag (`git tag v0.1.0 && git push origin v0.1.0`).
[cargo-dist](https://axodotdev.github.io/cargo-dist/) builds the binaries, creates the GitHub Release, and publishes the
Homebrew formula to [`k-sukke/homebrew-tap`](https://github.com/k-sukke/homebrew-tap).

The design and per-phase notes are in [PLAN.md](PLAN.md) (Japanese).

## License

[MIT](LICENSE)
