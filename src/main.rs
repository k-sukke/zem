use std::io::IsTerminal;
use std::process::ExitCode;

fn main() -> ExitCode {
    let wrap = std::io::stderr().is_terminal();
    let _ = miette::set_hook(Box::new(move |_| {
        Box::new(miette::MietteHandlerOpts::new().wrap_lines(wrap).build())
    }));

    match zem::cli::run() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("{:?}", miette::Report::new(err));
            ExitCode::FAILURE
        }
    }
}
