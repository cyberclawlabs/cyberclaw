//! Provenance verification tests for M4.8 (Runtime Isolation & Provenance)
//!
//! Tests the robustness and correctness of provenance tracking:
//! - Data integrity and completeness
//! - Concurrent execution isolation
//! - Error handling and edge cases
//! - RuntimeProvenance for different modes
//! - SecurityContext integration
//!
//! # Test Coverage
//!
//! 1. Complete provenance record verification
//! 2. Concurrent execution isolation
//! 3. Error scenarios and recovery
//! 4. Native/Process/Container runtime provenance
//! 5. Security context integration
//! 6. Provenance querying and retrieval

use cyberclaw_control_plane::provenance_tracker::{InMemoryProvenanceTracker, ProvenanceTracker};
use cyberclaw_core::ids::{
    AgentId, ArtifactId, CapabilityId, ConnectorId, ExecutionId, SkillId, TraceId,
};
use cyberclaw_core::provenance::{
    ContainerExecutionResult, ProcessExecutionResult, RuntimeMode, RuntimeProvenance,
    SecurityContext,
};
use std::collections::HashMap;
use tokio::task::JoinSet;

#[tokio::test]
async fn test_provenance_data_integrity() {
    let tracker = InMemoryProvenanceTracker::new();
    let execution_id = ExecutionId::new();
    let agent_id = AgentId::new();
    let trace_id = TraceId::new();
    let artifact_id = ArtifactId::new();

    // Start tracking
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

    // Record artifact
    tracker
        .record_artifact(&execution_id, artifact_id.clone())
        .await
        .expect("Failed to record artifact");

    // Record multiple capabilities
    let cap1 = CapabilityId::from_string("fs.read".to_string()).unwrap();
    let cap2 = CapabilityId::from_string("fs.write".to_string()).unwrap();
    let cap3 = CapabilityId::from_string("net.fetch".to_string()).unwrap();

    tracker
        .record_capability(&execution_id, cap1.clone())
        .await
        .unwrap();
    tracker
        .record_capability(&execution_id, cap2.clone())
        .await
        .unwrap();
    tracker
        .record_capability(&execution_id, cap3.clone())
        .await
        .unwrap();

    // Record connector
    let connector_id = ConnectorId::from_string("test-connector".to_string()).unwrap();
    tracker
        .record_connector(&execution_id, connector_id.clone())
        .await
        .unwrap();

    // Record skill
    let skill_id = SkillId::from_string("test-skill".to_string()).unwrap();
    tracker
        .record_skill(&execution_id, skill_id.clone())
        .await
        .unwrap();

    // Record runtime provenance (Process mode)
    let runtime_prov = RuntimeProvenance {
        runtime_mode: RuntimeMode::Process,
        selection_reason: "Medium risk capability".to_string(),
        process_result: Some(ProcessExecutionResult {
            exit_code: 0,
            timed_out: false,
            force_killed: false,
            duration_ms: 1234,
            stdout_digest: Some("sha256:stdout123".to_string()),
            stderr_digest: Some("sha256:stderr456".to_string()),
        }),
        container_result: None,
        configuration_digest: Some("sha256:config789".to_string()),
        resource_usage: {
            let mut usage = HashMap::new();
            usage.insert("cpu_ms".to_string(), "500".to_string());
            usage.insert("memory_kb".to_string(), "2048".to_string());
            usage
        },
    };

    tracker
        .record_runtime_provenance(&execution_id, runtime_prov.clone())
        .await
        .unwrap();

    // Record security context
    let sec_ctx = SecurityContext {
        governance_decision: "Allow (Low risk)".to_string(),
        security_events: vec!["PromptScanned".to_string(), "OutputFiltered".to_string()],
        secret_refs: vec!["secret_123".to_string()],
        policy_violations: vec![],
    };

    tracker
        .record_security_context(&execution_id, sec_ctx.clone())
        .await
        .unwrap();

    // Finalize and verify complete record
    let record = tracker
        .finalize(&execution_id)
        .await
        .expect("Failed to finalize");

    // Verify all data
    assert_eq!(record.execution_id, execution_id);
    assert_eq!(record.agent_id, agent_id);
    assert_eq!(record.trace_id, trace_id);
    assert_eq!(record.artifact_id, artifact_id);

    // Verify capabilities
    assert_eq!(record.capability_refs.len(), 3);
    assert!(record.capability_refs.contains(&cap1));
    assert!(record.capability_refs.contains(&cap2));
    assert!(record.capability_refs.contains(&cap3));

    // Verify connectors
    assert_eq!(record.connector_refs.len(), 1);
    assert_eq!(record.connector_refs[0], connector_id);

    // Verify skills
    assert_eq!(record.skill_refs.len(), 1);
    assert_eq!(record.skill_refs[0], skill_id);

    // Verify runtime provenance
    assert!(record.runtime_provenance.is_some());
    let stored_runtime = record.runtime_provenance.unwrap();
    assert_eq!(stored_runtime.runtime_mode, RuntimeMode::Process);
    assert_eq!(stored_runtime.selection_reason, "Medium risk capability");
    assert!(stored_runtime.process_result.is_some());

    let stored_process = stored_runtime.process_result.unwrap();
    assert_eq!(stored_process.exit_code, 0);
    assert_eq!(stored_process.duration_ms, 1234);
    assert_eq!(
        stored_process.stdout_digest,
        Some("sha256:stdout123".to_string())
    );

    assert_eq!(
        stored_runtime.resource_usage.get("cpu_ms"),
        Some(&"500".to_string())
    );

    // Verify security context
    assert!(record.security_context.is_some());
    let stored_sec = record.security_context.unwrap();
    assert_eq!(stored_sec.governance_decision, "Allow (Low risk)");
    assert_eq!(stored_sec.security_events.len(), 2);
    assert_eq!(stored_sec.secret_refs.len(), 1);
    assert!(stored_sec.policy_violations.is_empty());
}

