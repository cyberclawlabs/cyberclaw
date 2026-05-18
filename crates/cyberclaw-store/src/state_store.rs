//! 状态存储抽象层和 PostgreSQL 实现

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::error::Result;
#[cfg(feature = "postgres")]
use crate::error::StoreError;

/// 执行记录
#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// Artifact 记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRecord {
    pub id: Uuid,
    pub execution_id: Uuid,
    pub artifact_type: String,
    pub data: Value,
    pub metadata: Option<Value>,
}

/// 审计日志记录
#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// Sprint 10 (gradual landing): dedicated trace record. Distinct from
/// `AuditLogRecord` (which mixes auth/config/security events) — `TraceRecord`
/// is purpose-built for distributed tracing with parent_trace_id linking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceRecord {
    pub id: Uuid,
    pub agent_id: String,
    pub execution_id: Option<Uuid>,
    pub parent_trace_id: Option<Uuid>,
    pub event_type: String,
    pub severity: String,
    pub details: Option<Value>,
    pub timestamp: DateTime<Utc>,
}

/// Sprint 10 (gradual landing): journal iteration record. Mirrors the
/// in-memory `ProgressJournal` entries from `cyberclaw_control_plane`'s
/// `persistent_execution` module so daily-digest can surface "iteration N
/// produced verdict V" rows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalRecord {
    pub id: Uuid,
    pub agent_id: String,
    pub execution_id: Option<Uuid>,
    pub iteration: u32,
    pub verdict: String,
    pub story_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// 策略记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyRecord {
    pub id: Uuid,
    pub name: String,
    pub effect: String,
    pub conditions: Value,
    pub active: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// 状态存储 trait（支持多种后端实现）
