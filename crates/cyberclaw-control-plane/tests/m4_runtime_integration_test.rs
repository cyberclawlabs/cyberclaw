//! Runtime integration tests for M4 (Runtime Isolation & Provenance)
//!
//! Tests the integration between:
//! - RuntimeSelector: Selects runtime based on capability risk
//! - ProcessExecutor: Executes capabilities in isolated processes
//! - ProvenanceTracker: Records execution provenance
//!
//! # Test Coverage
//!
//! 1. Runtime selection based on risk level
//! 2. Process execution with timeout and resource limits
//! 3. Provenance collection and persistence
//! 4. End-to-end execution flow

use cyberclaw_connectors::runtime::{
    ProcessConfig, ProcessExecutor, ProcessRuntime, RuntimeMode, RuntimeSelector,
};
use cyberclaw_control_plane::provenance_tracker::{InMemoryProvenanceTracker, ProvenanceTracker};
use cyberclaw_core::capability::RiskLevel;
use cyberclaw_core::ids::{AgentId, ExecutionId, TraceId};
use cyberclaw_core::manifests::CapabilityContract;
use cyberclaw_core::prelude::CapabilityTimeouts;
use cyberclaw_core::provenance::{
    ProcessExecutionResult, RuntimeMode as ProvenanceRuntimeMode, RuntimeProvenance,
};
use std::collections::HashMap;
use std::time::Duration;

/// Helper function to create a test capability with specified risk level
fn create_test_capability(id: &str, risk: RiskLevel) -> CapabilityContract {
    CapabilityContract {
        id: id.to_string(),
        title: format!("Test Capability: {}", id),
        description: Some(format!(
            "Test capability with {} risk",
            format!("{:?}", risk).to_lowercase()
        )),
        input_schema: "Input".to_string(),
        output_schema: "Output".to_string(),
        risk,
        effects: vec![],
        placement: None,
        timeouts: CapabilityTimeouts {
            request_ms: Some(5000),
        },
    }
}

#[tokio::test]
async fn test_runtime_selector_risk_based_selection() {
    let selector = RuntimeSelector::default_config();

    // Low risk → Native
    let low_cap = create_test_capability("fs.read", RiskLevel::Low);
    assert_eq!(selector.select_runtime(&low_cap), RuntimeMode::Native);

    // Medium risk → Process
    let medium_cap = create_test_capability("fs.write", RiskLevel::Medium);
    assert_eq!(selector.select_runtime(&medium_cap), RuntimeMode::Process);

    // High risk → Container
    let high_cap = create_test_capability("cmd.exec", RiskLevel::High);
    assert_eq!(selector.select_runtime(&high_cap), RuntimeMode::Container);

    // Critical risk → Container
    let critical_cap = create_test_capability("pkg.install", RiskLevel::Critical);
    assert_eq!(
        selector.select_runtime(&critical_cap),
        RuntimeMode::Container
    );
}

#[tokio::test]
async fn test_process_executor_simple_command() {
    // Use new_unrestricted() for testing - production should use whitelist
    let executor = ProcessExecutor::new_unrestricted();

    // Simple echo command
    let config = ProcessConfig::builder()
        .timeout(Duration::from_secs(5))
        .build();

    let result = executor
        .execute_command("echo", vec!["hello world".to_string()], config)
        .await
        .expect("Failed to execute echo");

    assert_eq!(result.exit_code, 0);
    assert!(result.stdout.contains("hello world"));
    assert!(!result.timed_out);
    assert!(!result.force_killed);
}

