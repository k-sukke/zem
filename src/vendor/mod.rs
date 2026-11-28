//! What zem knows about each MCU vendor, as data.
//!
//! Vendor-specific values (SoCs, HAL and blob modules, conflicting host SDKs, USB IDs) live
//! in one [`VendorProfile`] per vendor. The rest of zem asks this module instead of naming a
//! vendor, so adding a vendor means adding a profile to [`VENDORS`].

mod espressif;

use crate::board::Arch;

pub const VENDORS: &[&VendorProfile] = &[&espressif::PROFILE];

#[derive(PartialEq, Eq)]
pub struct VendorProfile {
    /// Identifies the SoC vendor (recorded in state.json). Not a board.yml `vendor`, which
    /// names the board maker (`seeed` for `xiao_esp32c3`).
    pub id: &'static str,
    pub socs: &'static [SocInfo],
    /// Boards offered by `zem init` and resolvable without a Zephyr tree; not every board.
    pub boards: &'static [KnownBoard],
    /// West projects every target of this vendor builds with.
    pub hal_modules: &'static [&'static str],
    /// Modules that need `west blobs fetch`.
    pub blob_modules: &'static [&'static str],
    /// Variables set by this vendor's own SDK, removed from zem's child processes.
    pub env_prefixes: &'static [&'static str],
    pub env_vars: &'static [&'static str],
    /// Of those, the ones `zem doctor` warns about when set in the user's shell.
    pub notable_env_vars: &'static [&'static str],
    /// PATH components (compared in lower case) that belong to this vendor's own SDK.
    pub path_markers: &'static [PathMarker],
    /// USB devices shown with a friendly name by `zem doctor`.
    pub usb_ids: &'static [UsbId],
}

/// Only the id: the tables would bury every `ResolvedTarget` printed with `{:?}`.
impl std::fmt::Debug for VendorProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "VendorProfile({})", self.id)
    }
}

impl VendorProfile {
    pub fn soc(&self, name: &str) -> Option<&'static SocInfo> {
        self.socs.iter().find(|soc| soc.name == name)
    }
}

