//! Runtime selection strategy based on capability risk level
//!
//! Implements the runtime selection policy that maps capability risk levels
//! to appropriate execution runtimes (native, process, container).

use super::mode::RuntimeMode;
use cyberclaw_core::capability::RiskLevel;
use cyberclaw_core::manifests::CapabilityContract;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{debug, info};

/// Configuration for runtime selection policy
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeSelectorConfig {
    /// Default runtime selection strategy (if None, use RiskLevel-based mapping)
    pub default_strategy: RuntimeSelectionStrategy,

    /// Per-capability runtime overrides (capability_id → RuntimeMode)
    ///
    /// Allows forcing specific capabilities to use specific runtimes,
    /// overriding the default risk-based selection.
    pub capability_overrides: HashMap<String, RuntimeMode>,

    /// Enable strict mode (require container runtime for Critical capabilities)
    ///
    /// If true, Critical capabilities MUST use Container runtime.
    /// If false, Critical capabilities can fallback to Process runtime if Container unavailable.
    pub strict_mode: bool,
}

impl Default for RuntimeSelectorConfig {
    fn default() -> Self {
        Self {
            default_strategy: RuntimeSelectionStrategy::RiskBased,
            capability_overrides: HashMap::new(),
            strict_mode: true, // Secure by default
        }
    }
}

/// Runtime selection strategy
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuntimeSelectionStrategy {
    /// Always use Native runtime (no isolation) - INSECURE
    AlwaysNative,

    /// Always use Process runtime (subprocess isolation)
    AlwaysProcess,

    /// Always use Container runtime (full isolation)
    AlwaysContainer,

    /// Select runtime based on capability risk level (recommended)
    RiskBased,
}

/// Runtime selector service for capability execution
///
/// # Selection Algorithm
///
/// 1. Check capability-specific override (if configured)
/// 2. If no override, apply default strategy:
///    - `RiskBased`: RiskLevel::Low → Native, Medium → Process, High/Critical → Container
///    - `AlwaysNative`: All capabilities → Native (INSECURE)
///    - `AlwaysProcess`: All capabilities → Process
///    - `AlwaysContainer`: All capabilities → Container
/// 3. Apply strict mode validation (Critical capabilities must use Container)
///
/// # Examples
///
/// ```
/// use cyberclaw_connectors::runtime::{RuntimeSelector, RuntimeSelectorConfig};
/// use cyberclaw_core::capability::RiskLevel;
/// use cyberclaw_core::manifests::CapabilityContract;
///
/// let selector = RuntimeSelector::new(RuntimeSelectorConfig::default());
///
/// let capability = CapabilityContract {
///     id: "cmd.exec".to_string(),
///     risk: RiskLevel::High,
///     // ... other fields
/// #   title: "Test".to_string(),
/// #   description: None,
/// #   input_schema: "Input".to_string(),
/// #   output_schema: "Output".to_string(),
/// #   effects: vec![],
/// #   placement: None,
/// #   timeouts: Default::default(),
/// };
///
/// let mode = selector.select_runtime(&capability);
/// // High risk → Container runtime
/// ```
pub struct RuntimeSelector {
    config: RuntimeSelectorConfig,
}

impl RuntimeSelector {
    /// Create a new runtime selector with the given configuration
    pub fn new(config: RuntimeSelectorConfig) -> Self {
        info!(
            "RuntimeSelector initialized with strategy: {:?}, strict_mode: {}",
            config.default_strategy, config.strict_mode
        );
        Self { config }
    }

    /// Create a runtime selector with default configuration (risk-based, strict mode)
    pub fn default_config() -> Self {
        Self::new(RuntimeSelectorConfig::default())
    }

    /// Select the appropriate runtime for a capability
    ///
    /// # Arguments
    ///
    /// * `capability` - The capability contract containing risk level and metadata
    ///
    /// # Returns
    ///
    /// The selected `RuntimeMode` based on policy and risk level
    pub fn select_runtime(&self, capability: &CapabilityContract) -> RuntimeMode {
        // Step 1: Check for capability-specific override
        if let Some(&mode) = self.config.capability_overrides.get(&capability.id) {
            debug!(
                "Using override runtime for capability {}: {:?}",
                capability.id, mode
            );
            return mode;
        }

        // Step 2: Apply default strategy
        let selected_mode = match self.config.default_strategy {
            RuntimeSelectionStrategy::AlwaysNative => RuntimeMode::Native,
            RuntimeSelectionStrategy::AlwaysProcess => RuntimeMode::Process,
            RuntimeSelectionStrategy::AlwaysContainer => RuntimeMode::Container,
            RuntimeSelectionStrategy::RiskBased => RuntimeMode::from_risk_level(capability.risk),
        };

        // Step 3: Apply strict mode validation
        if self.config.strict_mode
            && capability.risk == RiskLevel::Critical
            && selected_mode != RuntimeMode::Container
        {
            info!(
                "Strict mode: Enforcing Container runtime for Critical capability {}",
                capability.id
            );
            return RuntimeMode::Container;
        }

        debug!(
            "Selected runtime for capability {} (risk: {:?}): {:?}",
            capability.id, capability.risk, selected_mode
        );

        selected_mode
    }