#[tokio::test]
async fn test_concurrent_execution_isolation() {
    let tracker = InMemoryProvenanceTracker::new();
    let mut join_set = JoinSet::new();

    // Spawn 10 concurrent executions
    for i in 0..10 {
        let tracker_clone = tracker.clone();
        join_set.spawn(async move {
            let execution_id = ExecutionId::new();
            let agent_id = AgentId::new();
            let trace_id = TraceId::new();

            // Start tracking
            tracker_clone
                .start_tracking(
                    execution_id.clone(),
                    agent_id.clone(),
                    None,
                    trace_id.clone(),
                    None,
                )
                .await
                .expect("Failed to start tracking");

            // Record unique capability for this execution
            let cap_id = CapabilityId::from_string(format!("cap.test{}", i)).unwrap();
            tracker_clone
                .record_capability(&execution_id, cap_id.clone())
                .await
                .expect("Failed to record capability");

            // Finalize
            let record = tracker_clone
                .finalize(&execution_id)
                .await
                .expect("Failed to finalize");

            // Verify isolation: each execution should have exactly 1 capability
            assert_eq!(record.capability_refs.len(), 1);
            assert_eq!(record.capability_refs[0], cap_id);

            record.execution_id
        });
    }

    // Collect all execution IDs
    let mut execution_ids = Vec::new();
    while let Some(result) = join_set.join_next().await {
        execution_ids.push(result.expect("Task failed"));
    }

    // Verify we have 10 unique executions
    assert_eq!(execution_ids.len(), 10);

    // Verify all execution IDs are unique
    let unique_count = execution_ids
        .iter()
        .collect::<std::collections::HashSet<_>>()
        .len();
    assert_eq!(unique_count, 10, "All execution IDs should be unique");
}

#[tokio::test]
async fn test_error_handling_finalize_before_start() {
    let tracker = InMemoryProvenanceTracker::new();
    let execution_id = ExecutionId::new();

    // Try to finalize without starting
    let result = tracker.finalize(&execution_id).await;
    assert!(result.is_err(), "Should fail when finalizing before start");
}

#[tokio::test]
async fn test_error_handling_record_before_start() {
    let tracker = InMemoryProvenanceTracker::new();
    let execution_id = ExecutionId::new();
    let cap_id = CapabilityId::from_string("test.cap".to_string()).unwrap();

    // Try to record capability without starting tracking
    let result = tracker.record_capability(&execution_id, cap_id).await;
    assert!(result.is_err(), "Should fail when recording before start");
}

#[tokio::test]
async fn test_error_handling_double_finalize() {
    let tracker = InMemoryProvenanceTracker::new();
    let execution_id = ExecutionId::new();
    let agent_id = AgentId::new();
    let trace_id = TraceId::new();

    // Start and finalize
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

    tracker
        .finalize(&execution_id)
        .await
        .expect("First finalize should succeed");

    // Try to finalize again
    let result = tracker.finalize(&execution_id).await;
    assert!(result.is_err(), "Should fail on second finalize");
}

