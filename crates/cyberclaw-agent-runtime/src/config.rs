//! Agent configuration types.

use cyberclaw_core::ids::{AgentId, SkillId};
use cyberclaw_core::validation::{Validate, ValidationError, ValidationResult};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Configuration for an individual agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Unique identifier for this agent.
    pub agent_id: AgentId,
    /// Human-readable name.
    pub name: String,
    /// Brief description of what this agent does.
    pub description: String,
    /// Arbitrary configuration parameters for the agent.
    #[serde(default)]
    pub params: HashMap<String, serde_json::Value>,
    /// Sprint 44+1 — default skill bindings activated when this agent
    /// runs. Mirrors `manifests::AgentSpec::default_skills` but uses the
    /// validated [`SkillId`] type so the runtime can pass them straight
    /// into [`ExecutionContext`] / `AgenticLoop::active_skill_bindings`.
    ///
    /// `#[serde(default)]` keeps backward compat: legacy persisted
    /// `AgentConfig` JSON deserializes with an empty vector.
    ///
    /// [`ExecutionContext`]: cyberclaw_core::execution::ExecutionContext
    #[serde(default)]
    pub default_skills: Vec<SkillId>,
}

impl AgentConfig {
    /// Create a new agent config with minimal fields.
    pub fn new(agent_id: AgentId, name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            agent_id,
            name: name.into(),
            description: description.into(),
            params: HashMap::new(),
            default_skills: Vec::new(),
        }
    }

    /// Add a parameter to the configuration.
    pub fn with_param(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.params.insert(key.into(), value);
        self
    }

    /// Sprint 44+1 — builder helper to attach default skill bindings.
    pub fn with_default_skills(mut self, skills: Vec<SkillId>) -> Self {
        self.default_skills = skills;
        self
    }
}

impl Validate for AgentConfig {
    fn validate(&self) -> ValidationResult {
        if self.name.is_empty() {
            return Err(ValidationError::new("agent name cannot be empty"));
        }
        if self.name.len() > 256 {
            return Err(ValidationError::new(format!(
                "agent name too long: {} chars (max 256)",
                self.name.len()
            )));
        }
        if self.description.len() > 1024 {
            return Err(ValidationError::new(format!(
                "agent description too long: {} chars (max 1024)",
                self.description.len()
            )));
        }
        Ok(())
    }
}

/// Runtime-level configuration controlling concurrency and timeouts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeConfig {
    /// Maximum number of agents that can execute concurrently.
    pub max_concurrent_agents: usize,
    /// Per-request execution timeout in seconds.
    pub request_timeout_secs: u64,
    /// Maximum number of items kept in the in-memory execution log.
    pub max_log_entries: usize,
}

impl RuntimeConfig {
    /// Create a `RuntimeConfig` with sensible production defaults.
    pub fn default_config() -> Self {
        Self {
            max_concurrent_agents: 16,
            request_timeout_secs: 30,
            max_log_entries: 10_000,
        }
    }
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self::default_config()
    }
}

impl Validate for RuntimeConfig {
    fn validate(&self) -> ValidationResult {
        if self.max_concurrent_agents == 0 {
            return Err(ValidationError::new("max_concurrent_agents must be > 0"));
        }
        if self.max_concurrent_agents > 1024 {
            return Err(ValidationError::new(format!(
                "max_concurrent_agents too large: {} (max 1024)",
                self.max_concurrent_agents
            )));
        }
        if self.request_timeout_secs == 0 {
            return Err(ValidationError::new("request_timeout_secs must be > 0"));
        }
        if self.request_timeout_secs > 3600 {
            return Err(ValidationError::new(format!(
                "request_timeout_secs too large: {} (max 3600)",
                self.request_timeout_secs
            )));
        }
        if self.max_log_entries == 0 {
            return Err(ValidationError::new("max_log_entries must be > 0"));
        }
        Ok(())
    }
}

/// Network service configuration for host / port bindings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceConfig {
    /// TCP port to bind to. Must be >= 1024 (non-privileged).
    pub port: u16,
    /// Host address or hostname to bind to.
    pub host: String,
    /// Maximum number of simultaneous connections.
    pub max_connections: usize,
    /// Connection / request timeout in seconds.
    pub timeout_secs: u64,
}

impl ServiceConfig {
    /// Create a `ServiceConfig` and immediately validate the result.
    ///
    /// # Errors
    /// Returns a [`ValidationError`] when `port < 1024` or `host` is empty.
    pub fn new(port: u16, host: impl Into<String>) -> Result<Self, ValidationError> {
        let config = Self {
            port,
            host: host.into(),
            max_connections: 100,
            timeout_secs: 30,
        };
        config.validate()?;
        Ok(config)
    }
}

