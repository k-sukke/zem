//! Verify a statically resolved target against the Zephyr tree (HWMv2 `board.yml` and
//! `soc.yml`), once a workspace exists.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::resolve::build;
use super::{Arch, BoardError, BoardId, ResolutionSource, ResolvedTarget};
use crate::vendor;

#[derive(Debug, Deserialize)]
struct BoardFile {
    board: Option<BoardEntry>,
    #[serde(default)]
    boards: Vec<BoardEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BoardEntry {
    pub name: String,
    #[serde(default)]
    pub socs: Vec<BoardSoc>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BoardSoc {
    pub name: String,
    #[serde(default)]
    pub variants: Vec<Variant>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Variant {
    pub name: String,
    pub cpucluster: Option<String>,
}

/// HWMv2 `soc.yml`. SoCs may be listed under `family → series`, directly under a family,
/// under a top-level series, or at the top level (e.g. `soc/native/inf_clock/soc.yml`).
#[derive(Debug, Deserialize)]
struct SocFile {
    #[serde(default)]
    family: Vec<Family>,
    #[serde(default)]
    series: Vec<Series>,
    #[serde(default)]
    socs: Vec<SocEntry>,
}

impl SocFile {
    fn into_socs(self) -> impl Iterator<Item = SocEntry> {
        self.family
            .into_iter()
            .flat_map(|f| {
                f.series
                    .into_iter()
                    .chain(std::iter::once(Series { socs: f.socs }))
            })
            .chain(self.series)
            .chain(std::iter::once(Series { socs: self.socs }))
            .flat_map(|s| s.socs)
    }
}

#[derive(Debug, Deserialize)]
struct Family {
    #[serde(default)]
    series: Vec<Series>,
    #[serde(default)]
    socs: Vec<SocEntry>,
}

#[derive(Debug, Deserialize)]
struct Series {
    #[serde(default)]
    socs: Vec<SocEntry>,
}

#[derive(Debug, Deserialize)]
struct SocEntry {
    name: String,
    #[serde(default)]
    cpuclusters: Vec<Named>,
}

#[derive(Debug, Deserialize)]
struct Named {
    name: String,
}

pub fn find_board(zephyr_base: &Path, name: &str) -> Option<BoardEntry> {
    let needle = format!("name: {name}");
    files_named(&zephyr_base.join("boards"), "board.yml")
        .into_iter()
        .filter_map(|path| fs::read_to_string(path).ok())
        .filter(|text| {
            text.lines()
                .any(|l| l.trim_start().trim_start_matches("- ") == needle)
        })
        .filter_map(|text| serde_yaml_ng::from_str::<BoardFile>(&text).ok())
        .flat_map(|f| f.board.into_iter().chain(f.boards))
        .find(|b| b.name == name)
}

fn files_named(dir: &Path, file_name: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().is_some_and(|n| n == file_name) {
                found.push(path);
            }
        }
    }
    found
}

/// CPU clusters of `soc`, from whichever `soc/**/soc.yml` defines it (vendors keep it at
/// different depths, e.g. `soc/espressif/soc.yml` and `soc/st/stm32/soc.yml`).
pub fn soc_cpuclusters(zephyr_base: &Path, soc: &str) -> Vec<String> {
    find_soc(zephyr_base, soc)
        .map(|s| s.cpuclusters.into_iter().map(|c| c.name).collect())
        .unwrap_or_default()
}

fn find_soc(zephyr_base: &Path, soc: &str) -> Option<SocEntry> {
    files_named(&zephyr_base.join("soc"), "soc.yml")
        .into_iter()
        .filter_map(|path| fs::read_to_string(path).ok())
        .filter_map(|text| serde_yaml_ng::from_str::<SocFile>(&text).ok())
        .flat_map(SocFile::into_socs)
        .find(|s| s.name == soc)
}

/// All `west build -b` targets the board offers for `soc`.
pub fn valid_targets(board: &BoardEntry, soc: &BoardSoc, clusters: &[String]) -> Vec<String> {
    let base = format!("{}/{}", board.name, soc.name);
    let mut targets = Vec::new();
    if clusters.is_empty() {
        targets.push(board.name.clone());
        targets.extend(
            soc.variants
                .iter()
                .filter(|v| v.cpucluster.is_none())
                .map(|v| format!("{base}/{}", v.name)),
        );
    } else {
        for cluster in clusters {
            targets.push(format!("{base}/{cluster}"));
            targets.extend(
                soc.variants
                    .iter()
                    .filter(|v| v.cpucluster.as_deref() == Some(cluster))
                    .map(|v| format!("{base}/{cluster}/{}", v.name)),
            );
        }
    }
    targets
}

/// The CPU cluster a name without one targets: the only cluster, or the vendor profile's
/// default for the SoC. With several clusters and no default the user has to pick one.
fn default_cluster<'a>(clusters: &'a [String], profile_default: Option<&str>) -> Option<&'a str> {
    match clusters {
        [only] => Some(only),
        _ => clusters
            .iter()
            .find(|c| Some(c.as_str()) == profile_default)
            .map(String::as_str),
    }
}

