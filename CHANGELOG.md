# Changelog

Releases before 0.3.0 are listed on the [GitHub releases page](https://github.com/k-sukke/zem/releases).

## 0.3.0

This release reworks how zem finds boards, as groundwork for supporting MCUs from more vendors. ESP32 projects
build and flash as before.

### Changed

- **Boards come from the Zephyr tree.** `zem sync` now looks the board up in the fetched Zephyr (`board.yml`,
  `soc.yml` and the board's twister metadata) and takes the SoC, CPU architecture, toolchain and HAL module from
  there. Any Zephyr board with an ESP32-series SoC works, not only the boards in zem's list (for example
  `ttgo_t8c3`, `tdongle_s3`, `m5stack_atoms3`). `zem init` accepts board names it does not know and shows that
  `zem sync` will check them.
- `zem sync` fetches Zephyr first, resolves the board, then adds the board's HAL module to the shared workspace if
  it is missing.
- A board name without a CPU cluster gets the SoC's main core (`procpu` on ESP32 / ESP32-S3, `hpcore` on ESP32-C6)
  instead of a guess. A SoC with several clusters and no known main core needs the cluster in the name, and the
  error lists the valid targets.
- `[target].arch` accepts every Zephyr architecture (`arc`, `arm`, `arm64`, `mips`, `posix`, `riscv`, `rx`,
  `sparc`, `x86`, `xtensa`).
- Error messages and the `zem.toml` template no longer assume ESP32. A board of an unsupported vendor stops at the
  Board step of `zem sync` and names the SoC's `soc/<vendor>` directory.
- Checks that need the Zephyr tree now happen in `zem sync` rather than `zem init`: whether the board and its SoC
  exist in the chosen Zephyr version (for example ESP32-H2 needs a Zephyr newer than 4.2), and whether a SoC named
  in the board qualifier is supported.
- `.zem/state.json` now records the target's vendor. A project synced with an older zem asks for `zem sync` once.

### Known limitations

- **Custom boards are not supported yet.** `zem sync` only finds boards in the Zephyr tree, not boards defined
  next to the application (`boards/<vendor>/<board>/`) or in a module's `board_root`. Products with their own PCB
  have to build as an existing Zephyr board with devicetree overlays for now.
- **Only Espressif SoCs.** Other vendors need a vendor profile, and their boards need host tools for flashing and
  monitoring (J-Link, OpenOCD, pyOCD, picotool, ...) that zem does not set up or check yet.

See [Known limitations](README.md#known-limitations) in the README.
