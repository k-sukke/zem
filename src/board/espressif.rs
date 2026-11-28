use super::Arch;

/// What zem knows about an ESP32-series SoC before a Zephyr tree is available.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SocInfo {
    pub name: &'static str,
    pub arch: Arch,
    /// Zephyr SDK toolchain directory / `west sdk install -t` name.
    pub toolchain: &'static str,
    /// Latest Zephyr release (major, minor) known to lack this SoC entirely.
    pub absent_through: Option<(u32, u32)>,
}

const RISCV: &str = "riscv64-zephyr-elf";

const fn soc(
    name: &'static str,
    arch: Arch,
    toolchain: &'static str,
    absent_through: Option<(u32, u32)>,
) -> SocInfo {
    SocInfo {
        name,
        arch,
        toolchain,
        absent_through,
    }
}

pub const SOCS: &[SocInfo] = &[
    soc(
        "esp32",
        Arch::Xtensa,
        "xtensa-espressif_esp32_zephyr-elf",
        None,
    ),
    soc(
        "esp32s2",
        Arch::Xtensa,
        "xtensa-espressif_esp32s2_zephyr-elf",
        None,
    ),
    soc(
        "esp32s3",
        Arch::Xtensa,
        "xtensa-espressif_esp32s3_zephyr-elf",
        None,
    ),
    soc("esp32c2", Arch::Riscv, RISCV, None),
    soc("esp32c3", Arch::Riscv, RISCV, None),
    soc("esp32c6", Arch::Riscv, RISCV, None),
    soc("esp32h2", Arch::Riscv, RISCV, Some((4, 2))),
];

pub fn soc_info(name: &str) -> Option<&'static SocInfo> {
    SOCS.iter().find(|soc| soc.name == name)
}

/// Longest SoC name that the board name equals or starts with followed by `_`
/// (so `esp32s3_devkitc` maps to esp32s3, not esp32).
pub fn soc_from_board_prefix(board_name: &str) -> Option<&'static SocInfo> {
    SOCS.iter()
        .filter(|soc| {
            board_name == soc.name
                || board_name
                    .strip_prefix(soc.name)
                    .is_some_and(|rest| rest.starts_with('_'))
        })
        .max_by_key(|soc| soc.name.len())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnownBoard {
    pub name: &'static str,
    pub soc: &'static str,
    /// Qualifiers appended when the user gives only the bare name (multi-core SoCs need them).
    pub default_qualifiers: Option<&'static str>,
    pub description: &'static str,
}

impl KnownBoard {
    pub fn full_name(&self) -> String {
        match self.default_qualifiers {
            Some(q) => format!("{}/{}", self.name, q),
            None => self.name.to_string(),
        }
    }
}

const fn board(
    name: &'static str,
    soc: &'static str,
    default_qualifiers: Option<&'static str>,
    description: &'static str,
) -> KnownBoard {
    KnownBoard {
        name,
        soc,
        default_qualifiers,
        description,
    }
}

/// Must be re-checked against `boards/espressif` of each supported Zephyr release.
pub const KNOWN_BOARDS: &[KnownBoard] = &[
    board(
        "esp32_devkitc",
        "esp32",
        Some("esp32/procpu"),
        "ESP32-DevKitC",
    ),
    board(
        "esp32_ethernet_kit",
        "esp32",
        Some("esp32/procpu"),
        "ESP32-Ethernet-Kit",
    ),
    board("esp32s2_saola", "esp32s2", None, "ESP32-S2-Saola-1"),
    board("esp32s2_devkitc", "esp32s2", None, "ESP32-S2-DevKitC"),
    board(
        "esp32s3_devkitc",
        "esp32s3",
        Some("esp32s3/procpu"),
        "ESP32-S3-DevKitC",
    ),
    board(
        "esp32s3_devkitm",
        "esp32s3",
        Some("esp32s3/procpu"),
        "ESP32-S3-DevKitM",
    ),
    board(
        "esp8684_devkitm",
        "esp32c2",
        None,
        "ESP8684-DevKitM (ESP32-C2)",
    ),
    board("esp32c3_devkitm", "esp32c3", None, "ESP32-C3-DevKitM"),
    board("esp32c3_devkitc", "esp32c3", None, "ESP32-C3-DevKitC"),
    board(
        "esp32c6_devkitc",
        "esp32c6",
        Some("esp32c6/hpcore"),
        "ESP32-C6-DevKitC",
    ),
    board("esp32h2_devkitm", "esp32h2", None, "ESP32-H2-DevKitM"),
    board(
        "xiao_esp32s3",
        "esp32s3",
        Some("esp32s3/procpu"),
        "Seeed XIAO ESP32S3",
    ),
    board("xiao_esp32c3", "esp32c3", None, "Seeed XIAO ESP32C3"),
    board(
        "xiao_esp32c6",
        "esp32c6",
        Some("esp32c6/hpcore"),
        "Seeed XIAO ESP32C6",
    ),
];

pub fn known_board(name: &str) -> Option<&'static KnownBoard> {
    KNOWN_BOARDS.iter().find(|b| b.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soc_names_are_unique() {
        for soc in SOCS {
            assert_eq!(soc_info(soc.name), Some(soc));
        }
        assert_eq!(soc_info("nrf52840"), None);
    }

    #[test]
    fn toolchain_matches_arch() {
        for soc in SOCS {
            match soc.arch {
                Arch::Xtensa => assert!(soc.toolchain.starts_with("xtensa-espressif_")),
                _ => assert_eq!(soc.toolchain, RISCV),
            }
        }
    }

    #[test]
    fn xtensa_toolchains_are_per_soc() {
        let tc = |name| soc_info(name).unwrap().toolchain;
        assert_eq!(tc("esp32"), "xtensa-espressif_esp32_zephyr-elf");
        assert_eq!(tc("esp32s2"), "xtensa-espressif_esp32s2_zephyr-elf");
        assert_eq!(tc("esp32s3"), "xtensa-espressif_esp32s3_zephyr-elf");
    }

    #[test]
    fn prefix_prefers_longest_match() {
        let prefix = |board| soc_from_board_prefix(board).map(|s| s.name);
        assert_eq!(prefix("esp32s3_foo"), Some("esp32s3"));
        assert_eq!(prefix("esp32_foo"), Some("esp32"));
        assert_eq!(prefix("esp32c6"), Some("esp32c6"));
        assert_eq!(prefix("esp32s3foo"), None);
        assert_eq!(prefix("nucleo_f401re"), None);
    }

    #[test]
    fn known_boards_are_unique_and_consistent() {
        let mut names: Vec<_> = KNOWN_BOARDS.iter().map(|b| b.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), KNOWN_BOARDS.len());
        for b in KNOWN_BOARDS {
            assert!(soc_info(b.soc).is_some(), "{}", b.name);
            if let Some(q) = b.default_qualifiers {
                assert_eq!(q.split('/').next(), Some(b.soc), "{}", b.name);
            }
        }
    }
}
