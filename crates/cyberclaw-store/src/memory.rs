//! 内存存储实现（用于测试和开发）

use std::collections::HashMap;
use std::sync::RwLock;

use async_trait::async_trait;
use uuid::Uuid;

use crate::error::Result;
use crate::state_store::{
    ArtifactRecord, AuditLogRecord, ExecutionRecord, JournalRecord, PolicyRecord, StateStore,
    TraceRecord,
};

/// 基于内存的状态存储实现
pub struct InMemoryStateStore {
    executions: RwLock<HashMap<Uuid, ExecutionRecord>>,
    artifacts: RwLock<HashMap<Uuid, Vec<ArtifactRecord>>>,
    audit_logs: RwLock<Vec<AuditLogRecord>>,
    policies: RwLock<HashMap<String, PolicyRecord>>,
    /// Sprint 10 (gradual landing): in-memory trace records.
    traces: RwLock<Vec<TraceRecord>>,
    /// Sprint 10 (gradual landing): in-memory journal iterations.
    journal: RwLock<Vec<JournalRecord>>,
}

impl InMemoryStateStore {
    /// 创建新的内存存储实例
    pub fn new() -> Self {
        Self {
            executions: RwLock::new(HashMap::new()),
            artifacts: RwLock::new(HashMap::new()),
            audit_logs: RwLock::new(Vec::new()),
            policies: RwLock::new(HashMap::new()),
            traces: RwLock::new(Vec::new()),
            journal: RwLock::new(Vec::new()),
        }
    }
}

impl Default for InMemoryStateStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl StateStore for InMemoryStateStore {
    async fn save_execution(&self, record: ExecutionRecord) -> Result<()> {
        let mut executions = self.executions.write().unwrap();
        executions.insert(record.id, record);
        Ok(())
    }

    async fn get_execution(&self, id: Uuid) -> Result<ExecutionRecord> {
        let executions = self.executions.read().unwrap();
        executions.get(&id).cloned().ok_or_else(|| {
            crate::error::StoreError::NotFound(format!("Execution {} not found", id))
        })
    }

    async fn update_execution(
        &self,
        id: Uuid,
        status: String,
        output: Option<serde_json::Value>,
        error: Option<String>,
    ) -> Result<()> {
        let mut executions = self.executions.write().unwrap();
        if let Some(exec) = executions.get_mut(&id) {
            exec.status = status.clone();
            exec.output = output;
            exec.error = error;
            if status == "completed" || status == "failed" {
                exec.completed_at = Some(chrono::Utc::now());
            }
            Ok(())
        } else {
            Err(crate::error::StoreError::NotFound(format!(
                "Execution {} not found",
                id
            )))
        }
    }

