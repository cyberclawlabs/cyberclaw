use crate::types::{Connector, ConnectorEntry};
use cyberclaw_core::capability_contract::CapabilityBehaviorContract;
use cyberclaw_core::prelude::*;
use dashmap::DashMap;
use once_cell::sync::OnceCell;
use std::sync::Arc;

static GLOBAL_REGISTRY: OnceCell<Arc<ConnectorRegistry>> = OnceCell::new();

/// IDs of core connectors that cannot be overwritten by dynamic registrations.
/// This prevents a malicious or misconfigured connector from shadowing
/// security-critical built-in connectors such as the local filesystem or
/// command executor.
const PROTECTED_CONNECTOR_IDS: &[&str] = &[
    "local-fs",
    "local-cmd",
    "local-search",
    "mcp-client",
    "http",
    "database",
    "github",
    "slack",
    "browser",
    "message-gateway",
    "retrieval",
    "rl-training",
    "acp-runtime",
    "openviking-memory",
];

/// Registry for all available connectors
pub struct ConnectorRegistry {
    connectors: DashMap<ConnectorId, ConnectorEntry>,
}

impl ConnectorRegistry {
    /// Create a new connector registry
    pub fn new() -> Self {
        Self {
            connectors: DashMap::new(),
        }
    }

    /// Get the global registry instance
    pub fn global() -> Arc<ConnectorRegistry> {
        GLOBAL_REGISTRY
            .get_or_init(|| Arc::new(ConnectorRegistry::new()))
            .clone()
    }

    /// Register a connector instance.
    ///
    /// If a connector with the same ID is already registered and that ID
    /// appears in [`PROTECTED_CONNECTOR_IDS`], the registration is rejected
    /// to prevent shadowing of security-critical built-in connectors.
    pub fn register(&self, connector: Arc<dyn Connector>) -> anyhow::Result<()> {
        let id = connector.id().clone();

        // Reject attempts to overwrite a protected connector that is already registered.
        if self.connectors.contains_key(&id) && Self::is_protected(id.as_str()) {
            anyhow::bail!(
                "Cannot overwrite protected connector '{}': shadowing built-in connectors is not allowed",
                id.as_str()
            );
        }

        let runtime = connector.runtime();
        let capabilities = connector.capabilities();

        let entry = ConnectorEntry {
            id: id.clone(),
            runtime,
            capabilities,
            connector: connector.clone(),
            behavior_contracts: std::collections::HashMap::new(),
        };

        self.connectors.insert(id, entry);
        Ok(())
    }

    /// Unregister a connector by ID (BT-37 — MCP hot-reload).
    ///
    /// Returns `Err` if the connector is protected (cannot be removed) or not
    /// found. After successful removal, all capabilities exposed by this
    /// connector vanish from `find_connector_for_capability` / `list_capabilities`,
    /// so subsequent agentic-loop iterations see the updated tool palette
    /// without restarting the server.
    pub fn unregister(&self, id: &ConnectorId) -> anyhow::Result<()> {
        if Self::is_protected(id.as_str()) {
            anyhow::bail!("Cannot unregister protected connector '{}'", id.as_str());
        }
        match self.connectors.remove(id) {
            Some(_) => Ok(()),
            None => anyhow::bail!("Connector '{}' is not registered", id.as_str()),
        }
    }

    /// Check whether the given connector ID is protected from being overwritten.
    pub fn is_protected(id: &str) -> bool {
        PROTECTED_CONNECTOR_IDS.contains(&id)
    }

    /// Get a connector by ID
    pub fn get(&self, id: &ConnectorId) -> Option<Arc<dyn Connector>> {
        self.connectors.get(id).map(|entry| entry.connector.clone())
    }

    /// Get connector entry by ID
    pub fn get_entry(&self, id: &ConnectorId) -> Option<ConnectorEntry> {
        self.connectors.get(id).map(|entry| entry.clone())
    }

