//! A small fake Zephyr tree with boards and SoCs of several vendors, laid out like the real
//! one (HWMv2 `boards/<vendor>/<board>/board.yml` and `soc/**/soc.yml`).

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
        "soc/espressif/soc.yml",
        "family:\n- name: espressif_esp32\n  series:\n  - name: esp32s3\n    socs:\n    - name: esp32s3\n      cpuclusters:\n      - name: procpu\n      - name: appcpu\n  - name: esp32c3\n    socs:\n    - name: esp32c3\n",
    ),
    // Nordic: two clusters, a TrustZone variant on the application core only.
    (
        "boards/nordic/nrf5340dk/board.yml",
        "board:\n  name: nrf5340dk\n  full_name: nRF5340 DK\n  vendor: nordic\n  socs:\n  - name: nrf5340\n    variants:\n    - name: ns\n      cpucluster: cpuapp\n",
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
    // Raspberry Pi: single core, no blobs.
    (
        "boards/raspberrypi/rpi_pico/board.yml",
        "board:\n  name: rpi_pico\n  vendor: raspberrypi\n  socs:\n  - name: rp2040\n",
    ),
    (
        "soc/raspberrypi/rpi_pico/soc.yml",
        "family:\n- name: rpi_pico\n  series:\n  - name: rp2xxx\n    socs:\n    - name: rp2040\n",
    ),
    // Host build (posix): a variant without a cpucluster.
    (
        "boards/native/native_sim/board.yml",
        "board:\n  name: native_sim\n  vendor: zephyr\n  socs:\n  - name: native\n    variants:\n    - name: '64'\n",
    ),
    (
        "soc/native/soc.yml",
        "family:\n- name: native\n  series:\n  - name: native\n    socs:\n    - name: native\n",
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
