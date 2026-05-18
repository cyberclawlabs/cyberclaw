//! Semantic memory persistent store (Sprint 10 L1).
//!
//! Replaces the Sprint 9 filesystem placeholder ([`crate::daily_digest_runtime::FileDigestRepository`]
//! in `cyberclaw-control-plane`) with a real persistence surface that backs
//! any write-through caller (Daily Digest, Reflection, Free-text notes,
//! Procedural rule candidates).
//!
//! # Why this lives in `cyberclaw-store` (not core)
//!
//! `cyberclaw-store` sits below `cyberclaw-control-plane` in the dependency
//! graph and **does not** depend on `cyberclaw-core`. To keep that boundary
//! intact, IDs and enums are carried as strings in [`SemanticMemoryEntry`]
//! (mirroring how [`crate::skill_archive_repository::SkillVariantRecord`] is
//! structured). The control-plane side is responsible for stringifying
//! `AgentId` / `ExecutionId` / `ArtifactId` / `TraceId` when writing.
//!
//! # Scope shape
//!
//! [`MemoryScope`] is a string-tagged discriminated union:
//!
//! - `Agent(agent_id)` — per-agent digests / reflections / notes.
//! - `Tenant(tenant_id)` — cross-agent but within a tenant.
//! - `Global` — platform-wide.
//!
//! The on-disk representation is the JSON form of [`MemoryScope`], stored in
//! a single TEXT column so the SQL layer stays agnostic to new variants.
//!
//! # TTL
//!
//! `ttl_secs` is applied **lazily on read** (see [`SemanticMemoryStore::sweep_expired`]
//! for the explicit GC entry point). `insert`/`get` never mutate expired rows;
//! queries simply filter them out.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::Result;

// ============================================================================
// DTOs
// ============================================================================

/// Scope of a semantic-memory entry.
///
/// Tagged JSON form: `{"kind":"agent","id":"agent-alpha"}` / `{"kind":"tenant","id":"t-01"}` /
/// `{"kind":"global"}`. Stored as a single TEXT column (`scope_json`) so
/// adding new variants does not require a schema migration.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MemoryScope {
    /// Per-agent scope. `id` is the stringified `AgentId`.
    Agent { id: String },
    /// Per-tenant scope. `id` is the stringified tenant identifier.
    Tenant { id: String },
    /// Platform-global scope (no id).
    Global,
}

impl MemoryScope {
    /// Convenience constructor mirroring `AgentId`-flavored call sites.
    pub fn agent(id: impl Into<String>) -> Self {
        Self::Agent { id: id.into() }
    }

    /// Convenience constructor.
    pub fn tenant(id: impl Into<String>) -> Self {
        Self::Tenant { id: id.into() }
    }

    /// Returns the agent id component if this is an `Agent` scope.
    pub fn agent_id(&self) -> Option<&str> {
        match self {
            Self::Agent { id } => Some(id.as_str()),
            _ => None,
        }
    }
}

/// The kind of entry stored in semantic memory.
///
/// Represented as a TEXT column so new kinds can be added without a migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    /// Output of the `daily-digest` skill (Stage 5 persist).
    DailyDigest,
    /// Reflection record produced by a reflection loop.
    Reflection,
    /// A procedural-memory rule.
    Rule,
    /// Free-form curated text (agent notes, user profile excerpts, ...).
    FreeText,
}

impl MemoryKind {
    /// Stable string representation used in the SQLite `kind` column.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DailyDigest => "daily_digest",
            Self::Reflection => "reflection",
            Self::Rule => "rule",
            Self::FreeText => "free_text",
        }
    }
}

/// A procedural-memory rule candidate attached to a semantic entry.
///
/// Mirrors `cyberclaw-control-plane::daily_digest::RuleCandidate` but uses
/// strings for IDs so this crate stays core-free.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProceduralRule {
    /// Rule text (≤ 100 chars per daily-digest SKILL.md).
    pub rule: String,
    /// Stringified `ExecutionId`s that supported this rule.
    pub source_executions: Vec<String>,
}