    /// Find connector that exposes a given capability
    pub fn find_connector_for_capability(
        &self,
        capability_id: &CapabilityId,
    ) -> Option<ConnectorId> {
        for entry in self.connectors.iter() {
            for cap in &entry.capabilities {
                if cap.id == capability_id.as_str() {
                    return Some(entry.id.clone());
                }
            }
        }
        None
    }

    /// Set a behavior contract for a specific capability on a registered connector.
    ///
    /// Used for cross-validation (e.g. S7: destructiveness vs risk level consistency).
    /// Returns `Err` if the connector is not registered.
    pub fn set_behavior_contract(
        &self,
        connector_id: &ConnectorId,
        capability_id: String,
        contract: CapabilityBehaviorContract,
    ) -> anyhow::Result<()> {
        let mut entry = self.connectors.get_mut(connector_id).ok_or_else(|| {
            anyhow::anyhow!("Connector '{}' not registered", connector_id.as_str())
        })?;
        if !entry.capabilities.iter().any(|c| c.id == capability_id) {
            anyhow::bail!(
                "Capability '{}' not found on connector '{}'",
                capability_id,
                connector_id.as_str()
            );
        }
        entry.behavior_contracts.insert(capability_id, contract);
        Ok(())
    }

    /// List all registered connectors
    pub fn list_connectors(&self) -> Vec<ConnectorId> {
        self.connectors
            .iter()
            .map(|entry| entry.key().clone())
            .collect()
    }

    /// List all available capabilities across all connectors
    pub fn list_capabilities(&self) -> Vec<(ConnectorId, CapabilityId)> {
        let mut result = Vec::new();
        for entry in self.connectors.iter() {
            for cap in &entry.capabilities {
                result.push((
                    entry.id.clone(),
                    CapabilityId::from_string(cap.id.clone()).unwrap(),
                ));
            }
        }
        result
    }

    /// Get capability details
    pub fn get_capability(
        &self,
        capability_id: &CapabilityId,
    ) -> Option<(ConnectorId, CapabilityContract)> {
        for entry in self.connectors.iter() {
            for cap in &entry.capabilities {
                if cap.id == capability_id.as_str() {
                    return Some((entry.id.clone(), cap.clone()));
                }
            }
        }
        None
    }

    /// Clear all registered connectors
    pub fn clear(&self) {
        self.connectors.clear();
    }
}

