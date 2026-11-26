use std::fs;
use std::io::IsTerminal;
use std::path::Path;
use std::process::ExitCode;

use clap::Args;
use console::style;
use dialoguer::{Input, Select, theme::ColorfulTheme};

use super::Context;
use crate::board::{self, KNOWN_BOARDS};
use crate::config::{self, CONFIG_FILE_NAME, Feature, InitParams};
use crate::error::{Result, ZemError};
use crate::project;

pub const DEFAULT_ZEPHYR_VERSION: &str = "v4.2.0";
const GITIGNORE_ENTRIES: &[&str] = &["build/"];

#[derive(Debug, Args)]
pub struct InitArgs {
    /// Project name (defaults to the directory name)
    #[arg(long)]
    pub name: Option<String>,

    /// Zephyr board, e.g. esp32s3_devkitc/esp32s3/procpu or esp32c3_devkitm
    #[arg(long)]
    pub board: Option<String>,

    /// Zephyr release tag (v4.0 or later) or commit SHA
    #[arg(long, value_name = "REV")]
    pub zephyr_version: Option<String>,

    /// Optional module feature; may be repeated (mbedtls, lvgl, littlefs, mcuboot, segger)
    #[arg(long = "feature", value_name = "FEATURE")]
    pub features: Vec<Feature>,

    /// Never prompt; use defaults for anything not given
    #[arg(short = 'y', long)]
    pub yes: bool,

    /// Overwrite an existing zem.toml
    #[arg(long)]
    pub force: bool,
}

pub fn run(ctx: &Context, args: InitArgs) -> Result<ExitCode> {
    let root = &ctx.cwd;
    fs::create_dir_all(root).map_err(ZemError::io(root))?;
    project::ensure_not_home(root)?;

    let path = root.join(CONFIG_FILE_NAME);
    if path.exists() && !args.force {
        return Err(ZemError::AlreadyExists(path));
    }

    let interactive =
        !args.yes && std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let theme = ColorfulTheme::default();

    let default_name = default_project_name(root);
    let name = match args.name {
        Some(name) => name,
        None if interactive => Input::with_theme(&theme)
            .with_prompt("Project name")
            .default(default_name)
            .interact_text()
            .map_err(ZemError::Prompt)?,
        None => default_name,
    };

    let board_input = match args.board {
        Some(board) => board,
        None if interactive => prompt_board(&theme)?,
        None => return Err(ZemError::MissingArgument("--board")),
    };

    let zephyr_version = match args.zephyr_version {
        Some(v) => v,
        None if interactive => Input::with_theme(&theme)
            .with_prompt("Zephyr version")
            .default(DEFAULT_ZEPHYR_VERSION.to_string())
            .interact_text()
            .map_err(ZemError::Prompt)?,
        None => DEFAULT_ZEPHYR_VERSION.to_string(),
    };

    let target = board::resolve_static(&board_input)?;
    let text = config::render(&InitParams {
        name: &name,
        board: &target.board,
        zephyr_version: &zephyr_version,
        features: &args.features,
    });
    let parsed = config::parse(&path, &text)?;
    let validated = config::validate(&parsed)?;

    fs::write(&path, &text).map_err(ZemError::io(&path))?;
    update_gitignore(root, GITIGNORE_ENTRIES)?;
    let scaffolded = scaffold_app(root, &name)?;

    println!("{} {}", style("Created").green().bold(), path.display());
    if scaffolded {
        println!(
            "{} hello world app in {}",
            style("Created").green().bold(),
            root.display()
        );
    }
    println!(
        "  board  {}\n  target {} / {} → {}",
        target.board,
        target.soc.name(),
        target.arch,
        target.toolchains.join(", ")
    );
    for warning in validated
        .warnings
        .iter()
        .filter(|w| !w.contains("was expanded"))
    {
        eprintln!("{} {warning}", style("warning:").yellow().bold());
    }
    println!(
        "\nNext: run {} to set up the environment.",
        style("zem sync").cyan()
    );
    Ok(ExitCode::SUCCESS)
}

fn prompt_board(theme: &ColorfulTheme) -> Result<String> {
    let mut items: Vec<String> = KNOWN_BOARDS
        .iter()
        .map(|b| format!("{:<34} {}", b.full_name(), b.description))
        .collect();
    items.push("Other (type a board name)".to_string());

    let choice = Select::with_theme(theme)
        .with_prompt("Target board")
        .items(&items)
        .default(0)
        .interact()
        .map_err(ZemError::Prompt)?;

    match KNOWN_BOARDS.get(choice) {
        Some(board) => Ok(board.full_name()),
        None => Input::with_theme(theme)
            .with_prompt("Board name")
            .interact_text()
            .map_err(ZemError::Prompt),
    }
}

fn default_project_name(root: &Path) -> String {
    let name: String = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if name.is_empty() || name.chars().all(|c| c == '_') {
        "app".to_string()
    } else {
        name
    }
}

/// Write the hello-world app unless the directory already has a CMake project.
fn scaffold_app(app_dir: &Path, name: &str) -> Result<bool> {
    if app_dir.join("CMakeLists.txt").exists() {
        return Ok(false);
    }
    for (rel, contents) in config::app_template::files(name) {
        let file = app_dir.join(rel);
        if file.exists() {
            continue;
        }
        if let Some(parent) = file.parent() {
            fs::create_dir_all(parent).map_err(ZemError::io(parent))?;
        }
        fs::write(&file, contents).map_err(ZemError::io(&file))?;
    }
    Ok(true)
}

fn update_gitignore(root: &Path, entries: &[&str]) -> Result<()> {
    let path = root.join(".gitignore");
    let existing = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(ZemError::io(&path)(e)),
    };
    let updated = with_gitignore_entries(&existing, entries);
    if updated != existing {
        fs::write(&path, updated).map_err(ZemError::io(&path))?;
    }
    Ok(())
}

fn with_gitignore_entries(existing: &str, entries: &[&str]) -> String {
    let normalize = |line: &str| line.trim().trim_matches('/').to_string();
    let present: Vec<String> = existing.lines().map(normalize).collect();
    let missing: Vec<&str> = entries
        .iter()
        .copied()
        .filter(|e| !present.contains(&normalize(e)))
        .collect();
    if missing.is_empty() {
        return existing.to_string();
    }
    let mut out = existing.to_string();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    for entry in missing {
        out.push_str(entry);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gitignore_appends_missing_entries_once() {
        assert_eq!(with_gitignore_entries("", &["build/"]), "build/\n");
        assert_eq!(
            with_gitignore_entries("target", &["build/"]),
            "target\nbuild/\n"
        );
        assert_eq!(with_gitignore_entries("/build\n", &["build/"]), "/build\n");
        assert_eq!(with_gitignore_entries("build\n", &["build/"]), "build\n");
    }

    #[test]
    fn project_name_from_directory() {
        assert_eq!(default_project_name(Path::new("/x/my app")), "my_app");
        assert_eq!(default_project_name(Path::new("/")), "app");
    }
}