/// Provenance trail attached to every entry.
///
/// All IDs are strings so the store does not depend on `cyberclaw-core`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryProvenance {
    /// Source executions (stringified `ExecutionId`s).
    pub source_executions: Vec<String>,
    /// Source artifacts (stringified `ArtifactId`s).
    pub source_artifacts: Vec<String>,
    /// Optional reflection trace (stringified `TraceId`).
    pub reflection_trace_id: Option<String>,
}

/// One durable semantic-memory record.
///
/// The `id` is client-provided (the control-plane typically picks a stable
/// `{agent}-{YYYY-MM-DD}` for digests so re-runs upsert). Implementations MUST
/// treat duplicate `id` as an upsert.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticMemoryEntry {
    /// Stable identifier; upsert on collision.
    pub id: String,
    /// Who can see this row.
    pub scope: MemoryScope,
    /// What kind of entry this is.
    pub kind: MemoryKind,
    /// Markdown or serialized JSON body.
    pub content: String,
    /// Structured rule candidates (may be empty).
    pub rules: Vec<ProceduralRule>,
    /// Source trail.
    pub provenance: MemoryProvenance,
    /// Creation timestamp (also the ORDER BY key).
    pub created_at: DateTime<Utc>,
    /// Optional TTL in seconds from `created_at`. `None` = never expires.
    pub ttl_secs: Option<i64>,
}

impl SemanticMemoryEntry {
    /// Returns `true` when `now` is past `created_at + ttl_secs`.
    pub fn is_expired_at(&self, now: DateTime<Utc>) -> bool {
        match self.ttl_secs {
            Some(secs) if secs >= 0 => {
                let age = now.signed_duration_since(self.created_at).num_seconds();
                age > secs
            }
            _ => false,
        }
    }
}

// ============================================================================
// Trait
// ============================================================================

/// Persistence interface for semantic memory.
///
/// Implementations MUST:
/// - Upsert on `insert` (duplicate `id` overwrites the prior row).
/// - Filter expired rows out of all read paths (never surface a row where
///   `created_at + ttl_secs < now`).
/// - Return results in **most-recent-first** order (`created_at DESC`).
#[async_trait]
pub trait SemanticMemoryStore: Send + Sync {
    /// Upsert one entry; returns the stored id.
    async fn insert(&self, entry: SemanticMemoryEntry) -> Result<String>;

    /// Fetch one entry by id. Returns `Ok(None)` for unknown ids or when the
    /// row has expired.
    async fn get(&self, id: &str) -> Result<Option<SemanticMemoryEntry>>;

    /// All non-expired entries for a scope, newest first, capped at `limit`.
    async fn query_by_scope(
        &self,
        scope: &MemoryScope,
        limit: usize,
    ) -> Result<Vec<SemanticMemoryEntry>>;

    /// All non-expired entries for an agent whose `created_at >= since`,
    /// newest first, capped at `limit`.
    async fn query_by_agent_window(
        &self,
        agent_id: &str,
        since: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<SemanticMemoryEntry>>;

    /// Physically delete every row whose TTL has elapsed. Returns the number
    /// of rows removed. Implementations may call this opportunistically or
    /// leave it to an external sweeper.
    async fn sweep_expired(&self) -> Result<u64>;
}

// ============================================================================
// SQLite implementation (feature `sqlite`)
// ============================================================================

#[cfg(feature = "sqlite")]
pub use sqlite_impl::SqliteSemanticMemoryStore;

#[cfg(feature = "sqlite")]
mod sqlite_impl {
    use super::*;
    use crate::error::StoreError;
    use rusqlite::{params, Connection};
    use std::sync::Mutex;

    /// rusqlite-backed [`SemanticMemoryStore`].
    ///
    /// Uses the same `Mutex<Connection>` + WAL + NORMAL-sync pattern as
    /// [`crate::sqlite::SqliteStateStore`] and
    /// [`crate::skill_archive_repository::SqliteSkillArchiveRepository`].
    pub struct SqliteSemanticMemoryStore {
        conn: Mutex<Connection>,
    }

