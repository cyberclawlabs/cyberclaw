//! Skill configuration types.

use cyberclaw_core::ids::SkillId;
use cyberclaw_core::validation::{Validate, ValidationError, ValidationResult};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Configuration for a registered skill.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillConfig {
    /// Unique identifier for this skill.
    pub skill_id: SkillId,
    /// Human-readable name.
    pub name: String,
    /// Brief description of what this skill does.
    pub description: String,
    /// Maximum execution time allowed for a single invocation (seconds).
    pub timeout_secs: u64,
    /// Arbitrary skill-specific parameters.
    #[serde(default)]
    pub params: HashMap<String, serde_json::Value>,
}

impl SkillConfig {
    /// Create a new `SkillConfig` with sensible defaults.
    pub fn new(skill_id: SkillId, name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            skill_id,
            name: name.into(),
            description: description.into(),
            timeout_secs: 30,
            params: HashMap::new(),
        }
    }

    /// Override the execution timeout.
    pub fn with_timeout(mut self, timeout_secs: u64) -> Self {
        self.timeout_secs = timeout_secs;
        self
    }

    /// Add an arbitrary parameter.
    pub fn with_param(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.params.insert(key.into(), value);
        self
    }
}

impl Validate for SkillConfig {
    fn validate(&self) -> ValidationResult {
        if self.name.is_empty() {
            return Err(ValidationError::new("skill name cannot be empty"));
        }
        if self.name.len() > 256 {
            return Err(ValidationError::new(format!(
                "skill name too long: {} chars (max 256)",
                self.name.len()
            )));
        }
        if self.description.len() > 1024 {
            return Err(ValidationError::new(format!(
                "skill description too long: {} chars (max 1024)",
                self.description.len()
            )));
        }
        if self.timeout_secs == 0 {
            return Err(ValidationError::new("timeout_secs must be > 0"));
        }
        if self.timeout_secs > 3600 {
            return Err(ValidationError::new(format!(
                "timeout_secs too large: {} (max 3600)",
                self.timeout_secs
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill_id(s: &str) -> SkillId {
        SkillId::from_string(s.to_owned()).expect("valid skill id")
    }

    #[test]
    fn skill_config_valid() {
        let cfg = SkillConfig::new(skill_id("echo"), "Echo", "Echoes input");
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn skill_config_empty_name_is_invalid() {
        let cfg = SkillConfig::new(skill_id("echo"), "", "desc");
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("name cannot be empty"));
    }

    #[test]
    fn skill_config_name_too_long_is_invalid() {
        let long_name = "s".repeat(257);
        let cfg = SkillConfig::new(skill_id("echo"), long_name, "desc");
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("name too long"));
    }

    #[test]
    fn skill_config_description_too_long_is_invalid() {
        let long_desc = "d".repeat(1025);
        let cfg = SkillConfig::new(skill_id("echo"), "Echo", long_desc);
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("description too long"));
    }

    #[test]
    fn skill_config_zero_timeout_is_invalid() {
        let cfg = SkillConfig::new(skill_id("echo"), "Echo", "desc").with_timeout(0);
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("timeout_secs must be > 0"));
    }

    #[test]
    fn skill_config_timeout_too_large_is_invalid() {
        let cfg = SkillConfig::new(skill_id("echo"), "Echo", "desc").with_timeout(7200);
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("timeout_secs too large"));
    }

    #[test]
    fn skill_config_with_params_valid() {
        let cfg = SkillConfig::new(skill_id("calc"), "Calculator", "Arithmetic")
            .with_param("precision", serde_json::json!(4));
        assert!(cfg.validate().is_ok());
        assert_eq!(cfg.params["precision"], serde_json::json!(4));
    }
}
