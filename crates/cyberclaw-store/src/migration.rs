//! Migration system and backend selection for CyberClaw storage layer.
//!
//! Provides versioned schema migrations for SQLite (and future Postgres) backends,
//! plus a [`StoreBackend`] enum and [`create_store`] factory for runtime backend selection.
//!
//! # Backend selection
//!
//! Set `CYBERCLAW_STORE_BACKEND` to one of:
//! - `"memory"` (default) — in-memory store, no persistence
//! - `"sqlite"` — SQLite file store, path from `CYBERCLAW_SQLITE_PATH` (default `cyberclaw.db`)
//! - `"postgres"` — PostgreSQL, connection URL from `DATABASE_URL` (not yet implemented)

use crate::error::{Result, StoreError};
use crate::state_store::StateStore;
use crate::InMemoryStateStore;

/// A single versioned migration.
#[derive(Debug, Clone)]
pub struct MigrationVersion {
    /// Monotonically increasing version number.
    pub version: u32,
    /// Human-readable migration name.
    pub name: String,
    /// SQL statement(s) to execute.
    pub sql: String,
    /// When this migration was applied (populated after querying the DB).
    pub applied_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Manages an ordered list of migrations and applies them to a database.
pub struct MigrationRunner {
    migrations: Vec<MigrationVersion>,
}

impl MigrationRunner {
    /// Create a new runner pre-loaded with built-in CyberClaw migrations.
    pub fn new() -> Self {
        Self {
            migrations: builtin_migrations(),
        }
    }

    /// Append a custom migration. The caller must ensure version numbers are unique
    /// and higher than any built-in migration.
    pub fn add(&mut self, migration: MigrationVersion) {
        self.migrations.push(migration);
        self.migrations.sort_by_key(|m| m.version);
    }

    /// Return references to migrations whose versions are not in `applied`.
    pub fn pending<'a>(&'a self, applied: &[u32]) -> Vec<&'a MigrationVersion> {
        self.migrations
            .iter()
            .filter(|m| !applied.contains(&m.version))
            .collect()
    }

    /// Execute all pending migrations against a rusqlite connection.
    ///
    /// Returns the list of version numbers that were applied during this call.
    #[cfg(feature = "sqlite")]
    pub fn run_sqlite(&self, conn: &rusqlite::Connection) -> Result<Vec<u32>> {
        // Ensure the tracking table exists before anything else.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS _migrations (
                version INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                applied_at TEXT NOT NULL
            )",
        )
        .map_err(|e| StoreError::MigrationError(format!("create _migrations table: {e}")))?;

        let applied = Self::get_applied_sqlite_inner(conn)?;
        let pending = self.pending(&applied);

        let mut newly_applied = Vec::new();
        for migration in pending {
            // Execute the migration SQL
            conn.execute_batch(&migration.sql).map_err(|e| {
                StoreError::MigrationError(format!(
                    "V{:03}_{}: {e}",
                    migration.version, migration.name
                ))
            })?;

            // Record the migration
            let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
            conn.execute(
                "INSERT INTO _migrations (version, name, applied_at) VALUES (?1, ?2, ?3)",
                rusqlite::params![migration.version as i64, &migration.name, &now],
            )
            .map_err(|e| {
                StoreError::MigrationError(format!(
                    "record V{:03}_{}: {e}",
                    migration.version, migration.name
                ))
            })?;

            newly_applied.push(migration.version);
        }

        Ok(newly_applied)
    }

    /// Query which migration versions have already been applied.
    #[cfg(feature = "sqlite")]
    pub fn get_applied_sqlite(conn: &rusqlite::Connection) -> Result<Vec<u32>> {
        // Ensure the tracking table exists so the query doesn't fail on a fresh DB.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS _migrations (
                version INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                applied_at TEXT NOT NULL
            )",
        )
        .map_err(|e| StoreError::MigrationError(format!("create _migrations table: {e}")))?;

        Self::get_applied_sqlite_inner(conn)
    }

    /// Inner helper — assumes `_migrations` table already exists.
    #[cfg(feature = "sqlite")]
    fn get_applied_sqlite_inner(conn: &rusqlite::Connection) -> Result<Vec<u32>> {
        let mut stmt = conn
            .prepare("SELECT version FROM _migrations ORDER BY version ASC")
            .map_err(|e| StoreError::MigrationError(format!("query _migrations: {e}")))?;

        let versions = stmt
            .query_map([], |row: &rusqlite::Row<'_>| {
                let v: i64 = row.get(0)?;
                Ok(v as u32)
            })
            .map_err(|e| StoreError::MigrationError(format!("query _migrations: {e}")))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| StoreError::MigrationError(format!("read _migrations rows: {e}")))?;

        Ok(versions)
    }
}