    impl SqliteSemanticMemoryStore {
        /// Open a store backed by `path` (file created if missing).
        pub fn new(path: &str) -> Result<Self> {
            let conn = Connection::open(path)
                .map_err(|e| StoreError::ConnectionError(format!("SQLite open: {}", e)))?;
            Self::configure_and_migrate(conn)
        }

        /// Open an in-memory store (tests / ephemeral).
        pub fn in_memory() -> Result<Self> {
            let conn = Connection::open_in_memory()
                .map_err(|e| StoreError::ConnectionError(format!("SQLite in-memory: {}", e)))?;
            Self::configure_and_migrate(conn)
        }

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

        fn run_migrations(&self) -> Result<()> {
            let conn = self
                .conn
                .lock()
                .map_err(|e| StoreError::InternalError(format!("mutex poisoned: {}", e)))?;
            let runner = crate::migration::MigrationRunner::new();
            runner.run_sqlite(&conn)?;
            Ok(())
        }

        fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
            self.conn
                .lock()
                .map_err(|e| StoreError::InternalError(format!("mutex poisoned: {}", e)))
        }
    }

    fn dt_to_string(dt: &DateTime<Utc>) -> String {
        dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    }

    fn parse_dt(s: &str) -> Result<DateTime<Utc>> {
        DateTime::parse_from_rfc3339(s)
            .map(|d| d.with_timezone(&Utc))
            .map_err(|e| StoreError::InternalError(format!("parse datetime: {}", e)))
    }

    fn row_to_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawRow> {
        Ok(RawRow {
            id: row.get(0)?,
            scope_json: row.get(1)?,
            kind: row.get(2)?,
            content: row.get(3)?,
            rules_json: row.get(4)?,
            provenance_json: row.get(5)?,
            created_at: row.get(6)?,
            ttl_secs: row.get(7)?,
        })
    }

    struct RawRow {
        id: String,
        scope_json: String,
        kind: String,
        content: String,
        rules_json: Option<String>,
        provenance_json: Option<String>,
        created_at: String,
        ttl_secs: Option<i64>,
    }

    fn raw_to_entry(raw: RawRow) -> Result<SemanticMemoryEntry> {
        let scope: MemoryScope = serde_json::from_str(&raw.scope_json)?;
        let kind = parse_kind(&raw.kind)?;
        let rules: Vec<ProceduralRule> = match raw.rules_json.as_deref() {
            Some(s) => serde_json::from_str(s)?,
            None => Vec::new(),
        };
        let provenance: MemoryProvenance = match raw.provenance_json.as_deref() {
            Some(s) => serde_json::from_str(s)?,
            None => MemoryProvenance::default(),
        };
        Ok(SemanticMemoryEntry {
            id: raw.id,
            scope,
            kind,
            content: raw.content,
            rules,
            provenance,
            created_at: parse_dt(&raw.created_at)?,
            ttl_secs: raw.ttl_secs,
        })
    }

    fn parse_kind(s: &str) -> Result<MemoryKind> {
        match s {
            "daily_digest" => Ok(MemoryKind::DailyDigest),
            "reflection" => Ok(MemoryKind::Reflection),
            "rule" => Ok(MemoryKind::Rule),
            "free_text" => Ok(MemoryKind::FreeText),
            other => Err(StoreError::InternalError(format!(
                "unknown MemoryKind tag: {other}"
            ))),
        }
    }

    /// SQL fragment that drops TTL-expired rows from any `SELECT`.
    ///
    /// Uses julianday arithmetic so TTL checks run entirely in SQLite without
    /// needing to materialise each row. `?N` is the RFC3339 `now` bind.
    const NOT_EXPIRED_CLAUSE: &str = "(ttl_secs IS NULL OR \
         (julianday(?1) - julianday(created_at)) * 86400.0 <= ttl_secs)";

