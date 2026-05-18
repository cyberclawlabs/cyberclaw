//! SkillValidator — pure-Rust validation of SKILL.md frontmatter.
//!
//! Ported from `claude-skill/skills/skill-creator/scripts/quick_validate.py`.
//! No Python subprocess, no shelling out. Input is a YAML frontmatter string
//! (already split from the surrounding SKILL.md body by the caller).
//!
//! # Rules (1:1 with quick_validate.py)
//!
//! - Allowed top-level keys: `{name, description, license, allowed-tools,
//!   metadata, compatibility}`.
//! - `name` required, kebab-case (`^[a-z0-9-]+$`), no leading/trailing/consecutive
//!   hyphens, ≤ 64 chars.
//! - `description` required, ≤ 1024 chars, no `<` or `>`.
//! - `compatibility` optional string, ≤ 500 chars.
//!
//! The validator never reads from disk and never executes anything — it is a
//! pure function on frontmatter text. A separate `validate_skill_file` helper
//! is provided for the file-on-disk variant to cover the `quick_validate.py`
//! surface area, but it is implemented entirely in terms of the pure function.

use std::collections::HashSet;
use std::path::Path;

use thiserror::Error;

/// Allowed top-level keys in SKILL.md frontmatter.
pub const ALLOWED_FRONTMATTER_KEYS: &[&str] = &[
    "name",
    "description",
    "license",
    "allowed-tools",
    "metadata",
    "compatibility",
];

/// Max allowed length for the `name` field.
pub const MAX_NAME_LEN: usize = 64;
/// Max allowed length for the `description` field.
pub const MAX_DESCRIPTION_LEN: usize = 1024;
/// Max allowed length for the `compatibility` field.
pub const MAX_COMPATIBILITY_LEN: usize = 500;

