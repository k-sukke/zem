//! Verify a statically resolved target against the Zephyr tree (HWMv2 `board.yml` and
//! `soc.yml`), once a workspace exists.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::{BoardError, BoardId, ResolvedTarget};

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

#[derive(Debug, Deserialize)]
struct SocFile {
    #[serde(default)]
    family: Vec<Family>,
}

#[derive(Debug, Deserialize)]
struct Family {
    #[serde(default)]
    series: Vec<Series>,
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
    board_files(&zephyr_base.join("boards"))
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

fn board_files(dir: &Path) -> Vec<PathBuf> {
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
            } else if path.file_name().is_some_and(|n| n == "board.yml") {
                found.push(path);
            }
        }
    }
    found
}

pub fn soc_cpuclusters(zephyr_base: &Path, soc: &str) -> Vec<String> {
    let path = zephyr_base.join("soc").join("espressif").join("soc.yml");
    let Some(file) = fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_yaml_ng::from_str::<SocFile>(&t).ok())
    else {
        return Vec::new();
    };
    file.family
        .into_iter()
        .flat_map(|f| f.series)
        .flat_map(|s| s.socs)
        .find(|s| s.name == soc)
        .map(|s| s.cpuclusters.into_iter().map(|c| c.name).collect())
        .unwrap_or_default()
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

pub fn verify(
    zephyr_base: &Path,
    target: &ResolvedTarget,
    zephyr_version: &str,
) -> Result<(), BoardError> {
    let id = BoardId::parse(&target.board)?;
    let board = find_board(zephyr_base, &id.name).ok_or_else(|| BoardError::NotInZephyr {
        board: id.name.clone(),
        version: zephyr_version.to_string(),
    })?;
    let soc_name = target.soc.name();
    let soc = board
        .socs
        .iter()
        .find(|s| s.name == soc_name)
        .ok_or_else(|| BoardError::SocNotOnBoard {
            board: board.name.clone(),
            soc: soc_name.to_string(),
            available: board
                .socs
                .iter()
                .map(|s| s.name.clone())
                .collect::<Vec<_>>()
                .join(", "),
        })?;
    let clusters = soc_cpuclusters(zephyr_base, soc_name);
    let valid = valid_targets(&board, soc, &clusters);

    let mut requested = id.clone();
    requested.revision = None;
    let requested = requested.to_string();
    let accepted = valid.contains(&requested)
        || (clusters.is_empty() && requested == format!("{}/{}", board.name, soc_name));
    if accepted {
        Ok(())
    } else {
        Err(BoardError::InvalidTarget {
            board: target.board.clone(),
            valid: valid.join(", "),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::resolve_static;

    fn fixture() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        let write = |rel: &str, text: &str| {
            let path = base.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        };
        write(
            "boards/espressif/esp32s3_devkitc/board.yml",
            "board:\n  name: esp32s3_devkitc\n  vendor: espressif\n  socs:\n  - name: esp32s3\n",
        );
        write(
            "boards/espressif/esp32c3_devkitm/board.yml",
            "board:\n  name: esp32c3_devkitm\n  socs:\n  - name: esp32c3\n",
        );
        write(
            "boards/seeed/xiao_esp32s3/board.yml",
            "board:\n  name: xiao_esp32s3\n  socs:\n  - name: esp32s3\n    variants:\n    - name: 'sense'\n      cpucluster: 'procpu'\n",
        );
        write(
            "boards/other/multi/board.yml",
            "boards:\n- name: some_other_board\n  socs:\n  - name: foo\n",
        );
        write(
            "soc/espressif/soc.yml",
            "family:\n- name: espressif_esp32\n  series:\n  - name: esp32s3\n    socs:\n    - name: esp32s3\n      cpuclusters:\n      - name: procpu\n      - name: appcpu\n  - name: esp32c3\n    socs:\n    - name: esp32c3\n",
        );
        tmp
    }

    fn check(base: &Path, board: &str) -> Result<(), BoardError> {
        verify(base, &resolve_static(board, None).unwrap(), "v4.2.0")
    }

    #[test]
    fn accepts_valid_targets() {
        let tmp = fixture();
        check(tmp.path(), "esp32s3_devkitc/esp32s3/procpu").unwrap();
        check(tmp.path(), "esp32s3_devkitc/esp32s3/appcpu").unwrap();
        check(tmp.path(), "esp32c3_devkitm").unwrap();
        check(tmp.path(), "esp32c3_devkitm/esp32c3").unwrap();
        check(tmp.path(), "xiao_esp32s3/esp32s3/procpu/sense").unwrap();
    }

    #[test]
    fn rejects_missing_cpucluster_and_unknown_boards() {
        let tmp = fixture();
        assert!(matches!(
            check(tmp.path(), "esp32s3_devkitc/esp32s3"),
            Err(BoardError::InvalidTarget { .. })
        ));
        assert!(matches!(
            check(tmp.path(), "esp32s3_devkitc/esp32s3/lpcore"),
            Err(BoardError::InvalidTarget { .. })
        ));
        assert!(matches!(
            check(tmp.path(), "esp32h2_devkitm"),
            Err(BoardError::NotInZephyr { .. })
        ));
    }

    #[test]
    fn lists_valid_targets_with_variants() {
        let tmp = fixture();
        let board = find_board(tmp.path(), "xiao_esp32s3").unwrap();
        let clusters = soc_cpuclusters(tmp.path(), "esp32s3");
        assert_eq!(
            valid_targets(&board, &board.socs[0], &clusters),
            [
                "xiao_esp32s3/esp32s3/procpu",
                "xiao_esp32s3/esp32s3/procpu/sense",
                "xiao_esp32s3/esp32s3/appcpu"
            ]
        );
    }
}