#[tokio::test]
async fn test_provenance_tracker_lifecycle() {
    let tracker = InMemoryProvenanceTracker::new();
    let execution_id = ExecutionId::new();
    let agent_id = AgentId::new();
    let trace_id = TraceId::new();

    // Start tracking
    let prov_id = tracker
        .start_tracking(
            execution_id.clone(),
            agent_id.clone(),
            None,
            trace_id.clone(),
            None,
        )
        .await
        .expect("Failed to start tracking");

    // Record capability
    let cap_id = cyberclaw_core::ids::CapabilityId::from_string("cmd.exec".to_string()).unwrap();
    tracker
        .record_capability(&execution_id, cap_id.clone())
        .await
        .expect("Failed to record capability");

    // Record runtime provenance (simulate Process execution)
    let runtime_prov = RuntimeProvenance {
        runtime_mode: ProvenanceRuntimeMode::Process,
        selection_reason: "Medium risk capability requires process isolation".to_string(),
        process_result: Some(ProcessExecutionResult {
            exit_code: 0,
            timed_out: false,
            force_killed: false,
            duration_ms: 1500,
            stdout_digest: Some("sha256:abc123".to_string()),
            stderr_digest: None,
        }),
        container_result: None,
        configuration_digest: Some("sha256:config456".to_string()),
        resource_usage: HashMap::new(),
    };

    tracker
        .record_runtime_provenance(&execution_id, runtime_prov.clone())
        .await
        .expect("Failed to record runtime provenance");

    // Finalize provenance
    let record = tracker
        .finalize(&execution_id)
        .await
        .expect("Failed to finalize provenance");

    // Verify provenance record
    assert_eq!(record.id, prov_id);
    assert_eq!(record.execution_id, execution_id);
    assert_eq!(record.capability_refs.len(), 1);
    assert!(record.runtime_provenance.is_some());
}

#[tokio::test]
async fn test_end_to_end_runtime_and_provenance() {
    // This test simulates the complete flow:
    // 1. RuntimeSelector chooses runtime based on capability risk
    // 2. ProcessExecutor executes the capability
    // 3. ProvenanceTracker records the execution

    let selector = RuntimeSelector::default_config();
    // Use new_unrestricted() for testing - production should use whitelist
    let executor = ProcessExecutor::new_unrestricted();
    let tracker = InMemoryProvenanceTracker::new();

    // Create a medium-risk capability
    let capability = create_test_capability("fs.write", RiskLevel::Medium);

    // Step 1: Select runtime based on risk
    let runtime_mode = selector.select_runtime(&capability);
    assert_eq!(
        runtime_mode,
        RuntimeMode::Process,
        "Medium risk should select Process runtime"
    );

    // Step 2: Start provenance tracking
    let execution_id = ExecutionId::new();
    let agent_id = AgentId::new();
    let trace_id = TraceId::new();

    tracker
        .start_tracking(
            execution_id.clone(),
            agent_id.clone(),
            None,
            trace_id.clone(),
            None,
        )
        .await
        .expect("Failed to start tracking");

    // Record capability
    let cap_id = cyberclaw_core::ids::CapabilityId::from_string("fs.write".to_string()).unwrap();
    tracker
        .record_capability(&execution_id, cap_id)
        .await
        .expect("Failed to record capability");

    // Step 3: Execute capability in Process runtime
    let config = ProcessConfig::builder()
        .timeout(Duration::from_secs(5))
        .build();

    let process_result = executor
        .execute_command("echo", vec!["test content".to_string()], config)
        .await
        .expect("Failed to execute command");

    // Step 4: Record runtime provenance
    let runtime_prov = RuntimeProvenance {
        runtime_mode: ProvenanceRuntimeMode::Process,
        selection_reason: "Medium risk capability (RiskLevel::Medium → RuntimeMode::Process)"
            .to_string(),
        process_result: Some(ProcessExecutionResult {
            exit_code: process_result.exit_code,
            timed_out: process_result.timed_out,
            force_killed: process_result.force_killed,
            duration_ms: process_result.duration_ms,
            stdout_digest: Some("sha256:test".to_string()),
            stderr_digest: None,
        }),
        container_result: None,
        configuration_digest: Some("sha256:test_config".to_string()),
        resource_usage: HashMap::new(),
    };

    tracker
        .record_runtime_provenance(&execution_id, runtime_prov)
        .await
        .expect("Failed to record runtime provenance");

    // Step 5: Finalize provenance and verify
    let final_record = tracker
        .finalize(&execution_id)
        .await
        .expect("Failed to finalize provenance");

    // Verify complete provenance record
    assert_eq!(final_record.execution_id, execution_id);
    assert!(final_record.runtime_provenance.is_some());

    let stored_prov = final_record.runtime_provenance.unwrap();
    assert_eq!(stored_prov.runtime_mode, ProvenanceRuntimeMode::Process);

    let stored_result = stored_prov.process_result.unwrap();
    assert_eq!(stored_result.exit_code, 0, "Command should succeed");
    assert!(!stored_result.timed_out, "Command should not timeout");
}