/// Validation errors produced by [`validate_skill_frontmatter`].
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum ValidationError {
    /// YAML failed to parse.
    #[error("invalid YAML in frontmatter: {0}")]
    InvalidYaml(String),
    /// Frontmatter root is not a YAML mapping.
    #[error("frontmatter must be a YAML dictionary")]
    NotMapping,
    /// A top-level key is not in the allowed set.
    #[error("unexpected key(s) in SKILL.md frontmatter: {unexpected}. allowed: {allowed}")]
    UnexpectedKey {
        /// Offending keys, comma-separated.
        unexpected: String,
        /// Allowed keys, comma-separated.
        allowed: String,
    },
    /// Required field missing.
    #[error("missing required field `{0}` in frontmatter")]
    MissingField(&'static str),
    /// Value has the wrong type.
    #[error("field `{field}` must be a string, got {actual}")]
    WrongType {
        /// Field that was wrongly typed.
        field: &'static str,
        /// Actual YAML tag name observed.
        actual: &'static str,
    },
    /// `name` doesn't match kebab-case rules.
    #[error("name `{0}` should be kebab-case (lowercase letters, digits, and hyphens only)")]
    NameNotKebabCase(String),
    /// `name` has invalid hyphen placement.
    #[error("name `{0}` cannot start/end with hyphen or contain consecutive hyphens")]
    NameBadHyphens(String),
    /// A length budget was exceeded.
    #[error("field `{field}` too long: {len} chars (max {max})")]
    TooLong {
        /// Which field.
        field: &'static str,
        /// Observed length.
        len: usize,
        /// Upper bound.
        max: usize,
    },
    /// `description` contains angle brackets.
    #[error("description cannot contain angle brackets (< or >)")]
    DescriptionAngleBrackets,
    /// File-level: SKILL.md not present at the given path.
    #[error("SKILL.md not found at {0}")]
    SkillMdNotFound(String),
    /// File-level: the SKILL.md has no frontmatter block.
    #[error("SKILL.md has no YAML frontmatter")]
    NoFrontmatter,
}

/// Validate a YAML frontmatter string against the SKILL.md rules.
///
/// The `yaml` input is just the text **between** the two `---` markers of a
/// SKILL.md — callers that have a full SKILL.md should use
/// [`validate_skill_file`] or split the frontmatter themselves.
///
/// Returns `Ok(())` on success, or a [`ValidationError`] describing the first
/// violation encountered.
pub fn validate_skill_frontmatter(yaml: &str) -> Result<(), ValidationError> {
    let root: serde_yaml::Value =
        serde_yaml::from_str(yaml).map_err(|e| ValidationError::InvalidYaml(e.to_string()))?;

    let map = match root {
        serde_yaml::Value::Mapping(m) => m,
        _ => return Err(ValidationError::NotMapping),
    };

    // 1. Unexpected-key check.
    let allowed: HashSet<&str> = ALLOWED_FRONTMATTER_KEYS.iter().copied().collect();
    let mut unexpected: Vec<String> = Vec::new();
    for (k, _) in map.iter() {
        let key_str = match k {
            serde_yaml::Value::String(s) => s.as_str(),
            _ => continue,
        };
        if !allowed.contains(key_str) {
            unexpected.push(key_str.to_string());
        }
    }
    if !unexpected.is_empty() {
        unexpected.sort();
        let mut allowed_sorted: Vec<&str> = allowed.iter().copied().collect();
        allowed_sorted.sort();
        return Err(ValidationError::UnexpectedKey {
            unexpected: unexpected.join(", "),
            allowed: allowed_sorted.join(", "),
        });
    }

    // 2. Required fields.
    let name_val = map
        .get(serde_yaml::Value::String("name".to_string()))
        .ok_or(ValidationError::MissingField("name"))?;
    let description_val = map
        .get(serde_yaml::Value::String("description".to_string()))
        .ok_or(ValidationError::MissingField("description"))?;

    // 3. name rules.
    let name = as_string_field("name", name_val)?;
    let name = name.trim();
    if !name.is_empty() {
        if !is_kebab_case(name) {
            return Err(ValidationError::NameNotKebabCase(name.to_string()));
        }
        if name.starts_with('-') || name.ends_with('-') || name.contains("--") {
            return Err(ValidationError::NameBadHyphens(name.to_string()));
        }
        if name.len() > MAX_NAME_LEN {
            return Err(ValidationError::TooLong {
                field: "name",
                len: name.len(),
                max: MAX_NAME_LEN,
            });
        }
    }

    // 4. description rules.
    let description = as_string_field("description", description_val)?;
    let description = description.trim();
    if !description.is_empty() {
        if description.contains('<') || description.contains('>') {
            return Err(ValidationError::DescriptionAngleBrackets);
        }
        if description.len() > MAX_DESCRIPTION_LEN {
            return Err(ValidationError::TooLong {
                field: "description",
                len: description.len(),
                max: MAX_DESCRIPTION_LEN,
            });
        }
    }

    // 5. compatibility (optional).
    if let Some(v) = map.get(serde_yaml::Value::String("compatibility".to_string())) {
        // Empty string and missing compatibility both count as "not present".
        if !matches!(v, serde_yaml::Value::Null) {
            let compat = as_string_field("compatibility", v)?;
            if !compat.is_empty() && compat.len() > MAX_COMPATIBILITY_LEN {
                return Err(ValidationError::TooLong {
                    field: "compatibility",
                    len: compat.len(),
                    max: MAX_COMPATIBILITY_LEN,
                });
            }
        }
    }

    Ok(())
}

/// Validate a SKILL.md on disk. Reads the file, splits the frontmatter, and
/// delegates to [`validate_skill_frontmatter`].
///
/// The caller provides the path to the **directory** holding the SKILL.md —
/// this matches the Python `validate_skill(skill_path)` signature.
pub fn validate_skill_file(skill_dir: &Path) -> Result<(), ValidationError> {
    let skill_md = skill_dir.join("SKILL.md");
    if !skill_md.exists() {
        return Err(ValidationError::SkillMdNotFound(
            skill_md.to_string_lossy().to_string(),
        ));
    }
    let content = std::fs::read_to_string(&skill_md)
        .map_err(|e| ValidationError::InvalidYaml(e.to_string()))?;
    let yaml = split_frontmatter(&content)?;
    validate_skill_frontmatter(yaml)
}

/// Split a SKILL.md into its frontmatter YAML block. Returns the YAML text
/// (between the two `---` markers), not the body.
pub fn split_frontmatter(skill_md: &str) -> Result<&str, ValidationError> {
    let trimmed = skill_md.trim_start();
    if !trimmed.starts_with("---") {
        return Err(ValidationError::NoFrontmatter);
    }
    let after_open = &trimmed[3..];
    let after_open = after_open.strip_prefix('\n').unwrap_or(after_open);

    // Find the next `---` on its own line.
    let mut cursor = 0usize;
    loop {
        let line_end = match after_open[cursor..].find('\n') {
            Some(n) => cursor + n,
            None => {
                // Possibly no trailing newline: check if the remaining slice IS `---`.
                if after_open[cursor..].trim_end() == "---" {
                    return Ok(&after_open[..cursor]);
                }
                return Err(ValidationError::NoFrontmatter);
            }
        };
        let line = after_open[cursor..line_end].trim_end();
        if line == "---" {
            return Ok(&after_open[..cursor]);
        }
        cursor = line_end + 1;
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn as_string_field<'a>(
    field: &'static str,
    v: &'a serde_yaml::Value,
) -> Result<&'a str, ValidationError> {
    match v {
        serde_yaml::Value::String(s) => Ok(s.as_str()),
        serde_yaml::Value::Null => Err(ValidationError::WrongType {
            field,
            actual: "null",
        }),
        serde_yaml::Value::Bool(_) => Err(ValidationError::WrongType {
            field,
            actual: "bool",
        }),
        serde_yaml::Value::Number(_) => Err(ValidationError::WrongType {
            field,
            actual: "number",
        }),
        serde_yaml::Value::Sequence(_) => Err(ValidationError::WrongType {
            field,
            actual: "sequence",
        }),
        serde_yaml::Value::Mapping(_) => Err(ValidationError::WrongType {
            field,
            actual: "mapping",
        }),
        serde_yaml::Value::Tagged(_) => Err(ValidationError::WrongType {
            field,
            actual: "tagged",
        }),
    }
}

fn is_kebab_case(s: &str) -> bool {
    s.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_accepts_standard_frontmatter() {
        let yaml = "name: my-skill\ndescription: does things for users\n";
        assert!(validate_skill_frontmatter(yaml).is_ok());
    }

    #[test]
    fn validate_accepts_all_allowed_fields() {
        let yaml = r#"
name: my-skill
description: does things
license: Apache-2.0
allowed-tools:
  - Read
  - Write
metadata:
  author: someone
compatibility: claude-3.5-sonnet
"#;
        assert!(
            validate_skill_frontmatter(yaml).is_ok(),
            "got: {:?}",
            validate_skill_frontmatter(yaml)
        );
    }

    #[test]
    fn validate_rejects_non_kebab_case_name() {
        let yaml = "name: MySkill\ndescription: desc\n";
        let err = validate_skill_frontmatter(yaml).unwrap_err();
        assert!(
            matches!(err, ValidationError::NameNotKebabCase(_)),
            "got: {err:?}"
        );
    }

    #[test]
    fn validate_rejects_name_with_consecutive_hyphens() {
        let yaml = "name: my--skill\ndescription: desc\n";
        let err = validate_skill_frontmatter(yaml).unwrap_err();
        assert!(
            matches!(err, ValidationError::NameBadHyphens(_)),
            "got: {err:?}"
        );
    }

    #[test]
    fn validate_rejects_oversized_description() {
        let huge = "a".repeat(MAX_DESCRIPTION_LEN + 1);
        let yaml = format!("name: s\ndescription: {huge}\n");
        let err = validate_skill_frontmatter(&yaml).unwrap_err();
        assert!(
            matches!(
                err,
                ValidationError::TooLong {
                    field: "description",
                    ..
                }
            ),
            "got: {err:?}"
        );
    }

    #[test]
    fn validate_rejects_description_angle_brackets() {
        let yaml = "name: s\ndescription: has <bracket>\n";
        let err = validate_skill_frontmatter(yaml).unwrap_err();
        assert!(
            matches!(err, ValidationError::DescriptionAngleBrackets),
            "got: {err:?}"
        );
    }

    #[test]
    fn validate_rejects_unknown_frontmatter_field() {
        let yaml = "name: s\ndescription: d\nunknown_field: x\n";
        let err = validate_skill_frontmatter(yaml).unwrap_err();
        assert!(
            matches!(err, ValidationError::UnexpectedKey { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn validate_rejects_missing_name() {
        let yaml = "description: x\n";
        let err = validate_skill_frontmatter(yaml).unwrap_err();
        assert!(
            matches!(err, ValidationError::MissingField("name")),
            "got: {err:?}"
        );
    }

    #[test]
    fn validate_rejects_missing_description() {
        let yaml = "name: s\n";
        let err = validate_skill_frontmatter(yaml).unwrap_err();
        assert!(
            matches!(err, ValidationError::MissingField("description")),
            "got: {err:?}"
        );
    }

    #[test]
    fn validate_rejects_oversized_name() {
        let huge = "a".repeat(MAX_NAME_LEN + 1);
        let yaml = format!("name: {huge}\ndescription: d\n");
        let err = validate_skill_frontmatter(&yaml).unwrap_err();
        assert!(
            matches!(err, ValidationError::TooLong { field: "name", .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn validate_rejects_oversized_compatibility() {
        let huge = "a".repeat(MAX_COMPATIBILITY_LEN + 1);
        let yaml = format!("name: s\ndescription: d\ncompatibility: {huge}\n");
        let err = validate_skill_frontmatter(&yaml).unwrap_err();
        assert!(
            matches!(
                err,
                ValidationError::TooLong {
                    field: "compatibility",
                    ..
                }
            ),
            "got: {err:?}"
        );
    }

    #[test]
    fn validate_rejects_invalid_yaml() {
        let yaml = "name: s\ndescription: [unclosed\n";
        let err = validate_skill_frontmatter(yaml).unwrap_err();
        assert!(
            matches!(err, ValidationError::InvalidYaml(_)),
            "got: {err:?}"
        );
    }

    #[test]
    fn split_frontmatter_happy() {
        let md = "---\nname: s\ndescription: d\n---\nbody here\n";
        let fm = split_frontmatter(md).expect("split ok");
        assert!(fm.contains("name: s"));
        assert!(fm.contains("description: d"));
        assert!(!fm.contains("body here"));
    }

    #[test]
    fn split_frontmatter_errors_without_opening_marker() {
        let md = "name: s\n";
        let err = split_frontmatter(md).unwrap_err();
        assert!(
            matches!(err, ValidationError::NoFrontmatter),
            "got: {err:?}"
        );
    }

    #[test]
    fn validate_skill_file_missing() {
        let dir = tempfile::tempdir().expect("tmp");
        let err = validate_skill_file(dir.path()).unwrap_err();
        assert!(
            matches!(err, ValidationError::SkillMdNotFound(_)),
            "got: {err:?}"
        );
    }

    #[test]
    fn validate_skill_file_happy() {
        let dir = tempfile::tempdir().expect("tmp");
        let md = "---\nname: ok\ndescription: fine\n---\nbody\n";
        std::fs::write(dir.path().join("SKILL.md"), md).expect("write");
        assert!(validate_skill_file(dir.path()).is_ok());
    }
}