    #[async_trait]
    impl SemanticMemoryStore for SqliteSemanticMemoryStore {
        async fn insert(&self, entry: SemanticMemoryEntry) -> Result<String> {
            let conn = self.lock()?;
            let scope_json = serde_json::to_string(&entry.scope)?;
            let kind = entry.kind.as_str();
            let rules_json = if entry.rules.is_empty() {
                None
            } else {
                Some(serde_json::to_string(&entry.rules)?)
            };
            let prov_json = serde_json::to_string(&entry.provenance)?;
            conn.execute(
                "INSERT INTO semantic_memory \
                 (id, scope_json, kind, content, rules_json, provenance_json, created_at, ttl_secs) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
                 ON CONFLICT(id) DO UPDATE SET \
                     scope_json = excluded.scope_json, \
                     kind = excluded.kind, \
                     content = excluded.content, \
                     rules_json = excluded.rules_json, \
                     provenance_json = excluded.provenance_json, \
                     created_at = excluded.created_at, \
                     ttl_secs = excluded.ttl_secs",
                params![
                    entry.id,
                    scope_json,
                    kind,
                    entry.content,
                    rules_json,
                    prov_json,
                    dt_to_string(&entry.created_at),
                    entry.ttl_secs,
                ],
            )?;
            Ok(entry.id)
        }

        async fn get(&self, id: &str) -> Result<Option<SemanticMemoryEntry>> {
            let conn = self.lock()?;
            let now = dt_to_string(&Utc::now());
            let sql = format!(
                "SELECT id, scope_json, kind, content, rules_json, provenance_json, created_at, ttl_secs \
                 FROM semantic_memory WHERE id = ?2 AND {NOT_EXPIRED_CLAUSE}"
            );
            let mut stmt = conn.prepare(&sql)?;
            match stmt.query_row(params![now, id], row_to_entry) {
                Ok(raw) => raw_to_entry(raw).map(Some),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(e.into()),
            }
        }

        async fn query_by_scope(
            &self,
            scope: &MemoryScope,
            limit: usize,
        ) -> Result<Vec<SemanticMemoryEntry>> {
            let conn = self.lock()?;
            let now = dt_to_string(&Utc::now());
            let scope_json = serde_json::to_string(scope)?;
            let sql = format!(
                "SELECT id, scope_json, kind, content, rules_json, provenance_json, created_at, ttl_secs \
                 FROM semantic_memory \
                 WHERE scope_json = ?2 AND {NOT_EXPIRED_CLAUSE} \
                 ORDER BY created_at DESC LIMIT ?3"
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(params![now, scope_json, limit as i64], row_to_entry)?;
            let mut out = Vec::new();
            for r in rows {
                out.push(raw_to_entry(r?)?);
            }
            Ok(out)
        }

        async fn query_by_agent_window(
            &self,
            agent_id: &str,
            since: DateTime<Utc>,
            limit: usize,
        ) -> Result<Vec<SemanticMemoryEntry>> {
            let conn = self.lock()?;
            let now = dt_to_string(&Utc::now());
            // Match the JSON we emit for `MemoryScope::Agent { id }`.
            let scope_json = serde_json::to_string(&MemoryScope::agent(agent_id))?;
            let since_str = dt_to_string(&since);
            let sql = format!(
                "SELECT id, scope_json, kind, content, rules_json, provenance_json, created_at, ttl_secs \
                 FROM semantic_memory \
                 WHERE scope_json = ?2 AND created_at >= ?3 AND {NOT_EXPIRED_CLAUSE} \
                 ORDER BY created_at DESC LIMIT ?4"
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(
                params![now, scope_json, since_str, limit as i64],
                row_to_entry,
            )?;
            let mut out = Vec::new();
            for r in rows {
                out.push(raw_to_entry(r?)?);
            }
            Ok(out)
        }

        async fn sweep_expired(&self) -> Result<u64> {
            let conn = self.lock()?;
            let now = dt_to_string(&Utc::now());
            let removed = conn.execute(
                "DELETE FROM semantic_memory \
                 WHERE ttl_secs IS NOT NULL \
                   AND (julianday(?1) - julianday(created_at)) * 86400.0 > ttl_secs",
                params![now],
            )?;
            Ok(removed as u64)
        }
    }
}

// ============================================================================
// In-memory implementation (tests / ephemeral deployments)
// ============================================================================

/// Thread-safe in-memory [`SemanticMemoryStore`].
#[derive(Default)]
pub struct InMemorySemanticMemoryStore {
    inner: tokio::sync::RwLock<Vec<SemanticMemoryEntry>>,
}

impl InMemorySemanticMemoryStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl SemanticMemoryStore for InMemorySemanticMemoryStore {
    async fn insert(&self, entry: SemanticMemoryEntry) -> Result<String> {
        let mut guard = self.inner.write().await;
        let id = entry.id.clone();
        if let Some(existing) = guard.iter_mut().find(|r| r.id == entry.id) {
            *existing = entry;
        } else {
            guard.push(entry);
        }
        Ok(id)
    }

