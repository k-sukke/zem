use std::process::ExitCode;

use clap::Args;

use super::Context;
use crate::doctor;
use crate::error::Result;

#[derive(Debug, Args)]
pub struct DoctorArgs {
    /// Also print commands that fix the reported problems (they are never run automatically)
    #[arg(long)]
    pub fix: bool,
}

pub fn run(ctx: &Context, args: DoctorArgs) -> Result<ExitCode> {
    let report = doctor::run(&ctx.cwd);
    print!("{}", report.render());
    if args.fix {
        print!("{}", report.render_fixes());
    }
    Ok(if report.has_failures() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}
