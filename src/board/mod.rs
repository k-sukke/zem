pub mod board_yml;
mod espressif;
mod qualifier;
mod resolve;

use std::fmt;

use miette::Diagnostic;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use espressif::{KNOWN_BOARDS, KnownBoard, Soc, known_board};
pub use qualifier::BoardId;
pub use resolve::{ResolutionSource, ResolvedTarget, resolve_static};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Arch {
    Xtensa,
    Riscv,
}

impl Arch {
    pub fn as_str(self) -> &'static str {
        match self {
            Arch::Xtensa => "xtensa",
            Arch::Riscv => "riscv",
        }
    }
}

impl fmt::Display for Arch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Error, Diagnostic, PartialEq, Eq)]
pub enum BoardError {
    #[error("invalid board name `{board}`: {reason}")]
    #[diagnostic(
        code(zem::board::invalid),
        help("board names look like `name[@revision][/soc[/cpucluster[/variant]]]`")
    )]
    Invalid { board: String, reason: &'static str },

    #[error("board `{board}` targets SoC `{soc}`, which is not an ESP32-series SoC")]
    #[diagnostic(
        code(zem::board::unsupported_soc),
        help(
            "zem currently supports esp32, esp32s2, esp32s3, esp32c2, esp32c3, esp32c6 and esp32h2"
        )
    )]
    UnsupportedSoc { board: String, soc: String },

    #[error("unknown board `{0}`")]
    #[diagnostic(
        code(zem::board::unknown),
        help(
            "zem currently supports ESP32-series boards only. If this is an ESP32 board, add the SoC \
             qualifier explicitly, e.g. `{0}/esp32s3/procpu`; run `zem init` interactively to see known boards"
        )
    )]
    Unknown(String),

    #[error("board `{board}` uses SoC {expected}, but the qualifier says `{actual}`")]
    #[diagnostic(code(zem::board::soc_mismatch))]
    SocMismatch {
        board: String,
        expected: &'static str,
        actual: String,
    },

    #[error("board `{board}` does not exist in Zephyr {version}")]
    #[diagnostic(
        code(zem::board::not_in_zephyr),
        help(
            "board names change between Zephyr releases; check boards/ in this release or pick another version"
        )
    )]
    NotInZephyr { board: String, version: String },

    #[error("board `{board}` does not have SoC {soc} (available: {available})")]
    #[diagnostic(code(zem::board::soc_not_on_board))]
    SocNotOnBoard {
        board: String,
        soc: String,
        available: String,
    },

    #[error("`{board}` is not a valid build target")]
    #[diagnostic(code(zem::board::invalid_target), help("valid targets: {valid}"))]
    InvalidTarget { board: String, valid: String },

    #[error("[target].arch = \"{requested}\" conflicts with SoC {soc}, which is {actual}")]
    #[diagnostic(
        code(zem::board::arch_mismatch),
        help("remove [target].arch; it is detected from the board automatically")
    )]
    ArchMismatch {
        requested: Arch,
        soc: &'static str,
        actual: Arch,
    },
}
