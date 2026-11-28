pub mod board_yml;
#[cfg(test)]
mod fixtures;
mod qualifier;
mod resolve;
mod soc;

use std::fmt;

use miette::Diagnostic;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use qualifier::BoardId;
pub use resolve::{ResolutionSource, ResolvedTarget, resolve_static};
pub use soc::SocId;

/// CPU architecture, one per directory under Zephyr's `arch/` (v4.0 and later).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Arch {
    Arc,
    Arm,
    Arm64,
    Mips,
    Posix,
    Riscv,
    Rx,
    Sparc,
    X86,
    Xtensa,
}

impl Arch {
    pub fn as_str(self) -> &'static str {
        match self {
            Arch::Arc => "arc",
            Arch::Arm => "arm",
            Arch::Arm64 => "arm64",
            Arch::Mips => "mips",
            Arch::Posix => "posix",
            Arch::Riscv => "riscv",
            Arch::Rx => "rx",
            Arch::Sparc => "sparc",
            Arch::X86 => "x86",
            Arch::Xtensa => "xtensa",
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

    #[error("board `{board}` targets SoC `{soc}`, which zem does not support yet")]
    #[diagnostic(code(zem::board::unsupported_soc), help("supported SoCs: {supported}"))]
    UnsupportedSoc {
        board: String,
        soc: String,
        supported: String,
    },

    #[error("board `{board}` uses SoC {expected}, but the qualifier says `{actual}`")]
    #[diagnostic(code(zem::board::soc_mismatch))]
    SocMismatch {
        board: String,
        expected: String,
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
        soc: String,
        actual: Arch,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arch_serde_matches_zephyr_arch_dirs() {
        for name in [
            "arc", "arm", "arm64", "mips", "posix", "riscv", "rx", "sparc", "x86", "xtensa",
        ] {
            let arch: Arch = serde_json::from_value(serde_json::json!(name)).unwrap();
            assert_eq!(arch.as_str(), name);
            assert_eq!(serde_json::to_value(arch).unwrap(), name);
        }
        assert!(serde_json::from_value::<Arch>(serde_json::json!("avr")).is_err());
    }
}
