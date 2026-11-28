use serde::{Deserialize, Serialize};

use super::{Arch, BoardError, BoardId, SocId};
use crate::vendor::{self, SocInfo, VendorProfile};

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
    pub vendor: &'static VendorProfile,
    pub soc: SocId,
    pub arch: Arch,
    pub toolchains: Vec<String>,
    pub hal_modules: Vec<String>,
    pub blob_modules: Vec<String>,
    pub source: ResolutionSource,
    /// Set when `board` differs from what the user wrote (default qualifiers were added).
    pub normalized_from: Option<String>,
}

/// Resolve a board without a Zephyr checkout, from its SoC qualifier, the vendors' known
/// boards or its name prefix. `None` means zem cannot tell the SoC from the name alone;
/// sync then resolves it from the Zephyr tree (`board_yml::resolve`), which also re-checks
/// every result of this function.
pub fn resolve_static(
    input: &str,
    arch_override: Option<Arch>,
) -> Result<Option<ResolvedTarget>, BoardError> {
    let id = BoardId::parse(input)?;
    let known = vendor::find_board(&id.name);

    let ((vendor, soc), source, board): ((&VendorProfile, &SocInfo), _, _) =
        if let Some(qualifier) = id.soc() {
            let found = vendor::find_soc(qualifier).ok_or_else(|| BoardError::UnsupportedSoc {
                board: input.trim().to_string(),
                soc: qualifier.to_string(),
                supported: vendor::supported_socs(),
            })?;
            if let Some((_, k)) = known.filter(|(_, k)| k.soc != found.1.name) {
                return Err(BoardError::SocMismatch {
                    board: id.name.clone(),
                    expected: k.soc.to_string(),
                    actual: qualifier.to_string(),
                });
            }
            (found, ResolutionSource::Qualifier, id.clone())
        } else if let Some((vendor, k)) = known {
            let soc = vendor
                .soc(k.soc)
                .expect("known boards use their vendor's SoCs");
            (
                (vendor, soc),
                ResolutionSource::StaticTable,
                with_defaults(&id, soc),
            )
        } else if let Some(found) = vendor::soc_from_board_prefix(&id.name) {
            // Only a guess: keep the name as written and let the tree add qualifiers, so
            // `zem init` does not write guessed qualifiers into zem.toml.
            (found, ResolutionSource::Heuristic, id.clone())
        } else {
            return Ok(None);
        };

    build(input, &board, vendor, soc, source, arch_override).map(Some)
}

/// A bare board name plus the SoC's default qualifiers, if it has any.
fn with_defaults(id: &BoardId, soc: &SocInfo) -> BoardId {
    match soc.default_qualifiers() {
        Some(q) if id.qualifiers.is_empty() => id.with_qualifiers(&q),
        _ => id.clone(),
    }
}

/// The target for `board` (the full name passed to `west build -b`) on `soc` of `vendor`.
pub(super) fn build(
    input: &str,
    board: &BoardId,
    vendor: &'static VendorProfile,
    soc: &'static SocInfo,
    source: ResolutionSource,
    arch_override: Option<Arch>,
) -> Result<ResolvedTarget, BoardError> {
    if let Some(requested) = arch_override.filter(|a| *a != soc.arch) {
        return Err(BoardError::ArchMismatch {
            requested,
            soc: soc.name.to_string(),
            actual: soc.arch,
        });
    }

    let board = board.to_string();
    let normalized_from = (board != input.trim()).then(|| input.trim().to_string());
    Ok(ResolvedTarget {
        board,
        vendor,
        soc: SocId::new(soc.name),
        arch: soc.arch,
        toolchains: vec![soc.toolchain.to_string()],
        hal_modules: vendor.hal_modules.iter().map(|m| m.to_string()).collect(),
        blob_modules: vendor.blob_modules.iter().map(|m| m.to_string()).collect(),
        source,
        normalized_from,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(board: &str) -> ResolvedTarget {
        resolve_static(board, None).unwrap().unwrap()
    }

    #[test]
    fn qualifier_wins() {
        let t = resolve("esp32s3_devkitc/esp32s3/procpu");
        assert_eq!(t.soc.as_str(), "esp32s3");
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
        assert_eq!(t.soc.as_str(), "esp32c3");
        assert_eq!(t.arch, Arch::Riscv);
        assert_eq!(t.toolchains, ["riscv64-zephyr-elf"]);
    }

    #[test]
    fn table_covers_boards_without_soc_prefix() {
        assert_eq!(resolve("esp8684_devkitm").soc.as_str(), "esp32c2");
        assert_eq!(resolve("xiao_esp32c6").board, "xiao_esp32c6/esp32c6/hpcore");
    }

    #[test]
    fn unknown_board_uses_soc_prefix() {
        let t = resolve("esp32h2_custom");
        assert_eq!(t.soc.as_str(), "esp32h2");
        assert_eq!(t.source, ResolutionSource::Heuristic);
    }

    #[test]
    fn lpcore_uses_riscv_toolchain() {
        let t = resolve("esp32c6_devkitc/esp32c6/lpcore");
        assert_eq!(t.toolchains, ["riscv64-zephyr-elf"]);
    }

    #[test]
    fn unsupported_socs_are_rejected_and_unknown_boards_deferred() {
        assert!(matches!(
            resolve_static("nrf52840dk/nrf52840", None),
            Err(BoardError::UnsupportedSoc { .. })
        ));
        assert_eq!(resolve_static("nucleo_f401re", None), Ok(None));
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

    #[test]
    fn guessed_socs_keep_the_name_as_written() {
        let t = resolve("esp32s3_custom");
        assert_eq!(t.source, ResolutionSource::Heuristic);
        assert_eq!(
            (t.board.as_str(), t.normalized_from),
            ("esp32s3_custom", None)
        );
    }
}
