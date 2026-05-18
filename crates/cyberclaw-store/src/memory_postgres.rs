//! InMemoryStateStore - PostgreSQL trait 的内存实现

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::error::{StoreError, StoreResult};
use crate::postgres::{
    ArtifactRecord, AuditLogRecord, ExecutionRecord, PolicyRecord, StateStore,
};

/// 内存存储实现（用于测试）
#[derive(Clone)]
pub struct InMemoryPostgresStore {
    executions: Arc<RwLock<HashMap<Uuid, ExecutionRecord>>>,
    artifacts: Arc<RwLock<HashMap<Uuid, Vec<ArtifactRecord>>>>,
    audit_logs: Arc<RwLock<Vec<AuditLogRecord>>>,
    policies: Arc<RwLock<HashMap<Uuid, PolicyRecord>>>,
}

impl Default for InMemoryPostgresStore {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryPostgresStore {
    /// 创建新的内存存储实例
    pub fn new() -> Self {
        Self {
            executions: Arc::new(RwLock::new(HashMap::new())),
            artifacts: Arc::new(RwLock::new(HashMap::new())),
            audit_logs: Arc::new(RwLock::new(Vec::new())),
            policies: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl StateStore for InMemoryPostgresStore {
    async fn save_execution(&self, record: ExecutionRecord) -> StoreResult<()> {
        let mut executions = self.executions.write().await;
        if executions.contains_key(&record.id) {
            return Err(StoreError::AlreadyExists(format!(
                "Execution {} already exists",
                record.id
            )));
        }
        executions.insert(record.id, record);
        Ok(())
    }

    async fn get_execution(&self, id: Uuid) -> StoreResult<ExecutionRecord> {
        let executions = self.executions.read().await;
        executions
            .get(&id)
            .cloned()
            .ok_or_else(|| StoreError::NotFound(format!("Execution {} not found", id)))
    }

    async fn update_execution(
        &self,
        id: Uuid,
        status: String,
        output: Option<serde_json::Value>,
        error: Option<String>,
    ) -> StoreResult<()> {
        let mut executions = self.executions.write().await;
        let execution = executions
            .get_mut(&id)
            .ok_or_else(|| StoreError::NotFound(format!("Execution {} not found", id)))?;

        execution.status = status.clone();
        execution.output = output;
        execution.error = error;

        if status == "completed" || status == "failed" {
            execution.completed_at = Some(chrono::Utc::now());
        }

        Ok(())
    }

    async fn list_executions(
        &self,
        agent_id: Option<String>,
        limit: usize,
        offset: usize,
    ) -> StoreResult<Vec<ExecutionRecord>> {
        let executions = self.executions.read().await;
        let mut result: Vec<ExecutionRecord> = executions
            .values()
            .filter(|e| agent_id.is_none() || e.agent_id == agent_id.as_ref().unwrap())
            .cloned()
            .collect();

        // 按开始时间倒序排列
        result.sort_by(|a, b| b.started_at.cmp(&a.started_at));

        // 分页
        let start = offset.min(result.len());
        let end = (start + limit).min(result.len());
        Ok(result[start..end].to_vec())
    }

    async fn save_artifact(&self, record: ArtifactRecord) -> StoreResult<()> {
        let mut artifacts = self.artifacts.write().await;
        artifacts
            .entry(record.execution_id)
            .or_insert_with(Vec::new)
            .push(record);
        Ok(())
    }

    async fn list_artifacts(&self, execution_id: Uuid) -> StoreResult<Vec<ArtifactRecord>> {
        let artifacts = self.artifacts.read().await;
        Ok(artifacts.get(&execution_id).cloned().unwrap_or_default())
    }

    async fn save_audit_log(&self, record: AuditLogRecord) -> StoreResult<()> {
        let mut audit_logs = self.audit_logs.write().await;
        audit_logs.push(record);
        Ok(())
    }

    async fn list_audit_logs(
        &self,
        execution_id: Option<Uuid>,
        limit: usize,
        offset: usize,
    ) -> StoreResult<Vec<AuditLogRecord>> {
        let audit_logs = self.audit_logs.read().await;
        let mut result: Vec<AuditLogRecord> = audit_logs
            .iter()
            .filter(|log| execution_id.is_none() || log.execution_id == execution_id)
            .cloned()
            .collect();

        // 按时间戳倒序排列
        result.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

        // 分页
        let start = offset.min(result.len());
        let end = (start + limit).min(result.len());
        Ok(result[start..end].to_vec())
    }

    async fn save_policy(&self, record: PolicyRecord) -> StoreResult<()> {
        let mut policies = self.policies.write().await;

        // 检查名称唯一性
        for policy in policies.values() {
            if policy.name == record.name {
                return Err(StoreError::AlreadyExists(format!(
                    "Policy with name '{}' already exists",
                    record.name
                )));
            }
        }

        policies.insert(record.id, record);
        Ok(())
    }

    async fn get_policy(&self, id: Uuid) -> StoreResult<PolicyRecord> {
        let policies = self.policies.read().await;
        policies
            .get(&id)
            .cloned()
            .ok_or_else(|| StoreError::NotFound(format!("Policy {} not found", id)))
    }

    async fn get_policy_by_name(&self, name: &str) -> StoreResult<PolicyRecord> {
        let policies = self.policies.read().await;
        policies
            .values()
            .find(|p| p.name == name)
            .cloned()
            .ok_or_else(|| StoreError::NotFound(format!("Policy '{}' not found", name)))
    }

    async fn list_active_policies(&self) -> StoreResult<Vec<PolicyRecord>> {
        let policies = self.policies.read().await;
        let mut result: Vec<PolicyRecord> = policies
            .values()
            .filter(|p| p.active)
            .cloned()
            .collect();

        // 按创建时间倒序排列
        result.sort_by(|a, b| b.created_at.cmp(&a.created_at));

        Ok(result)
    }

    async fn update_policy_status(&self, id: Uuid, active: bool) -> StoreResult<()> {
        let mut policies = self.policies.write().await;
        let policy = policies
            .get_mut(&id)
            .ok_or_else(|| StoreError::NotFound(format!("Policy {} not found", id)))?;

        policy.active = active;
        policy.updated_at = chrono::Utc::now();

        Ok(())
    }
}