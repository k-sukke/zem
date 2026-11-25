use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::str::FromStr;

use super::EnvBuilder;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellKind {
    Posix,
    Fish,
    Json,
}

impl ShellKind {
    /// From `$SHELL`; anything that is not fish gets POSIX syntax.
    pub fn detect(shell_var: Option<&OsStr>) -> Self {
        let is_fish = shell_var
            .and_then(|s| std::path::Path::new(s).file_name())
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
            "json" => Ok(ShellKind::Json),
            other => Err(format!(
                "unsupported shell `{other}` (expected bash, zsh, sh, fish or json)"
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
        assert_eq!(ShellKind::detect(None), ShellKind::Posix);
        assert_eq!("bash".parse::<ShellKind>(), Ok(ShellKind::Posix));
        assert!("powershell".parse::<ShellKind>().is_err());
    }
}
