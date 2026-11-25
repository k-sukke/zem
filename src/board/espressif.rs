use super::Arch;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Soc {
    Esp32,
    Esp32s2,
    Esp32s3,
    Esp32c2,
    Esp32c3,
    Esp32c6,
    Esp32h2,
}

impl Soc {
    pub const ALL: [Soc; 7] = [
        Soc::Esp32,
        Soc::Esp32s2,
        Soc::Esp32s3,
        Soc::Esp32c2,
        Soc::Esp32c3,
        Soc::Esp32c6,
        Soc::Esp32h2,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Soc::Esp32 => "esp32",
            Soc::Esp32s2 => "esp32s2",
            Soc::Esp32s3 => "esp32s3",
            Soc::Esp32c2 => "esp32c2",
            Soc::Esp32c3 => "esp32c3",
            Soc::Esp32c6 => "esp32c6",
            Soc::Esp32h2 => "esp32h2",
        }
    }

    pub fn from_name(name: &str) -> Option<Soc> {
        Soc::ALL.into_iter().find(|soc| soc.name() == name)
    }

    pub fn arch(self) -> Arch {
        match self {
            Soc::Esp32 | Soc::Esp32s2 | Soc::Esp32s3 => Arch::Xtensa,
            Soc::Esp32c2 | Soc::Esp32c3 | Soc::Esp32c6 | Soc::Esp32h2 => Arch::Riscv,
        }
    }

    /// Zephyr SDK toolchain directory / `west sdk install -t` name.
    pub fn toolchain(self) -> &'static str {
        match self {
            Soc::Esp32 => "xtensa-espressif_esp32_zephyr-elf",
            Soc::Esp32s2 => "xtensa-espressif_esp32s2_zephyr-elf",
            Soc::Esp32s3 => "xtensa-espressif_esp32s3_zephyr-elf",
            Soc::Esp32c2 | Soc::Esp32c3 | Soc::Esp32c6 | Soc::Esp32h2 => "riscv64-zephyr-elf",
        }
    }

    /// Latest Zephyr release (major, minor) known to lack this SoC entirely.
    pub fn absent_through(self) -> Option<(u32, u32)> {
        match self {
            Soc::Esp32h2 => Some((4, 2)),
            _ => None,
        }
    }

    /// Longest SoC name that the board name equals or starts with followed by `_`
    /// (so `esp32s3_devkitc` maps to esp32s3, not esp32).
    pub fn from_board_prefix(board_name: &str) -> Option<Soc> {
        Soc::ALL
            .into_iter()
            .filter(|soc| {
                let name = soc.name();
                board_name == name
                    || board_name
                        .strip_prefix(name)
                        .is_some_and(|rest| rest.starts_with('_'))
            })
            .max_by_key(|soc| soc.name().len())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnownBoard {
    pub name: &'static str,
    pub soc: Soc,
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
    soc: Soc,
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
        Soc::Esp32,
        Some("esp32/procpu"),
        "ESP32-DevKitC",
    ),
    board(
        "esp32_ethernet_kit",
        Soc::Esp32,
        Some("esp32/procpu"),
        "ESP32-Ethernet-Kit",
    ),
    board("esp32s2_saola", Soc::Esp32s2, None, "ESP32-S2-Saola-1"),
    board("esp32s2_devkitc", Soc::Esp32s2, None, "ESP32-S2-DevKitC"),
    board(
        "esp32s3_devkitc",
        Soc::Esp32s3,
        Some("esp32s3/procpu"),
        "ESP32-S3-DevKitC",
    ),
    board(
        "esp32s3_devkitm",
        Soc::Esp32s3,
        Some("esp32s3/procpu"),
        "ESP32-S3-DevKitM",
    ),
    board(
        "esp8684_devkitm",
        Soc::Esp32c2,
        None,
        "ESP8684-DevKitM (ESP32-C2)",
    ),
    board("esp32c3_devkitm", Soc::Esp32c3, None, "ESP32-C3-DevKitM"),
    board("esp32c3_devkitc", Soc::Esp32c3, None, "ESP32-C3-DevKitC"),
    board(
        "esp32c6_devkitc",
        Soc::Esp32c6,
        Some("esp32c6/hpcore"),
        "ESP32-C6-DevKitC",
    ),
    board("esp32h2_devkitm", Soc::Esp32h2, None, "ESP32-H2-DevKitM"),
    board(
        "xiao_esp32s3",
        Soc::Esp32s3,
        Some("esp32s3/procpu"),
        "Seeed XIAO ESP32S3",
    ),
    board("xiao_esp32c3", Soc::Esp32c3, None, "Seeed XIAO ESP32C3"),
    board(
        "xiao_esp32c6",
        Soc::Esp32c6,
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
    fn soc_names_round_trip() {
        for soc in Soc::ALL {
            assert_eq!(Soc::from_name(soc.name()), Some(soc));
        }
        assert_eq!(Soc::from_name("nrf52840"), None);
    }

    #[test]
    fn toolchain_matches_arch() {
        for soc in Soc::ALL {
            match soc.arch() {
                Arch::Xtensa => assert!(soc.toolchain().starts_with("xtensa-espressif_")),
                Arch::Riscv => assert_eq!(soc.toolchain(), "riscv64-zephyr-elf"),
            }
        }
    }

    #[test]
    fn xtensa_toolchains_are_per_soc() {
        assert_eq!(Soc::Esp32.toolchain(), "xtensa-espressif_esp32_zephyr-elf");
        assert_eq!(
            Soc::Esp32s2.toolchain(),
            "xtensa-espressif_esp32s2_zephyr-elf"
        );
        assert_eq!(
            Soc::Esp32s3.toolchain(),
            "xtensa-espressif_esp32s3_zephyr-elf"
        );
    }

    #[test]
    fn prefix_prefers_longest_match() {
        assert_eq!(Soc::from_board_prefix("esp32s3_foo"), Some(Soc::Esp32s3));
        assert_eq!(Soc::from_board_prefix("esp32_foo"), Some(Soc::Esp32));
        assert_eq!(Soc::from_board_prefix("esp32c6"), Some(Soc::Esp32c6));
        assert_eq!(Soc::from_board_prefix("esp32s3foo"), None);
        assert_eq!(Soc::from_board_prefix("nucleo_f401re"), None);
    }

    #[test]
    fn known_boards_are_unique_and_consistent() {
        let mut names: Vec<_> = KNOWN_BOARDS.iter().map(|b| b.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), KNOWN_BOARDS.len());
        for b in KNOWN_BOARDS {
            if let Some(q) = b.default_qualifiers {
                assert_eq!(q.split('/').next(), Some(b.soc.name()), "{}", b.name);
            }
        }
    }
}
