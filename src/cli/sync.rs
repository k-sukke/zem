use std::process::ExitCode;

use clap::Args;
use console::style;

use super::Context;
use crate::config;
use crate::error::Result;
use crate::home::ZemHome;
use crate::sync::{self, SyncOptions};

#[derive(Debug, Args)]
pub struct SyncArgs {
    /// Show what would be created or reused without changing anything
    #[arg(long)]
    pub dry_run: bool,

    /// Stream the output of west, uv and pip instead of only logging it
    #[arg(short, long)]
    pub verbose: bool,

    /// Accept the licenses of binary blobs (same as [blobs].accept_licenses = true)
    #[arg(long)]
    pub accept_blob_licenses: bool,

    /// Re-run `west update` and blob fetching, and re-resolve what zem.lock pins
    #[arg(long, conflicts_with = "locked")]
    pub update: bool,

    /// Require zem.lock to exist and match exactly; never modify it (for CI)
    #[arg(long)]
    pub locked: bool,
}

pub fn run(ctx: &Context, args: SyncArgs) -> Result<ExitCode> {
    let loaded = config::load(&ctx.cwd)?;
    let home = ZemHome::resolve()?;
    let plan = sync::plan(&loaded, &home)?;

    for warning in &plan.warnings {
        eprintln!("{} {warning}", style("warning:").yellow().bold());
    }
    println!("{}", style("Sync plan").bold());
    print!("{}", plan.render());

    if args.dry_run {
        return Ok(ExitCode::SUCCESS);
    }
    println!();
    let opts = SyncOptions {
        verbose: args.verbose,
        accept_blob_licenses: args.accept_blob_licenses,
        update: args.update,
        locked: args.locked,
    };
    let state = sync::run(&loaded, &home, &plan, &opts)?;
    println!(
        "\n{} {} is ready for {}",
        style("✓").green().bold(),
        loaded.config.project.name,
        state.target.board
    );
    Ok(ExitCode::SUCCESS)
}
