//! Integration tests for cyberclaw-store

use chrono::Utc;
use cyberclaw_store::*;
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
async fn test_memory_store_execution_workflow() {
    let store = InMemoryStateStore::new();

    // Create execution
    let exec_id = Uuid::new_v4();
    let record = ExecutionRecord {
        id: exec_id,
        agent_id: "agent-1".to_string(),
        skill_id: Some("skill-a".to_string()),
        status: "running".to_string(),
        input: json!({"command": "test"}),
        output: None,
        error: None,
        started_at: Utc::now(),
        completed_at: None,
    };

    store.save_execution(record.clone()).await.unwrap();

    // Retrieve and verify
    let retrieved = store.get_execution(exec_id).await.unwrap();
    assert_eq!(retrieved.agent_id, "agent-1");
    assert_eq!(retrieved.status, "running");

    // Update to completed
    store
        .update_execution(
            exec_id,
            "completed".to_string(),
            Some(json!({"result": "success"})),
            None,
        )
        .await
        .unwrap();

    let updated = store.get_execution(exec_id).await.unwrap();
    assert_eq!(updated.status, "completed");
    assert!(updated.output.is_some());
    assert!(updated.completed_at.is_some());
}

#[tokio::test]
async fn test_memory_store_list_executions_pagination() {
    let store = InMemoryStateStore::new();

    // Create 10 executions
    for i in 0..10 {
        let record = ExecutionRecord {
            id: Uuid::new_v4(),
            agent_id: format!("agent-{}", i % 3), // 3 different agents
            skill_id: None,
            status: "completed".to_string(),
            input: json!({"index": i}),
            output: None,
            error: None,
            started_at: Utc::now(),
            completed_at: Some(Utc::now()),
        };
        store.save_execution(record).await.unwrap();
    }

    // Test pagination
    let page1 = store.list_executions(None, 5, 0).await.unwrap();
    assert_eq!(page1.len(), 5);

    let page2 = store.list_executions(None, 5, 5).await.unwrap();
    assert_eq!(page2.len(), 5);

    // Test filtering by agent_id
    let agent0_execs = store
        .list_executions(Some("agent-0".to_string()), 10, 0)
        .await
        .unwrap();
    assert!(agent0_execs.iter().all(|e| e.agent_id == "agent-0"));
}

#[tokio::test]
async fn test_memory_store_artifacts() {
    let store = InMemoryStateStore::new();
    let exec_id = Uuid::new_v4();

    // Create 3 artifacts for same execution
    for i in 0..3 {
        let artifact = ArtifactRecord {
            id: Uuid::new_v4(),
            execution_id: exec_id,
            artifact_type: format!("type-{}", i),
            data: json!({"index": i}),
            metadata: Some(json!({"tag": "test"})),
        };
        store.save_artifact(artifact).await.unwrap();
    }

    // List artifacts
    let artifacts = store.list_artifacts(exec_id).await.unwrap();
    assert_eq!(artifacts.len(), 3);

    // Verify all belong to same execution
    assert!(artifacts.iter().all(|a| a.execution_id == exec_id));
}

#[tokio::test]
async fn test_memory_store_audit_logs() {
    let store = InMemoryStateStore::new();
    let exec_id = Uuid::new_v4();

    // Create audit logs
    for i in 0..5 {
        let log = AuditLogRecord {
            id: Uuid::new_v4(),
            execution_id: Some(exec_id),
            event_type: "execution.progress".to_string(),
            actor: Some("system".to_string()),
            action: "update".to_string(),
            resource: Some(format!("execution:{}", exec_id)),
            details: Some(json!({"step": i})),
            timestamp: Utc::now(),
        };
        store.save_audit_log(log).await.unwrap();
    }

    // List all logs
    let all_logs = store.list_audit_logs(None, 10, 0).await.unwrap();
    assert_eq!(all_logs.len(), 5);

    // Filter by execution_id
    let exec_logs = store.list_audit_logs(Some(exec_id), 10, 0).await.unwrap();
    assert_eq!(exec_logs.len(), 5);
    assert!(exec_logs.iter().all(|l| l.execution_id == Some(exec_id)));
}

#[tokio::test]
async fn test_memory_store_policies() {
    let store = InMemoryStateStore::new();

    // Create policies
    for i in 0..3 {
        let policy = PolicyRecord {
            id: Uuid::new_v4(),
            name: format!("policy-{}", i),
            effect: if i % 2 == 0 { "allow" } else { "deny" }.to_string(),
            conditions: json!({"resource": format!("agent:agent-{}", i)}),
            active: i != 1, // policy-1 inactive
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        store.save_policy(policy).await.unwrap();
    }

    // List all policies
    let all_policies = store.list_policies(false).await.unwrap();
    assert_eq!(all_policies.len(), 3);

    // List only active policies
    let active_policies = store.list_policies(true).await.unwrap();
    assert_eq!(active_policies.len(), 2);
    assert!(active_policies.iter().all(|p| p.active));

    // Update policy
    store.update_policy("policy-1", true).await.unwrap();
    let updated = store.get_policy("policy-1").await.unwrap();
    assert!(updated.active);
}

#[tokio::test]
async fn test_memory_store_error_handling() {
    let store = InMemoryStateStore::new();

    // Test not found errors
    let non_existent_id = Uuid::new_v4();
    let result = store.get_execution(non_existent_id).await;
    assert!(result.is_err());
    assert!(matches!(result.err().unwrap(), StoreError::NotFound(_)));

    // Test policy not found
    let result = store.get_policy("non-existent").await;
    assert!(result.is_err());
    assert!(matches!(result.err().unwrap(), StoreError::NotFound(_)));
}
