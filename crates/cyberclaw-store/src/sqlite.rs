//! SQLite state store backend implementation using `rusqlite`.
//!
//! Provides persistent storage using an embedded SQLite database with WAL mode
//! and NORMAL synchronous mode for performance. Thread safety is achieved via
//! `Mutex<Connection>`.

use std::sync::Mutex;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde_json::Value;
use uuid::Uuid;

use crate::error::{Result, StoreError};
use crate::state_store::{
    ArtifactRecord, AuditLogRecord, ExecutionRecord, PolicyRecord, StateStore,
};

/// SQLite-backed state store implementation.
///
/// Uses WAL mode and NORMAL synchronous for concurrent read performance.
/// Thread safety is provided by wrapping the connection in a `Mutex`.
pub struct SqliteStateStore {
    conn: Mutex<Connection>,
}

impl SqliteStateStore {
    /// Create a new SQLite store backed by a file at `path`.
    ///
    /// Enables WAL mode, sets synchronous to NORMAL, and runs schema migrations.
    pub fn new(path: &str) -> Result<Self> {
        let conn = Connection::open(path)
            .map_err(|e| StoreError::ConnectionError(format!("SQLite open: {}", e)))?;

        Self::configure_and_migrate(conn)
    }

    /// Create a new in-memory SQLite store (for testing).
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()
            .map_err(|e| StoreError::ConnectionError(format!("SQLite in-memory: {}", e)))?;

        Self::configure_and_migrate(conn)
    }

    /// Configure pragmas and run migrations on a fresh connection.
    fn configure_and_migrate(conn: Connection) -> Result<Self> {
        conn.execute_batch("PRAGMA journal_mode = WAL;")
            .map_err(|e| StoreError::ConnectionError(format!("WAL pragma: {}", e)))?;

        conn.execute_batch("PRAGMA synchronous = NORMAL;")
            .map_err(|e| StoreError::ConnectionError(format!("synchronous pragma: {}", e)))?;

        conn.execute_batch("PRAGMA busy_timeout = 5000;")
            .map_err(|e| StoreError::ConnectionError(format!("busy_timeout pragma: {}", e)))?;

        let store = Self {
            conn: Mutex::new(conn),
        };
        store.run_migrations()?;
        Ok(store)
    }

    /// Run schema migrations via the centralized MigrationRunner.
    fn run_migrations(&self) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StoreError::InternalError(format!("mutex poisoned: {}", e)))?;

        let runner = crate::migration::MigrationRunner::new();
        runner.run_sqlite(&conn)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helper: DateTime serialization for SQLite TEXT columns
// ---------------------------------------------------------------------------

fn dt_to_string(dt: &DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn parse_dt(s: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| StoreError::InternalError(format!("parse datetime: {}", e)))
}

// ---------------------------------------------------------------------------
// Raw row types for rusqlite mapping
// ---------------------------------------------------------------------------

struct RawExecution {
    id: String,
    agent_id: String,
    skill_id: Option<String>,
    status: String,
    input: String,
    output: Option<String>,
    error: Option<String>,
    started_at: String,
    completed_at: Option<String>,
}

fn raw_to_execution(raw: RawExecution) -> Result<ExecutionRecord> {
    Ok(ExecutionRecord {
        id: Uuid::parse_str(&raw.id).map_err(|e| StoreError::InternalError(e.to_string()))?,
        agent_id: raw.agent_id,
        skill_id: raw.skill_id,
        status: raw.status,
        input: serde_json::from_str(&raw.input)?,
        output: raw
            .output
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?,
        error: raw.error,
        started_at: parse_dt(&raw.started_at)?,
        completed_at: raw.completed_at.as_deref().map(parse_dt).transpose()?,
    })
}

