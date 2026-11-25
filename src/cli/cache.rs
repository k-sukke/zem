use std::process::ExitCode;

use clap::{Args, Subcommand};
use console::style;

use crate::error::Result;
use crate::home::ZemHome;
use crate::store::gc::{self, Store};

#[derive(Debug, Args)]
pub struct CacheArgs {
    #[command(subcommand)]
    pub command: CacheCommand,
}

#[derive(Debug, Subcommand)]
pub enum CacheCommand {
    /// Show what ZEM_HOME contains, how large it is and which projects use it
    List,
    /// Delete workspaces, venvs and toolchains no registered project uses anymore
    Gc {
        /// Only show what would be deleted
        #[arg(long)]
        dry_run: bool,
    },
}

pub fn run(args: CacheArgs) -> Result<ExitCode> {
    let home = ZemHome::resolve()?;
    match args.command {
        CacheCommand::List => list(&home),
        CacheCommand::Gc { dry_run } => collect(&home, dry_run),
    }
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn row(kind: &str, name: &str, size: u64, users: &str) {
    println!("  {kind:<10} {name:<44} {:>10}  {users}", human_size(size));
}

const SHOWN_USERS: usize = 2;

fn users(store: &Store) -> String {
    if !store.in_use() {
        return style("unused").yellow().to_string();
    }
    let mut shown: Vec<String> = store
        .users
        .iter()
        .take(SHOWN_USERS)
        .map(|p| p.display().to_string())
        .collect();
    if store.users.len() > SHOWN_USERS {
        shown.push(format!("+{} more", store.users.len() - SHOWN_USERS));
    }
    shown.join(", ")
}

fn list(home: &ZemHome) -> Result<ExitCode> {
    let usage = gc::usage(home)?;
    println!(
        "{} {} ({})",
        style("ZEM_HOME").bold(),
        home.root().display(),
        human_size(usage.total())
    );
    println!();
    println!(
        "  {:<10} {:<44} {:>10}  {}",
        style("KIND").dim(),
        style("NAME").dim(),
        style("SIZE").dim(),
        style("USED BY").dim()
    );
    for store in &usage.stores {
        row(store.kind.label(), &store.name, store.size, &users(store));
    }
    for (name, size) in &usage.shared {
        row(
            "shared",
            name,
            *size,
            &style("kept by gc").dim().to_string(),
        );
    }

    let unused: u64 = usage
        .stores
        .iter()
        .filter(|s| !s.in_use())
        .map(|s| s.size)
        .sum();
    println!();
    println!(
        "{} project(s) registered{}",
        usage.projects.len(),
        if usage.stale.is_empty() {
            String::new()
        } else {
            format!(", {} no longer exist", usage.stale.len())
        }
    );
    if unused > 0 {
        println!("{} reclaimable: run `zem cache gc`", human_size(unused));
    }
    Ok(ExitCode::SUCCESS)
}

fn collect(home: &ZemHome, dry_run: bool) -> Result<ExitCode> {
    let report = gc::collect(home, dry_run)?;
    let verb = if dry_run { "would remove" } else { "removed" };
    for key in &report.pruned {
        println!(
            "  {} project {key} (no longer exists)",
            style("forget").dim()
        );
    }
    for store in &report.removed {
        println!(
            "  {} {} {} ({})",
            style(verb).red(),
            store.kind.label(),
            store.name,
            human_size(store.size)
        );
    }
    for store in &report.busy {
        println!(
            "  {} {} {} (in use by another zem process)",
            style("skipped").yellow(),
            store.kind.label(),
            store.name
        );
    }
    if report.removed.is_empty() && report.pruned.is_empty() {
        println!("nothing to clean up");
    } else {
        println!(
            "\n{} {}",
            if dry_run { "would free" } else { "freed" },
            human_size(report.freed())
        );
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_human_readable() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1536), "1.5 KiB");
        assert_eq!(human_size(3 * 1024 * 1024 * 1024), "3.0 GiB");
    }
}