    /// Get the current configuration
    pub fn config(&self) -> &RuntimeSelectorConfig {
        &self.config
    }

    /// Update the configuration (allows runtime reconfiguration)
    pub fn update_config(&mut self, config: RuntimeSelectorConfig) {
        info!("RuntimeSelector configuration updated: {:?}", config);
        self.config = config;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyberclaw_core::capability::CapabilityEffect;
    use cyberclaw_core::prelude::CapabilityTimeouts;

    fn create_test_capability(id: &str, risk: RiskLevel) -> CapabilityContract {
        CapabilityContract {
            id: id.to_string(),
            title: format!("Test Capability {}", id),
            description: None,
            input_schema: "Input".to_string(),
            output_schema: "Output".to_string(),
            risk,
            effects: vec![CapabilityEffect::Read],
            placement: None,
            timeouts: CapabilityTimeouts {
                request_ms: Some(5000),
            },
        }
    }

    #[test]
    fn test_risk_based_selection() {
        let selector = RuntimeSelector::default_config();

        let low_risk = create_test_capability("fs.read", RiskLevel::Low);
        assert_eq!(selector.select_runtime(&low_risk), RuntimeMode::Native);

        let medium_risk = create_test_capability("fs.write", RiskLevel::Medium);
        assert_eq!(selector.select_runtime(&medium_risk), RuntimeMode::Process);

        let high_risk = create_test_capability("cmd.exec", RiskLevel::High);
        assert_eq!(selector.select_runtime(&high_risk), RuntimeMode::Container);

        let critical_risk = create_test_capability("pkg.install", RiskLevel::Critical);
        assert_eq!(
            selector.select_runtime(&critical_risk),
            RuntimeMode::Container
        );
    }

    #[test]
    fn test_capability_override() {
        let mut config = RuntimeSelectorConfig::default();
        config
            .capability_overrides
            .insert("fs.read".to_string(), RuntimeMode::Process);

        let selector = RuntimeSelector::new(config);

        let capability = create_test_capability("fs.read", RiskLevel::Low);
        // Normally Low → Native, but override forces Process
        assert_eq!(selector.select_runtime(&capability), RuntimeMode::Process);
    }

    #[test]
    fn test_always_native_strategy() {
        let config = RuntimeSelectorConfig {
            default_strategy: RuntimeSelectionStrategy::AlwaysNative,
            capability_overrides: HashMap::new(),
            strict_mode: false, // Disable strict mode to allow Native for Critical
        };
        let selector = RuntimeSelector::new(config);

        let high_risk = create_test_capability("cmd.exec", RiskLevel::High);
        assert_eq!(selector.select_runtime(&high_risk), RuntimeMode::Native);
    }

    #[test]
    fn test_always_process_strategy() {
        let config = RuntimeSelectorConfig {
            default_strategy: RuntimeSelectionStrategy::AlwaysProcess,
            capability_overrides: HashMap::new(),
            strict_mode: false,
        };
        let selector = RuntimeSelector::new(config);

        let low_risk = create_test_capability("fs.read", RiskLevel::Low);
        assert_eq!(selector.select_runtime(&low_risk), RuntimeMode::Process);
    }

    #[test]
    fn test_strict_mode_enforces_container_for_critical() {
        let config = RuntimeSelectorConfig {
            default_strategy: RuntimeSelectionStrategy::AlwaysProcess,
            capability_overrides: HashMap::new(),
            strict_mode: true, // Strict mode enabled
        };
        let selector = RuntimeSelector::new(config);

        let critical_risk = create_test_capability("pkg.install", RiskLevel::Critical);
        // AlwaysProcess would select Process, but strict mode overrides to Container
        assert_eq!(
            selector.select_runtime(&critical_risk),
            RuntimeMode::Container
        );
    }

    #[test]
    fn test_update_config() {
        let mut selector = RuntimeSelector::default_config();

        let capability = create_test_capability("fs.read", RiskLevel::Low);
        assert_eq!(selector.select_runtime(&capability), RuntimeMode::Native);

        // Update config to always use Process
        let new_config = RuntimeSelectorConfig {
            default_strategy: RuntimeSelectionStrategy::AlwaysProcess,
            capability_overrides: HashMap::new(),
            strict_mode: false,
        };
        selector.update_config(new_config);

        assert_eq!(selector.select_runtime(&capability), RuntimeMode::Process);
    }
}
