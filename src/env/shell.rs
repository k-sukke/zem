use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::str::FromStr;

use super::EnvBuilder;
use crate::platform::Os;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellKind {
    Posix,
    Fish,
    PowerShell,
    Cmd,
    Json,
}

impl ShellKind {
    /// From the user's shell (`$SHELL` on Unix); anything that is not fish gets POSIX syntax.
    /// Without one, POSIX on Unix (matching `zem shell`'s `/bin/sh`) and PowerShell, the
    /// default shell of Windows Terminal, on Windows.
    pub fn detect(user_shell: Option<&OsStr>, os: Os) -> Self {
        let Some(shell) = user_shell.filter(|s| !s.is_empty()) else {
            return if os == Os::Windows {
                ShellKind::PowerShell
            } else {
                ShellKind::Posix
            };
        };
        let is_fish = std::path::Path::new(shell)
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("fish"));
        if is_fish {
            ShellKind::Fish
        } else {
            ShellKind::Posix
        }
    }
}

impl FromStr for ShellKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "posix" | "sh" | "bash" | "zsh" => Ok(ShellKind::Posix),
            "fish" => Ok(ShellKind::Fish),
            "powershell" | "pwsh" => Ok(ShellKind::PowerShell),
            "cmd" => Ok(ShellKind::Cmd),
            "json" => Ok(ShellKind::Json),
            other => Err(format!(
                "unsupported shell `{other}` (expected bash, zsh, sh, fish, powershell, cmd or json)"
            )),
        }
    }
}

/// What has to change in the current shell to arrive at `target`.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct EnvDiff {
    pub set: BTreeMap<String, String>,
    pub unset: Vec<String>,
}

pub fn diff(current: &EnvBuilder, target: &EnvBuilder) -> EnvDiff {
    let mut out = EnvDiff::default();
    for (key, value) in target.vars() {
        if current.get(key) != Some(value) {
            out.set.insert(
                key.to_string_lossy().into_owned(),
                value.to_string_lossy().into_owned(),
            );
        }
    }
    for (key, _) in current.vars() {
        if target.get(key).is_none() {
            out.unset.push(key.to_string_lossy().into_owned());
        }
    }
    out.unset.sort();
    out
}

fn single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

fn fish_quote(value: &str) -> String {
    format!("'{}'", value.replace('\\', r"\\").replace('\'', r"\'"))
}

/// Characters PowerShell accepts as single quotes: `'` and the typographic `‘ ’ ‚ ‛`.
const POWERSHELL_SINGLE_QUOTES: &[char] = &['\'', '\u{2018}', '\u{2019}', '\u{201A}', '\u{201B}'];

/// A PowerShell single-quoted string. Every single-quote character ends the string unless it
/// is doubled, so each one is doubled.
pub fn powershell_quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for c in value.chars() {
        if POWERSHELL_SINGLE_QUOTES.contains(&c) {
            out.push(c);
        }
        out.push(c);
    }
    out.push('\'');
    out
}

/// For text inside `"..."` in a batch file (`.zem\activate.bat`), where only `%` needs
/// doubling. Windows paths cannot contain `"`.
pub fn batch_escape(value: &str) -> String {
    value.replace('%', "%%")
}