    async fn get(&self, id: &str) -> Result<Option<SemanticMemoryEntry>> {
        let guard = self.inner.read().await;
        let now = Utc::now();
        Ok(guard
            .iter()
            .find(|r| r.id == id && !r.is_expired_at(now))
            .cloned())
    }

    async fn query_by_scope(
        &self,
        scope: &MemoryScope,
        limit: usize,
    ) -> Result<Vec<SemanticMemoryEntry>> {
        let guard = self.inner.read().await;
        let now = Utc::now();
        let mut out: Vec<_> = guard
            .iter()
            .filter(|r| &r.scope == scope && !r.is_expired_at(now))
            .cloned()
            .collect();
        out.sort_by_key(|r| std::cmp::Reverse(r.created_at));
        out.truncate(limit);
        Ok(out)
    }

    async fn query_by_agent_window(
        &self,
        agent_id: &str,
        since: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<SemanticMemoryEntry>> {
        let guard = self.inner.read().await;
        let now = Utc::now();
        let mut out: Vec<_> = guard
            .iter()
            .filter(|r| {
                matches!(&r.scope, MemoryScope::Agent { id } if id == agent_id)
                    && r.created_at >= since
                    && !r.is_expired_at(now)
            })
            .cloned()
            .collect();
        out.sort_by_key(|r| std::cmp::Reverse(r.created_at));
        out.truncate(limit);
        Ok(out)
    }

    async fn sweep_expired(&self) -> Result<u64> {
        let mut guard = self.inner.write().await;
        let now = Utc::now();
        let before = guard.len();
        guard.retain(|r| !r.is_expired_at(now));
        Ok((before - guard.len()) as u64)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entry(id: &str, agent: &str, created_at: DateTime<Utc>) -> SemanticMemoryEntry {
        SemanticMemoryEntry {
            id: id.to_string(),
            scope: MemoryScope::agent(agent),
            kind: MemoryKind::DailyDigest,
            content: "# facts\n...\n# problems\n...\n# learnings\n...".to_string(),
            rules: vec![ProceduralRule {
                rule: "always run cargo fmt before commit".to_string(),
                source_executions: vec!["exec-1".to_string()],
            }],
            provenance: MemoryProvenance {
                source_executions: vec!["exec-1".to_string(), "exec-2".to_string()],
                source_artifacts: vec!["artifact-1".to_string()],
                reflection_trace_id: Some("trace-abc".to_string()),
            },
            created_at,
            ttl_secs: None,
        }
    }

    // -----------------------------------------------------------------------
    // DTO / kind / scope
    // -----------------------------------------------------------------------

    #[test]
    fn scope_serde_tagged_json() {
        let agent = MemoryScope::agent("alpha");
        let json = serde_json::to_string(&agent).unwrap();
        assert!(json.contains("\"kind\":\"agent\""));
        assert!(json.contains("\"id\":\"alpha\""));

        let global = MemoryScope::Global;
        let gjson = serde_json::to_string(&global).unwrap();
        assert_eq!(gjson, "{\"kind\":\"global\"}");

        let back: MemoryScope = serde_json::from_str(&json).unwrap();
        assert_eq!(back, agent);
    }

    #[test]
    fn kind_as_str_values_are_stable() {
        assert_eq!(MemoryKind::DailyDigest.as_str(), "daily_digest");
        assert_eq!(MemoryKind::Reflection.as_str(), "reflection");
        assert_eq!(MemoryKind::Rule.as_str(), "rule");
        assert_eq!(MemoryKind::FreeText.as_str(), "free_text");
    }

    #[test]
    fn is_expired_at_respects_ttl() {
        let mut e = sample_entry("e", "a", "2026-04-10T00:00:00Z".parse().unwrap());
        // No TTL -> never expired.
        assert!(!e.is_expired_at("2099-01-01T00:00:00Z".parse().unwrap()));

        e.ttl_secs = Some(60);
        assert!(!e.is_expired_at("2026-04-10T00:00:30Z".parse().unwrap()));
        assert!(e.is_expired_at("2026-04-10T00:02:00Z".parse().unwrap()));
    }

    // -----------------------------------------------------------------------
    // In-memory impl smoke tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn inmem_insert_get_roundtrip() {
        let store = InMemorySemanticMemoryStore::new();
        let e = sample_entry("d-1", "alpha", Utc::now());
        let id = store.insert(e.clone()).await.unwrap();
        assert_eq!(id, "d-1");
        let got = store.get("d-1").await.unwrap().unwrap();
        assert_eq!(got, e);
    }

    #[tokio::test]
    async fn inmem_query_by_scope_newest_first() {
        let store = InMemorySemanticMemoryStore::new();
        store
            .insert(sample_entry(
                "old",
                "alpha",
                "2026-04-10T00:00:00Z".parse().unwrap(),
            ))
            .await
            .unwrap();
        store
            .insert(sample_entry(
                "new",
                "alpha",
                "2026-04-18T00:00:00Z".parse().unwrap(),
            ))
            .await
            .unwrap();
        let rows = store
            .query_by_scope(&MemoryScope::agent("alpha"), 10)
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "new");
        assert_eq!(rows[1].id, "old");
    }