impl Default for ConnectorRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CapabilityExecutionRequest, CapabilityExecutionResult};

    /// Minimal mock connector for registry tests.
    #[derive(Debug)]
    struct MockConnector {
        id: ConnectorId,
    }

    impl MockConnector {
        fn with_id(id: &str) -> Arc<dyn Connector> {
            Arc::new(Self {
                id: ConnectorId::from_string(id.to_string()).unwrap(),
            })
        }
    }

    #[async_trait::async_trait]
    impl Connector for MockConnector {
        fn id(&self) -> &ConnectorId {
            &self.id
        }

        fn runtime(&self) -> ConnectorRuntime {
            ConnectorRuntime::Native
        }

        fn capabilities(&self) -> Vec<CapabilityContract> {
            vec![]
        }

        async fn execute(
            &self,
            _request: CapabilityExecutionRequest,
        ) -> anyhow::Result<CapabilityExecutionResult> {
            anyhow::bail!("mock connector does not execute")
        }
    }

    #[test]
    fn is_protected_returns_true_for_known_ids() {
        assert!(ConnectorRegistry::is_protected("local-fs"));
        assert!(ConnectorRegistry::is_protected("local-cmd"));
        assert!(ConnectorRegistry::is_protected("mcp-client"));
    }

    #[test]
    fn is_protected_returns_false_for_custom_ids() {
        assert!(!ConnectorRegistry::is_protected("my-custom-connector"));
        assert!(!ConnectorRegistry::is_protected("user-plugin"));
    }

    #[test]
    fn first_registration_of_protected_id_succeeds() {
        let registry = ConnectorRegistry::new();
        let connector = MockConnector::with_id("local-fs");
        assert!(registry.register(connector).is_ok());
        assert!(registry
            .get(&ConnectorId::from_string("local-fs".to_string()).unwrap())
            .is_some());
    }

    #[test]
    fn overwrite_of_protected_connector_rejected() {
        let registry = ConnectorRegistry::new();
        let first = MockConnector::with_id("local-fs");
        registry.register(first).unwrap();

        let second = MockConnector::with_id("local-fs");
        let result = registry.register(second);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Cannot overwrite protected connector"));
    }

    #[test]
    fn overwrite_of_non_protected_connector_allowed() {
        let registry = ConnectorRegistry::new();
        let first = MockConnector::with_id("my-custom");
        registry.register(first).unwrap();

        let second = MockConnector::with_id("my-custom");
        assert!(registry.register(second).is_ok());
    }

    /// MockConnector variant that exposes a single capability — used to verify
    /// the BT-37 hot-reload round-trip (register → list_capabilities → unregister
    /// → capability vanishes).
    #[derive(Debug)]
    struct MockConnectorWithCap {
        id: ConnectorId,
        cap_id: String,
    }

    #[async_trait::async_trait]
    impl Connector for MockConnectorWithCap {
        fn id(&self) -> &ConnectorId {
            &self.id
        }
        fn runtime(&self) -> ConnectorRuntime {
            ConnectorRuntime::Native
        }
        fn capabilities(&self) -> Vec<CapabilityContract> {
            vec![CapabilityContract {
                id: self.cap_id.clone(),
                title: self.cap_id.clone(),
                description: None,
                input_schema: String::new(),
                output_schema: String::new(),
                risk: cyberclaw_core::capability::RiskLevel::Low,
                effects: vec![cyberclaw_core::capability::CapabilityEffect::Read],
                placement: None,
                timeouts: cyberclaw_core::manifests::CapabilityTimeouts { request_ms: None },
            }]
        }
        async fn execute(
            &self,
            _request: CapabilityExecutionRequest,
        ) -> anyhow::Result<CapabilityExecutionResult> {
            anyhow::bail!("not used in this test")
        }
    }

    #[test]
    fn unregister_removes_connector_and_caps_bt37() {
        // BT-37: hot-reload — registering then unregistering an MCP-style
        // connector must update the tool palette without restart.
        let registry = ConnectorRegistry::new();
        let connector_id = ConnectorId::from_string("mcp-runtime-1".to_string()).unwrap();
        let cap_id = CapabilityId::from_string("mcp.tool.echo".to_string()).unwrap();

        let connector = Arc::new(MockConnectorWithCap {
            id: connector_id.clone(),
            cap_id: "mcp.tool.echo".to_string(),
        });
        registry.register(connector).unwrap();

        // Capability is visible.
        assert_eq!(
            registry.find_connector_for_capability(&cap_id),
            Some(connector_id.clone())
        );

        // Hot-reload: remove the runtime and re-check.
        registry.unregister(&connector_id).unwrap();
        assert!(registry.find_connector_for_capability(&cap_id).is_none());
        assert!(registry.get(&connector_id).is_none());
    }

    #[test]
    fn unregister_protected_connector_rejected() {
        let registry = ConnectorRegistry::new();
        let id = ConnectorId::from_string("local-fs".to_string()).unwrap();
        registry
            .register(MockConnector::with_id("local-fs"))
            .unwrap();

        let result = registry.unregister(&id);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Cannot unregister protected connector"));
    }

    #[test]
    fn unregister_unknown_connector_errors() {
        let registry = ConnectorRegistry::new();
        let id = ConnectorId::from_string("never-registered".to_string()).unwrap();
        let result = registry.unregister(&id);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("is not registered"));
    }
}
