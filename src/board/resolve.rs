use serde::{Deserialize, Serialize};

use super::{Arch, BoardError, BoardId, Soc, known_board};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionSource {
    Qualifier,
    StaticTable,
    Heuristic,
    BoardYml,
}

impl ResolutionSource {
    pub fn describe(self) -> &'static str {
        match self {
            ResolutionSource::Qualifier => "board qualifier",
            ResolutionSource::StaticTable => "known board table",
            ResolutionSource::Heuristic => "board name prefix",
            ResolutionSource::BoardYml => "board.yml",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTarget {
    /// Passed verbatim to `west build -b`.
    pub board: String,
    pub soc: Soc,
    pub arch: Arch,
    pub toolchains: Vec<&'static str>,
    pub hal_modules: Vec<&'static str>,
    pub blob_modules: Vec<&'static str>,
    pub source: ResolutionSource,
    /// Set when `board` differs from what the user wrote (default qualifiers were added).
    pub normalized_from: Option<String>,
}

/// Resolve a board without a Zephyr checkout. The result is re-validated against
/// `board.yml` once the workspace exists.
pub fn resolve_static(
    input: &str,
    arch_override: Option<Arch>,
) -> Result<ResolvedTarget, BoardError> {
    let id = BoardId::parse(input)?;
    let known = known_board(&id.name);

    let (soc, source, board) = if let Some(qualifier) = id.soc() {
        let soc = Soc::from_name(qualifier).ok_or_else(|| BoardError::UnsupportedSoc {
            board: input.trim().to_string(),
            soc: qualifier.to_string(),
        })?;
        if let Some(k) = known.filter(|k| k.soc != soc) {
            return Err(BoardError::SocMismatch {
                board: id.name.clone(),
                expected: k.soc.name(),
                actual: qualifier.to_string(),
            });
        }
        (soc, ResolutionSource::Qualifier, id.clone())
    } else if let Some(k) = known {
        let board = match k.default_qualifiers {
            Some(q) => id.with_qualifiers(q),
            None => id.clone(),
        };
        (k.soc, ResolutionSource::StaticTable, board)
    } else if let Some(soc) = Soc::from_board_prefix(&id.name) {
        (soc, ResolutionSource::Heuristic, id.clone())
    } else {
        return Err(BoardError::Unknown(input.trim().to_string()));
    };

    if let Some(requested) = arch_override.filter(|a| *a != soc.arch()) {
        return Err(BoardError::ArchMismatch {
            requested,
            soc: soc.name(),
            actual: soc.arch(),
        });
    }

    let board = board.to_string();
    let normalized_from = (board != input.trim()).then(|| input.trim().to_string());
    Ok(ResolvedTarget {
        board,
        soc,
        arch: soc.arch(),
        toolchains: vec![soc.toolchain()],
        hal_modules: vec!["hal_espressif"],
        blob_modules: vec!["hal_espressif"],
        source,
        normalized_from,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(board: &str) -> ResolvedTarget {
        resolve_static(board, None).unwrap()
    }

    #[test]
    fn qualifier_wins() {
        let t = resolve("esp32s3_devkitc/esp32s3/procpu");
        assert_eq!(t.soc, Soc::Esp32s3);
        assert_eq!(t.arch, Arch::Xtensa);
        assert_eq!(t.toolchains, ["xtensa-espressif_esp32s3_zephyr-elf"]);
        assert_eq!(t.source, ResolutionSource::Qualifier);
        assert_eq!(t.normalized_from, None);
    }

    #[test]
    fn known_multicore_board_gets_default_qualifiers() {
        let t = resolve("esp32s3_devkitc");
        assert_eq!(t.board, "esp32s3_devkitc/esp32s3/procpu");
        assert_eq!(t.normalized_from.as_deref(), Some("esp32s3_devkitc"));
        assert_eq!(t.source, ResolutionSource::StaticTable);
    }

    #[test]
    fn known_single_core_board_is_unchanged() {
        let t = resolve("esp32c3_devkitm");
        assert_eq!(t.board, "esp32c3_devkitm");
        assert_eq!(t.soc, Soc::Esp32c3);
        assert_eq!(t.arch, Arch::Riscv);
        assert_eq!(t.toolchains, ["riscv64-zephyr-elf"]);
    }

    #[test]
    fn table_covers_boards_without_soc_prefix() {
        assert_eq!(resolve("esp8684_devkitm").soc, Soc::Esp32c2);
        assert_eq!(resolve("xiao_esp32c6").board, "xiao_esp32c6/esp32c6/hpcore");
    }

    #[test]
    fn unknown_esp_board_uses_prefix() {
        let t = resolve("esp32h2_custom");
        assert_eq!(t.soc, Soc::Esp32h2);
        assert_eq!(t.source, ResolutionSource::Heuristic);
    }

    #[test]
    fn lpcore_uses_riscv_toolchain() {
        let t = resolve("esp32c6_devkitc/esp32c6/lpcore");
        assert_eq!(t.toolchains, ["riscv64-zephyr-elf"]);
    }

    #[test]
    fn non_esp_boards_are_rejected() {
        assert!(matches!(
            resolve_static("nrf52840dk/nrf52840", None),
            Err(BoardError::UnsupportedSoc { .. })
        ));
        assert!(matches!(
            resolve_static("nucleo_f401re", None),
            Err(BoardError::Unknown(_))
        ));
    }

    #[test]
    fn mismatched_qualifier_is_rejected() {
        assert!(matches!(
            resolve_static("esp32s3_devkitc/esp32/procpu", None),
            Err(BoardError::SocMismatch { .. })
        ));
    }

    #[test]
    fn arch_override_must_agree() {
        assert!(resolve_static("esp32c3_devkitm", Some(Arch::Riscv)).is_ok());
        assert!(matches!(
            resolve_static("esp32c3_devkitm", Some(Arch::Xtensa)),
            Err(BoardError::ArchMismatch { .. })
        ));
    }
}