    async fn list_executions(
        &self,
        agent_id: Option<String>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<ExecutionRecord>> {
        let executions = self.executions.read().unwrap();
        let mut records: Vec<_> = executions.values().cloned().collect();

        // Filter by agent_id if provided
        if let Some(aid) = agent_id {
            records.retain(|r| r.agent_id == aid);
        }

        // Sort by started_at descending
        records.sort_by_key(|r| std::cmp::Reverse(r.started_at));

        // Apply pagination
        Ok(records.into_iter().skip(offset).take(limit).collect())
    }

    async fn save_artifact(&self, record: ArtifactRecord) -> Result<()> {
        let mut artifacts = self.artifacts.write().unwrap();
        artifacts
            .entry(record.execution_id)
            .or_default()
            .push(record);
        Ok(())
    }

    async fn list_artifacts(&self, execution_id: Uuid) -> Result<Vec<ArtifactRecord>> {
        let artifacts = self.artifacts.read().unwrap();
        Ok(artifacts.get(&execution_id).cloned().unwrap_or_default())
    }

    async fn save_audit_log(&self, record: AuditLogRecord) -> Result<()> {
        let mut logs = self.audit_logs.write().unwrap();
        logs.push(record);
        Ok(())
    }

    async fn list_audit_logs(
        &self,
        execution_id: Option<Uuid>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<AuditLogRecord>> {
        let logs = self.audit_logs.read().unwrap();
        let mut records: Vec<_> = logs.clone();

        // Filter by execution_id if provided
        if let Some(eid) = execution_id {
            records.retain(|r| r.execution_id == Some(eid));
        }

        // Sort by timestamp descending
        records.sort_by_key(|r| std::cmp::Reverse(r.timestamp));

        // Apply pagination
        Ok(records.into_iter().skip(offset).take(limit).collect())
    }

    async fn save_policy(&self, record: PolicyRecord) -> Result<()> {
        let mut policies = self.policies.write().unwrap();
        policies.insert(record.name.clone(), record);
        Ok(())
    }

    async fn get_policy(&self, name: &str) -> Result<PolicyRecord> {
        let policies = self.policies.read().unwrap();
        policies
            .get(name)
            .cloned()
            .ok_or_else(|| crate::error::StoreError::NotFound(format!("Policy {} not found", name)))
    }

    async fn list_policies(&self, active_only: bool) -> Result<Vec<PolicyRecord>> {
        let policies = self.policies.read().unwrap();
        let mut records: Vec<_> = policies.values().cloned().collect();

        if active_only {
            records.retain(|p| p.active);
        }

        // Sort by created_at descending
        records.sort_by_key(|r| std::cmp::Reverse(r.created_at));

        Ok(records)
    }

    async fn update_policy(&self, name: &str, active: bool) -> Result<()> {
        let mut policies = self.policies.write().unwrap();
        if let Some(policy) = policies.get_mut(name) {
            policy.active = active;
            policy.updated_at = chrono::Utc::now();
            Ok(())
        } else {
            Err(crate::error::StoreError::NotFound(format!(
                "Policy {} not found",
                name
            )))
        }
    }

    // Sprint 10 (gradual landing): trace + journal storage overrides.

    async fn save_trace(&self, record: TraceRecord) -> Result<()> {
        let mut traces = self.traces.write().unwrap();
        traces.push(record);
        Ok(())
    }

    async fn list_traces_by_agent_window(
        &self,
        agent_id: &str,
        window_start: chrono::DateTime<chrono::Utc>,
        window_end: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<TraceRecord>> {
        let traces = self.traces.read().unwrap();
        Ok(traces
            .iter()
            .filter(|r| {
                r.agent_id == agent_id && r.timestamp >= window_start && r.timestamp < window_end
            })
            .cloned()
            .collect())
    }

    async fn save_journal_iteration(&self, record: JournalRecord) -> Result<()> {
        let mut journal = self.journal.write().unwrap();
        journal.push(record);
        Ok(())
    }

    async fn list_journal_iterations_by_agent_window(
        &self,
        agent_id: &str,
        window_start: chrono::DateTime<chrono::Utc>,
        window_end: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<JournalRecord>> {
        let journal = self.journal.read().unwrap();
        Ok(journal
            .iter()
            .filter(|r| {
                r.agent_id == agent_id && r.created_at >= window_start && r.created_at < window_end
            })
            .cloned()
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serde_json::json;

    #[tokio::test]
    async fn test_execution_crud() {
        let store = InMemoryStateStore::new();
        let exec_id = Uuid::new_v4();

        // Create
        let record = ExecutionRecord {
            id: exec_id,
            agent_id: "test-agent".to_string(),
            skill_id: Some("test-skill".to_string()),
            status: "running".to_string(),
            input: json!({"test": "input"}),
            output: None,
            error: None,
            started_at: Utc::now(),
            completed_at: None,
        };
        store.save_execution(record.clone()).await.unwrap();

        // Read
        let retrieved = store.get_execution(exec_id).await.unwrap();
        assert_eq!(retrieved.id, exec_id);
        assert_eq!(retrieved.status, "running");

        // Update
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
        assert!(updated.completed_at.is_some());
    }

    #[tokio::test]
    async fn test_artifact_crud() {
        let store = InMemoryStateStore::new();
        let exec_id = Uuid::new_v4();
        let artifact_id = Uuid::new_v4();

        let artifact = ArtifactRecord {
            id: artifact_id,
            execution_id: exec_id,
            artifact_type: "log".to_string(),
            data: json!({"message": "test log"}),
            metadata: None,
        };

        store.save_artifact(artifact).await.unwrap();

        let artifacts = store.list_artifacts(exec_id).await.unwrap();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].id, artifact_id);
    }

    #[tokio::test]
    async fn test_policy_crud() {
        let store = InMemoryStateStore::new();

        let policy = PolicyRecord {
            id: Uuid::new_v4(),
            name: "test-policy".to_string(),
            effect: "allow".to_string(),
            conditions: json!({"resource": "agent:*"}),
            active: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        store.save_policy(policy.clone()).await.unwrap();

        let retrieved = store.get_policy("test-policy").await.unwrap();
        assert_eq!(retrieved.name, "test-policy");
        assert!(retrieved.active);

        store.update_policy("test-policy", false).await.unwrap();

        let updated = store.get_policy("test-policy").await.unwrap();
        assert!(!updated.active);
    }
}
