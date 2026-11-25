use std::io;
use std::path::PathBuf;

use miette::{Diagnostic, NamedSource, SourceSpan};
use thiserror::Error;

use crate::board::BoardError;

pub type Result<T, E = ZemError> = std::result::Result<T, E>;

#[derive(Debug, Error, Diagnostic)]
pub enum ZemError {
    #[error("zem.toml not found in {} or any parent directory", start.display())]
    #[diagnostic(code(zem::config::not_found), help("run `zem init` to create one"))]
    ConfigNotFound { start: PathBuf },

    #[error(transparent)]
    #[diagnostic(transparent)]
    ConfigParse(Box<ConfigParseError>),

    #[error("invalid zem.toml:{}", bullet_list(.problems))]
    #[diagnostic(code(zem::config::invalid))]
    ConfigInvalid { problems: Vec<String> },

    #[error(transparent)]
    #[diagnostic(transparent)]
    Board(#[from] BoardError),

    #[error("{feature} is not supported on {os} yet")]
    #[diagnostic(
        code(zem::unsupported_platform),
        help(
            "this version of zem runs environments on macOS and Linux only; Windows support is planned"
        )
    )]
    Unsupported {
        feature: &'static str,
        os: &'static str,
    },

    #[error("{} already exists", .0.display())]
    #[diagnostic(code(zem::init::exists), help("pass --force to overwrite it"))]
    AlreadyExists(PathBuf),

    #[error("missing required option {0}")]
    #[diagnostic(
        code(zem::cli::missing_argument),
        help("run in an interactive terminal, or pass the option explicitly")
    )]
    MissingArgument(&'static str),

    #[error("the home directory {} cannot be used as a project root", .0.display())]
    #[diagnostic(
        code(zem::project::is_home),
        help(
            "the project's .zem/ would collide with the global ~/.zem; create a subdirectory for the project"
        )
    )]
    ProjectIsHome(PathBuf),

    #[error("ZEM_HOME ({}) collides with the project state directory", zem_home.display())]
    #[diagnostic(
        code(zem::project::zem_home_collision),
        help("point ZEM_HOME somewhere else, e.g. ZEM_HOME=./.zem-home")
    )]
    ZemHomeCollision { zem_home: PathBuf },

    #[error("could not determine the home directory")]
    #[diagnostic(code(zem::no_home), help("set ZEM_HOME explicitly"))]
    NoHomeDir,

    #[error("I/O error on {}", path.display())]
    #[diagnostic(code(zem::io))]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("state file {} is corrupt: {message}", path.display())]
    #[diagnostic(
        code(zem::state::corrupt),
        help("delete the file and run `zem sync` again")
    )]
    CorruptState { path: PathBuf, message: String },

    #[error("failed to start `{command}`")]
    #[diagnostic(code(zem::process::spawn))]
    Spawn {
        command: String,
        #[source]
        source: io::Error,
    },

    #[error("`{command}` failed ({status})\n{tail}")]
    #[diagnostic(code(zem::process::failed))]
    CommandFailed {
        command: String,
        status: String,
        tail: String,
        #[help]
        advice: Option<String>,
    },

    #[error("{tool} was not found")]
    #[diagnostic(code(zem::tool::missing))]
    ToolNotFound {
        tool: &'static str,
        #[help]
        advice: Option<String>,
    },

    #[error("module `{module}` does not exist in Zephyr {revision}'s west.yml")]
    #[diagnostic(
        code(zem::modules::unknown),
        help(
            "check [modules].features / [modules].extra against zephyr/west.yml for this release"
        )
    )]
    ModuleNotInManifest { module: String, revision: String },

    #[error("the environment is not ready: {reason}")]
    #[diagnostic(code(zem::not_synced), help("run `zem sync`"))]
    NotSynced { reason: String },

    #[error("download of {url} failed: {message}")]
    #[diagnostic(
        code(zem::download),
        help("check the network connection; proxies are taken from HTTPS_PROXY / ALL_PROXY")
    )]
    Download { url: String, message: String },

    #[error("checksum mismatch for {}", file.display())]
    #[diagnostic(
        code(zem::download::checksum),
        help(
            "expected {expected}, got {actual}; the corrupt file was removed, run the command again"
        )
    )]
    ChecksumMismatch {
        file: PathBuf,
        expected: String,
        actual: String,
    },

    #[error("Zephyr SDK {version} has no asset `{asset}`")]
    #[diagnostic(
        code(zem::sdk::asset),
        help(
            "this host or toolchain may not be supported by that SDK release; set [sdk].version explicitly"
        )
    )]
    SdkAsset { version: String, asset: String },

    #[error("`{0}` was not found in the zem environment's PATH")]
    #[diagnostic(code(zem::run::not_found))]
    CommandNotFound(String),

    #[error("already inside a zem shell for {}", .0.display())]
    #[diagnostic(code(zem::shell::nested), help("run `exit` to leave it first"))]
    NestedShell(PathBuf),

    #[error("zem.lock not found")]
    #[diagnostic(
        code(zem::lock::missing),
        help("run `zem sync` without --locked to create it, then commit zem.lock")
    )]
    LockMissing,

    #[error("zem.lock is out of date: {reason}")]
    #[diagnostic(
        code(zem::lock::stale),
        help("run `zem sync` without --locked to update it, then commit zem.lock")
    )]
    LockStale { reason: String },

    #[error("{what} does not match zem.lock (locked {expected}, got {actual})")]
    #[diagnostic(
        code(zem::lock::mismatch),
        help("run `zem sync` without --locked to accept the new state and update zem.lock")
    )]
    LockMismatch {
        what: String,
        expected: String,
        actual: String,
    },

    #[error("{} is invalid: {message}", path.display())]
    #[diagnostic(
        code(zem::lock::invalid),
        help("restore it from version control, or delete it and run `zem sync`")
    )]
    LockInvalid { path: PathBuf, message: String },

    #[error("could not resolve Zephyr revision `{revision}` at {url}")]
    #[diagnostic(
        code(zem::lock::resolve),
        help("check that the branch exists and the network is reachable")
    )]
    ResolveRevision { url: String, revision: String },

    #[error("interactive prompt failed")]
    #[diagnostic(code(zem::prompt))]
    Prompt(#[source] dialoguer::Error),
}

#[derive(Debug, Error, Diagnostic)]
#[error("failed to parse {}: {message}", path.display())]
#[diagnostic(code(zem::config::parse))]
pub struct ConfigParseError {
    pub path: PathBuf,
    pub message: String,
    #[source_code]
    pub src: NamedSource<String>,
    #[label("{message}")]
    pub span: Option<SourceSpan>,
}

impl ZemError {
    pub fn io(path: impl Into<PathBuf>) -> impl FnOnce(io::Error) -> ZemError {
        let path = path.into();
        move |source| ZemError::Io { path, source }
    }
}

fn bullet_list(items: &[String]) -> String {
    items.iter().map(|item| format!("\n  - {item}")).collect()
}