#[async_trait]
pub trait StateStore: Send + Sync {
    // Execution CRUD
    async fn save_execution(&self, record: ExecutionRecord) -> Result<()>;
    async fn get_execution(&self, id: Uuid) -> Result<ExecutionRecord>;
    async fn update_execution(
        &self,
        id: Uuid,
        status: String,
        output: Option<Value>,
        error: Option<String>,
    ) -> Result<()>;
    async fn list_executions(
        &self,
        agent_id: Option<String>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<ExecutionRecord>>;

    // Artifact CRUD
    async fn save_artifact(&self, record: ArtifactRecord) -> Result<()>;
    async fn list_artifacts(&self, execution_id: Uuid) -> Result<Vec<ArtifactRecord>>;

    /// Sprint 9 follow-up: query artifacts produced by an agent within a time
    /// window. Default implementation chains
    /// `list_executions(agent) → list_artifacts(exec_id)` and filters by
    /// `started_at`. Backends with native indexes (e.g. a future
    /// `artifacts(agent_id, created_at)` index) should override this for
    /// O(matched rows) scans instead of O(executions × artifacts).
    async fn list_artifacts_by_agent_window(
        &self,
        agent_id: &str,
        window_start: DateTime<Utc>,
        window_end: DateTime<Utc>,
    ) -> Result<Vec<ArtifactRecord>> {
        // Pull a generous slice of recent executions for this agent. 1000 is
        // a fine ceiling for daily-digest windows; long-window callers should
        // implement their own pagination.
        let executions = self
            .list_executions(Some(agent_id.to_string()), 1000, 0)
            .await?;
        let mut out = Vec::new();
        for exec in executions {
            if exec.started_at >= window_start && exec.started_at < window_end {
                let mut artifacts = self.list_artifacts(exec.id).await?;
                out.append(&mut artifacts);
            }
        }
        Ok(out)
    }

    // Audit log CRUD
    async fn save_audit_log(&self, record: AuditLogRecord) -> Result<()>;
    async fn list_audit_logs(
        &self,
        execution_id: Option<Uuid>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<AuditLogRecord>>;

    /// Sprint 9 follow-up: audit logs are the closest available proxy for
    /// "traces produced by an agent in a time window" while a dedicated
    /// `TraceStore` is still pending. Default impl chains
    /// `list_executions(agent) → list_audit_logs(exec_id)` and filters by
    /// `record.timestamp` (audit log's own time field, not the execution's).
    /// Native-indexed backends should override.
    async fn list_audit_logs_by_agent_window(
        &self,
        agent_id: &str,
        window_start: DateTime<Utc>,
        window_end: DateTime<Utc>,
    ) -> Result<Vec<AuditLogRecord>> {
        let executions = self
            .list_executions(Some(agent_id.to_string()), 1000, 0)
            .await?;
        let mut out = Vec::new();
        for exec in executions {
            // Skip executions clearly out of window — saves the inner
            // list_audit_logs call. We compare exec.started_at since audit
            // logs cluster around execution time.
            if exec.started_at >= window_end {
                continue;
            }
            let logs = self.list_audit_logs(Some(exec.id), 1000, 0).await?;
            for log in logs {
                if log.timestamp >= window_start && log.timestamp < window_end {
                    out.push(log);
                }
            }
        }
        Ok(out)
    }

    // Sprint 10 (gradual landing): TraceStore + JournalStore methods.
    // Default impls return empty/Ok so external StateStore implementations
    // don't need to change immediately. InMemoryStateStore overrides these.

    /// Persist a trace record. Default: no-op (silently dropped) for stores
    /// that don't yet have a trace table.
    async fn save_trace(&self, record: TraceRecord) -> Result<()> {
        let _ = record;
        Ok(())
    }

    /// Per-agent + window query for traces. Default: empty Vec (legacy stores
    /// fall back to the audit-log proxy).
    async fn list_traces_by_agent_window(
        &self,
        agent_id: &str,
        window_start: DateTime<Utc>,
        window_end: DateTime<Utc>,
    ) -> Result<Vec<TraceRecord>> {
        let _ = (agent_id, window_start, window_end);
        Ok(Vec::new())
    }

    /// Persist a journal iteration record. Default: no-op.
    async fn save_journal_iteration(&self, record: JournalRecord) -> Result<()> {
        let _ = record;
        Ok(())
    }

    /// Per-agent + window query for journal iterations. Default: empty Vec.
    async fn list_journal_iterations_by_agent_window(
        &self,
        agent_id: &str,
        window_start: DateTime<Utc>,
        window_end: DateTime<Utc>,
    ) -> Result<Vec<JournalRecord>> {
        let _ = (agent_id, window_start, window_end);
        Ok(Vec::new())
    }

    // Policy CRUD
    async fn save_policy(&self, record: PolicyRecord) -> Result<()>;
    async fn get_policy(&self, name: &str) -> Result<PolicyRecord>;
    async fn list_policies(&self, active_only: bool) -> Result<Vec<PolicyRecord>>;
    async fn update_policy(&self, name: &str, active: bool) -> Result<()>;
}

// PostgreSQL implementation (feature-gated)
#[cfg(feature = "postgres")]
pub use postgres_impl::{PostgresConfig, PostgresMemoryStore, PostgresStateStore};

#[cfg(feature = "postgres")]
mod postgres_impl {
    use super::*;
    use crate::memory_store::{LeveledMemoryRecord, LeveledMemoryStore, MemoryLevel};
    use std::collections::HashMap;

    /// PostgreSQL 连接池配置
    #[derive(Debug, Clone)]
    pub struct PostgresConfig {
        /// 数据库连接 URL
        pub database_url: String,
        /// 最大连接数（默认 10）
        pub max_connections: u32,
        /// 空闲连接超时秒数（默认 300）
        pub idle_timeout_seconds: u64,
        /// 连接获取超时秒数（默认 30）
        pub acquire_timeout_seconds: u64,
    }

    impl PostgresConfig {
        /// 使用数据库 URL 创建默认配置
        pub fn new(database_url: impl Into<String>) -> Self {
            Self {
                database_url: database_url.into(),
                max_connections: 10,
                idle_timeout_seconds: 300,
                acquire_timeout_seconds: 30,
            }
        }

        /// 验证配置有效性
        pub fn validate(&self) -> Result<()> {
            if self.database_url.is_empty() {
                return Err(StoreError::ConnectionError(
                    "database_url 不能为空".to_string(),
                ));
            }
            if self.max_connections == 0 {
                return Err(StoreError::ConnectionError(
                    "max_connections 必须大于 0".to_string(),
                ));
            }
            if self.idle_timeout_seconds == 0 {
                return Err(StoreError::ConnectionError(
                    "idle_timeout_seconds 必须大于 0".to_string(),
                ));
            }
            if self.acquire_timeout_seconds == 0 {
                return Err(StoreError::ConnectionError(
                    "acquire_timeout_seconds 必须大于 0".to_string(),
                ));
            }
            Ok(())
        }
    }

    /// PostgreSQL 状态存储实现
    pub struct PostgresStateStore {
        pool: sqlx::PgPool,
    }

    impl PostgresStateStore {
        /// 使用连接 URL 创建（使用默认连接池配置）
        pub async fn new(database_url: &str) -> Result<Self> {
            let config = PostgresConfig::new(database_url);
            Self::with_config(config).await
        }

        /// 使用完整配置创建 PostgreSQL 存储实例
        pub async fn with_config(config: PostgresConfig) -> Result<Self> {
            config.validate()?;

            let pool = sqlx::postgres::PgPoolOptions::new()
                .max_connections(config.max_connections)
                .idle_timeout(std::time::Duration::from_secs(config.idle_timeout_seconds))
                .acquire_timeout(std::time::Duration::from_secs(
                    config.acquire_timeout_seconds,
                ))
                .connect(&config.database_url)
                .await
                .map_err(|e| StoreError::ConnectionError(e.to_string()))?;

            Ok(Self { pool })
        }

        /// 连接池健康检查
        ///
        /// 尝试获取一个连接并执行简单查询，验证数据库可达且连接池正常。
        pub async fn health_check(&self) -> Result<()> {
            sqlx::query("SELECT 1")
                .execute(&self.pool)
                .await
                .map_err(|e| StoreError::ConnectionError(format!("健康检查失败: {}", e)))?;
            Ok(())
        }

        /// 执行数据库迁移
        ///
        /// 使用幂等 SQL 语句（CREATE TABLE IF NOT EXISTS）创建所有必要的表结构。
        /// 多次执行不会产生错误。
        pub async fn run_migrations(&self) -> Result<()> {
            sqlx::query(
                r#"
                CREATE TABLE IF NOT EXISTS executions (
                    id UUID PRIMARY KEY,
                    agent_id TEXT NOT NULL,
                    skill_id TEXT,
                    status TEXT NOT NULL,
                    input JSONB NOT NULL,
                    output JSONB,
                    error TEXT,
                    started_at TIMESTAMPTZ NOT NULL,
                    completed_at TIMESTAMPTZ,
                    updated_at TIMESTAMPTZ
                )
                "#,
            )
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::ConnectionError(format!("创建 executions 表失败: {}", e)))?;

            sqlx::query(
                r#"
                CREATE TABLE IF NOT EXISTS artifacts (
                    id UUID PRIMARY KEY,
                    execution_id UUID NOT NULL,
                    artifact_type TEXT NOT NULL,
                    data JSONB NOT NULL,
                    metadata JSONB,
                    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
                )
                "#,
            )
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::ConnectionError(format!("创建 artifacts 表失败: {}", e)))?;

            sqlx::query(
                r#"
                CREATE TABLE IF NOT EXISTS audit_logs (
                    id UUID PRIMARY KEY,
                    execution_id UUID,
                    event_type TEXT NOT NULL,
                    actor TEXT,
                    action TEXT NOT NULL,
                    resource TEXT,
                    details JSONB,
                    timestamp TIMESTAMPTZ NOT NULL
                )
                "#,
            )
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::ConnectionError(format!("创建 audit_logs 表失败: {}", e)))?;

            sqlx::query(
                r#"
                CREATE TABLE IF NOT EXISTS policies (
                    id UUID PRIMARY KEY,
                    name TEXT NOT NULL UNIQUE,
                    effect TEXT NOT NULL,
                    conditions JSONB NOT NULL,
                    active BOOLEAN NOT NULL DEFAULT TRUE,
                    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
                )
                "#,
            )
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::ConnectionError(format!("创建 policies 表失败: {}", e)))?;

