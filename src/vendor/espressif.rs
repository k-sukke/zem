//! Espressif ESP32 series.

use super::{KnownBoard, PathMarker, SocInfo, UsbId, VendorProfile, board, soc};
use crate::board::Arch;

pub const PROFILE: VendorProfile = VendorProfile {
    id: "espressif",
    socs: SOCS,
    boards: BOARDS,
    hal_modules: &["hal_espressif"],
    blob_modules: &["hal_espressif"],
    env_prefixes: &["IDF_"],
    env_vars: &[
        "ESP_IDF_VERSION",
        "ESP_ROM_ELF_DIR",
        "ESPRESSIF_TOOLCHAIN_PATH",
    ],
    notable_env_vars: &[
        "IDF_PATH",
        "IDF_TOOLS_PATH",
        "IDF_PYTHON_ENV_PATH",
        "ESPRESSIF_TOOLCHAIN_PATH",
    ],
    path_markers: &[
        PathMarker::Exact(".espressif"),
        PathMarker::Prefix("esp-idf"),
    ],
    usb_ids: &[
        UsbId::exact(0x303a, 0x1001, "ESP USB-Serial/JTAG"),
        UsbId::any(0x303a, "Espressif USB"),
    ],
};

const RISCV: &str = "riscv64-zephyr-elf";

const SOCS: &[SocInfo] = &[
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

/// Must be re-checked against `boards/espressif` of each supported Zephyr release.
const BOARDS: &[KnownBoard] = &[
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let tc = |name| PROFILE.soc(name).unwrap().toolchain;
        assert_eq!(tc("esp32"), "xtensa-espressif_esp32_zephyr-elf");
        assert_eq!(tc("esp32s2"), "xtensa-espressif_esp32s2_zephyr-elf");
        assert_eq!(tc("esp32s3"), "xtensa-espressif_esp32s3_zephyr-elf");
    }
}