impl Default for MigrationRunner {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Built-in migrations
// ---------------------------------------------------------------------------

fn builtin_migrations() -> Vec<MigrationVersion> {
    vec![
        MigrationVersion {
            version: 1,
            name: "initial".to_string(),
            sql: "CREATE TABLE IF NOT EXISTS _migrations (
                version INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                applied_at TEXT NOT NULL
            )"
            .to_string(),
            applied_at: None,
        },
        MigrationVersion {
            version: 2,
            name: "core_tables".to_string(),
            sql: concat!(
                "CREATE TABLE IF NOT EXISTS executions (",
                "id TEXT PRIMARY KEY,",
                "agent_id TEXT NOT NULL,",
                "skill_id TEXT,",
                "status TEXT NOT NULL,",
                "input TEXT NOT NULL,",
                "output TEXT,",
                "error TEXT,",
                "started_at TEXT NOT NULL,",
                "completed_at TEXT",
                ");\n",
                "CREATE TABLE IF NOT EXISTS artifacts (",
                "id TEXT PRIMARY KEY,",
                "execution_id TEXT NOT NULL,",
                "artifact_type TEXT NOT NULL,",
                "data TEXT NOT NULL,",
                "metadata TEXT,",
                "created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
                ");\n",
                "CREATE TABLE IF NOT EXISTS audit_logs (",
                "id TEXT PRIMARY KEY,",
                "execution_id TEXT,",
                "event_type TEXT NOT NULL,",
                "actor TEXT,",
                "action TEXT NOT NULL,",
                "resource TEXT,",
                "details TEXT,",
                "timestamp TEXT NOT NULL",
                ");"
            )
            .to_string(),
            applied_at: None,
        },
        MigrationVersion {
            version: 3,
            name: "policy_tables".to_string(),
            sql: concat!(
                "CREATE TABLE IF NOT EXISTS policies (",
                "id TEXT PRIMARY KEY,",
                "name TEXT NOT NULL UNIQUE,",
                "effect TEXT NOT NULL,",
                "conditions TEXT NOT NULL,",
                "active INTEGER NOT NULL DEFAULT 1,",
                "created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),",
                "updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
                ");"
            )
            .to_string(),
            applied_at: None,
        },
        MigrationVersion {
            version: 4,
            name: "memory_tables".to_string(),
            sql: concat!(
                "CREATE TABLE IF NOT EXISTS memories (",
                "id TEXT PRIMARY KEY,",
                "agent_id TEXT NOT NULL,",
                "key TEXT NOT NULL,",
                "value TEXT NOT NULL,",
                "memory_type TEXT NOT NULL DEFAULT 'semantic',",
                "created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),",
                "updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),",
                "UNIQUE(agent_id, key)",
                ");\n",
                "CREATE TABLE IF NOT EXISTS sessions (",
                "id TEXT PRIMARY KEY,",
                "agent_id TEXT NOT NULL,",
                "started_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),",
                "ended_at TEXT,",
                "metadata TEXT",
                ");"
            )
            .to_string(),
            applied_at: None,
        },
        MigrationVersion {
            version: 5,
            name: "skill_archive".to_string(),
            sql: concat!(
                "CREATE TABLE IF NOT EXISTS skill_variants (",
                "variant_id TEXT PRIMARY KEY,",
                "skill_id TEXT NOT NULL,",
                "parent_variant_id TEXT,",
                "score REAL NOT NULL,",
                "child_count INTEGER NOT NULL DEFAULT 0,",
                "track TEXT NOT NULL,",
                "patch_artifact_id TEXT,",
                "created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
                ");\n",
                "CREATE INDEX IF NOT EXISTS idx_skill_variants_skill_id ON skill_variants(skill_id);\n",
                "CREATE INDEX IF NOT EXISTS idx_skill_variants_parent ON skill_variants(parent_variant_id);"
            )
            .to_string(),
            applied_at: None,
        },
        MigrationVersion {
            version: 6,
            name: "semantic_memory".to_string(),
            sql: concat!(
                "CREATE TABLE IF NOT EXISTS semantic_memory (",
                "id TEXT PRIMARY KEY,",
                "scope_json TEXT NOT NULL,",
                "kind TEXT NOT NULL,",
                "content TEXT NOT NULL,",
                "rules_json TEXT,",
                "provenance_json TEXT,",
                "created_at TEXT NOT NULL,",
                "ttl_secs INTEGER",
                ");\n",
                "CREATE INDEX IF NOT EXISTS idx_sm_scope ON semantic_memory(scope_json);\n",
                "CREATE INDEX IF NOT EXISTS idx_sm_created_at ON semantic_memory(created_at);"
            )
            .to_string(),
            applied_at: None,
        },
    ]
}