    #[tokio::test]
    async fn inmem_query_by_agent_window_filters_since() {
        let store = InMemorySemanticMemoryStore::new();
        store
            .insert(sample_entry(
                "old",
                "alpha",
                "2026-04-10T00:00:00Z".parse().unwrap(),
            ))
            .await
            .unwrap();
        store
            .insert(sample_entry(
                "new",
                "alpha",
                "2026-04-18T00:00:00Z".parse().unwrap(),
            ))
            .await
            .unwrap();
        let rows = store
            .query_by_agent_window("alpha", "2026-04-15T00:00:00Z".parse().unwrap(), 10)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "new");
    }

    // -----------------------------------------------------------------------
    // SQLite impl tests (feature-gated)
    // -----------------------------------------------------------------------

    #[cfg(feature = "sqlite")]
    mod sqlite_tests {
        use super::*;

        fn store() -> SqliteSemanticMemoryStore {
            SqliteSemanticMemoryStore::in_memory().expect("open in-memory sqlite store")
        }

        #[tokio::test]
        async fn sqlite_semantic_memory_insert_get_roundtrip() {
            let store = store();
            let e = sample_entry("d-1", "alpha", "2026-04-18T03:00:00Z".parse().unwrap());
            store.insert(e.clone()).await.unwrap();
            let got = store.get("d-1").await.unwrap().expect("row present");
            assert_eq!(got.id, e.id);
            assert_eq!(got.scope, e.scope);
            assert_eq!(got.kind, e.kind);
            assert_eq!(got.content, e.content);
            assert_eq!(got.rules, e.rules);
            assert_eq!(got.provenance, e.provenance);
            // RFC3339 roundtrip is millisecond-precise by construction.
            assert_eq!(got.created_at, e.created_at);
        }

        #[tokio::test]
        async fn sqlite_semantic_memory_query_by_scope_returns_most_recent_first() {
            let store = store();
            store
                .insert(sample_entry(
                    "a",
                    "alpha",
                    "2026-04-10T00:00:00Z".parse().unwrap(),
                ))
                .await
                .unwrap();
            store
                .insert(sample_entry(
                    "b",
                    "alpha",
                    "2026-04-18T00:00:00Z".parse().unwrap(),
                ))
                .await
                .unwrap();
            store
                .insert(sample_entry(
                    "c-other",
                    "beta",
                    "2026-04-17T00:00:00Z".parse().unwrap(),
                ))
                .await
                .unwrap();

            let alpha = store
                .query_by_scope(&MemoryScope::agent("alpha"), 10)
                .await
                .unwrap();
            assert_eq!(
                alpha.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
                vec!["b", "a"]
            );

            let beta = store
                .query_by_scope(&MemoryScope::agent("beta"), 10)
                .await
                .unwrap();
            assert_eq!(beta.len(), 1);
            assert_eq!(beta[0].id, "c-other");
        }

