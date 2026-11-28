use std::fmt;

/// A Zephyr SoC name (`esp32s3`, `nrf52840`, `stm32f401xe`, ...).
///
/// SoCs are an open set defined by the Zephyr tree, so this is a name rather than an enum.
/// Values are only created inside `board`, from a board qualifier, `soc.yml` or a vendor
/// table, so a `SocId` always comes from somewhere that names a SoC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SocId(String);

impl SocId {
    pub(super) fn new(name: impl Into<String>) -> Self {
        SocId(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SocId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
