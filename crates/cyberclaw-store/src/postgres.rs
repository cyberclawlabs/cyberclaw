//! PostgreSQL 存储实现

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::postgres::{PgPool, PgPoolOptions};
use uuid::Uuid;

use crate::error::{StoreError, StoreResult};

/// 执行记录
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ExecutionRecord {
    pub id: Uuid,
    pub agent_id: String,
    pub skill_id: Option<String>,
    pub status: String,
    pub input: Value,
    pub output: Option<Value>,
    pub error: Option<String>,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

/// 工件记录
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ArtifactRecord {
    pub id: Uuid,
    pub execution_id: Uuid,
    pub artifact_type: String,
    pub data: Value,
    pub metadata: Option<Value>,
}

/// 审计日志记录
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct AuditLogRecord {
    pub id: Uuid,
    pub execution_id: Option<Uuid>,
    pub event_type: String,
    pub actor: Option<String>,
    pub action: String,
    pub resource: Option<String>,
    pub details: Option<Value>,
    pub timestamp: DateTime<Utc>,
}

/// 策略记录
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct PolicyRecord {
    pub id: Uuid,
    pub name: String,
    pub effect: String,
    pub conditions: Value,
    pub active: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// 状态存储 trait
#[async_trait]
pub trait StateStore: Send + Sync {
    // Execution operations
    async fn save_execution(&self, record: ExecutionRecord) -> StoreResult<()>;
    async fn get_execution(&self, id: Uuid) -> StoreResult<ExecutionRecord>;
    async fn update_execution(
        &self,
        id: Uuid,
        status: String,
        output: Option<Value>,
        error: Option<String>,
    ) -> StoreResult<()>;
    async fn list_executions(
        &self,
        agent_id: Option<String>,
        limit: usize,
        offset: usize,
    ) -> StoreResult<Vec<ExecutionRecord>>;

    // Artifact operations
    async fn save_artifact(&self, record: ArtifactRecord) -> StoreResult<()>;
    async fn list_artifacts(&self, execution_id: Uuid) -> StoreResult<Vec<ArtifactRecord>>;

    // Audit log operations
    async fn save_audit_log(&self, record: AuditLogRecord) -> StoreResult<()>;
    async fn list_audit_logs(
        &self,
        execution_id: Option<Uuid>,
        limit: usize,
        offset: usize,
    ) -> StoreResult<Vec<AuditLogRecord>>;

    // Policy operations
    async fn save_policy(&self, record: PolicyRecord) -> StoreResult<()>;
    async fn get_policy(&self, id: Uuid) -> StoreResult<PolicyRecord>;
    async fn get_policy_by_name(&self, name: &str) -> StoreResult<PolicyRecord>;
    async fn list_active_policies(&self) -> StoreResult<Vec<PolicyRecord>>;
    async fn update_policy_status(&self, id: Uuid, active: bool) -> StoreResult<()>;
}

/// PostgreSQL 状态存储实现
pub struct PostgresStateStore {
    pool: PgPool,
}

impl PostgresStateStore {
    /// 创建新的 PostgreSQL 存储实例
    pub async fn new(database_url: &str) -> StoreResult<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(10)
            .connect(database_url)
            .await
            .map_err(|e| StoreError::ConnectionError(e.to_string()))?;

        Ok(Self { pool })
    }

    /// 获取数据库连接池的引用
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// 运行数据库迁移
    pub async fn run_migrations(&self) -> StoreResult<()> {
        // 创建 executions 表
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS executions (
                id UUID PRIMARY KEY,
                agent_id VARCHAR(255) NOT NULL,
                skill_id VARCHAR(255),
                status VARCHAR(50) NOT NULL,
                input JSONB NOT NULL,
                output JSONB,
                error TEXT,
                started_at TIMESTAMPTZ NOT NULL,
                completed_at TIMESTAMPTZ,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // 创建索引
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_executions_agent_id ON executions(agent_id)",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_executions_status ON executions(status)")
            .execute(&self.pool)
            .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_executions_created_at ON executions(created_at DESC)",
        )
        .execute(&self.pool)
        .await?;

        // 创建 artifacts 表
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS artifacts (
                id UUID PRIMARY KEY,
                execution_id UUID NOT NULL REFERENCES executions(id) ON DELETE CASCADE,
                artifact_type VARCHAR(100) NOT NULL,
                data JSONB NOT NULL,
                metadata JSONB,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // artifacts 索引
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_artifacts_execution_id ON artifacts(execution_id)",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_artifacts_type ON artifacts(artifact_type)",
        )
        .execute(&self.pool)
        .await?;

        // 创建 audit_logs 表
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS audit_logs (
                id UUID PRIMARY KEY,
                execution_id UUID REFERENCES executions(id) ON DELETE SET NULL,
                event_type VARCHAR(100) NOT NULL,
                actor VARCHAR(255),
                action VARCHAR(255) NOT NULL,
                resource VARCHAR(255),
                details JSONB,
                timestamp TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // audit_logs 索引
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_audit_logs_execution_id ON audit_logs(execution_id)",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_audit_logs_event_type ON audit_logs(event_type)",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_audit_logs_timestamp ON audit_logs(timestamp DESC)",
        )
        .execute(&self.pool)
        .await?;

        // 创建 policies 表
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS policies (
                id UUID PRIMARY KEY,
                name VARCHAR(255) NOT NULL UNIQUE,
                effect VARCHAR(10) NOT NULL CHECK (effect IN ('allow', 'deny')),
                conditions JSONB NOT NULL,
                active BOOLEAN NOT NULL DEFAULT true,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // policies 索引
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_policies_name ON policies(name)")
            .execute(&self.pool)
            .await?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_policies_active ON policies(active)")
            .execute(&self.pool)
            .await?;

        Ok(())
    }
}

#[async_trait]
impl StateStore for PostgresStateStore {
    async fn save_execution(&self, record: ExecutionRecord) -> StoreResult<()> {
        sqlx::query(
            r#"
            INSERT INTO executions (id, agent_id, skill_id, status, input, output, error, started_at, completed_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            "#,
        )
        .bind(record.id)
        .bind(record.agent_id)
        .bind(record.skill_id)
        .bind(record.status)
        .bind(record.input)
        .bind(record.output)
        .bind(record.error)
        .bind(record.started_at)
        .bind(record.completed_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn get_execution(&self, id: Uuid) -> StoreResult<ExecutionRecord> {
        let row: ExecutionRecord = sqlx::query_as(
            r#"
            SELECT id, agent_id, skill_id, status, input, output, error, started_at, completed_at
            FROM executions
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::NotFound(format!("Execution {} not found", id)))?;

        Ok(row)
    }

    async fn update_execution(
        &self,
        id: Uuid,
        status: String,
        output: Option<Value>,
        error: Option<String>,
    ) -> StoreResult<()> {
        let completed_at = if status == "completed" || status == "failed" {
            Some(Utc::now())
        } else {
            None
        };

        sqlx::query(
            r#"
            UPDATE executions
            SET status = $2, output = $3, error = $4, completed_at = $5, updated_at = NOW()
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(status)
        .bind(output)
        .bind(error)
        .bind(completed_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn list_executions(
        &self,
        agent_id: Option<String>,
        limit: usize,
        offset: usize,
    ) -> StoreResult<Vec<ExecutionRecord>> {
        let rows: Vec<ExecutionRecord> = if let Some(aid) = agent_id {
            sqlx::query_as(
                r#"
                SELECT id, agent_id, skill_id, status, input, output, error, started_at, completed_at
                FROM executions
                WHERE agent_id = $1
                ORDER BY created_at DESC
                LIMIT $2 OFFSET $3
                "#,
            )
            .bind(aid)
            .bind(limit as i64)
            .bind(offset as i64)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query_as(
                r#"
                SELECT id, agent_id, skill_id, status, input, output, error, started_at, completed_at
                FROM executions
                ORDER BY created_at DESC
                LIMIT $1 OFFSET $2
                "#,
            )
            .bind(limit as i64)
            .bind(offset as i64)
            .fetch_all(&self.pool)
            .await?
        };

        Ok(rows)
    }

    async fn save_artifact(&self, record: ArtifactRecord) -> StoreResult<()> {
        sqlx::query(
            r#"
            INSERT INTO artifacts (id, execution_id, artifact_type, data, metadata)
            VALUES ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(record.id)
        .bind(record.execution_id)
        .bind(record.artifact_type)
        .bind(record.data)
        .bind(record.metadata)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn list_artifacts(&self, execution_id: Uuid) -> StoreResult<Vec<ArtifactRecord>> {
        let rows: Vec<ArtifactRecord> = sqlx::query_as(
            r#"
            SELECT id, execution_id, artifact_type, data, metadata
            FROM artifacts
            WHERE execution_id = $1
            ORDER BY created_at DESC
            "#,
        )
        .bind(execution_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows)
    }

    async fn save_audit_log(&self, record: AuditLogRecord) -> StoreResult<()> {
        sqlx::query(
            r#"
            INSERT INTO audit_logs (id, execution_id, event_type, actor, action, resource, details, timestamp)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            "#,
        )
        .bind(record.id)
        .bind(record.execution_id)
        .bind(record.event_type)
        .bind(record.actor)
        .bind(record.action)
        .bind(record.resource)
        .bind(record.details)
        .bind(record.timestamp)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn list_audit_logs(
        &self,
        execution_id: Option<Uuid>,
        limit: usize,
        offset: usize,
    ) -> StoreResult<Vec<AuditLogRecord>> {
        let rows: Vec<AuditLogRecord> = if let Some(eid) = execution_id {
            sqlx::query_as(
                r#"
                SELECT id, execution_id, event_type, actor, action, resource, details, timestamp
                FROM audit_logs
                WHERE execution_id = $1
                ORDER BY timestamp DESC
                LIMIT $2 OFFSET $3
                "#,
            )
            .bind(eid)
            .bind(limit as i64)
            .bind(offset as i64)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query_as(
                r#"
                SELECT id, execution_id, event_type, actor, action, resource, details, timestamp
                FROM audit_logs
                ORDER BY timestamp DESC
                LIMIT $1 OFFSET $2
                "#,
            )
            .bind(limit as i64)
            .bind(offset as i64)
            .fetch_all(&self.pool)
            .await?
        };

        Ok(rows)
    }

    async fn save_policy(&self, record: PolicyRecord) -> StoreResult<()> {
        sqlx::query(
            r#"
            INSERT INTO policies (id, name, effect, conditions, active, created_at, updated_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#,
        )
        .bind(record.id)
        .bind(record.name)
        .bind(record.effect)
        .bind(record.conditions)
        .bind(record.active)
        .bind(record.created_at)
        .bind(record.updated_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn get_policy(&self, id: Uuid) -> StoreResult<PolicyRecord> {
        let row: PolicyRecord = sqlx::query_as(
            r#"
            SELECT id, name, effect, conditions, active, created_at, updated_at
            FROM policies
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::NotFound(format!("Policy {} not found", id)))?;

        Ok(row)
    }

    async fn get_policy_by_name(&self, name: &str) -> StoreResult<PolicyRecord> {
        let row: PolicyRecord = sqlx::query_as(
            r#"
            SELECT id, name, effect, conditions, active, created_at, updated_at
            FROM policies
            WHERE name = $1
            "#,
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::NotFound(format!("Policy {} not found", name)))?;

        Ok(row)
    }

    async fn list_active_policies(&self) -> StoreResult<Vec<PolicyRecord>> {
        let rows: Vec<PolicyRecord> = sqlx::query_as(
            r#"
            SELECT id, name, effect, conditions, active, created_at, updated_at
            FROM policies
            WHERE active = true
            ORDER BY created_at DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows)
    }

    async fn update_policy_status(&self, id: Uuid, active: bool) -> StoreResult<()> {
        sqlx::query(
            r#"
            UPDATE policies
            SET active = $2, updated_at = NOW()
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(active)
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}