/// Resolve `input` against the Zephyr tree: the board must exist, have the SoC, and name a
/// valid build target. A bare name of a single-SoC board picks that SoC, and a name without a
/// CPU cluster gets the SoC's only cluster or the vendor profile's default (`procpu`, ...).
/// The SoC must belong to a vendor profile, which supplies the toolchain and HAL modules.
pub fn resolve(
    zephyr_base: &Path,
    input: &str,
    arch_override: Option<Arch>,
    zephyr_version: &str,
) -> Result<ResolvedTarget, BoardError> {
    let id = BoardId::parse(input)?;
    let board = find_board(zephyr_base, &id.name).ok_or_else(|| BoardError::NotInZephyr {
        board: id.name.clone(),
        version: zephyr_version.to_string(),
    })?;
    let soc =
        match id.soc() {
            Some(name) => board.socs.iter().find(|s| s.name == name).ok_or_else(|| {
                BoardError::SocNotOnBoard {
                    board: board.name.clone(),
                    soc: name.to_string(),
                    available: board
                        .socs
                        .iter()
                        .map(|s| s.name.clone())
                        .collect::<Vec<_>>()
                        .join(", "),
                }
            })?,
            None => match board.socs.as_slice() {
                [only] => only,
                socs => {
                    return Err(BoardError::InvalidTarget {
                        board: input.trim().to_string(),
                        valid: socs
                            .iter()
                            .flat_map(|s| {
                                valid_targets(&board, s, &soc_cpuclusters(zephyr_base, &s.name))
                            })
                            .collect::<Vec<_>>()
                            .join(", "),
                    });
                }
            },
        };
    let (vendor, info) = vendor::find_soc(&soc.name).ok_or_else(|| BoardError::UnsupportedSoc {
        board: input.trim().to_string(),
        soc: soc.name.clone(),
        supported: vendor::supported_socs(),
    })?;
    let clusters = soc_cpuclusters(zephyr_base, &soc.name);
    let valid = valid_targets(&board, soc, &clusters);

    let is_valid = |id: &BoardId| {
        let mut requested = id.clone();
        requested.revision = None;
        let requested = requested.to_string();
        valid.contains(&requested)
            || (clusters.is_empty() && requested == format!("{}/{}", board.name, soc.name))
    };
    let target = if is_valid(&id) {
        id.clone()
    } else if let Some(cluster) =
        default_cluster(&clusters, info.default_cpucluster).filter(|_| id.qualifiers.len() <= 1)
    {
        id.with_qualifiers(&format!("{}/{cluster}", soc.name))
    } else {
        return Err(BoardError::InvalidTarget {
            board: input.trim().to_string(),
            valid: valid.join(", "),
        });
    };

    build(
        input,
        &target,
        vendor,
        info,
        ResolutionSource::BoardYml,
        arch_override,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::fixtures;

    fn check(base: &Path, board: &str) -> Result<ResolvedTarget, BoardError> {
        resolve(base, board, None, "v4.2.0")
    }

    /// All `west build -b` targets of `board` on its first SoC, as the tree describes them.
    fn targets(base: &Path, board: &str) -> Vec<String> {
        let entry = find_board(base, board).unwrap();
        let clusters = soc_cpuclusters(base, &entry.socs[0].name);
        valid_targets(&entry, &entry.socs[0], &clusters)
    }

    #[test]
    fn accepts_valid_targets() {
        let tree = fixtures::zephyr_tree();
        check(tree.path(), "esp32s3_devkitc/esp32s3/procpu").unwrap();
        check(tree.path(), "esp32s3_devkitc/esp32s3/appcpu").unwrap();
        check(tree.path(), "esp32c3_devkitm").unwrap();
        check(tree.path(), "esp32c3_devkitm/esp32c3").unwrap();
        check(tree.path(), "xiao_esp32s3/esp32s3/procpu/sense").unwrap();
    }

    #[test]
    fn rejects_missing_cpucluster_and_unknown_boards() {
        let tree = fixtures::zephyr_tree();
        assert!(matches!(
            check(tree.path(), "esp32s3_devkitc/esp32s3/lpcore"),
            Err(BoardError::InvalidTarget { .. })
        ));
        assert!(matches!(
            check(tree.path(), "esp32h2_devkitm"),
            Err(BoardError::NotInZephyr { .. })
        ));
    }

    #[test]
    fn lists_valid_targets_with_variants() {
        let tree = fixtures::zephyr_tree();
        assert_eq!(
            targets(tree.path(), "xiao_esp32s3"),
            [
                "xiao_esp32s3/esp32s3/procpu",
                "xiao_esp32s3/esp32s3/procpu/sense",
                "xiao_esp32s3/esp32s3/appcpu"
            ]
        );
    }

    #[test]
    fn reads_boards_of_other_vendors() {
        let tree = fixtures::zephyr_tree();
        let base = tree.path();
        // Multi-core SoC with a variant on one cluster only.
        assert_eq!(
            targets(base, "nrf5340dk"),
            [
                "nrf5340dk/nrf5340/cpuapp",
                "nrf5340dk/nrf5340/cpuapp/ns",
                "nrf5340dk/nrf5340/cpunet"
            ]
        );
        // soc.yml one level deeper (soc/st/stm32/), and a board name that does not name its SoC.
        assert_eq!(targets(base, "nucleo_f401re"), ["nucleo_f401re"]);
        assert_eq!(
            find_board(base, "nucleo_f401re").unwrap().socs[0].name,
            "stm32f401xe"
        );
        assert_eq!(targets(base, "rpi_pico"), ["rpi_pico", "rpi_pico/rp2040/w"]);
        assert_eq!(
            targets(base, "max32690evkit"),
            ["max32690evkit/max32690/m4"]
        );
        assert_eq!(
            targets(base, "native_sim"),
            ["native_sim", "native_sim/native/64"]
        );
    }

    #[test]
    fn reads_socs_in_every_soc_yml_layout() {
        let tree = fixtures::zephyr_tree();
        let base = tree.path();
        // family → series, family → socs, top-level series, top-level socs.
        assert_eq!(soc_cpuclusters(base, "nrf5340"), ["cpuapp", "cpunet"]);
        assert_eq!(soc_cpuclusters(base, "max32690"), ["m4"]);
        for soc in ["native", "xenvm", "rts5912"] {
            assert!(find_soc(base, soc).is_some(), "{soc}");
        }
        assert!(find_soc(base, "missing_soc").is_none());
    }

    #[test]
    fn reads_multi_board_files() {
        let tree = fixtures::zephyr_tree();
        let base = tree.path();
        assert!(find_board(base, "some_other_board").is_some());
        assert!(find_board(base, "missing_board").is_none());
    }

    #[test]
    fn expands_names_without_a_cpucluster() {
        let tree = fixtures::zephyr_tree();
        for input in ["esp32s3_devkitc", "esp32s3_devkitc/esp32s3"] {
            let t = check(tree.path(), input).unwrap();
            assert_eq!(t.board, "esp32s3_devkitc/esp32s3/procpu");
            assert_eq!(t.normalized_from.as_deref(), Some(input));
        }
        let t = check(tree.path(), "esp32c3_devkitm").unwrap();
        assert_eq!(
            (t.board.as_str(), t.normalized_from),
            ("esp32c3_devkitm", None)
        );
    }

    #[test]
    fn resolves_boards_missing_from_the_vendor_tables() {
        let tree = fixtures::zephyr_tree();
        // Not a known board and its name does not start with a SoC name.
        let t = check(tree.path(), "m5stack_atoms3").unwrap();
        assert_eq!(t.board, "m5stack_atoms3/esp32s3/procpu");
        assert_eq!(t.vendor.id, "espressif");
        assert_eq!(t.soc.as_str(), "esp32s3");
        assert_eq!(t.toolchains, ["xtensa-espressif_esp32s3_zephyr-elf"]);
        assert_eq!(t.hal_modules, ["hal_espressif"]);
        assert_eq!(t.source, ResolutionSource::BoardYml);
    }

    #[test]
    fn rejects_socs_without_a_vendor_profile() {
        let tree = fixtures::zephyr_tree();
        for board in ["nucleo_f401re", "nrf5340dk", "rpi_pico"] {
            assert!(
                matches!(
                    check(tree.path(), board),
                    Err(BoardError::UnsupportedSoc { .. })
                ),
                "{board}"
            );
        }
    }

    #[test]
    fn checks_the_arch_override() {
        let tree = fixtures::zephyr_tree();
        assert!(matches!(
            resolve(tree.path(), "m5stack_atoms3", Some(Arch::Riscv), "v4.2.0"),
            Err(BoardError::ArchMismatch { .. })
        ));
    }

    #[test]
    fn default_cluster_needs_a_single_cluster_or_a_profile_default() {
        let clusters = |names: &[&str]| names.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(default_cluster(&clusters(&["m4"]), None), Some("m4"));
        assert_eq!(
            default_cluster(&clusters(&["procpu", "appcpu"]), Some("procpu")),
            Some("procpu")
        );
        // The soc.yml order does not decide: mimx8ml8 lists its DSP first.
        assert_eq!(
            default_cluster(&clusters(&["adsp", "a53", "m7"]), None),
            None
        );
        assert_eq!(default_cluster(&clusters(&["a", "b"]), Some("c")), None);
        assert_eq!(default_cluster(&[], Some("procpu")), None);
    }

    #[test]
    fn profile_defaults_exist_in_the_tree() {
        let tree = fixtures::zephyr_tree();
        // Every default must be one of the SoC's clusters (the fixture mirrors v4.2.0).
        for soc in crate::vendor::VENDORS.iter().flat_map(|v| v.socs) {
            if let Some(cluster) = soc.default_cpucluster {
                let clusters = soc_cpuclusters(tree.path(), soc.name);
                assert!(
                    clusters.iter().any(|c| c == cluster),
                    "{}: {clusters:?}",
                    soc.name
                );
            }
        }
    }
}