            sqlx::query(
                r#"
                CREATE TABLE IF NOT EXISTS leveled_memories (
                    id TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    agent_id TEXT NOT NULL,
                    level TEXT NOT NULL,
                    key TEXT NOT NULL,
                    content JSONB NOT NULL,
                    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                    ttl_seconds BIGINT,
                    expires_at TIMESTAMPTZ
                )
                "#,
            )
            .execute(&self.pool)
            .await
            .map_err(|e| {
                StoreError::ConnectionError(format!("创建 leveled_memories 表失败: {}", e))
            })?;

            // 命名空间隔离索引（agent_id + session_id）
            sqlx::query(
                r#"
                CREATE INDEX IF NOT EXISTS idx_leveled_memories_namespace
                ON leveled_memories (agent_id, session_id)
                "#,
            )
            .execute(&self.pool)
            .await
            .map_err(|e| {
                StoreError::ConnectionError(format!("创建 leveled_memories 索引失败: {}", e))
            })?;

            Ok(())
        }

        /// 获取数据库连接池引用
        pub fn pool(&self) -> &sqlx::PgPool {
            &self.pool
        }
    }

    /// Execution 数据库行结构
    #[derive(Debug, sqlx::FromRow)]
    struct ExecutionRow {
        id: Uuid,
        agent_id: String,
        skill_id: Option<String>,
        status: String,
        input: Value,
        output: Option<Value>,
        error: Option<String>,
        started_at: DateTime<Utc>,
        completed_at: Option<DateTime<Utc>>,
    }

    /// Artifact 数据库行结构
    #[derive(Debug, sqlx::FromRow)]
    struct ArtifactRow {
        id: Uuid,
        execution_id: Uuid,
        artifact_type: String,
        data: Value,
        metadata: Option<Value>,
    }

    /// AuditLog 数据库行结构
    #[derive(Debug, sqlx::FromRow)]
    struct AuditLogRow {
        id: Uuid,
        execution_id: Option<Uuid>,
        event_type: String,
        actor: Option<String>,
        action: String,
        resource: Option<String>,
        details: Option<Value>,
        timestamp: DateTime<Utc>,
    }

    /// Policy 数据库行结构
    #[derive(Debug, sqlx::FromRow)]
    struct PolicyRow {
        id: Uuid,
        name: String,
        effect: String,
        conditions: Value,
        active: bool,
        created_at: DateTime<Utc>,
        updated_at: DateTime<Utc>,
    }

    #[async_trait]
    impl StateStore for PostgresStateStore {
        async fn save_execution(&self, record: ExecutionRecord) -> Result<()> {
            sqlx::query(
                r#"
                INSERT INTO executions (id, agent_id, skill_id, status, input, output, error, started_at, completed_at)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                "#
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

        async fn get_execution(&self, id: Uuid) -> Result<ExecutionRecord> {
            let row: ExecutionRow = sqlx::query_as(
                r#"
                SELECT id, agent_id, skill_id, status, input, output, error, started_at, completed_at
                FROM executions
                WHERE id = $1
                "#
            )
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| StoreError::NotFound(format!("Execution {} not found", id)))?;

            Ok(ExecutionRecord {
                id: row.id,
                agent_id: row.agent_id,
                skill_id: row.skill_id,
                status: row.status,
                input: row.input,
                output: row.output,
                error: row.error,
                started_at: row.started_at,
                completed_at: row.completed_at,
            })
        }

        async fn update_execution(
            &self,
            id: Uuid,
            status: String,
            output: Option<Value>,
            error: Option<String>,
        ) -> Result<()> {
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
        ) -> Result<Vec<ExecutionRecord>> {
            let rows: Vec<ExecutionRow> = if let Some(aid) = agent_id {
                sqlx::query_as(
                    r#"
                    SELECT id, agent_id, skill_id, status, input, output, error, started_at, completed_at
                    FROM executions
                    WHERE agent_id = $1
                    ORDER BY created_at DESC
                    LIMIT $2 OFFSET $3
                    "#
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
                    "#
                )
                .bind(limit as i64)
                .bind(offset as i64)
                .fetch_all(&self.pool)
                .await?
            };

            Ok(rows
                .into_iter()
                .map(|row| ExecutionRecord {
                    id: row.id,
                    agent_id: row.agent_id,
                    skill_id: row.skill_id,
                    status: row.status,
                    input: row.input,
                    output: row.output,
                    error: row.error,
                    started_at: row.started_at,
                    completed_at: row.completed_at,
                })
                .collect())
        }

        async fn save_artifact(&self, record: ArtifactRecord) -> Result<()> {
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

        async fn list_artifacts(&self, execution_id: Uuid) -> Result<Vec<ArtifactRecord>> {
            let rows: Vec<ArtifactRow> = sqlx::query_as(
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

            Ok(rows
                .into_iter()
                .map(|row| ArtifactRecord {
                    id: row.id,
                    execution_id: row.execution_id,
                    artifact_type: row.artifact_type,
                    data: row.data,
                    metadata: row.metadata,
                })
                .collect())
        }

        async fn save_audit_log(&self, record: AuditLogRecord) -> Result<()> {
            sqlx::query(
                r#"
                INSERT INTO audit_logs (id, execution_id, event_type, actor, action, resource, details, timestamp)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                "#
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
        ) -> Result<Vec<AuditLogRecord>> {
            let rows: Vec<AuditLogRow> = if let Some(eid) = execution_id {
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

            Ok(rows
                .into_iter()
                .map(|row| AuditLogRecord {
                    id: row.id,
                    execution_id: row.execution_id,
                    event_type: row.event_type,
                    actor: row.actor,
                    action: row.action,
                    resource: row.resource,
                    details: row.details,
                    timestamp: row.timestamp,
                })
                .collect())
        }

        async fn save_policy(&self, record: PolicyRecord) -> Result<()> {
            sqlx::query(
                r#"
                INSERT INTO policies (id, name, effect, conditions, active)
                VALUES ($1, $2, $3, $4, $5)
                "#,
            )
            .bind(record.id)
            .bind(record.name)
            .bind(record.effect)
            .bind(record.conditions)
            .bind(record.active)
            .execute(&self.pool)
            .await?;

            Ok(())
        }

        async fn get_policy(&self, name: &str) -> Result<PolicyRecord> {
            let row: PolicyRow = sqlx::query_as(
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

            Ok(PolicyRecord {
                id: row.id,
                name: row.name,
                effect: row.effect,
                conditions: row.conditions,
                active: row.active,
                created_at: row.created_at,
                updated_at: row.updated_at,
            })
        }

        async fn list_policies(&self, active_only: bool) -> Result<Vec<PolicyRecord>> {
            let rows: Vec<PolicyRow> = if active_only {
                sqlx::query_as(
                    r#"
                    SELECT id, name, effect, conditions, active, created_at, updated_at
                    FROM policies
                    WHERE active = true
                    ORDER BY created_at DESC
                    "#,
                )
                .fetch_all(&self.pool)
                .await?
            } else {
                sqlx::query_as(
                    r#"
                    SELECT id, name, effect, conditions, active, created_at, updated_at
                    FROM policies
                    ORDER BY created_at DESC
                    "#,
                )
                .fetch_all(&self.pool)
                .await?
            };

            Ok(rows
                .into_iter()
                .map(|row| PolicyRecord {
                    id: row.id,
                    name: row.name,
                    effect: row.effect,
                    conditions: row.conditions,
                    active: row.active,
                    created_at: row.created_at,
                    updated_at: row.updated_at,
                })
                .collect())
        }

        async fn update_policy(&self, name: &str, active: bool) -> Result<()> {
            sqlx::query(
                r#"
                UPDATE policies
                SET active = $2, updated_at = NOW()
                WHERE name = $1
                "#,
            )
            .bind(name)
            .bind(active)
            .execute(&self.pool)
            .await?;

            Ok(())
        }
    }

    // -----------------------------------------------------------------------
    // PostgresMemoryStore — LeveledMemoryStore 的 PostgreSQL 实现
    // -----------------------------------------------------------------------

    /// LeveledMemoryRecord 数据库行结构
    #[derive(Debug, sqlx::FromRow)]
    struct LeveledMemoryRow {
        id: String,
        session_id: String,
        agent_id: String,
        level: String,
        key: String,
        content: Value,
        created_at: DateTime<Utc>,
        updated_at: DateTime<Utc>,
        ttl_seconds: Option<i64>,
    }

    impl LeveledMemoryRow {
        fn into_record(self) -> LeveledMemoryRecord {
            let level = match self.level.as_str() {
                "L0Full" => MemoryLevel::L0Full,
                "L1Summary" => MemoryLevel::L1Summary,
                "L2Metadata" => MemoryLevel::L2Metadata,
                _ => MemoryLevel::L0Full,
            };
            LeveledMemoryRecord {
                id: self.id,
                session_id: self.session_id,
                agent_id: self.agent_id,
                level,
                key: self.key,
                content: self.content,
                created_at: self.created_at,
                updated_at: self.updated_at,
                ttl_seconds: self.ttl_seconds,
                source_execution_id: None,
                embedding: None,
                tags: Vec::new(),
            }
        }
    }

    /// PostgreSQL 分层记忆存储实现
    pub struct PostgresMemoryStore {
        pool: sqlx::PgPool,
    }

    impl PostgresMemoryStore {
        /// 使用连接 URL 创建（使用默认连接池配置）
        pub async fn new(database_url: &str) -> Result<Self> {
            let config = PostgresConfig::new(database_url);
            Self::with_config(config).await
        }

        /// 使用完整配置创建
        pub async fn with_config(config: PostgresConfig) -> Result<Self> {
            config.validate()?;

            let pool = sqlx::postgres::PgPoolOptions::new()
                .max_connections(config.max_connections)
                .idle_timeout(std::time::Duration::from_secs(config.idle_timeout_seconds))
                .acquire_timeout(std::time::Duration::from_secs(
                    config.acquire_timeout_seconds,
                ))
                .connect(&config.database_url)
                .await
                .map_err(|e| StoreError::ConnectionError(e.to_string()))?;

            Ok(Self { pool })
        }

        /// 从已有连接池创建
        pub fn from_pool(pool: sqlx::PgPool) -> Self {
            Self { pool }
        }

        /// 连接池健康检查
        pub async fn health_check(&self) -> Result<()> {
            sqlx::query("SELECT 1")
                .execute(&self.pool)
                .await
                .map_err(|e| StoreError::ConnectionError(format!("健康检查失败: {}", e)))?;
            Ok(())
        }

        /// 计算 expires_at 时间戳
        pub(crate) fn compute_expires_at(
            created_at: DateTime<Utc>,
            ttl_seconds: Option<i64>,
        ) -> Option<DateTime<Utc>> {
            ttl_seconds.map(|ttl| created_at + chrono::Duration::seconds(ttl))
        }
    }

    #[async_trait]
    impl LeveledMemoryStore for PostgresMemoryStore {
        async fn store_leveled(&self, record: LeveledMemoryRecord) -> Result<()> {
            let level_str = record.level.to_string();
            let expires_at = Self::compute_expires_at(record.created_at, record.ttl_seconds);

            sqlx::query(
                r#"
                INSERT INTO leveled_memories (id, session_id, agent_id, level, key, content, created_at, updated_at, ttl_seconds, expires_at)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
                ON CONFLICT (id) DO UPDATE SET
                    content = EXCLUDED.content,
                    level = EXCLUDED.level,
                    updated_at = EXCLUDED.updated_at,
                    ttl_seconds = EXCLUDED.ttl_seconds,
                    expires_at = EXCLUDED.expires_at
                "#,
            )
            .bind(&record.id)
            .bind(&record.session_id)
            .bind(&record.agent_id)
            .bind(&level_str)
            .bind(&record.key)
            .bind(&record.content)
            .bind(record.created_at)
            .bind(record.updated_at)
            .bind(record.ttl_seconds)
            .bind(expires_at)
            .execute(&self.pool)
            .await?;

            Ok(())
        }

        async fn query_by_level(
            &self,
            session_id: &str,
            level: MemoryLevel,
        ) -> Result<Vec<LeveledMemoryRecord>> {
            let level_str = level.to_string();
            let rows: Vec<LeveledMemoryRow> = sqlx::query_as(
                r#"
                SELECT id, session_id, agent_id, level, key, content, created_at, updated_at, ttl_seconds
                FROM leveled_memories
                WHERE session_id = $1 AND level = $2
                  AND (expires_at IS NULL OR expires_at > NOW())
                ORDER BY created_at DESC
                "#,
            )
            .bind(session_id)
            .bind(&level_str)
            .fetch_all(&self.pool)
            .await?;

            Ok(rows.into_iter().map(|r| r.into_record()).collect())
        }

        async fn query_by_key(
            &self,
            session_id: &str,
            key: &str,
        ) -> Result<Option<LeveledMemoryRecord>> {
            let row: Option<LeveledMemoryRow> = sqlx::query_as(
                r#"
                SELECT id, session_id, agent_id, level, key, content, created_at, updated_at, ttl_seconds
                FROM leveled_memories
                WHERE session_id = $1 AND key = $2
                  AND (expires_at IS NULL OR expires_at > NOW())
                "#,
            )
            .bind(session_id)
            .bind(key)
            .fetch_optional(&self.pool)
            .await?;

            Ok(row.map(|r| r.into_record()))
        }

        async fn promote(&self, id: &str, new_level: MemoryLevel) -> Result<()> {
            let level_str = new_level.to_string();
            let new_ttl = new_level.default_ttl_seconds();
            let now = Utc::now();
            let expires_at = Self::compute_expires_at(now, new_ttl);

            let result = sqlx::query(
                r#"
                UPDATE leveled_memories
                SET level = $2, ttl_seconds = $3, updated_at = $4, expires_at = $5
                WHERE id = $1
                "#,
            )
            .bind(id)
            .bind(&level_str)
            .bind(new_ttl)
            .bind(now)
            .bind(expires_at)
            .execute(&self.pool)
            .await?;

            if result.rows_affected() == 0 {
                return Err(StoreError::NotFound(format!(
                    "LeveledMemoryRecord {} not found",
                    id
                )));
            }
            Ok(())
        }

        async fn demote(&self, id: &str, new_level: MemoryLevel) -> Result<()> {
            // promote 和 demote 的实现逻辑相同，仅语义不同
            self.promote(id, new_level).await
        }

        async fn expire_stale(&self, max_age: chrono::Duration) -> Result<u64> {
            let cutoff = Utc::now() - max_age;

            let result = sqlx::query(
                r#"
                DELETE FROM leveled_memories
                WHERE level != 'L2Metadata'
                  AND (
                    (expires_at IS NOT NULL AND expires_at < NOW())
                    OR created_at < $1
                  )
                "#,
            )
            .bind(cutoff)
            .execute(&self.pool)
            .await?;

            Ok(result.rows_affected())
        }

        async fn count_by_level(&self, session_id: &str) -> Result<HashMap<MemoryLevel, usize>> {
            // 使用简单行结构查询各层级计数
            let rows: Vec<LevelCountRow> = sqlx::query_as(
                r#"
                SELECT level, COUNT(*)::BIGINT as count
                FROM leveled_memories
                WHERE session_id = $1
                  AND (expires_at IS NULL OR expires_at > NOW())
                GROUP BY level
                "#,
            )
            .bind(session_id)
            .fetch_all(&self.pool)
            .await?;

            let mut counts = HashMap::new();
            for row in rows {
                let level = match row.level.as_str() {
                    "L0Full" => MemoryLevel::L0Full,
                    "L1Summary" => MemoryLevel::L1Summary,
                    "L2Metadata" => MemoryLevel::L2Metadata,
                    _ => continue,
                };
                counts.insert(level, row.count as usize);
            }
            Ok(counts)
        }
    }

    /// 层级计数查询行结构
    #[derive(Debug, sqlx::FromRow)]
    struct LevelCountRow {
        level: String,
        count: i64,
    }
}

// ---------------------------------------------------------------------------
// PostgreSQL 后端测试（不依赖真实数据库）
// ---------------------------------------------------------------------------

#[cfg(test)]
#[cfg(feature = "postgres")]
mod postgres_tests {
    use super::*;

    // -- PostgresConfig 验证测试 --

    #[test]
    fn test_postgres_config_default_values() {
        let config = postgres_impl::PostgresConfig::new("postgres://localhost/test");
        assert_eq!(config.database_url, "postgres://localhost/test");
        assert_eq!(config.max_connections, 10);
        assert_eq!(config.idle_timeout_seconds, 300);
        assert_eq!(config.acquire_timeout_seconds, 30);
    }

    #[test]
    fn test_postgres_config_validate_empty_url() {
        let config = postgres_impl::PostgresConfig::new("");
        let result = config.validate();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("database_url"));
    }

    #[test]
    fn test_postgres_config_validate_zero_max_connections() {
        let mut config = postgres_impl::PostgresConfig::new("postgres://localhost/test");
        config.max_connections = 0;
        let result = config.validate();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("max_connections"));
    }

    #[test]
    fn test_postgres_config_validate_zero_idle_timeout() {
        let mut config = postgres_impl::PostgresConfig::new("postgres://localhost/test");
        config.idle_timeout_seconds = 0;
        let result = config.validate();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("idle_timeout_seconds"));
    }

    #[test]
    fn test_postgres_config_validate_zero_acquire_timeout() {
        let mut config = postgres_impl::PostgresConfig::new("postgres://localhost/test");
        config.acquire_timeout_seconds = 0;
        let result = config.validate();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("acquire_timeout_seconds"));
    }

    #[test]
    fn test_postgres_config_validate_success() {
        let config = postgres_impl::PostgresConfig::new("postgres://localhost/test");
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_postgres_config_custom_values() {
        let mut config = postgres_impl::PostgresConfig::new("postgres://db:5432/mydb");
        config.max_connections = 20;
        config.idle_timeout_seconds = 600;
        config.acquire_timeout_seconds = 60;
        assert!(config.validate().is_ok());
        assert_eq!(config.max_connections, 20);
        assert_eq!(config.idle_timeout_seconds, 600);
        assert_eq!(config.acquire_timeout_seconds, 60);
    }

    #[test]
    fn test_postgres_config_clone() {
        let config = postgres_impl::PostgresConfig::new("postgres://localhost/test");
        let cloned = config.clone();
        assert_eq!(cloned.database_url, config.database_url);
        assert_eq!(cloned.max_connections, config.max_connections);
        assert_eq!(cloned.idle_timeout_seconds, config.idle_timeout_seconds);
        assert_eq!(
            cloned.acquire_timeout_seconds,
            config.acquire_timeout_seconds
        );
    }

    // -- SQL 查询构建 / MemoryLevel 序列化测试 --

    #[test]
    fn test_memory_level_to_string_roundtrip() {
        use crate::memory_store::MemoryLevel;

        // 验证 MemoryLevel::to_string 产出正确的数据库值
        assert_eq!(MemoryLevel::L0Full.to_string(), "L0Full");
        assert_eq!(MemoryLevel::L1Summary.to_string(), "L1Summary");
        assert_eq!(MemoryLevel::L2Metadata.to_string(), "L2Metadata");
    }

    #[test]
    fn test_compute_expires_at_with_ttl() {
        let now = chrono::Utc::now();
        let expires = postgres_impl::PostgresMemoryStore::compute_expires_at(now, Some(3600));
        assert!(expires.is_some());
        let diff = expires.unwrap() - now;
        assert_eq!(diff.num_seconds(), 3600);
    }

    #[test]
    fn test_compute_expires_at_without_ttl() {
        let now = chrono::Utc::now();
        let expires = postgres_impl::PostgresMemoryStore::compute_expires_at(now, None);
        assert!(expires.is_none());
    }

    // -- 错误映射测试 --

    #[test]
    fn test_store_error_not_found_display() {
        let err = crate::error::StoreError::NotFound("Policy x not found".to_string());
        assert_eq!(err.to_string(), "Not found: Policy x not found");
    }

    #[test]
    fn test_store_error_connection_display() {
        let err = crate::error::StoreError::ConnectionError("connection refused".to_string());
        assert_eq!(err.to_string(), "Connection error: connection refused");
    }
}