        #[tokio::test]
        async fn sqlite_semantic_memory_upsert_on_duplicate_id() {
            let store = store();
            let mut e = sample_entry("d-1", "alpha", "2026-04-18T00:00:00Z".parse().unwrap());
            store.insert(e.clone()).await.unwrap();
            e.content = "updated content".into();
            e.kind = MemoryKind::Reflection;
            store.insert(e.clone()).await.unwrap();

            let got = store.get("d-1").await.unwrap().unwrap();
            assert_eq!(got.content, "updated content");
            assert_eq!(got.kind, MemoryKind::Reflection);

            // Still one row
            let all = store
                .query_by_scope(&MemoryScope::agent("alpha"), 10)
                .await
                .unwrap();
            assert_eq!(all.len(), 1);
        }

        #[tokio::test]
        async fn sqlite_semantic_memory_query_by_agent_window_respects_since() {
            let store = store();
            store
                .insert(sample_entry(
                    "old",
                    "alpha",
                    "2026-04-10T00:00:00Z".parse().unwrap(),
                ))
                .await
                .unwrap();
            store
                .insert(sample_entry(
                    "new",
                    "alpha",
                    "2026-04-18T00:00:00Z".parse().unwrap(),
                ))
                .await
                .unwrap();

            let rows = store
                .query_by_agent_window("alpha", "2026-04-15T00:00:00Z".parse().unwrap(), 10)
                .await
                .unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].id, "new");
        }

        #[tokio::test]
        async fn ttl_expiry_sweep_skips_expired_entries_from_reads() {
            let store = store();
            // Row that is already expired: created 2 hours ago with 60s TTL.
            let expired_at = Utc::now() - chrono::Duration::hours(2);
            let mut expired = sample_entry("expired", "alpha", expired_at);
            expired.ttl_secs = Some(60);
            store.insert(expired).await.unwrap();

            // Fresh row, no TTL.
            store
                .insert(sample_entry("fresh", "alpha", Utc::now()))
                .await
                .unwrap();

            // Reads must skip the expired row.
            assert!(store.get("expired").await.unwrap().is_none());
            let fresh = store.get("fresh").await.unwrap();
            assert!(fresh.is_some());

            let rows = store
                .query_by_scope(&MemoryScope::agent("alpha"), 10)
                .await
                .unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].id, "fresh");

            // Explicit sweep removes the physical row.
            let removed = store.sweep_expired().await.unwrap();
            assert_eq!(removed, 1);
            let removed_again = store.sweep_expired().await.unwrap();
            assert_eq!(removed_again, 0);
        }

        #[tokio::test]
        async fn sqlite_semantic_memory_get_unknown_is_none() {
            let store = store();
            assert!(store.get("does-not-exist").await.unwrap().is_none());
        }

        #[tokio::test]
        async fn sqlite_semantic_memory_scopes_are_isolated() {
            let store = store();
            store
                .insert(sample_entry(
                    "t",
                    "alpha",
                    "2026-04-18T00:00:00Z".parse().unwrap(),
                ))
                .await
                .unwrap();
            let mut tenant_entry =
                sample_entry("t-2", "alpha", "2026-04-18T00:00:00Z".parse().unwrap());
            tenant_entry.scope = MemoryScope::tenant("t-01");
            store.insert(tenant_entry).await.unwrap();

            let agent = store
                .query_by_scope(&MemoryScope::agent("alpha"), 10)
                .await
                .unwrap();
            assert_eq!(agent.len(), 1);
            assert_eq!(agent[0].id, "t");

            let tenant = store
                .query_by_scope(&MemoryScope::tenant("t-01"), 10)
                .await
                .unwrap();
            assert_eq!(tenant.len(), 1);
            assert_eq!(tenant[0].id, "t-2");
        }
    }
}