// ---------------------------------------------------------------------------
// Backend selection
// ---------------------------------------------------------------------------

/// Storage backend descriptor, typically derived from environment variables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreBackend {
    /// In-memory store — no persistence, suitable for testing.
    Memory,
    /// SQLite file-based store.
    Sqlite {
        /// Filesystem path to the SQLite database file.
        path: String,
    },
    /// PostgreSQL store (not yet implemented).
    Postgres {
        /// Connection URL (e.g. `postgres://user:pass@host/db`).
        url: String,
    },
}

impl StoreBackend {
    /// Derive a [`StoreBackend`] from environment variables.
    ///
    /// | `CYBERCLAW_STORE_BACKEND` | Result |
    /// |--------------------------|--------|
    /// | absent or `"memory"` | `Memory` |
    /// | `"sqlite"` | `Sqlite` with path from `CYBERCLAW_SQLITE_PATH` (default `cyberclaw.db`) |
    /// | `"postgres"` | `Postgres` with url from `DATABASE_URL` |
    pub fn from_env() -> Self {
        match std::env::var("CYBERCLAW_STORE_BACKEND")
            .unwrap_or_default()
            .to_lowercase()
            .as_str()
        {
            "sqlite" => StoreBackend::Sqlite {
                path: std::env::var("CYBERCLAW_SQLITE_PATH")
                    .unwrap_or_else(|_| "cyberclaw.db".to_string()),
            },
            "postgres" => StoreBackend::Postgres {
                url: std::env::var("DATABASE_URL").unwrap_or_default(),
            },
            // "memory" or anything else
            _ => StoreBackend::Memory,
        }
    }
}

