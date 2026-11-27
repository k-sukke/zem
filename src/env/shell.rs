use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::str::FromStr;

use super::EnvBuilder;

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
    /// Without one (Windows), PowerShell, the default shell of Windows Terminal.
    pub fn detect(user_shell: Option<&OsStr>) -> Self {
        let Some(shell) = user_shell else {
            return ShellKind::PowerShell;
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

/// A PowerShell single-quoted string: only `'` is special and is doubled.
pub fn powershell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// cmd output is a batch file (`call`ed by `.zem\activate.bat`), where `%` must be doubled.
/// Inside `set "K=V"` no other character needs escaping.
pub fn batch_escape(value: &str) -> String {
    value.replace('%', "%%")
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
        ShellKind::Cmd => {
            for key in &diff.unset {
                out.push_str(&format!("@set \"{}=\"\n", batch_escape(key)));
            }
            for (key, value) in &diff.set {
                out.push_str(&format!(
                    "@set \"{}={}\"\n",
                    batch_escape(key),
                    batch_escape(value)
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
            ShellKind::detect(Some(OsStr::new("/opt/homebrew/bin/fish"))),
            ShellKind::Fish
        );
        assert_eq!(
            ShellKind::detect(Some(OsStr::new("/bin/zsh"))),
            ShellKind::Posix
        );
        assert_eq!(ShellKind::detect(None), ShellKind::PowerShell);
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
    fn renders_cmd_batch() {
        insta::assert_snapshot!(render(&windows_sample(), ShellKind::Cmd), @r#"
        @set "IDF_PATH="
        @set "PROGRAMFILES(X86)=C:\Program Files (x86)"
        @set "ZEPHYR_BASE=C:\Users\it's me\100%%\zephyr"
        "#);
    }
}
