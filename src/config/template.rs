use super::Feature;

pub struct InitParams<'a> {
    pub name: &'a str,
    pub board: &'a str,
    pub zephyr_version: &'a str,
    pub features: &'a [Feature],
}

pub fn render(p: &InitParams<'_>) -> String {
    let features: Vec<String> = p.features.iter().map(|f| quote(f.as_str())).collect();
    format!(
        r#"# zem project manifest. Run `zem sync` after editing this file.
schema = 1

[project]
name = {name}

[zephyr]
# Zephyr release tag (v4.0 or later) or a full commit SHA.
version = {version}

[target]
board = {board}

[modules]
# Optional features: mbedtls, lvgl, littlefs, mcuboot, segger.
# The target's HAL module (e.g. hal_espressif) is added automatically.
features = [{features}]
# Additional west project names from Zephyr's west.yml.
extra = []

[blobs]
# Set to true to accept the vendor's binary blob licenses non-interactively.
accept_licenses = false

[python]
version = "3.12"

[build]
dir = "build"
"#,
        name = quote(p.name),
        version = quote(p.zephyr_version),
        board = quote(p.board),
        features = features.join(", "),
    )
}

fn quote(s: &str) -> String {
    toml::Value::String(s.to_string()).to_string()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::config::{parse, validate};

    #[test]
    fn rendered_template_is_valid() {
        let text = render(&InitParams {
            name: "blinky",
            board: "esp32s3_devkitc/esp32s3/procpu",
            zephyr_version: "v4.2.0",
            features: &[Feature::Mbedtls],
        });
        let cfg = parse(Path::new("zem.toml"), &text).unwrap();
        assert_eq!(cfg.modules.features, [Feature::Mbedtls]);
        validate(&cfg).unwrap();
        insta::assert_snapshot!(text);
    }

    #[test]
    fn quotes_special_characters() {
        let text = render(&InitParams {
            name: "a\"b",
            board: "esp32c3_devkitm",
            zephyr_version: "v4.2.0",
            features: &[],
        });
        let cfg = parse(Path::new("zem.toml"), &text).unwrap();
        assert_eq!(cfg.project.name, "a\"b");
    }
}