/// What zem knows about a SoC before a Zephyr tree is available.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SocInfo {
    pub name: &'static str,
    pub arch: Arch,
    /// Zephyr SDK toolchain directory / `west sdk install -t` name.
    pub toolchain: &'static str,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathMarker {
    Exact(&'static str),
    Prefix(&'static str),
}

impl PathMarker {
    fn matches(self, component: &str) -> bool {
        match self {
            PathMarker::Exact(name) => component == name,
            PathMarker::Prefix(prefix) => component.starts_with(prefix),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsbId {
    pub vid: u16,
    /// `None` matches every product of the vendor ID.
    pub pid: Option<u16>,
    pub name: &'static str,
}

impl UsbId {
    pub const fn exact(vid: u16, pid: u16, name: &'static str) -> Self {
        UsbId {
            vid,
            pid: Some(pid),
            name,
        }
    }

    pub const fn any(vid: u16, name: &'static str) -> Self {
        UsbId {
            vid,
            pid: None,
            name,
        }
    }

    pub fn matches(&self, vid: u16, pid: u16) -> bool {
        self.vid == vid && self.pid.is_none_or(|p| p == pid)
    }
}

const fn soc(name: &'static str, arch: Arch, toolchain: &'static str) -> SocInfo {
    SocInfo {
        name,
        arch,
        toolchain,
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

/// The vendor and entry of a SoC name.
pub fn find_soc(name: &str) -> Option<(&'static VendorProfile, &'static SocInfo)> {
    VENDORS
        .iter()
        .find_map(|v| v.soc(name).map(|soc| (*v, soc)))
}

/// The vendor and entry of a known board name.
pub fn find_board(name: &str) -> Option<(&'static VendorProfile, &'static KnownBoard)> {
    VENDORS
        .iter()
        .find_map(|v| v.boards.iter().find(|b| b.name == name).map(|b| (*v, b)))
}

/// Every SoC zem knows, for error messages (`esp32, esp32s2, ...`).
pub fn supported_socs() -> String {
    VENDORS
        .iter()
        .flat_map(|v| v.socs)
        .map(|s| s.name)
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn known_boards() -> impl Iterator<Item = &'static KnownBoard> {
    VENDORS.iter().flat_map(|v| v.boards)
}

/// Longest SoC name that the board name equals or starts with followed by `_`
/// (so `esp32s3_devkitc` maps to esp32s3, not esp32).
pub fn soc_from_board_prefix(
    board_name: &str,
) -> Option<(&'static VendorProfile, &'static SocInfo)> {
    VENDORS
        .iter()
        .flat_map(|v| v.socs.iter().map(move |soc| (*v, soc)))
        .filter(|(_, soc)| {
            board_name == soc.name
                || board_name
                    .strip_prefix(soc.name)
                    .is_some_and(|rest| rest.starts_with('_'))
        })
        .max_by_key(|(_, soc)| soc.name.len())
}

/// Variable-name prefixes of every vendor's SDK. Variables of all vendors are removed, not
/// only the target's: an ESP32 project should not see another vendor's SDK either.
pub fn env_prefixes() -> impl Iterator<Item = &'static str> {
    VENDORS.iter().flat_map(|v| v.env_prefixes.iter().copied())
}

pub fn env_vars() -> impl Iterator<Item = &'static str> {
    VENDORS.iter().flat_map(|v| v.env_vars.iter().copied())
}

pub fn notable_env_vars() -> impl Iterator<Item = &'static str> {
    VENDORS
        .iter()
        .flat_map(|v| v.notable_env_vars.iter().copied())
}

/// Whether a PATH component (already lower-cased) belongs to some vendor's SDK.
pub fn is_vendor_path_component(component: &str) -> bool {
    VENDORS
        .iter()
        .flat_map(|v| v.path_markers)
        .any(|m| m.matches(component))
}

/// Friendly name of a vendor's USB device.
pub fn usb_name(vid: u16, pid: u16) -> Option<&'static str> {
    VENDORS
        .iter()
        .flat_map(|v| v.usb_ids)
        .find(|id| id.matches(vid, pid))
        .map(|id| id.name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soc_and_board_names_are_unique_across_vendors() {
        let mut socs: Vec<_> = VENDORS
            .iter()
            .flat_map(|v| v.socs)
            .map(|s| s.name)
            .collect();
        let mut boards: Vec<_> = known_boards().map(|b| b.name).collect();
        let (n_socs, n_boards) = (socs.len(), boards.len());
        socs.sort_unstable();
        socs.dedup();
        boards.sort_unstable();
        boards.dedup();
        assert_eq!(socs.len(), n_socs);
        assert_eq!(boards.len(), n_boards);
        assert_eq!(find_soc("nrf52840"), None);
    }

    #[test]
    fn known_boards_use_their_vendors_socs() {
        for vendor in VENDORS {
            for b in vendor.boards {
                assert!(vendor.soc(b.soc).is_some(), "{}", b.name);
                if let Some(q) = b.default_qualifiers {
                    assert_eq!(q.split('/').next(), Some(b.soc), "{}", b.name);
                }
            }
        }
    }

    #[test]
    fn prefix_prefers_longest_match() {
        let prefix = |board| soc_from_board_prefix(board).map(|(_, s)| s.name);
        assert_eq!(prefix("esp32s3_foo"), Some("esp32s3"));
        assert_eq!(prefix("esp32_foo"), Some("esp32"));
        assert_eq!(prefix("esp32c6"), Some("esp32c6"));
        assert_eq!(prefix("esp32s3foo"), None);
        assert_eq!(prefix("nucleo_f401re"), None);
    }

    #[test]
    fn usb_ids_prefer_listed_order() {
        assert_eq!(usb_name(0x303a, 0x1001), Some("ESP USB-Serial/JTAG"));
        assert_eq!(usb_name(0x303a, 0x0002), Some("Espressif USB"));
        assert_eq!(usb_name(0x1234, 0x0001), None);
    }

    #[test]
    fn path_markers() {
        assert!(is_vendor_path_component(".espressif"));
        assert!(is_vendor_path_component("esp-idf-v5.3"));
        assert!(!is_vendor_path_component("espressif"));
    }
}
