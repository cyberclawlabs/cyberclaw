//! # Orchestrator Gateway
//!
//! Defines the `OrchestratorGateway` trait, the sole interface through which
//! `agent-runtime` invokes `control-plane` capabilities.
//!
//! This trait lives in `cyberclaw-core` so that:
//! - `agent-runtime` depends on the trait (not on `control-plane`)
//! - `control-plane` provides the concrete implementation
//!
//! This breaks the cyclic dependency: agent-runtime -> core <- control-plane.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::capability::{CapabilityEffect, RiskLevel};
use crate::identity::ActorRef;
use crate::ids::{CapabilityId, ConnectorId, ExecutionId};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A request to execute a single capability through the orchestrator.
///
/// Full execution path:
/// Agent -> OrchestratorGateway -> Orchestrator -> PolicyEngine -> Connector -> Capability
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityRequest {
    /// Execution session identifier.
    pub execution_id: ExecutionId,
    /// The actor that initiated this request.
    pub requested_by: ActorRef,
    /// Target capability identifier.
    pub capability_id: CapabilityId,
    /// Connector that provides the capability.
    pub connector_id: ConnectorId,
    /// JSON-encoded input parameters for the capability.
    pub input: serde_json::Value,
    /// Human-readable reason for the invocation (used for audit).
    pub reason: String,
}

/// The result of a successfully executed capability.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityResult {
    /// The execution session this result belongs to.
    pub execution_id: ExecutionId,
    /// The capability that was executed.
    pub capability_id: CapabilityId,
    /// JSON-encoded output produced by the capability.
    pub output: serde_json::Value,
}

/// Metadata describing an available capability.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityInfo {
    /// Capability identifier.
    pub id: CapabilityId,
    /// Connector that provides this capability.
    pub connector_id: ConnectorId,
    /// Risk level of the capability.
    pub risk: RiskLevel,
    /// Side-effects produced by the capability.
    pub effects: Vec<CapabilityEffect>,
    /// Human-readable description.
    pub description: String,
}

/// Errors that can occur when interacting with the orchestrator gateway.
#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    /// The requested capability was not found in the registry.
    #[error("capability not found: {0}")]
    CapabilityNotFound(String),
    /// The governance / policy engine denied the request.
    #[error("governance denied: {0}")]
    GovernanceDenied(String),
    /// The governance engine requires human review before proceeding.
    #[error("review required: {0}")]
    ReviewRequired(String),
    /// The connector failed to execute the capability.
    #[error("connector error: {0}")]
    ConnectorError(String),
    /// An internal error occurred inside the gateway implementation.
    #[error("internal error: {0}")]
    Internal(String),
}

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// The sole interface through which agent-runtime invokes control-plane capabilities.
///
/// By depending only on this trait (defined in `cyberclaw-core`), `agent-runtime`
/// avoids a direct dependency on `control-plane`. The concrete implementation
/// is provided by the control-plane crate.
///
/// # Execution path
///
/// ```text
/// Agent -> OrchestratorGateway -> Orchestrator -> PolicyEngine -> Connector -> Capability
/// ```
#[async_trait]
pub trait OrchestratorGateway: Send + Sync {
    /// Execute a capability through the orchestrator pipeline.
    ///
    /// The implementation is expected to:
    /// 1. Resolve the capability in the registry
    /// 2. Evaluate governance / policy rules
    /// 3. Dispatch to the appropriate connector
    /// 4. Return the execution result
    async fn execute_capability(
        &self,
        request: CapabilityRequest,
    ) -> Result<CapabilityResult, GatewayError>;

    /// List all capabilities currently available in the platform.
    ///
    /// This is used by the agentic loop to build the tool list that the
    /// LLM can choose from at each step.
    async fn list_capabilities(&self) -> Result<Vec<CapabilityInfo>, GatewayError>;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// A mock implementation used to verify the trait compiles and is callable.
    struct MockGateway;

    #[async_trait]
    impl OrchestratorGateway for MockGateway {
        async fn execute_capability(
            &self,
            request: CapabilityRequest,
        ) -> Result<CapabilityResult, GatewayError> {
            Ok(CapabilityResult {
                execution_id: request.execution_id,
                capability_id: request.capability_id,
                output: serde_json::json!({"status": "ok"}),
            })
        }

        async fn list_capabilities(&self) -> Result<Vec<CapabilityInfo>, GatewayError> {
            Ok(vec![CapabilityInfo {
                id: CapabilityId::from_string("file.read".to_string()).unwrap(),
                connector_id: ConnectorId::from_string("filesystem".to_string()).unwrap(),
                risk: RiskLevel::Low,
                effects: vec![CapabilityEffect::Read],
                description: "Read a file from the filesystem".to_string(),
            }])
        }
    }

    #[tokio::test]
    async fn test_execute_capability() {
        let gw = MockGateway;
        let request = CapabilityRequest {
            execution_id: ExecutionId::new(),
            requested_by: crate::identity::Identity::System
                .to_actor_ref(None)
                .unwrap(),
            capability_id: CapabilityId::from_string("file.read".to_string()).unwrap(),
            connector_id: ConnectorId::from_string("filesystem".to_string()).unwrap(),
            input: serde_json::json!({"path": "/tmp/test.txt"}),
            reason: "unit test".to_string(),
        };

        let result = gw.execute_capability(request).await;
        assert!(result.is_ok());
        let res = result.unwrap();
        assert_eq!(res.output, serde_json::json!({"status": "ok"}));
    }

    #[tokio::test]
    async fn test_list_capabilities() {
        let gw = MockGateway;
        let caps = gw.list_capabilities().await.unwrap();
        assert_eq!(caps.len(), 1);
        assert_eq!(caps[0].id.as_str(), "file.read");
    }

    #[tokio::test]
    async fn test_gateway_error_display() {
        let err = GatewayError::CapabilityNotFound("file.delete".to_string());
        assert_eq!(err.to_string(), "capability not found: file.delete");

        let err = GatewayError::GovernanceDenied("insufficient trust level".to_string());
        assert!(err.to_string().contains("governance denied"));

        let err = GatewayError::ReviewRequired("high risk operation".to_string());
        assert!(err.to_string().contains("review required"));
    }

    #[tokio::test]
    async fn test_trait_object_safety() {
        // Verify OrchestratorGateway can be used as a trait object (dyn dispatch).
        let gw: Box<dyn OrchestratorGateway> = Box::new(MockGateway);
        let caps = gw.list_capabilities().await.unwrap();
        assert!(!caps.is_empty());
    }
}