/// For unquoted text in a batch file: `%` is doubled and cmd's metacharacters, `"` included,
/// get a caret. Unlike `set "K=V"`, a `"` in the value cannot end the quoting and expose
/// `&` or `|` to the parser.
fn batch_caret_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '%' => out.push_str("%%"),
            '^' | '&' | '|' | '<' | '>' | '"' | '(' | ')' => {
                out.push('^');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

pub fn render(diff: &EnvDiff, shell: ShellKind) -> String {
    let mut out = String::new();
    match shell {
        ShellKind::Posix => {
            for key in &diff.unset {
                out.push_str(&format!("unset {key}\n"));
            }
            for (key, value) in &diff.set {
                out.push_str(&format!("export {key}={}\n", single_quote(value)));
            }
        }
        ShellKind::Fish => {
            for key in &diff.unset {
                out.push_str(&format!("set -e {key}\n"));
            }
            for (key, value) in &diff.set {
                if key == "PATH" {
                    let parts: Vec<String> = std::env::split_paths(value)
                        .map(|p| fish_quote(&p.to_string_lossy()))
                        .collect();
                    out.push_str(&format!("set -gx PATH {}\n", parts.join(" ")));
                } else {
                    out.push_str(&format!("set -gx {key} {}\n", fish_quote(value)));
                }
            }
        }
        ShellKind::PowerShell => {
            for key in &diff.unset {
                out.push_str(&format!(
                    "Remove-Item -LiteralPath {} -ErrorAction SilentlyContinue\n",
                    powershell_quote(&format!("Env:{key}"))
                ));
            }
            for (key, value) in &diff.set {
                out.push_str(&format!(
                    "Set-Item -LiteralPath {} -Value {}\n",
                    powershell_quote(&format!("Env:{key}")),
                    powershell_quote(value)
                ));
            }
        }
        // cmd output is a batch file that `.zem\activate.bat` `call`s.
        ShellKind::Cmd => {
            for key in &diff.unset {
                out.push_str(&format!("@set {}=\n", batch_caret_escape(key)));
            }
            for (key, value) in &diff.set {
                if value.contains(['\r', '\n']) {
                    // A line break would end the command and run the rest as another one.
                    out.push_str(&format!(
                        "@echo zem: {} is not set: cmd cannot hold a line break in a variable 1>&2\n",
                        batch_caret_escape(key)
                    ));
                    continue;
                }
                out.push_str(&format!(
                    "@set {}={}\n",
                    batch_caret_escape(key),
                    batch_caret_escape(value)
                ));
            }
        }
        ShellKind::Json => {
            out = serde_json::to_string_pretty(diff).unwrap_or_default();
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> EnvDiff {
        let current = EnvBuilder::from_vars([
            ("IDF_PATH", "/opt/idf"),
            ("HOME", "/home/me"),
            ("PATH", "/usr/bin"),
        ]);
        let target = EnvBuilder::from_vars([
            ("HOME", "/home/me"),
            ("PATH", "/zh/venv/bin:/usr/bin"),
            ("ZEPHYR_BASE", "/zh/it's here"),
        ]);
        diff(&current, &target)
    }

    #[test]
    fn diff_only_contains_changes() {
        let d = sample();
        assert_eq!(d.unset, ["IDF_PATH"]);
        assert_eq!(d.set.keys().collect::<Vec<_>>(), ["PATH", "ZEPHYR_BASE"]);
    }

    #[test]
    fn renders_posix() {
        insta::assert_snapshot!(render(&sample(), ShellKind::Posix), @r"
        unset IDF_PATH
        export PATH='/zh/venv/bin:/usr/bin'
        export ZEPHYR_BASE='/zh/it'\''s here'
        ");
    }

    #[test]
    fn renders_fish_with_path_list() {
        let out = render(&sample(), ShellKind::Fish);
        assert!(out.contains("set -e IDF_PATH\n"));
        assert!(out.contains(r"set -gx ZEPHYR_BASE '/zh/it\'s here'"));
        let expected_path = std::env::split_paths("/zh/venv/bin:/usr/bin")
            .map(|p| fish_quote(&p.to_string_lossy()))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            out.contains(&format!("set -gx PATH {expected_path}\n")),
            "{out}"
        );
    }

    #[test]
    fn detects_shell() {
        assert_eq!(
            ShellKind::detect(Some(OsStr::new("/opt/homebrew/bin/fish")), Os::MacOs),
            ShellKind::Fish
        );
        assert_eq!(
            ShellKind::detect(Some(OsStr::new("/bin/zsh")), Os::Linux),
            ShellKind::Posix
        );
        assert_eq!(ShellKind::detect(None, Os::Linux), ShellKind::Posix);
        assert_eq!(
            ShellKind::detect(Some(OsStr::new("")), Os::MacOs),
            ShellKind::Posix
        );
        assert_eq!(ShellKind::detect(None, Os::Windows), ShellKind::PowerShell);
        assert_eq!("bash".parse::<ShellKind>(), Ok(ShellKind::Posix));
        assert_eq!("pwsh".parse::<ShellKind>(), Ok(ShellKind::PowerShell));
        assert_eq!("cmd".parse::<ShellKind>(), Ok(ShellKind::Cmd));
        assert!("tcsh".parse::<ShellKind>().is_err());
    }

    fn windows_sample() -> EnvDiff {
        let current = EnvBuilder::from_vars([("IDF_PATH", r"C:\esp")]);
        let target = EnvBuilder::from_vars([
            ("PROGRAMFILES(X86)", r"C:\Program Files (x86)"),
            ("ZEPHYR_BASE", r"C:\Users\it's me\100%\zephyr"),
        ]);
        diff(&current, &target)
    }

    #[test]
    fn renders_powershell() {
        insta::assert_snapshot!(render(&windows_sample(), ShellKind::PowerShell), @r"
        Remove-Item -LiteralPath 'Env:IDF_PATH' -ErrorAction SilentlyContinue
        Set-Item -LiteralPath 'Env:PROGRAMFILES(X86)' -Value 'C:\Program Files (x86)'
        Set-Item -LiteralPath 'Env:ZEPHYR_BASE' -Value 'C:\Users\it''s me\100%\zephyr'
        ");
    }

    #[test]
    fn powershell_quote_doubles_typographic_quotes() {
        assert_eq!(powershell_quote("it's"), "'it''s'");
        assert_eq!(
            powershell_quote("C:\\work\\O\u{2019}Brien \u{2018}x\u{201A}\u{201B}"),
            "'C:\\work\\O\u{2019}\u{2019}Brien \u{2018}\u{2018}x\u{201A}\u{201A}\u{201B}\u{201B}'"
        );
    }

    #[test]
    fn renders_cmd_batch() {
        insta::assert_snapshot!(render(&windows_sample(), ShellKind::Cmd), @r#"
        @set IDF_PATH=
        @set PROGRAMFILES^(X86^)=C:\Program Files ^(x86^)
        @set ZEPHYR_BASE=C:\Users\it's me\100%%\zephyr
        "#);
    }

    #[test]
    fn cmd_values_with_quotes_keep_metacharacters_escaped() {
        let current = EnvBuilder::from_vars([("A", "1")]);
        let target = EnvBuilder::from_vars([
            ("A", "1"),
            ("CFLAGS", r#"-DNAME="a&b" ^x|y<z>"#),
            ("BAD", "one\r\nexit"),
        ]);
        let out = render(&diff(&current, &target), ShellKind::Cmd);
        assert!(
            out.contains(r#"@set CFLAGS=-DNAME=^"a^&b^" ^^x^|y^<z^>"#),
            "{out}"
        );
        assert!(!out.contains("exit"), "{out}");
        assert!(out.contains("@echo zem: BAD is not set"), "{out}");
    }
}
