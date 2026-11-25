use std::io::IsTerminal;
use std::process::ExitCode;

use clap::Args;

use super::Context;
use crate::env::EnvBuilder;
use crate::env::shell::{self, ShellKind};
use crate::error::Result;
use crate::platform;
use crate::session::Session;

#[derive(Debug, Args)]
pub struct EnvArgs {
    /// Output syntax: bash, zsh, sh, fish or json (default: detected from $SHELL)
    #[arg(long, value_parser = parse_shell)]
    pub shell: Option<ShellKind>,
}

fn parse_shell(s: &str) -> std::result::Result<ShellKind, String> {
    s.parse()
}

pub fn run(ctx: &Context, args: EnvArgs) -> Result<ExitCode> {
    platform::ensure_supported("zem env")?;
    let session = Session::open(&ctx.cwd)?;
    let kind = args
        .shell
        .unwrap_or_else(|| ShellKind::detect(std::env::var_os("SHELL").as_deref()));
    let diff = shell::diff(&EnvBuilder::from_current(), &session.activation_env());
    print!("{}", shell::render(&diff, kind));
    if std::io::stdout().is_terminal() && kind != ShellKind::Json {
        let hint = match kind {
            ShellKind::Fish => "zem env | source",
            _ => "eval \"$(zem env)\"",
        };
        eprintln!("# to apply: {hint}  (or source .zem/activate.*)");
    }
    Ok(ExitCode::SUCCESS)
}
