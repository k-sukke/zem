mod cache;
mod doctor;
mod env;
mod exec;
mod init;
mod sync;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use crate::error::{Result, ZemError};

#[derive(Debug, Parser)]
#[command(name = "zem", version, about = "Zephyr Environment Manager")]
pub struct Cli {
    /// Run as if zem was started in DIR
    #[arg(long, global = true, value_name = "DIR")]
    pub project_dir: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Create zem.toml for a new project
    Init(init::InitArgs),
    /// Prepare the Zephyr workspace, Python environment and toolchain
    Sync(sync::SyncArgs),
    /// Run west (build, flash, espressif monitor, ...) in the project root inside the isolated
    /// environment, configured for this project's board, modules and build directory
    #[command(disable_help_flag = true)]
    West(exec::WestArgs),
    /// Print shell commands that activate the environment
    Env(env::EnvArgs),
    /// Run any command inside the isolated environment
    Run(exec::RunArgs),
    /// Start an interactive shell inside the isolated environment
    Shell(exec::ShellArgs),
    /// Inspect and clean up the shared stores in ZEM_HOME
    Cache(cache::CacheArgs),
    /// Check host tools and the project configuration
    Doctor(doctor::DoctorArgs),
}

pub struct Context {
    pub cwd: PathBuf,
}

pub fn run() -> Result<ExitCode> {
    let cli = Cli::parse();
    let cwd = match cli.project_dir {
        Some(dir) => dir,
        None => std::env::current_dir().map_err(ZemError::io("."))?,
    };
    let ctx = Context { cwd };

    match cli.command {
        Command::Init(args) => init::run(&ctx, args),
        Command::Sync(args) => sync::run(&ctx, args),
        Command::West(args) => exec::west(&ctx, args),
        Command::Env(args) => env::run(&ctx, args),
        Command::Run(args) => exec::run(&ctx, args),
        Command::Shell(args) => exec::shell(&ctx, args),
        Command::Cache(args) => cache::run(args),
        Command::Doctor(args) => doctor::run(&ctx, args),
    }
}