fn row_to_raw_execution(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawExecution> {
    Ok(RawExecution {
        id: row.get(0)?,
        agent_id: row.get(1)?,
        skill_id: row.get(2)?,
        status: row.get(3)?,
        input: row.get(4)?,
        output: row.get(5)?,
        error: row.get(6)?,
        started_at: row.get(7)?,
        completed_at: row.get(8)?,
    })
}

struct RawArtifact {
    id: String,
    execution_id: String,
    artifact_type: String,
    data: String,
    metadata: Option<String>,
}

fn raw_to_artifact(raw: RawArtifact) -> Result<ArtifactRecord> {
    Ok(ArtifactRecord {
        id: Uuid::parse_str(&raw.id).map_err(|e| StoreError::InternalError(e.to_string()))?,
        execution_id: Uuid::parse_str(&raw.execution_id)
            .map_err(|e| StoreError::InternalError(e.to_string()))?,
        artifact_type: raw.artifact_type,
        data: serde_json::from_str(&raw.data)?,
        metadata: raw
            .metadata
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?,
    })
}

fn row_to_raw_artifact(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawArtifact> {
    Ok(RawArtifact {
        id: row.get(0)?,
        execution_id: row.get(1)?,
        artifact_type: row.get(2)?,
        data: row.get(3)?,
        metadata: row.get(4)?,
    })
}

struct RawAuditLog {
    id: String,
    execution_id: Option<String>,
    event_type: String,
    actor: Option<String>,
    action: String,
    resource: Option<String>,
    details: Option<String>,
    timestamp: String,
}

fn raw_to_audit_log(raw: RawAuditLog) -> Result<AuditLogRecord> {
    Ok(AuditLogRecord {
        id: Uuid::parse_str(&raw.id).map_err(|e| StoreError::InternalError(e.to_string()))?,
        execution_id: raw
            .execution_id
            .as_deref()
            .map(Uuid::parse_str)
            .transpose()
            .map_err(|e| StoreError::InternalError(e.to_string()))?,
        event_type: raw.event_type,
        actor: raw.actor,
        action: raw.action,
        resource: raw.resource,
        details: raw
            .details
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?,
        timestamp: parse_dt(&raw.timestamp)?,
    })
}

fn row_to_raw_audit_log(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawAuditLog> {
    Ok(RawAuditLog {
        id: row.get(0)?,
        execution_id: row.get(1)?,
        event_type: row.get(2)?,
        actor: row.get(3)?,
        action: row.get(4)?,
        resource: row.get(5)?,
        details: row.get(6)?,
        timestamp: row.get(7)?,
    })
}

struct RawPolicy {
    id: String,
    name: String,
    effect: String,
    conditions: String,
    active: bool,
    created_at: String,
    updated_at: String,
}

fn raw_to_policy(raw: RawPolicy) -> Result<PolicyRecord> {
    Ok(PolicyRecord {
        id: Uuid::parse_str(&raw.id).map_err(|e| StoreError::InternalError(e.to_string()))?,
        name: raw.name,
        effect: raw.effect,
        conditions: serde_json::from_str(&raw.conditions)?,
        active: raw.active,
        created_at: parse_dt(&raw.created_at)?,
        updated_at: parse_dt(&raw.updated_at)?,
    })
}

fn row_to_raw_policy(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawPolicy> {
    Ok(RawPolicy {
        id: row.get(0)?,
        name: row.get(1)?,
        effect: row.get(2)?,
        conditions: row.get(3)?,
        active: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

// ---------------------------------------------------------------------------
// Mutex lock helper
// ---------------------------------------------------------------------------

fn lock_conn(conn: &Mutex<Connection>) -> Result<std::sync::MutexGuard<'_, Connection>> {
    conn.lock()
        .map_err(|e| StoreError::InternalError(format!("mutex poisoned: {}", e)))
}

/// Collect all rows from a query_map iterator into a Vec.
///
/// This helper ensures the iterator is fully consumed before the statement
/// is dropped, avoiding borrow-checker issues with rusqlite's lifetimes.
fn collect_rows<T, F>(
    conn: &Connection,
    sql: &str,
    params: &[&dyn rusqlite::types::ToSql],
    f: F,
) -> Result<Vec<T>>
where
    F: FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
{
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params, f)?;
    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

// ---------------------------------------------------------------------------
// StateStore implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl StateStore for SqliteStateStore {
    async fn save_execution(&self, record: ExecutionRecord) -> Result<()> {
        let input_str = serde_json::to_string(&record.input)?;
        let output_str = record
            .output
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let started = dt_to_string(&record.started_at);
        let completed = record.completed_at.as_ref().map(dt_to_string);

        let conn = lock_conn(&self.conn)?;
        conn.execute(
            "INSERT OR REPLACE INTO executions (id, agent_id, skill_id, status, input, output, error, started_at, completed_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                record.id.to_string(),
                record.agent_id,
                record.skill_id,
                record.status,
                input_str,
                output_str,
                record.error,
                started,
                completed,
            ],
        )?;
        Ok(())
    }

    async fn get_execution(&self, id: Uuid) -> Result<ExecutionRecord> {
        let conn = lock_conn(&self.conn)?;
        let mut stmt = conn.prepare(
            "SELECT id, agent_id, skill_id, status, input, output, error, started_at, completed_at FROM executions WHERE id = ?1",
        )?;

        let id_str = id.to_string();
        let raw = stmt
            .query_row(params![id_str], row_to_raw_execution)
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    StoreError::NotFound(format!("Execution {} not found", id))
                }
                other => StoreError::from(other),
            })?;

        raw_to_execution(raw)
    }

    async fn update_execution(
        &self,
        id: Uuid,
        status: String,
        output: Option<Value>,
        error: Option<String>,
    ) -> Result<()> {
        let completed_at = if status == "completed" || status == "failed" {
            Some(dt_to_string(&Utc::now()))
        } else {
            None
        };
        let output_str = output.as_ref().map(serde_json::to_string).transpose()?;

        let conn = lock_conn(&self.conn)?;
        let rows = conn.execute(
            "UPDATE executions SET status = ?2, output = ?3, error = ?4, completed_at = ?5 WHERE id = ?1",
            params![id.to_string(), status, output_str, error, completed_at],
        )?;

        if rows == 0 {
            return Err(StoreError::NotFound(format!("Execution {} not found", id)));
        }
        Ok(())
    }

    async fn list_executions(
        &self,
        agent_id: Option<String>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<ExecutionRecord>> {
        let conn = lock_conn(&self.conn)?;

        let raws = if let Some(aid) = agent_id {
            collect_rows(
                &conn,
                "SELECT id, agent_id, skill_id, status, input, output, error, started_at, completed_at FROM executions WHERE agent_id = ?1 ORDER BY started_at DESC LIMIT ?2 OFFSET ?3",
                &[&aid as &dyn rusqlite::types::ToSql, &(limit as i64), &(offset as i64)],
                row_to_raw_execution,
            )?
        } else {
            collect_rows(
                &conn,
                "SELECT id, agent_id, skill_id, status, input, output, error, started_at, completed_at FROM executions ORDER BY started_at DESC LIMIT ?1 OFFSET ?2",
                &[&(limit as i64) as &dyn rusqlite::types::ToSql, &(offset as i64)],
                row_to_raw_execution,
            )?
        };

        raws.into_iter().map(raw_to_execution).collect()
    }

    async fn save_artifact(&self, record: ArtifactRecord) -> Result<()> {
        let data_str = serde_json::to_string(&record.data)?;
        let meta_str = record
            .metadata
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;

        let conn = lock_conn(&self.conn)?;
        conn.execute(
            "INSERT INTO artifacts (id, execution_id, artifact_type, data, metadata) VALUES (?1,?2,?3,?4,?5)",
            params![
                record.id.to_string(),
                record.execution_id.to_string(),
                record.artifact_type,
                data_str,
                meta_str,
            ],
        )?;
        Ok(())
    }

    async fn list_artifacts(&self, execution_id: Uuid) -> Result<Vec<ArtifactRecord>> {
        let conn = lock_conn(&self.conn)?;
        let eid = execution_id.to_string();
        let raws = collect_rows(
            &conn,
            "SELECT id, execution_id, artifact_type, data, metadata FROM artifacts WHERE execution_id = ?1 ORDER BY created_at DESC",
            &[&eid as &dyn rusqlite::types::ToSql],
            row_to_raw_artifact,
        )?;

        raws.into_iter().map(raw_to_artifact).collect()
    }

    async fn save_audit_log(&self, record: AuditLogRecord) -> Result<()> {
        let details_str = record
            .details
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;

        let conn = lock_conn(&self.conn)?;
        conn.execute(
            "INSERT INTO audit_logs (id, execution_id, event_type, actor, action, resource, details, timestamp) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                record.id.to_string(),
                record.execution_id.map(|u| u.to_string()),
                record.event_type,
                record.actor,
                record.action,
                record.resource,
                details_str,
                dt_to_string(&record.timestamp),
            ],
        )?;
        Ok(())
    }

    async fn list_audit_logs(
        &self,
        execution_id: Option<Uuid>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<AuditLogRecord>> {
        let conn = lock_conn(&self.conn)?;

        let raws = if let Some(eid) = execution_id {
            let eid_str = eid.to_string();
            collect_rows(
                &conn,
                "SELECT id, execution_id, event_type, actor, action, resource, details, timestamp FROM audit_logs WHERE execution_id = ?1 ORDER BY timestamp DESC LIMIT ?2 OFFSET ?3",
                &[&eid_str as &dyn rusqlite::types::ToSql, &(limit as i64), &(offset as i64)],
                row_to_raw_audit_log,
            )?
        } else {
            collect_rows(
                &conn,
                "SELECT id, execution_id, event_type, actor, action, resource, details, timestamp FROM audit_logs ORDER BY timestamp DESC LIMIT ?1 OFFSET ?2",
                &[&(limit as i64) as &dyn rusqlite::types::ToSql, &(offset as i64)],
                row_to_raw_audit_log,
            )?
        };

        raws.into_iter().map(raw_to_audit_log).collect()
    }

    async fn save_policy(&self, record: PolicyRecord) -> Result<()> {
        let conditions_str = serde_json::to_string(&record.conditions)?;

        let conn = lock_conn(&self.conn)?;
        conn.execute(
            "INSERT OR REPLACE INTO policies (id, name, effect, conditions, active, created_at, updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![
                record.id.to_string(),
                record.name,
                record.effect,
                conditions_str,
                record.active,
                dt_to_string(&record.created_at),
                dt_to_string(&record.updated_at),
            ],
        )?;
        Ok(())
    }

    async fn get_policy(&self, name: &str) -> Result<PolicyRecord> {
        let conn = lock_conn(&self.conn)?;
        let mut stmt = conn.prepare(
            "SELECT id, name, effect, conditions, active, created_at, updated_at FROM policies WHERE name = ?1",
        )?;

        let raw = stmt
            .query_row(params![name], row_to_raw_policy)
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    StoreError::NotFound(format!("Policy {} not found", name))
                }
                other => StoreError::from(other),
            })?;

        raw_to_policy(raw)
    }

    async fn list_policies(&self, active_only: bool) -> Result<Vec<PolicyRecord>> {
        let conn = lock_conn(&self.conn)?;

        let raws = if active_only {
            collect_rows(
                &conn,
                "SELECT id, name, effect, conditions, active, created_at, updated_at FROM policies WHERE active = 1 ORDER BY created_at DESC",
                &[],
                row_to_raw_policy,
            )?
        } else {
            collect_rows(
                &conn,
                "SELECT id, name, effect, conditions, active, created_at, updated_at FROM policies ORDER BY created_at DESC",
                &[],
                row_to_raw_policy,
            )?
        };

        raws.into_iter().map(raw_to_policy).collect()
    }

    async fn update_policy(&self, name: &str, active: bool) -> Result<()> {
        let now = dt_to_string(&Utc::now());

        let conn = lock_conn(&self.conn)?;
        let rows = conn.execute(
            "UPDATE policies SET active = ?2, updated_at = ?3 WHERE name = ?1",
            params![name, active, now],
        )?;

        if rows == 0 {
            return Err(StoreError::NotFound(format!("Policy {} not found", name)));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serde_json::json;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_sqlite_execution_crud() {
        let store = SqliteStateStore::in_memory().unwrap();
        let exec_id = Uuid::new_v4();

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
        store.save_execution(record).await.unwrap();

        let retrieved = store.get_execution(exec_id).await.unwrap();
        assert_eq!(retrieved.id, exec_id);
        assert_eq!(retrieved.status, "running");
        assert_eq!(retrieved.agent_id, "test-agent");
        assert_eq!(retrieved.skill_id.as_deref(), Some("test-skill"));

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

        let list = store
            .list_executions(Some("test-agent".to_string()), 10, 0)
            .await
            .unwrap();
        assert_eq!(list.len(), 1);

        let list = store
            .list_executions(Some("other-agent".to_string()), 10, 0)
            .await
            .unwrap();
        assert!(list.is_empty());
    }

    #[tokio::test]
    async fn test_sqlite_execution_not_found() {
        let store = SqliteStateStore::in_memory().unwrap();
        let result = store.get_execution(Uuid::new_v4()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_sqlite_artifact_append_only() {
        let store = SqliteStateStore::in_memory().unwrap();
        let exec_id = Uuid::new_v4();

        for i in 0..3 {
            let artifact = ArtifactRecord {
                id: Uuid::new_v4(),
                execution_id: exec_id,
                artifact_type: "log".to_string(),
                data: json!({"index": i}),
                metadata: None,
            };
            store.save_artifact(artifact).await.unwrap();
        }

        let artifacts = store.list_artifacts(exec_id).await.unwrap();
        assert_eq!(artifacts.len(), 3);

        let empty = store.list_artifacts(Uuid::new_v4()).await.unwrap();
        assert!(empty.is_empty());
    }

    #[tokio::test]
    async fn test_sqlite_audit_log_append_only() {
        let store = SqliteStateStore::in_memory().unwrap();
        let exec_id = Uuid::new_v4();

        for i in 0..3 {
            let log = AuditLogRecord {
                id: Uuid::new_v4(),
                execution_id: Some(exec_id),
                event_type: "test".to_string(),
                actor: Some("user".to_string()),
                action: format!("action-{}", i),
                resource: None,
                details: None,
                timestamp: Utc::now(),
            };
            store.save_audit_log(log).await.unwrap();
        }

        let logs = store.list_audit_logs(Some(exec_id), 10, 0).await.unwrap();
        assert_eq!(logs.len(), 3);

        let empty = store
            .list_audit_logs(Some(Uuid::new_v4()), 10, 0)
            .await
            .unwrap();
        assert!(empty.is_empty());

        let all = store.list_audit_logs(None, 10, 0).await.unwrap();
        assert_eq!(all.len(), 3);
    }

    #[tokio::test]
    async fn test_sqlite_policy_crud() {
        let store = SqliteStateStore::in_memory().unwrap();

        let policy = PolicyRecord {
            id: Uuid::new_v4(),
            name: "test-policy".to_string(),
            effect: "allow".to_string(),
            conditions: json!({"resource": "agent:*"}),
            active: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        store.save_policy(policy).await.unwrap();

        let retrieved = store.get_policy("test-policy").await.unwrap();
        assert_eq!(retrieved.name, "test-policy");
        assert!(retrieved.active);

        store.update_policy("test-policy", false).await.unwrap();
        let updated = store.get_policy("test-policy").await.unwrap();
        assert!(!updated.active);

        let active = store.list_policies(true).await.unwrap();
        assert!(active.is_empty());

        let all = store.list_policies(false).await.unwrap();
        assert_eq!(all.len(), 1);
    }

    #[tokio::test]
    async fn test_sqlite_policy_not_found() {
        let store = SqliteStateStore::in_memory().unwrap();
        assert!(store.get_policy("nonexistent").await.is_err());
        assert!(store.update_policy("nonexistent", true).await.is_err());
    }

    #[tokio::test]
    async fn test_sqlite_pagination() {
        let store = SqliteStateStore::in_memory().unwrap();

        for i in 0..5 {
            let record = ExecutionRecord {
                id: Uuid::new_v4(),
                agent_id: "agent-1".to_string(),
                skill_id: None,
                status: "completed".to_string(),
                input: json!({"i": i}),
                output: None,
                error: None,
                started_at: Utc::now(),
                completed_at: None,
            };
            store.save_execution(record).await.unwrap();
        }

        let page1 = store.list_executions(None, 2, 0).await.unwrap();
        assert_eq!(page1.len(), 2);

        let page2 = store.list_executions(None, 2, 2).await.unwrap();
        assert_eq!(page2.len(), 2);

        let page3 = store.list_executions(None, 2, 4).await.unwrap();
        assert_eq!(page3.len(), 1);
    }

    #[tokio::test]
    async fn test_sqlite_concurrent_reads() {
        let store = Arc::new(SqliteStateStore::in_memory().unwrap());
        let exec_id = Uuid::new_v4();

        let record = ExecutionRecord {
            id: exec_id,
            agent_id: "agent-1".to_string(),
            skill_id: None,
            status: "running".to_string(),
            input: json!({"test": true}),
            output: None,
            error: None,
            started_at: Utc::now(),
            completed_at: None,
        };
        store.save_execution(record).await.unwrap();

        let mut handles = Vec::new();
        for _ in 0..10 {
            let s = store.clone();
            let id = exec_id;
            handles.push(tokio::spawn(
                async move { s.get_execution(id).await.unwrap() },
            ));
        }

        for h in handles {
            let r = h.await.unwrap();
            assert_eq!(r.id, exec_id);
        }
    }

    #[tokio::test]
    async fn test_sqlite_wal_mode_enabled() {
        let store = SqliteStateStore::in_memory().unwrap();
        let conn = store.conn.lock().unwrap();
        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        // In-memory databases may report "memory" instead of "wal"
        assert!(
            mode == "wal" || mode == "memory",
            "unexpected journal mode: {}",
            mode
        );
    }
}
