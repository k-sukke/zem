use std::collections::BTreeSet;

use crate::board::ResolvedTarget;
use crate::config::{Feature, ZemConfig};

/// West project name (in Zephyr's west.yml) providing a feature.
pub fn module_for(feature: Feature) -> &'static str {
    match feature {
        Feature::Mbedtls => "mbedtls",
        Feature::Lvgl => "lvgl",
        Feature::Littlefs => "littlefs",
        Feature::Mcuboot => "mcuboot",
        Feature::Segger => "segger",
    }
}

/// Modules this project builds with; a subset of what the shared workspace contains.
pub fn project_modules(config: &ZemConfig, target: &ResolvedTarget) -> BTreeSet<String> {
    target
        .hal_modules
        .iter()
        .cloned()
        .chain(
            config
                .modules
                .features
                .iter()
                .map(|f| module_for(*f).to_string()),
        )
        .chain(config.modules.extra.iter().cloned())
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::config::{parse, validate};

    #[test]
    fn combines_hal_features_and_extra() {
        let text = r#"
schema = 1
[project]
name = "demo"
[zephyr]
version = "v4.2.0"
[target]
board = "esp32c3_devkitm"
[modules]
features = ["mbedtls"]
extra = ["littlefs", "hal_espressif"]
"#;
        let cfg = parse(Path::new("zem.toml"), text).unwrap();
        let target = validate(&cfg).unwrap().target;
        let modules: Vec<_> = project_modules(&cfg, &target).into_iter().collect();
        assert_eq!(modules, ["hal_espressif", "littlefs", "mbedtls"]);
    }
}
