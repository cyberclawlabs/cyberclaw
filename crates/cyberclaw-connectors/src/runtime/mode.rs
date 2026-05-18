//! Runtime mode enumeration for capability execution isolation
//!
//! Defines the execution environment for capabilities: native, process, or container.

use serde::{Deserialize, Serialize};

/// Runtime execution mode determining the level of isolation for capability execution
///
/// # Security Model
///
/// - `Native`: No isolation, direct execution in the current process (highest performance, lowest security)
/// - `Process`: Subprocess isolation with resource limits (balanced security/performance)
/// - `Container`: Full container isolation with namespace/cgroup controls (highest security, lower performance)
///
/// # Selection Strategy
///
/// Runtime mode should be selected based on:
/// - Capability risk level (from PolicyEngine governance decision)
/// - Tenant isolation requirements
/// - Performance constraints
///
/// # Examples
///
/// ```
/// use cyberclaw_connectors::runtime::RuntimeMode;
/// use cyberclaw_core::capability::RiskLevel;
///
/// fn select_runtime(risk: RiskLevel) -> RuntimeMode {
///     match risk {
///         RiskLevel::Low => RuntimeMode::Native,
///         RiskLevel::Medium => RuntimeMode::Process,
///         RiskLevel::High | RiskLevel::Critical => RuntimeMode::Container,
///     }
/// }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RuntimeMode {
    /// Execute capability directly in the current process (no isolation)
    ///
    /// **Use case**: Low-risk read-only operations (fs.read, search.grep)
    ///
    /// **Security**: No isolation, shares process memory/file descriptors
    Native,

    /// Execute capability in an isolated subprocess with resource limits
    ///
    /// **Use case**: Medium-risk operations requiring isolation (fs.write, cmd.exec)
    ///
    /// **Security**: Process isolation, timeout enforcement, cwd/env separation
    Process,

    /// Execute capability in a full container runtime (Docker/podman)
    ///
    /// **Use case**: High-risk operations requiring full isolation (package installs, code execution)
    ///
    /// **Security**: Namespace/cgroup isolation, network restrictions, read-only root filesystem
    Container,
}

impl Default for RuntimeMode {
    /// Default runtime mode is Process (balanced security/performance)
    fn default() -> Self {
        RuntimeMode::Process
    }
}

impl RuntimeMode {
    /// Returns true if this runtime mode requires subprocess creation
    pub fn requires_subprocess(&self) -> bool {
        matches!(self, RuntimeMode::Process | RuntimeMode::Container)
    }

    /// Returns true if this runtime mode provides full filesystem isolation
    pub fn has_filesystem_isolation(&self) -> bool {
        matches!(self, RuntimeMode::Container)
    }

    /// Returns true if this runtime mode provides network isolation
    pub fn has_network_isolation(&self) -> bool {
        matches!(self, RuntimeMode::Container)
    }

    /// Returns the recommended mode for a given risk level
    ///
    /// # Mapping
    ///
    /// - `Low` → `Native`
    /// - `Medium` → `Process`
    /// - `High` → `Container`
    /// - `Critical` → `Container`
    pub fn from_risk_level(risk: cyberclaw_core::capability::RiskLevel) -> Self {
        use cyberclaw_core::capability::RiskLevel;
        match risk {
            RiskLevel::Low => RuntimeMode::Native,
            RiskLevel::Medium => RuntimeMode::Process,
            RiskLevel::High | RiskLevel::Critical => RuntimeMode::Container,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyberclaw_core::capability::RiskLevel;

    #[test]
    fn test_default_mode_is_process() {
        assert_eq!(RuntimeMode::default(), RuntimeMode::Process);
    }

    #[test]
    fn test_requires_subprocess() {
        assert!(!RuntimeMode::Native.requires_subprocess());
        assert!(RuntimeMode::Process.requires_subprocess());
        assert!(RuntimeMode::Container.requires_subprocess());
    }

    #[test]
    fn test_filesystem_isolation() {
        assert!(!RuntimeMode::Native.has_filesystem_isolation());
        assert!(!RuntimeMode::Process.has_filesystem_isolation());
        assert!(RuntimeMode::Container.has_filesystem_isolation());
    }

    #[test]
    fn test_network_isolation() {
        assert!(!RuntimeMode::Native.has_network_isolation());
        assert!(!RuntimeMode::Process.has_network_isolation());
        assert!(RuntimeMode::Container.has_network_isolation());
    }

    #[test]
    fn test_from_risk_level() {
        assert_eq!(
            RuntimeMode::from_risk_level(RiskLevel::Low),
            RuntimeMode::Native
        );
        assert_eq!(
            RuntimeMode::from_risk_level(RiskLevel::Medium),
            RuntimeMode::Process
        );
        assert_eq!(
            RuntimeMode::from_risk_level(RiskLevel::High),
            RuntimeMode::Container
        );
        assert_eq!(
            RuntimeMode::from_risk_level(RiskLevel::Critical),
            RuntimeMode::Container
        );
    }

    #[test]
    fn test_serde_round_trip() {
        let modes = vec![
            RuntimeMode::Native,
            RuntimeMode::Process,
            RuntimeMode::Container,
        ];
        for mode in modes {
            let json = serde_json::to_string(&mode).unwrap();
            let deserialized: RuntimeMode = serde_json::from_str(&json).unwrap();
            assert_eq!(mode, deserialized);
        }
    }
}
