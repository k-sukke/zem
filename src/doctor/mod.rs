mod checks;

use std::fmt::Write as _;
use std::path::Path;

use console::style;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    Ok,
    Info,
    Warn,
    Fail,
}

#[derive(Debug, Clone)]
pub struct Check {
    pub title: String,
    pub status: Status,
    pub detail: String,
    pub hint: Option<String>,
    /// A command that resolves the problem; `zem doctor --fix` prints these but never runs them.
    pub fix: Option<String>,
}

impl Check {
    pub fn new(status: Status, title: impl Into<String>, detail: impl Into<String>) -> Self {
        Check {
            title: title.into(),
            status,
            detail: detail.into(),
            hint: None,
            fix: None,
        }
    }

    pub fn hint(mut self, hint: Option<impl Into<String>>) -> Self {
        self.hint = hint.map(Into::into);
        self
    }

    pub fn fix(mut self, command: impl Into<String>) -> Self {
        self.fix = Some(command.into());
        self
    }

    /// Hint and fix from a platform install hint.
    pub fn install(self, hint: Option<crate::platform::InstallHint>) -> Self {
        match hint {
            Some(h) => self.hint(Some(h.to_string())).fix(h.command),
            None => self,
        }
    }
}

#[derive(Debug, Default)]
pub struct Report {
    pub sections: Vec<(&'static str, Vec<Check>)>,
}

impl Report {
    pub fn has_failures(&self) -> bool {
        self.checks().any(|c| c.status == Status::Fail)
    }

    pub fn checks(&self) -> impl Iterator<Item = &Check> {
        self.sections.iter().flat_map(|(_, checks)| checks)
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        for (name, checks) in &self.sections {
            let _ = writeln!(out, "{}", style(name).bold());
            for check in checks {
                let symbol = match check.status {
                    Status::Ok => style("✓").green(),
                    Status::Info => style("·").cyan(),
                    Status::Warn => style("!").yellow(),
                    Status::Fail => style("✗").red(),
                };
                let _ = writeln!(out, "  {symbol} {:<16} {}", check.title, check.detail);
                if let Some(hint) = &check.hint {
                    let _ = writeln!(out, "      {}", style(format!("→ {hint}")).dim());
                }
            }
            out.push('\n');
        }
        let (fails, warns) = self.checks().fold((0, 0), |(f, w), c| match c.status {
            Status::Fail => (f + 1, w),
            Status::Warn => (f, w + 1),
            _ => (f, w),
        });
        let summary = match (fails, warns) {
            (0, 0) => style("All checks passed.".to_string()).green(),
            (0, w) => style(format!("{w} warning(s), no blocking problems.")).yellow(),
            (f, w) => style(format!("{f} problem(s) and {w} warning(s) found.")).red(),
        };
        let _ = writeln!(out, "{summary}");
        out
    }

    /// Commands for the warnings and failures, deduplicated, in report order.
    pub fn fixes(&self) -> Vec<&str> {
        let mut fixes: Vec<&str> = Vec::new();
        for check in self.checks().filter(|c| c.status >= Status::Warn) {
            if let Some(fix) = check.fix.as_deref()
                && !fixes.contains(&fix)
            {
                fixes.push(fix);
            }
        }
        fixes
    }

    pub fn render_fixes(&self) -> String {
        let fixes = self.fixes();
        if fixes.is_empty() {
            return format!("\n{}\n", style("Nothing to fix.").green());
        }
        let mut out = format!(
            "\n{}\n",
            style("Suggested commands (zem does not run them; review before running):").bold()
        );
        for fix in fixes {
            let _ = writeln!(out, "  {fix}");
        }
        out
    }
}

pub fn run(cwd: &Path) -> Report {
    checks::collect(cwd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixes_cover_only_problems_and_are_deduplicated() {
        let report = Report {
            sections: vec![
                (
                    "a",
                    vec![
                        Check::new(Status::Fail, "git", "missing").fix("brew install git"),
                        Check::new(Status::Info, "uv", "missing").fix("brew install uv"),
                    ],
                ),
                (
                    "b",
                    vec![
                        Check::new(Status::Warn, "dtc", "missing").fix("brew install dtc"),
                        Check::new(Status::Warn, "git2", "x").fix("brew install git"),
                    ],
                ),
            ],
        };
        assert_eq!(report.fixes(), ["brew install git", "brew install dtc"]);
        assert!(report.render_fixes().contains("  brew install dtc\n"));
        assert!(Report::default().render_fixes().contains("Nothing to fix"));
    }
}
