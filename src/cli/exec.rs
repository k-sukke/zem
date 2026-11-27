use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Args;
use console::style;

use super::Context;
use crate::error::{Result, ZemError};
use crate::fsutil;
use crate::platform;
use crate::process::display_command;
use crate::project::activate;
use crate::session::Session;

#[derive(Debug, Args)]
pub struct WestArgs {
    /// west arguments, e.g. `build`, `flash` or `espressif monitor -d`
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

#[derive(Debug, Args)]
pub struct RunArgs {
    /// Command and its arguments
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
    pub command: Vec<String>,
}

#[derive(Debug, Args)]
pub struct ShellArgs {
    /// Shell to start (default: $SHELL; PowerShell on Windows)
    #[arg(long)]
    pub shell: Option<PathBuf>,
}

/// Set inside `zem shell` to the project root, to refuse nesting.
pub const ZEM_SHELL_ENV: &str = "ZEM_SHELL";

fn open(ctx: &Context, command: &'static str) -> Result<Session> {
    platform::ensure_supported(command)?;
    Session::open(&ctx.cwd)
}

fn hand_over(cmd: std::process::Command) -> Result<ExitCode> {
    eprintln!("{}", style(format!("$ {}", display_command(&cmd))).dim());
    Err(platform::run_replacing(cmd))
}

pub fn west(ctx: &Context, args: WestArgs) -> Result<ExitCode> {
    let session = open(ctx, "zem west")?;
    let env = session.env();
    let mut cmd = session.west_command(&env);
    cmd.args(&args.args);
    hand_over(cmd)
}

pub fn shell(ctx: &Context, args: ShellArgs) -> Result<ExitCode> {
    platform::ensure_supported("zem shell")?;
    if let Some(active) = std::env::var_os(ZEM_SHELL_ENV).filter(|v| !v.is_empty()) {
        return Err(ZemError::NestedShell(PathBuf::from(active)));
    }
    let session = Session::open(&ctx.cwd)?;
    let program = args
        .shell
        .or_else(platform::user_shell)
        .unwrap_or_else(platform::fallback_shell);
    let exe = std::env::current_exe().map_err(ZemError::io("zem"))?;
    let root = session.project.root();
    let name = &session.config().project.name;
    let prompt = format!("(zem:{name}) ");
    let rc_dir = session.project.state_dir().join("shell");

    let mut env = session.env();
    env.set(ZEM_SHELL_ENV, root);
    let mut args: Vec<OsString> = Vec::new();
    let stem = program
        .file_stem()
        .and_then(|s| s.to_str())
        .map(str::to_ascii_lowercase);
    match stem.as_deref() {
        Some("zsh") => {
            let dir = rc_dir.join("zsh");
            fsutil::atomic_write(
                &dir.join(".zshenv"),
                activate::render_zshenv(&dir).as_bytes(),
            )?;
            fsutil::atomic_write(
                &dir.join(".zshrc"),
                activate::render_zshrc(root, &exe, &prompt).as_bytes(),
            )?;
            let user_dir = std::env::var_os("ZDOTDIR")
                .filter(|v| !v.is_empty())
                .or_else(|| platform::home_dir().map(OsString::from))
                .unwrap_or_default();
            env.set("ZEM_USER_ZDOTDIR", user_dir);
            env.set("ZDOTDIR", &dir);
        }
        Some("bash") => {
            let rc = rc_dir.join("bashrc");
            fsutil::atomic_write(&rc, activate::render_bashrc(root, &exe, &prompt).as_bytes())?;
            args.extend(["--rcfile".into(), rc.into(), "-i".into()]);
        }
        Some("fish") => {
            args.extend(["-C".into(), activate::fish_init_command(root, &exe).into()]);
        }
        Some("pwsh" | "powershell") => {
            let script = activate::powershell_init_command(root, &exe, &prompt);
            args.extend([
                "-NoLogo".into(),
                "-NoExit".into(),
                "-EncodedCommand".into(),
                activate::powershell_encode(&script).into(),
            ]);
        }
        Some("cmd") => {
            let current = std::env::var("PROMPT").unwrap_or_else(|_| "$P$G".into());
            env.set("PROMPT", format!("{prompt}{current}"));
        }
        _ => {}
    }
    let mut cmd = env.command(&program);
    cmd.args(args);
    eprintln!(
        "{} {name} ({}); run `exit` to leave",
        style("entering the zem environment for").dim(),
        session.state.target.board
    );
    hand_over(cmd)
}

pub fn run(ctx: &Context, args: RunArgs) -> Result<ExitCode> {
    let session = open(ctx, "zem run")?;
    let env = session.env();
    let (program, rest) = args
        .command
        .split_first()
        .ok_or(ZemError::MissingArgument("command"))?;
    let program = session.resolve_program(&env, program)?;
    let mut cmd = env.command(program);
    cmd.args(rest.iter().map(OsString::from));
    hand_over(cmd)
}