#[tokio::test]
async fn test_runtime_provenance_native_mode() {
    let tracker = InMemoryProvenanceTracker::new();
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
        .unwrap();

    // Record Native runtime (no isolation)
    let runtime_prov = RuntimeProvenance {
        runtime_mode: RuntimeMode::Native,
        selection_reason: "Low risk capability, no isolation needed".to_string(),
        process_result: None,
        container_result: None,
        configuration_digest: None,
        resource_usage: HashMap::new(),
    };

    tracker
        .record_runtime_provenance(&execution_id, runtime_prov)
        .await
        .unwrap();

    let record = tracker.finalize(&execution_id).await.unwrap();
    let stored_runtime = record.runtime_provenance.unwrap();

    assert_eq!(stored_runtime.runtime_mode, RuntimeMode::Native);
    assert!(stored_runtime.process_result.is_none());
    assert!(stored_runtime.container_result.is_none());
}

#[tokio::test]
async fn test_runtime_provenance_container_mode() {
    let tracker = InMemoryProvenanceTracker::new();
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
        .unwrap();

    // Record Container runtime (full isolation)
    let runtime_prov = RuntimeProvenance {
        runtime_mode: RuntimeMode::Container,
        selection_reason: "High risk capability requires full isolation".to_string(),
        process_result: None,
        container_result: Some(ContainerExecutionResult {
            container_id: "container_abc123".to_string(),
            exit_code: 0,
            timed_out: false,
            duration_ms: 3456,
            image_digest: "sha256:image_def456".to_string(),
            network_mode: "none".to_string(),
        }),
        configuration_digest: Some("sha256:container_config".to_string()),
        resource_usage: HashMap::new(),
    };

    tracker
        .record_runtime_provenance(&execution_id, runtime_prov)
        .await
        .unwrap();

    let record = tracker.finalize(&execution_id).await.unwrap();
    let stored_runtime = record.runtime_provenance.unwrap();

    assert_eq!(stored_runtime.runtime_mode, RuntimeMode::Container);
    assert!(stored_runtime.container_result.is_some());
    assert!(stored_runtime.process_result.is_none());

    let container_result = stored_runtime.container_result.unwrap();
    assert_eq!(container_result.container_id, "container_abc123");
    assert_eq!(container_result.network_mode, "none");
    assert_eq!(container_result.duration_ms, 3456);
}

#[tokio::test]
async fn test_provenance_retrieval_after_finalization() {
    let tracker = InMemoryProvenanceTracker::new();
    let execution_id = ExecutionId::new();
    let agent_id = AgentId::new();
    let trace_id = TraceId::new();

    // Start and finalize
    tracker
        .start_tracking(
            execution_id.clone(),
            agent_id.clone(),
            None,
            trace_id.clone(),
            None,
        )
        .await
        .unwrap();

    let cap_id = CapabilityId::from_string("test.cap".to_string()).unwrap();
    tracker
        .record_capability(&execution_id, cap_id.clone())
        .await
        .unwrap();

    let finalized_record = tracker
        .finalize(&execution_id)
        .await
        .expect("Finalize should succeed");

    // Retrieve finalized record
    let retrieved_record = tracker
        .get(&execution_id)
        .await
        .expect("Get should succeed")
        .expect("Record should exist");

    // Verify retrieved record matches finalized record
    assert_eq!(retrieved_record.id, finalized_record.id);
    assert_eq!(retrieved_record.execution_id, finalized_record.execution_id);
    assert_eq!(retrieved_record.capability_refs.len(), 1);
    assert_eq!(retrieved_record.capability_refs[0], cap_id);
}

#[tokio::test]
async fn test_security_context_policy_violations() {
    let tracker = InMemoryProvenanceTracker::new();
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
        .unwrap();

    // Record security context with policy violations
    let sec_ctx = SecurityContext {
        governance_decision: "Deny (Policy violation detected)".to_string(),
        security_events: vec![
            "PromptScanned".to_string(),
            "PolicyViolationDetected".to_string(),
        ],
        secret_refs: vec![],
        policy_violations: vec![
            "Attempted to access restricted resource".to_string(),
            "Exceeded rate limit".to_string(),
        ],
    };

    tracker
        .record_security_context(&execution_id, sec_ctx)
        .await
        .unwrap();

    let record = tracker.finalize(&execution_id).await.unwrap();
    let stored_sec = record.security_context.unwrap();

    assert_eq!(stored_sec.policy_violations.len(), 2);
    assert!(stored_sec
        .policy_violations
        .contains(&"Attempted to access restricted resource".to_string()));
    assert!(stored_sec
        .policy_violations
        .contains(&"Exceeded rate limit".to_string()));
}