/// Create a [`StateStore`] implementation for the given backend.
///
/// - `Memory` — returns an [`InMemoryStateStore`]
/// - `Sqlite` — opens/creates the database, runs migrations, returns [`SqliteStateStore`]
/// - `Postgres` — currently returns an error (not yet implemented)
pub async fn create_store(backend: StoreBackend) -> Result<Box<dyn StateStore>> {
    match backend {
        StoreBackend::Memory => Ok(Box::new(InMemoryStateStore::new())),

        #[cfg(feature = "sqlite")]
        StoreBackend::Sqlite { path } => {
            let store = crate::sqlite::SqliteStateStore::new(&path)?;
            Ok(Box::new(store))
        }

        #[cfg(not(feature = "sqlite"))]
        StoreBackend::Sqlite { .. } => Err(StoreError::InternalError(
            "SQLite backend requested but the 'sqlite' feature is not enabled".to_string(),
        )),

        #[cfg(feature = "postgres")]
        StoreBackend::Postgres { url } => {
            let config = crate::state_store::PostgresConfig::new(url);
            let store = crate::state_store::PostgresStateStore::with_config(config).await?;
            store.run_migrations().await?;
            Ok(Box::new(store))
        }

        #[cfg(not(feature = "postgres"))]
        StoreBackend::Postgres { .. } => Err(StoreError::InternalError(
            "Postgres backend requested but the 'postgres' feature is not enabled".to_string(),
        )),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serializes tests that modify environment variables.
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn test_builtin_migrations_ordered() {
        let migrations = builtin_migrations();
        for window in migrations.windows(2) {
            assert!(
                window[0].version < window[1].version,
                "migrations must be strictly ordered: V{} should come before V{}",
                window[0].version,
                window[1].version
            );
        }
    }

    #[test]
    fn test_pending_all_when_none_applied() {
        let runner = MigrationRunner::new();
        let pending = runner.pending(&[]);
        assert_eq!(pending.len(), 6);
        assert_eq!(pending[0].version, 1);
        assert_eq!(pending[5].version, 6);
    }

    #[test]
    fn test_pending_skips_applied() {
        let runner = MigrationRunner::new();
        let pending = runner.pending(&[1, 2]);
        assert_eq!(pending.len(), 4);
        assert_eq!(pending[0].version, 3);
        assert_eq!(pending[3].version, 6);
    }

    #[test]
    fn test_pending_none_when_all_applied() {
        let runner = MigrationRunner::new();
        let pending = runner.pending(&[1, 2, 3, 4, 5, 6]);
        assert!(pending.is_empty());
    }

    #[test]
    fn test_add_custom_migration() {
        let mut runner = MigrationRunner::new();
        runner.add(MigrationVersion {
            version: 100,
            name: "custom".to_string(),
            sql: "SELECT 1".to_string(),
            applied_at: None,
        });
        let pending = runner.pending(&[1, 2, 3, 4, 5, 6]);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].version, 100);
    }

    #[test]
    fn test_from_env_default_is_memory() {
        let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        // Clear any existing value
        std::env::remove_var("CYBERCLAW_STORE_BACKEND");
        let backend = StoreBackend::from_env();
        assert_eq!(backend, StoreBackend::Memory);
    }

    #[test]
    fn test_from_env_memory_explicit() {
        let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("CYBERCLAW_STORE_BACKEND", "memory");
        let backend = StoreBackend::from_env();
        assert_eq!(backend, StoreBackend::Memory);
        std::env::remove_var("CYBERCLAW_STORE_BACKEND");
    }

    #[test]
    fn test_from_env_sqlite() {
        let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("CYBERCLAW_STORE_BACKEND", "sqlite");
        std::env::remove_var("CYBERCLAW_SQLITE_PATH");
        let backend = StoreBackend::from_env();
        assert_eq!(
            backend,
            StoreBackend::Sqlite {
                path: "cyberclaw.db".to_string()
            }
        );
        std::env::remove_var("CYBERCLAW_STORE_BACKEND");
    }

    #[test]
    fn test_from_env_sqlite_custom_path() {
        let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("CYBERCLAW_STORE_BACKEND", "sqlite");
        std::env::set_var("CYBERCLAW_SQLITE_PATH", "/tmp/test.db");
        let backend = StoreBackend::from_env();
        assert_eq!(
            backend,
            StoreBackend::Sqlite {
                path: "/tmp/test.db".to_string()
            }
        );
        std::env::remove_var("CYBERCLAW_STORE_BACKEND");
        std::env::remove_var("CYBERCLAW_SQLITE_PATH");
    }

    #[test]
    fn test_from_env_postgres() {
        let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("CYBERCLAW_STORE_BACKEND", "postgres");
        std::env::set_var("DATABASE_URL", "postgres://localhost/test");
        let backend = StoreBackend::from_env();
        assert_eq!(
            backend,
            StoreBackend::Postgres {
                url: "postgres://localhost/test".to_string()
            }
        );
        std::env::remove_var("CYBERCLAW_STORE_BACKEND");
        std::env::remove_var("DATABASE_URL");
    }

    #[tokio::test]
    async fn test_create_store_memory() {
        let store = create_store(StoreBackend::Memory).await.unwrap();
        // Verify it works by saving and retrieving an execution
        let record = crate::state_store::ExecutionRecord {
            id: uuid::Uuid::new_v4(),
            agent_id: "test".to_string(),
            skill_id: None,
            status: "running".to_string(),
            input: serde_json::json!({}),
            output: None,
            error: None,
            started_at: chrono::Utc::now(),
            completed_at: None,
        };
        store.save_execution(record.clone()).await.unwrap();
        let retrieved = store.get_execution(record.id).await.unwrap();
        assert_eq!(retrieved.id, record.id);
    }

    #[tokio::test]
    async fn test_create_store_postgres_connection_error() {
        let result = create_store(StoreBackend::Postgres {
            url: "postgres://invalid:5432/nonexistent".to_string(),
        })
        .await;
        assert!(result.is_err());
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn test_run_sqlite_migrations() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();

        let runner = MigrationRunner::new();

        // First run: all 6 migrations applied
        let applied = runner.run_sqlite(&conn).unwrap();
        assert_eq!(applied, vec![1, 2, 3, 4, 5, 6]);

        // Second run: nothing pending
        let applied = runner.run_sqlite(&conn).unwrap();
        assert!(applied.is_empty());

        // Verify tracking table has all versions
        let versions = MigrationRunner::get_applied_sqlite(&conn).unwrap();
        assert_eq!(versions, vec![1, 2, 3, 4, 5, 6]);
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn test_sqlite_migration_creates_tables() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();

        let runner = MigrationRunner::new();
        runner.run_sqlite(&conn).unwrap();

        // Verify all expected tables exist by querying sqlite_master
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap();
        let table_names: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();

        let table_refs: Vec<&str> = table_names.iter().map(|s| s.as_str()).collect();
        assert!(table_refs.contains(&"_migrations"), "missing _migrations");
        assert!(table_refs.contains(&"executions"), "missing executions");
        assert!(table_refs.contains(&"artifacts"), "missing artifacts");
        assert!(table_refs.contains(&"audit_logs"), "missing audit_logs");
        assert!(table_refs.contains(&"policies"), "missing policies");
        assert!(table_refs.contains(&"memories"), "missing memories");
        assert!(table_refs.contains(&"sessions"), "missing sessions");
        assert!(
            table_refs.contains(&"skill_variants"),
            "missing skill_variants"
        );
        assert!(
            table_refs.contains(&"semantic_memory"),
            "missing semantic_memory"
        );
    }
}