impl Validate for ServiceConfig {
    fn validate(&self) -> ValidationResult {
        if self.port < 1024 {
            return Err(ValidationError::new(format!(
                "port must be >= 1024, got {}",
                self.port
            )));
        }
        if self.host.is_empty() {
            return Err(ValidationError::new("host cannot be empty"));
        }
        if self.max_connections == 0 {
            return Err(ValidationError::new("max_connections must be > 0"));
        }
        if self.timeout_secs == 0 {
            return Err(ValidationError::new("timeout_secs must be > 0"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent_id(s: &str) -> AgentId {
        AgentId::from_string(s.to_owned()).expect("valid agent id")
    }

    // ---- AgentConfig ----

    #[test]
    fn agent_config_valid() {
        let cfg = AgentConfig::new(agent_id("agent-1"), "My Agent", "Does things");
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn agent_config_empty_name_is_invalid() {
        let cfg = AgentConfig::new(agent_id("agent-1"), "", "description");
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("name cannot be empty"));
    }

    #[test]
    fn agent_config_name_too_long_is_invalid() {
        let long_name = "a".repeat(257);
        let cfg = AgentConfig::new(agent_id("agent-1"), long_name, "desc");
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("name too long"));
    }

    #[test]
    fn agent_config_description_too_long_is_invalid() {
        let long_desc = "d".repeat(1025);
        let cfg = AgentConfig::new(agent_id("agent-1"), "name", long_desc);
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("description too long"));
    }

    #[test]
    fn agent_config_default_skills_empty_by_default() {
        let cfg = AgentConfig::new(agent_id("agent-1"), "My Agent", "desc");
        assert!(cfg.default_skills.is_empty());
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn agent_config_with_default_skills_carries_bindings() {
        let sk = SkillId::from_string("powerpoint".to_string()).expect("valid skill id");
        let cfg =
            AgentConfig::new(agent_id("agent-1"), "My Agent", "desc").with_default_skills(vec![sk]);
        assert_eq!(cfg.default_skills.len(), 1);
        assert_eq!(cfg.default_skills[0].as_str(), "powerpoint");
    }

    #[test]
    fn agent_config_serde_default_skills_legacy_payload() {
        // Sprint 44+1 backward-compat: a JSON payload from before the
        // field existed must deserialize with default_skills = [].
        let legacy = r#"{
            "agent_id": "agent-1",
            "name": "Legacy",
            "description": "old shape"
        }"#;
        let cfg: AgentConfig = serde_json::from_str(legacy).expect("deserialize legacy");
        assert!(cfg.default_skills.is_empty());
    }

    #[test]
    fn agent_config_serde_roundtrip_with_default_skills() {
        let sk = SkillId::from_string("powerpoint".to_string()).expect("valid skill id");
        let cfg =
            AgentConfig::new(agent_id("agent-1"), "Skilled", "desc").with_default_skills(vec![sk]);
        let json = serde_json::to_string(&cfg).expect("serialize");
        let back: AgentConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.default_skills.len(), 1);
        assert_eq!(back.default_skills[0].as_str(), "powerpoint");
    }

    // ---- RuntimeConfig ----

    #[test]
    fn runtime_config_default_is_valid() {
        assert!(RuntimeConfig::default().validate().is_ok());
    }

    #[test]
    fn runtime_config_zero_concurrent_agents_is_invalid() {
        let cfg = RuntimeConfig {
            max_concurrent_agents: 0,
            ..RuntimeConfig::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(err
            .to_string()
            .contains("max_concurrent_agents must be > 0"));
    }

    #[test]
    fn runtime_config_too_many_concurrent_agents_is_invalid() {
        let cfg = RuntimeConfig {
            max_concurrent_agents: 2000,
            ..RuntimeConfig::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("max_concurrent_agents too large"));
    }

    #[test]
    fn runtime_config_zero_timeout_is_invalid() {
        let cfg = RuntimeConfig {
            request_timeout_secs: 0,
            ..RuntimeConfig::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("request_timeout_secs must be > 0"));
    }

    #[test]
    fn runtime_config_timeout_too_large_is_invalid() {
        let cfg = RuntimeConfig {
            request_timeout_secs: 7200,
            ..RuntimeConfig::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("request_timeout_secs too large"));
    }

    #[test]
    fn runtime_config_zero_log_entries_is_invalid() {
        let cfg = RuntimeConfig {
            max_log_entries: 0,
            ..RuntimeConfig::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("max_log_entries must be > 0"));
    }

    // ---- ServiceConfig ----

    #[test]
    fn service_config_new_valid() {
        let cfg = ServiceConfig::new(8080, "127.0.0.1").unwrap();
        assert_eq!(cfg.port, 8080);
        assert_eq!(cfg.host, "127.0.0.1");
    }

    #[test]
    fn service_config_privileged_port_rejected() {
        let err = ServiceConfig::new(80, "localhost").unwrap_err();
        assert!(err.to_string().contains("port must be >= 1024"));
    }

    #[test]
    fn service_config_port_1023_rejected() {
        let err = ServiceConfig::new(1023, "localhost").unwrap_err();
        assert!(err.to_string().contains("port must be >= 1024"));
    }

    #[test]
    fn service_config_port_1024_accepted() {
        assert!(ServiceConfig::new(1024, "localhost").is_ok());
    }

    #[test]
    fn service_config_empty_host_rejected() {
        let cfg = ServiceConfig {
            port: 8080,
            host: String::new(),
            max_connections: 10,
            timeout_secs: 30,
        };
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("host cannot be empty"));
    }

    #[test]
    fn service_config_zero_connections_rejected() {
        let cfg = ServiceConfig {
            port: 8080,
            host: "localhost".to_owned(),
            max_connections: 0,
            timeout_secs: 30,
        };
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("max_connections must be > 0"));
    }

    #[test]
    fn service_config_zero_timeout_rejected() {
        let cfg = ServiceConfig {
            port: 8080,
            host: "localhost".to_owned(),
            max_connections: 10,
            timeout_secs: 0,
        };
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("timeout_secs must be > 0"));
    }
}
