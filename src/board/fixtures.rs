//! A small fake Zephyr tree with boards and SoCs of several vendors, laid out like the real
//! one (HWMv2 `boards/<vendor>/<board>/board.yml` and `soc/**/soc.yml`). Files of other
//! vendors are copied (trimmed) from Zephyr v4.2.0, so the fixture tracks real layouts.

use std::fs;

use tempfile::TempDir;

const FILES: &[(&str, &str)] = &[
    // Espressif: multi-core Xtensa, single-core RISC-V, a third-party board with a variant.
    (
        "boards/espressif/esp32s3_devkitc/board.yml",
        "board:\n  name: esp32s3_devkitc\n  vendor: espressif\n  socs:\n  - name: esp32s3\n",
    ),
    (
        "boards/espressif/esp32c3_devkitm/board.yml",
        "board:\n  name: esp32c3_devkitm\n  socs:\n  - name: esp32c3\n",
    ),
    (
        "boards/seeed/xiao_esp32s3/board.yml",
        "board:\n  name: xiao_esp32s3\n  socs:\n  - name: esp32s3\n    variants:\n    - name: 'sense'\n      cpucluster: 'procpu'\n",
    ),
    (
        "boards/m5stack/m5stack_atoms3/board.yml",
        "board:\n  name: m5stack_atoms3\n  vendor: m5stack\n  socs:\n  - name: esp32s3\n",
    ),
    (
        "soc/espressif/soc.yml",
        "family:\n- name: espressif_esp32\n  series:\n  - name: esp32\n    socs:\n    - name: esp32\n      cpuclusters:\n      - name: procpu\n      - name: appcpu\n  - name: esp32s3\n    socs:\n    - name: esp32s3\n      cpuclusters:\n      - name: procpu\n      - name: appcpu\n  - name: esp32c3\n    socs:\n    - name: esp32c3\n  - name: esp32c6\n    socs:\n    - name: esp32c6\n      cpuclusters:\n      - name: hpcore\n      - name: lpcore\n  - name: esp32c5\n    socs:\n    - name: esp32c5\n      cpuclusters:\n      - name: hpcore\n      - name: lpcore\n",
    ),
    // Twister metadata, as in v4.2.0 (`identifier` + `arch`).
    (
        "boards/espressif/esp32s3_devkitc/esp32s3_devkitc_procpu.yaml",
        "identifier: esp32s3_devkitc/esp32s3/procpu\nname: ESP32-S3 DevKitC PROCPU\ntype: mcu\narch: xtensa\n",
    ),
    (
        "boards/espressif/esp32c3_devkitm/esp32c3_devkitm.yaml",
        "identifier: esp32c3_devkitm\nname: ESP32-C3-DevKitM\ntype: mcu\narch: riscv\n",
    ),
    (
        "boards/raspberrypi/rpi_pico/rpi_pico.yaml",
        "identifier: rpi_pico\nname: Raspberry Pi Pico\ntype: mcu\narch: arm\n",
    ),
    // Hypothetical: a SoC of a supported vendor that zem's tables do not list, as a later
    // Zephyr release would add (v4.2.0 has no esp32c5). Twister covers only one cluster.
    (
        "boards/espressif/esp32c5_devkitc/board.yml",
        "board:\n  name: esp32c5_devkitc\n  vendor: espressif\n  socs:\n  - name: esp32c5\n",
    ),
    (
        "boards/espressif/esp32c5_devkitc/esp32c5_devkitc_hpcore.yaml",
        "identifier: esp32c5_devkitc/esp32c5/hpcore\ntype: mcu\narch: riscv\n",
    ),
    // Nordic: two clusters, a TrustZone variant on the application core only.
    (
        "boards/nordic/nrf5340dk/board.yml",
        "board:\n  name: nrf5340dk\n  full_name: nRF5340 DK\n  vendor: nordic\n  socs:\n  - name: 'nrf5340'\n    variants:\n    - name: 'ns'\n      cpucluster: 'cpuapp'\n",
    ),
    (
        "soc/nordic/soc.yml",
        "family:\n- name: nordic_nrf\n  series:\n  - name: nrf53\n    socs:\n    - name: nrf5340\n      cpuclusters:\n      - name: cpuapp\n      - name: cpunet\n",
    ),
    // ST: soc.yml one level deeper, board name unrelated to the SoC name.
    (
        "boards/st/nucleo_f401re/board.yml",
        "board:\n  name: nucleo_f401re\n  vendor: st\n  socs:\n  - name: stm32f401xe\n",
    ),
    (
        "soc/st/stm32/soc.yml",
        "family:\n- name: stm32\n  series:\n  - name: stm32f4x\n    socs:\n    - name: stm32f401xe\n",
    ),
    // Raspberry Pi: a variant without a cpucluster; rp2350 has clusters.
    (
        "boards/raspberrypi/rpi_pico/board.yml",
        "board:\n  name: rpi_pico\n  full_name: Raspberry Pi Pico\n  vendor: raspberrypi\n  socs:\n  - name: rp2040\n    variants:\n    - name: w\n",
    ),
    (
        "soc/raspberrypi/rpi_pico/soc.yml",
        "family:\n- name: rpi_pico\n  series:\n  - name: rp2040\n    socs:\n    - name: rp2040\n  - name: rp2350\n    socs:\n    - name: rp2350a\n      cpuclusters:\n      - name: m33\n",
    ),
    // Host build (posix): `boards:` list form; soc.yml lists SoCs directly under the family.
    (
        "boards/native/native_sim/board.yml",
        "boards:\n- name: native_sim\n  full_name: Native simulator - native_sim\n  vendor: zephyr\n  socs:\n  - name: native\n    variants:\n    - name: \"64\"\n",
    ),
    (
        "soc/native/inf_clock/soc.yml",
        "family:\n- name: native\n  socs:\n  - name: native\n",
    ),
    // Analog Devices: SoCs with cpuclusters directly under the family.
    (
        "boards/adi/max32690evkit/board.yml",
        "board:\n  name: max32690evkit\n  full_name: MAX32690EVKIT\n  vendor: adi\n  socs:\n    - name: max32690\n",
    ),
    (
        "soc/adi/max32/soc.yml",
        "family:\n- name: max32\n  socs:\n  - name: max32650\n  - name: max32690\n    cpuclusters:\n    - name: m4\n",
    ),
    // Top-level series (Xen) and top-level socs (Realtek EC).
    (
        "soc/xen/soc.yml",
        "series:\n- name: xenvm\n  socs:\n  - name: xenvm\n",
    ),
    (
        "soc/realtek/ec/soc.yml",
        "socs:\n- name: rts5912\n- name: rts5915\n",
    ),
    // A board with two SoCs (like sam_v71_xult or nrf52840dk): the bare name is ambiguous.
    (
        "boards/other/dual_soc_board/board.yml",
        "board:\n  name: dual_soc_board\n  socs:\n  - name: esp32c3\n  - name: esp32c6\n",
    ),
    // A file that defines several boards.
    (
        "boards/other/multi/board.yml",
        "boards:\n- name: some_other_board\n  socs:\n  - name: foo\n",
    ),
];

pub fn zephyr_tree() -> TempDir {
    let tmp = tempfile::tempdir().unwrap();
    for (rel, text) in FILES {
        let path = tmp.path().join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    tmp
}
