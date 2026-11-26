use std::fmt;

use super::BoardError;

const MAX_QUALIFIERS: usize = 3;

/// A Zephyr HWMv2 board target: `name[@revision][/soc[/cpucluster[/variant]]]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardId {
    pub name: String,
    pub revision: Option<String>,
    pub qualifiers: Vec<String>,
}

impl BoardId {
    pub fn parse(input: &str) -> Result<BoardId, BoardError> {
        let invalid = |reason| BoardError::Invalid {
            board: input.to_string(),
            reason,
        };

        let s = input.trim();
        if s.is_empty() {
            return Err(invalid("empty"));
        }
        let mut segments = s.split('/');
        let head = segments.next().unwrap_or_default();
        let qualifiers: Vec<String> = segments.map(str::to_string).collect();
        if qualifiers.len() > MAX_QUALIFIERS {
            return Err(invalid("too many qualifiers"));
        }

        let (name, revision) = match head.split_once('@') {
            Some((name, rev)) => (name, Some(rev)),
            None => (head, None),
        };
        if !is_identifier(name) || !name.starts_with(|c: char| c.is_ascii_alphanumeric()) {
            return Err(invalid(
                "the board name must be alphanumeric with `_`, `-` or `.`",
            ));
        }
        if revision.is_some_and(|r| !is_identifier(r)) {
            return Err(invalid("invalid revision"));
        }
        if qualifiers.iter().any(|q| !is_identifier(q)) {
            return Err(invalid("qualifiers must be non-empty and alphanumeric"));
        }

        Ok(BoardId {
            name: name.to_string(),
            revision: revision.map(str::to_string),
            qualifiers,
        })
    }

    pub fn soc(&self) -> Option<&str> {
        self.qualifiers.first().map(String::as_str)
    }

    pub fn with_qualifiers(&self, qualifiers: &str) -> BoardId {
        BoardId {
            qualifiers: qualifiers.split('/').map(str::to_string).collect(),
            ..self.clone()
        }
    }
}

impl fmt::Display for BoardId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)?;
        if let Some(rev) = &self.revision {
            write!(f, "@{rev}")?;
        }
        for q in &self.qualifiers {
            write!(f, "/{q}")?;
        }
        Ok(())
    }
}

fn is_identifier(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bare_name() {
        let id = BoardId::parse("esp32c3_devkitm").unwrap();
        assert_eq!(id.name, "esp32c3_devkitm");
        assert_eq!(id.revision, None);
        assert!(id.qualifiers.is_empty());
        assert_eq!(id.soc(), None);
    }

    #[test]
    fn parses_full_qualifiers() {
        let id = BoardId::parse("esp32s3_devkitc/esp32s3/procpu").unwrap();
        assert_eq!(id.soc(), Some("esp32s3"));
        assert_eq!(id.qualifiers, ["esp32s3", "procpu"]);
        assert_eq!(id.to_string(), "esp32s3_devkitc/esp32s3/procpu");
    }

    #[test]
    fn parses_revision() {
        let id = BoardId::parse("  esp32_devkitc@1.0.0/esp32/appcpu ").unwrap();
        assert_eq!(id.revision.as_deref(), Some("1.0.0"));
        assert_eq!(id.to_string(), "esp32_devkitc@1.0.0/esp32/appcpu");
    }

    #[test]
    fn rejects_malformed_names() {
        for bad in [
            "",
            "   ",
            "esp32//procpu",
            "esp32/",
            "_esp32",
            "esp32 devkit",
            "a/b/c/d/e",
            "x@",
        ] {
            assert!(BoardId::parse(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn with_qualifiers_keeps_revision() {
        let id = BoardId::parse("esp32_devkitc@1.0.0").unwrap();
        assert_eq!(
            id.with_qualifiers("esp32/procpu").to_string(),
            "esp32_devkitc@1.0.0/esp32/procpu"
        );
    }
}
