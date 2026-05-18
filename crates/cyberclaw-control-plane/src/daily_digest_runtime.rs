//! Daily Digest runtime implementations (Sprint 9 L9).
//!
//! Wires the three traits declared in [`crate::daily_digest`] to the real
//! platform — [`ExecutionService`](crate::execution_service::ExecutionService)
//! for collection and a file-system-backed placeholder for persistence.
//!
//! # Architectural placement
//!
//! This file is strictly **runtime glue**: it does not change the trait
//! contracts (L6 scaffold is authoritative) and does not decide what a
//! "valid" digest looks like. All decisions — filtering, formatting,
//! provenance — are driven by [`DailyDigestConfig`].
//!
//! # Semantic Memory (Sprint 10 L1)
//!
//! [`SemanticMemoryDigestRepository`] is the real repository: it delegates to
//! any [`cyberclaw_store::SemanticMemoryStore`] (SQLite, in-memory, ...), so
//! each [`DailyDigestEntry`] lands in the shared `semantic_memory` table next
//! to reflections and rule candidates.
//!
//! [`FileDigestRepository`] is retained as a **fallback** for environments
//! without a configured store (tests, local scripts, doc examples). Both
//! implementations satisfy the same [`DigestRepository`] trait, so the
//! `GET /api/v1/agents/:id/digest` surface is unaffected by which one is
//! mounted.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tracing::{debug, warn};

use cyberclaw_core::ids::AgentId;

use crate::daily_digest::{
    DailyDigestConfig, DailyDigestEntry, DigestCollector, DigestError, DigestInputs,
    DigestPersister, ExecutionFact,
};
use crate::execution_service::ExecutionService;

// ============================================================================
// StoreDigestCollector — ExecutionService-backed Stage 1
// ============================================================================

// ============================================================================
// Sprint 9 (gradual landing): provider traits for artifact / trace / journal
//
// `cyberclaw-store` doesn't yet expose per-agent+window APIs for these record
// types, and `InMemoryExecutionService` doesn't hold artifact/trace
// references. Rather than blocking the digest runtime on a multi-crate
// refactor, we pin the **collector ↔ data-source** seam right here as
// optional traits. A `StoreDigestCollector` configured without providers
// preserves the legacy "executions only" behavior; once stores grow the
// real APIs, new impls just need to satisfy these trait surfaces.
// ============================================================================

/// Provider for artifact facts in a daily-digest window.
///
/// Implementations typically wrap a `cyberclaw_store` artifact API once it
/// gains per-agent+time queries. For tests, write a struct that returns a
/// fixed `Vec<ArtifactFact>`.
#[async_trait]
pub trait DigestArtifactProvider: Send + Sync {
    async fn list_by_agent_window(
        &self,
        agent_id: &AgentId,
        window_start: DateTime<Utc>,
        window_end: DateTime<Utc>,
    ) -> Result<Vec<crate::daily_digest::ArtifactFact>, DigestError>;
}

/// Provider for trace facts in a daily-digest window.
#[async_trait]
pub trait DigestTraceProvider: Send + Sync {
    async fn list_by_agent_window(
        &self,
        agent_id: &AgentId,
        window_start: DateTime<Utc>,
        window_end: DateTime<Utc>,
    ) -> Result<Vec<crate::daily_digest::TraceFact>, DigestError>;
}

/// Provider for journal iteration facts in a daily-digest window.
///
/// Returns `Vec::new()` for agents that didn't run under a persistent loop.
#[async_trait]
pub trait DigestJournalProvider: Send + Sync {
    async fn list_by_agent_window(
        &self,
        agent_id: &AgentId,
        window_start: DateTime<Utc>,
        window_end: DateTime<Utc>,
    ) -> Result<Vec<crate::daily_digest::JournalFact>, DigestError>;
}

/// Sprint 10 (gradual landing) — bridge from native `TraceRecord` rows to
/// [`DigestTraceProvider`]. Distinct from [`StateStoreTraceProvider`] which
/// uses audit logs as a proxy: this one consumes the dedicated `traces`
/// store API directly. Use this when the StateStore impl actually overrides
/// `save_trace` / `list_traces_by_agent_window` (e.g. `InMemoryStateStore`
/// post-Sprint-10).
pub struct NativeTraceStoreProvider {
    store: std::sync::Arc<dyn cyberclaw_store::StateStore>,
}

impl NativeTraceStoreProvider {
    pub fn new(store: std::sync::Arc<dyn cyberclaw_store::StateStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl DigestTraceProvider for NativeTraceStoreProvider {
    async fn list_by_agent_window(
        &self,
        agent_id: &AgentId,
        window_start: DateTime<Utc>,
        window_end: DateTime<Utc>,
    ) -> Result<Vec<crate::daily_digest::TraceFact>, DigestError> {
        let records = self
            .store
            .list_traces_by_agent_window(agent_id.as_str(), window_start, window_end)
            .await
            .map_err(|e| DigestError::Collect(format!("StateStore native traces: {}", e)))?;
        Ok(records
            .into_iter()
            .map(|r| crate::daily_digest::TraceFact {
                trace_id: cyberclaw_core::ids::TraceId::from_string(r.id.to_string())
                    .unwrap_or_else(|_| cyberclaw_core::ids::TraceId::new()),
                event_type: r.event_type,
                severity: r.severity,
            })
            .collect())
    }
}

/// Sprint 10 (gradual landing) — bridge from native `JournalRecord` rows to
/// [`DigestJournalProvider`]. Surfaces (iteration, verdict) tuples to daily
/// digest. Returns empty when the store doesn't override the journal methods
/// (legacy stores fall back to default impl returning empty Vec).
pub struct NativeJournalStoreProvider {
    store: std::sync::Arc<dyn cyberclaw_store::StateStore>,
}

impl NativeJournalStoreProvider {
    pub fn new(store: std::sync::Arc<dyn cyberclaw_store::StateStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl DigestJournalProvider for NativeJournalStoreProvider {
    async fn list_by_agent_window(
        &self,
        agent_id: &AgentId,
        window_start: DateTime<Utc>,
        window_end: DateTime<Utc>,
    ) -> Result<Vec<crate::daily_digest::JournalFact>, DigestError> {
        let records = self
            .store
            .list_journal_iterations_by_agent_window(agent_id.as_str(), window_start, window_end)
            .await
            .map_err(|e| DigestError::Collect(format!("StateStore journal: {}", e)))?;
        Ok(records
            .into_iter()
            .map(|r| crate::daily_digest::JournalFact {
                iteration: r.iteration,
                verdict: r.verdict,
            })
            .collect())
    }
}

/// Sprint 9 (gradual landing) — bridge from `cyberclaw_store::StateStore` to
/// [`DigestArtifactProvider`]. Fetches `ArtifactRecord`s in the agent+window
/// via the trait's new `list_artifacts_by_agent_window` and converts each row
/// into an [`crate::daily_digest::ArtifactFact`].
///
/// The conversion is best-effort: row `data` JSON byte size is used as the
/// `size_bytes` proxy (real size tracking would require a schema field —
/// future sprint). `metadata.size_bytes`, when present, takes precedence.
pub struct StateStoreArtifactProvider {
    store: std::sync::Arc<dyn cyberclaw_store::StateStore>,
}

impl StateStoreArtifactProvider {
    pub fn new(store: std::sync::Arc<dyn cyberclaw_store::StateStore>) -> Self {
        Self { store }
    }

    fn record_to_fact(
        record: cyberclaw_store::ArtifactRecord,
    ) -> crate::daily_digest::ArtifactFact {
        // Prefer metadata.size_bytes if the writer surfaced it; else fall back
        // to the byte length of the serialised data field as a rough proxy.
        let size_bytes = record
            .metadata
            .as_ref()
            .and_then(|m| m.get("size_bytes"))
            .and_then(|v| v.as_u64())
            .unwrap_or_else(|| {
                serde_json::to_string(&record.data)
                    .map(|s| s.len() as u64)
                    .unwrap_or(0)
            });
        crate::daily_digest::ArtifactFact {
            artifact_id: cyberclaw_core::ids::ArtifactId::from_string(record.id.to_string())
                .unwrap_or_else(|_| cyberclaw_core::ids::ArtifactId::new()),
            kind: record.artifact_type,
            size_bytes,
        }
    }
}

#[async_trait]
impl DigestArtifactProvider for StateStoreArtifactProvider {
    async fn list_by_agent_window(
        &self,
        agent_id: &AgentId,
        window_start: DateTime<Utc>,
        window_end: DateTime<Utc>,
    ) -> Result<Vec<crate::daily_digest::ArtifactFact>, DigestError> {
        let records = self
            .store
            .list_artifacts_by_agent_window(agent_id.as_str(), window_start, window_end)
            .await
            .map_err(|e| DigestError::Collect(format!("StateStore artifact query: {}", e)))?;
        Ok(records.into_iter().map(Self::record_to_fact).collect())
    }
}

/// Sprint 9 (gradual landing) — bridge from `cyberclaw_store::StateStore`
/// audit logs to [`DigestTraceProvider`]. While a dedicated `TraceStore`
/// remains a future-sprint item, audit logs are the closest available proxy
/// for "traces this agent emitted in the window".
///
/// Each `AuditLogRecord` is mapped to a [`crate::daily_digest::TraceFact`]:
/// - `trace_id` ← derived from the audit row id
/// - `event_type` ← `record.event_type`
/// - `severity` ← inferred from `event_type` keywords (`error` → "error",
///   `warn` → "warning", else "info"). Stays a string so future native
///   `TraceStore` rows can preserve their own severity column unchanged.
pub struct StateStoreTraceProvider {
    store: std::sync::Arc<dyn cyberclaw_store::StateStore>,
}

impl StateStoreTraceProvider {
    pub fn new(store: std::sync::Arc<dyn cyberclaw_store::StateStore>) -> Self {
        Self { store }
    }

    fn record_to_fact(record: cyberclaw_store::AuditLogRecord) -> crate::daily_digest::TraceFact {
        let event_lower = record.event_type.to_lowercase();
        let severity = if event_lower.contains("error") || event_lower.contains("fail") {
            "error"
        } else if event_lower.contains("warn") {
            "warning"
        } else {
            "info"
        };
        crate::daily_digest::TraceFact {
            trace_id: cyberclaw_core::ids::TraceId::from_string(record.id.to_string())
                .unwrap_or_else(|_| cyberclaw_core::ids::TraceId::new()),
            event_type: record.event_type,
            severity: severity.to_string(),
        }
    }
}

#[async_trait]
impl DigestTraceProvider for StateStoreTraceProvider {
    async fn list_by_agent_window(
        &self,
        agent_id: &AgentId,
        window_start: DateTime<Utc>,
        window_end: DateTime<Utc>,
    ) -> Result<Vec<crate::daily_digest::TraceFact>, DigestError> {
        let records = self
            .store
            .list_audit_logs_by_agent_window(agent_id.as_str(), window_start, window_end)
            .await
            .map_err(|e| DigestError::Collect(format!("StateStore audit log query: {}", e)))?;
        Ok(records.into_iter().map(Self::record_to_fact).collect())
    }
}

/// `DigestCollector` that queries the in-process [`ExecutionService`] for the
/// agent's executions in the window via
/// [`ExecutionService::list_by_agent_window`], with optional provider hooks
/// for artifacts / traces / journal iterations.
///
/// # Configuration
///
/// - `new(execution_service)` — executions-only mode (legacy default).
/// - `with_artifact_provider`, `with_trace_provider`, `with_journal_provider` —
///   builder-style hooks. Each is optional; absent providers contribute
///   `Vec::new()` to the corresponding `DigestInputs` field.
pub struct StoreDigestCollector {
    execution_service: Arc<dyn ExecutionService>,
    artifact_provider: Option<Arc<dyn DigestArtifactProvider>>,
    trace_provider: Option<Arc<dyn DigestTraceProvider>>,
    journal_provider: Option<Arc<dyn DigestJournalProvider>>,
}

impl StoreDigestCollector {
    /// Build a collector backed by the given execution service. No artifact /
    /// trace / journal providers are wired by default.
    pub fn new(execution_service: Arc<dyn ExecutionService>) -> Self {
        Self {
            execution_service,
            artifact_provider: None,
            trace_provider: None,
            journal_provider: None,
        }
    }

    /// Wire an artifact provider (consumes self for builder chaining).
    pub fn with_artifact_provider(mut self, provider: Arc<dyn DigestArtifactProvider>) -> Self {
        self.artifact_provider = Some(provider);
        self
    }

    /// Wire a trace provider (consumes self for builder chaining).
    pub fn with_trace_provider(mut self, provider: Arc<dyn DigestTraceProvider>) -> Self {
        self.trace_provider = Some(provider);
        self
    }

    /// Wire a journal provider (consumes self for builder chaining).
    pub fn with_journal_provider(mut self, provider: Arc<dyn DigestJournalProvider>) -> Self {
        self.journal_provider = Some(provider);
        self
    }
}

#[async_trait]
impl DigestCollector for StoreDigestCollector {
    async fn collect(&self, config: &DailyDigestConfig) -> Result<DigestInputs, DigestError> {
        // Trait-level agent+window filter (default impl is in-process scan;
        // InMemoryExecutionService overrides for direct map access). Removes
        // the historical "list_all then filter" detour from this path.
        let in_window = self
            .execution_service
            .list_by_agent_window(&config.agent_id, config.window_start, config.window_end)
            .await
            .map_err(|e| DigestError::Collect(e.to_string()))?;

        let executions = in_window
            .into_iter()
            .map(|exec| {
                let started = exec
                    .started_at
                    .expect("list_by_agent_window guarantees started_at");
                ExecutionFact {
                    execution_id: exec.id.clone(),
                    status: format!("{:?}", exec.status).to_lowercase(),
                    execution_mode: format!("{:?}", exec.execution_mode).to_lowercase(),
                    started_at: started,
                    completed_at: exec.finished_at,
                }
            })
            .collect();

        // Pull provider data when configured; absence falls back to empty Vec
        // (the historical behavior pre-Sprint-9-providers landing).
        let artifacts = match &self.artifact_provider {
            Some(p) => {
                p.list_by_agent_window(&config.agent_id, config.window_start, config.window_end)
                    .await?
            }
            None => Vec::new(),
        };
        let traces = match &self.trace_provider {
            Some(p) => {
                p.list_by_agent_window(&config.agent_id, config.window_start, config.window_end)
                    .await?
            }
            None => Vec::new(),
        };
        let journal_iterations = match &self.journal_provider {
            Some(p) => {
                p.list_by_agent_window(&config.agent_id, config.window_start, config.window_end)
                    .await?
            }
            None => Vec::new(),
        };

        Ok(DigestInputs {
            executions,
            artifacts,
            traces,
            journal_iterations,
        })
    }
}

// ============================================================================
// DigestRepository — read + write surface for digest entries
// ============================================================================

/// Persistence + query surface for digest entries.
///
/// The [`DigestPersister`] trait from L6 is **write-only**; this trait adds
/// the read path that the `GET /api/v1/agents/:id/digest` endpoint needs.
/// A single implementation backs both sides so the on-disk shape stays in
/// one place.
#[async_trait]
pub trait DigestRepository: Send + Sync {
    /// Write one entry (Stage 5 persist).
    async fn save(&self, entry: &DailyDigestEntry) -> Result<(), DigestError>;

    /// List entries for one agent whose `window_end` falls inside
    /// `since..Utc::now()`. Most-recent-first.
    async fn list_for_agent(
        &self,
        agent_id: &AgentId,
        since: DateTime<Utc>,
    ) -> Result<Vec<DailyDigestEntry>, DigestError>;
}

/// Adapter that lets any [`DigestRepository`] satisfy the L6 `DigestPersister`
/// contract without reimplementing the serialization layer.
pub struct RepositoryPersister {
    repo: Arc<dyn DigestRepository>,
}

impl RepositoryPersister {
    pub fn new(repo: Arc<dyn DigestRepository>) -> Self {
        Self { repo }
    }
}

#[async_trait]
impl DigestPersister for RepositoryPersister {
    async fn persist(&self, entry: &DailyDigestEntry) -> Result<(), DigestError> {
        self.repo.save(entry).await
    }
}

// ============================================================================
// FileDigestRepository — placeholder Semantic Memory (Sprint 9 backlog)
// ============================================================================

/// File-system-backed repository.
///
/// Layout: `<root>/<agent_id>/<YYYY-MM-DD>.json`.
/// `<YYYY-MM-DD>` is derived from `entry.window_end` in UTC to guarantee a
/// stable key even if multiple runs collide on the same day.
///
/// **This is a Sprint 9 placeholder.** `cyberclaw-store` will grow a
/// `SemanticMemory` write API; when it does, we replace the body of
/// `save` / `list_for_agent` to hit the store. The repository trait stays.
pub struct FileDigestRepository {
    root: PathBuf,
}

impl FileDigestRepository {
    /// Create a repository rooted at `root` (created lazily on first write).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Default root: `$HOME/.cyberclaw/digests` (fallback `./.cyberclaw/digests`).
    pub fn default_root() -> Self {
        let root = std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(".cyberclaw").join("digests"))
            .unwrap_or_else(|| PathBuf::from(".cyberclaw").join("digests"));
        Self::new(root)
    }

    fn agent_dir(&self, agent_id: &AgentId) -> PathBuf {
        // Replace path separators from the ID to avoid escaping the root,
        // even though AgentId validation forbids `\\` and `..`.
        let safe = agent_id.as_str().replace('/', "_");
        self.root.join(safe)
    }

    fn entry_filename(entry: &DailyDigestEntry) -> String {
        format!("{}.json", entry.window_end.format("%Y-%m-%d"))
    }

    fn is_json(path: &Path) -> bool {
        path.extension().and_then(|e| e.to_str()) == Some("json")
    }
}

#[async_trait]
impl DigestRepository for FileDigestRepository {
    async fn save(&self, entry: &DailyDigestEntry) -> Result<(), DigestError> {
        let dir = self.agent_dir(&entry.agent_id);
        std::fs::create_dir_all(&dir)
            .map_err(|e| DigestError::Persist(format!("create {}: {}", dir.display(), e)))?;
        let path = dir.join(Self::entry_filename(entry));
        let json = serde_json::to_vec_pretty(entry)
            .map_err(|e| DigestError::Persist(format!("serialize: {}", e)))?;
        std::fs::write(&path, json)
            .map_err(|e| DigestError::Persist(format!("write {}: {}", path.display(), e)))?;
        debug!(path = %path.display(), "daily digest persisted (placeholder fs)");
        Ok(())
    }

    async fn list_for_agent(
        &self,
        agent_id: &AgentId,
        since: DateTime<Utc>,
    ) -> Result<Vec<DailyDigestEntry>, DigestError> {
        let dir = self.agent_dir(agent_id);
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let read = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(e) => {
                warn!(path = %dir.display(), %e, "failed to read digest dir");
                return Ok(Vec::new());
            }
        };

        let mut entries = Vec::new();
        for dirent in read.flatten() {
            let path = dirent.path();
            if !Self::is_json(&path) {
                continue;
            }
            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(e) => {
                    warn!(path = %path.display(), %e, "failed to read digest file");
                    continue;
                }
            };
            let entry: DailyDigestEntry = match serde_json::from_slice(&bytes) {
                Ok(e) => e,
                Err(e) => {
                    warn!(path = %path.display(), %e, "failed to parse digest file");
                    continue;
                }
            };
            if entry.window_end >= since {
                entries.push(entry);
            }
        }

        entries.sort_by_key(|e| std::cmp::Reverse(e.window_end));
        Ok(entries)
    }
}

// ============================================================================
// SemanticMemoryDigestRepository — Sprint 10 L1 real backend
// ============================================================================

/// [`DigestRepository`] backed by any [`cyberclaw_store::SemanticMemoryStore`].
///
/// This is the production path: a `DailyDigestEntry` is mapped to a
/// [`cyberclaw_store::SemanticMemoryEntry`] (kind = `DailyDigest`,
/// scope = `Agent(agent_id)`) and written via `insert`. Reads go through
/// `query_by_agent_window` and the body is deserialised back from the JSON
/// `content` column.
///
/// # ID scheme
///
/// To keep re-runs idempotent the entry id is `digest:{agent_id}:{YYYY-MM-DD}`
/// where the date is taken from `entry.window_end` in UTC. Running the
/// coordinator twice on the same day upserts rather than duplicates.
pub struct SemanticMemoryDigestRepository {
    store: Arc<dyn cyberclaw_store::SemanticMemoryStore>,
}

impl SemanticMemoryDigestRepository {
    /// Wrap any `SemanticMemoryStore` as a `DigestRepository`.
    pub fn new(store: Arc<dyn cyberclaw_store::SemanticMemoryStore>) -> Self {
        Self { store }
    }

    fn entry_id(entry: &DailyDigestEntry) -> String {
        format!(
            "digest:{}:{}",
            entry.agent_id.as_str(),
            entry.window_end.format("%Y-%m-%d")
        )
    }

    /// Convert a `DailyDigestEntry` into a `SemanticMemoryEntry`.
    ///
    /// `content` holds the full JSON form of the digest entry so readers can
    /// recover every field (window bounds, summary, rules, provenance) from
    /// a single column without schema drift.
    fn to_store_entry(
        entry: &DailyDigestEntry,
    ) -> Result<cyberclaw_store::SemanticMemoryEntry, DigestError> {
        let content = serde_json::to_string(entry)
            .map_err(|e| DigestError::Persist(format!("serialize digest entry: {}", e)))?;

        let rules = entry
            .rules
            .iter()
            .map(|r| cyberclaw_store::ProceduralRule {
                rule: r.rule.clone(),
                source_executions: r
                    .source_executions
                    .iter()
                    .map(|id| id.as_str().to_string())
                    .collect(),
            })
            .collect();

        let provenance = cyberclaw_store::MemoryProvenance {
            source_executions: entry
                .source_executions
                .iter()
                .map(|id| id.as_str().to_string())
                .collect(),
            source_artifacts: entry
                .source_artifacts
                .iter()
                .map(|id| id.as_str().to_string())
                .collect(),
            reflection_trace_id: entry
                .reflection_trace_id
                .as_ref()
                .map(|t| t.as_str().to_string()),
        };

        Ok(cyberclaw_store::SemanticMemoryEntry {
            id: Self::entry_id(entry),
            scope: cyberclaw_store::MemoryScope::agent(entry.agent_id.as_str()),
            kind: cyberclaw_store::MemoryKind::DailyDigest,
            content,
            rules,
            provenance,
            created_at: entry.created_at,
            ttl_secs: None,
        })
    }

    fn from_store_entry(
        row: cyberclaw_store::SemanticMemoryEntry,
    ) -> Result<DailyDigestEntry, DigestError> {
        serde_json::from_str::<DailyDigestEntry>(&row.content)
            .map_err(|e| DigestError::Persist(format!("deserialize digest entry: {}", e)))
    }
}

#[async_trait]
impl DigestRepository for SemanticMemoryDigestRepository {
    async fn save(&self, entry: &DailyDigestEntry) -> Result<(), DigestError> {
        let row = Self::to_store_entry(entry)?;
        self.store
            .insert(row)
            .await
            .map_err(|e| DigestError::Persist(format!("semantic_memory insert: {}", e)))?;
        debug!(
            agent = entry.agent_id.as_str(),
            "daily digest persisted via SemanticMemoryStore"
        );
        Ok(())
    }

    async fn list_for_agent(
        &self,
        agent_id: &AgentId,
        since: DateTime<Utc>,
    ) -> Result<Vec<DailyDigestEntry>, DigestError> {
        // Pull DailyDigest entries for this agent since the cutoff. `limit`
        // matches the current API surface (24 entries covers ~a month of
        // daily runs) and the store already enforces newest-first order.
        let rows = self
            .store
            .query_by_agent_window(agent_id.as_str(), since, 256)
            .await
            .map_err(|e| DigestError::Persist(format!("semantic_memory query: {}", e)))?;

        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            // Skip non-digest entries in case the agent scope grows other
            // kinds in the future. `query_by_agent_window` is kind-agnostic.
            if !matches!(row.kind, cyberclaw_store::MemoryKind::DailyDigest) {
                continue;
            }
            out.push(Self::from_store_entry(row)?);
        }
        Ok(out)
    }
}

// ============================================================================
// In-memory repository (tests + ephemeral deployments)
// ============================================================================

/// Thread-safe in-memory [`DigestRepository`]. Used for integration tests
/// and environments that do not need durability.
#[derive(Default, Clone)]
pub struct InMemoryDigestRepository {
    inner: Arc<std::sync::RwLock<Vec<DailyDigestEntry>>>,
}

impl InMemoryDigestRepository {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl DigestRepository for InMemoryDigestRepository {
    async fn save(&self, entry: &DailyDigestEntry) -> Result<(), DigestError> {
        self.inner
            .write()
            .map_err(|_| DigestError::Persist("in-memory lock poisoned".into()))?
            .push(entry.clone());
        Ok(())
    }

    async fn list_for_agent(
        &self,
        agent_id: &AgentId,
        since: DateTime<Utc>,
    ) -> Result<Vec<DailyDigestEntry>, DigestError> {
        let guard = self
            .inner
            .read()
            .map_err(|_| DigestError::Persist("in-memory lock poisoned".into()))?;
        let mut out: Vec<DailyDigestEntry> = guard
            .iter()
            .filter(|e| e.agent_id == *agent_id && e.window_end >= since)
            .cloned()
            .collect();
        out.sort_by_key(|e| std::cmp::Reverse(e.window_end));
        Ok(out)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daily_digest::{
        DailyDigestConfig, DefaultDailyDigestCoordinator, DigestSummarizer, RuleCandidate,
    };
    use crate::daily_digest::{DigestSummary, ExecutionFact as _ExecutionFact};
    use async_trait::async_trait;
    use cyberclaw_core::execution::{
        AgentRef, Execution, ExecutionBudget, ExecutionMode, ExecutionStatus,
    };
    use cyberclaw_core::ids::{AgentId, ExecutionId, TraceId};
    use tempfile::tempdir;

    // -- helpers -------------------------------------------------------

    fn exec_for(agent_id: &AgentId, started_at: DateTime<Utc>) -> Execution {
        let id = ExecutionId::new();
        Execution {
            id: id.clone(),
            root_execution_id: id,
            parent_execution_id: None,
            owner_node_id: None,
            scheduled_node_id: None,
            placement_group: None,
            lease_id: None,
            handoff_count: 0,
            case_id: None,
            task_id: None,
            agent: AgentRef {
                id: agent_id.clone(),
                role: "test".into(),
            },
            status: ExecutionStatus::Completed,
            join_strategy: None,
            budget: ExecutionBudget::default(),
            workspace: None,
            trace_id: TraceId::new(),
            started_at: Some(started_at),
            finished_at: Some(started_at + chrono::Duration::seconds(5)),
            risk_level: cyberclaw_core::capability::RiskLevel::Low,
            execution_mode: ExecutionMode::Normal,
        }
    }

    struct StubExecutionService {
        executions: Vec<Execution>,
    }

    #[async_trait]
    impl ExecutionService for StubExecutionService {
        async fn submit(
            &self,
            _req: crate::execution_service::ExecutionRequest,
        ) -> anyhow::Result<ExecutionId> {
            unreachable!()
        }
        async fn submit_plan(
            &self,
            _plan: crate::types::ExecutionPlan,
        ) -> anyhow::Result<ExecutionId> {
            unreachable!()
        }
        async fn cancel(&self, _id: &ExecutionId) -> anyhow::Result<()> {
            Ok(())
        }
        async fn get(&self, _id: &ExecutionId) -> anyhow::Result<Option<Execution>> {
            Ok(None)
        }
        async fn list_all(
            &self,
            _filter: Option<ExecutionStatus>,
        ) -> anyhow::Result<Vec<Execution>> {
            Ok(self.executions.clone())
        }
        async fn list_by_task_id(
            &self,
            _t: &cyberclaw_core::ids::TaskId,
        ) -> anyhow::Result<Vec<Execution>> {
            Ok(Vec::new())
        }
        async fn update_status(
            &self,
            _id: &ExecutionId,
            _s: ExecutionStatus,
        ) -> anyhow::Result<()> {
            Ok(())
        }
        async fn execute(&self, _id: &ExecutionId) -> anyhow::Result<()> {
            Ok(())
        }
        async fn execute_autopilot_iteration(
            &self,
            _id: &ExecutionId,
            _iter: u32,
        ) -> anyhow::Result<crate::execution_service::IterationResult> {
            unreachable!("autopilot paths not exercised in digest collector tests")
        }
        async fn on_iteration_start(&self, _id: &ExecutionId, _iter: u32) -> anyhow::Result<()> {
            Ok(())
        }
        async fn on_step_complete(
            &self,
            _id: &ExecutionId,
            _step: crate::execution_service::AutopilotStep,
            _result: crate::execution_service::StepResult,
        ) -> anyhow::Result<()> {
            Ok(())
        }
        async fn on_stuck_detected(
            &self,
            _id: &ExecutionId,
            _iter: u32,
            _reason: String,
        ) -> anyhow::Result<crate::execution_service::StuckResolution> {
            unreachable!("stuck detection not exercised in digest collector tests")
        }
        async fn checkpoint_iteration(
            &self,
            _id: &ExecutionId,
            _iter: u32,
            _state: crate::execution_service::IterationState,
        ) -> anyhow::Result<()> {
            Ok(())
        }
        async fn resume_from_checkpoint(
            &self,
            _id: &ExecutionId,
        ) -> anyhow::Result<Option<crate::execution_service::IterationState>> {
            Ok(None)
        }
        async fn get_iteration_history(
            &self,
            _id: &ExecutionId,
        ) -> anyhow::Result<Vec<crate::execution_service::IterationSummary>> {
            Ok(Vec::new())
        }
    }

    // -- StoreDigestCollector -----------------------------------------

    #[tokio::test]
    async fn collector_filters_by_agent_and_window() {
        let agent_a = AgentId::from_string("agent-a".into()).unwrap();
        let agent_b = AgentId::from_string("agent-b".into()).unwrap();
        let in_window: DateTime<Utc> = "2026-04-18T03:00:00Z".parse().unwrap();
        let out_of_window: DateTime<Utc> = "2026-04-17T03:00:00Z".parse().unwrap();

        let svc = Arc::new(StubExecutionService {
            executions: vec![
                exec_for(&agent_a, in_window),
                exec_for(&agent_a, out_of_window), // excluded by window
                exec_for(&agent_b, in_window),     // excluded by agent
            ],
        });
        let collector = StoreDigestCollector::new(svc);
        let cfg = DailyDigestConfig {
            agent_id: agent_a.clone(),
            window_start: "2026-04-18T00:00:00Z".parse().unwrap(),
            window_end: "2026-04-19T00:00:00Z".parse().unwrap(),
            max_rules: 5,
        };
        let inputs = collector.collect(&cfg).await.unwrap();
        assert_eq!(inputs.executions.len(), 1);
        assert_eq!(inputs.executions[0].status, "completed");
        assert_eq!(inputs.executions[0].execution_mode, "normal");
    }

    #[tokio::test]
    async fn state_store_artifact_provider_bridges_to_digest_facts() {
        // Sprint 9 (gradual landing) end-to-end: write an Execution +
        // ArtifactRecord into InMemoryStateStore, query via the bridge, verify
        // DigestArtifactProvider surface returns one ArtifactFact in the
        // window and zero outside.
        use chrono::Duration;
        use cyberclaw_store::{ArtifactRecord, ExecutionRecord, InMemoryStateStore, StateStore};
        use uuid::Uuid;

        let store = Arc::new(InMemoryStateStore::default());
        let agent = AgentId::from_string("agent-bridge".into()).unwrap();
        let exec_id = Uuid::new_v4();
        let in_window: DateTime<Utc> = "2026-04-18T12:00:00Z".parse().unwrap();
        store
            .save_execution(ExecutionRecord {
                id: exec_id,
                agent_id: agent.as_str().to_string(),
                skill_id: None,
                status: "completed".to_string(),
                input: serde_json::json!({}),
                output: None,
                error: None,
                started_at: in_window,
                completed_at: Some(in_window + Duration::seconds(5)),
            })
            .await
            .unwrap();
        store
            .save_artifact(ArtifactRecord {
                id: Uuid::new_v4(),
                execution_id: exec_id,
                artifact_type: "log".to_string(),
                data: serde_json::json!({"hello": "world"}),
                metadata: Some(serde_json::json!({"size_bytes": 256})),
            })
            .await
            .unwrap();

        let provider: Arc<dyn DigestArtifactProvider> = Arc::new(StateStoreArtifactProvider::new(
            store as Arc<dyn cyberclaw_store::StateStore>,
        ));

        let window_start: DateTime<Utc> = "2026-04-18T00:00:00Z".parse().unwrap();
        let window_end: DateTime<Utc> = "2026-04-19T00:00:00Z".parse().unwrap();
        let in_window_facts = provider
            .list_by_agent_window(&agent, window_start, window_end)
            .await
            .unwrap();
        assert_eq!(in_window_facts.len(), 1);
        assert_eq!(in_window_facts[0].kind, "log");
        assert_eq!(
            in_window_facts[0].size_bytes, 256,
            "metadata.size_bytes wins over data fallback"
        );

        // Window that excludes the execution → zero facts.
        let outside_start: DateTime<Utc> = "2026-04-20T00:00:00Z".parse().unwrap();
        let outside_end: DateTime<Utc> = "2026-04-21T00:00:00Z".parse().unwrap();
        let none = provider
            .list_by_agent_window(&agent, outside_start, outside_end)
            .await
            .unwrap();
        assert!(none.is_empty());
    }

    #[tokio::test]
    async fn native_trace_store_provider_surfaces_dedicated_trace_records() {
        // Sprint 10 (gradual landing): when StateStore overrides the dedicated
        // trace methods (InMemoryStateStore does), NativeTraceStoreProvider
        // surfaces TraceRecord rows directly — no audit-log proxy.
        use chrono::Duration;
        use cyberclaw_store::{InMemoryStateStore, StateStore, TraceRecord};
        use uuid::Uuid;

        let store = Arc::new(InMemoryStateStore::default());
        let agent = AgentId::from_string("agent-trace-native".into()).unwrap();
        let in_window: DateTime<Utc> = "2026-04-26T12:00:00Z".parse().unwrap();
        store
            .save_trace(TraceRecord {
                id: Uuid::new_v4(),
                agent_id: agent.as_str().to_string(),
                execution_id: None,
                parent_trace_id: None,
                event_type: "iteration.complete".to_string(),
                severity: "info".to_string(),
                details: None,
                timestamp: in_window,
            })
            .await
            .unwrap();
        // Out-of-window record (must be filtered).
        store
            .save_trace(TraceRecord {
                id: Uuid::new_v4(),
                agent_id: agent.as_str().to_string(),
                execution_id: None,
                parent_trace_id: None,
                event_type: "policy.violation".to_string(),
                severity: "error".to_string(),
                details: None,
                timestamp: in_window - Duration::days(2),
            })
            .await
            .unwrap();

        let provider: Arc<dyn DigestTraceProvider> =
            Arc::new(NativeTraceStoreProvider::new(store as Arc<dyn StateStore>));
        let window_start: DateTime<Utc> = "2026-04-26T00:00:00Z".parse().unwrap();
        let window_end: DateTime<Utc> = "2026-04-27T00:00:00Z".parse().unwrap();
        let facts = provider
            .list_by_agent_window(&agent, window_start, window_end)
            .await
            .unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].event_type, "iteration.complete");
        assert_eq!(facts[0].severity, "info");
    }

    #[tokio::test]
    async fn native_journal_store_provider_surfaces_journal_records() {
        // Sprint 10 (gradual landing): JournalStore via dedicated trait methods.
        use cyberclaw_store::{InMemoryStateStore, JournalRecord, StateStore};
        use uuid::Uuid;

        let store = Arc::new(InMemoryStateStore::default());
        let agent = AgentId::from_string("agent-journal".into()).unwrap();
        let in_window: DateTime<Utc> = "2026-04-26T12:00:00Z".parse().unwrap();
        for (it, verdict) in &[(1u32, "pass"), (2, "fail"), (3, "pass")] {
            store
                .save_journal_iteration(JournalRecord {
                    id: Uuid::new_v4(),
                    agent_id: agent.as_str().to_string(),
                    execution_id: None,
                    iteration: *it,
                    verdict: verdict.to_string(),
                    story_id: None,
                    created_at: in_window,
                })
                .await
                .unwrap();
        }

        let provider: Arc<dyn DigestJournalProvider> = Arc::new(NativeJournalStoreProvider::new(
            store as Arc<dyn StateStore>,
        ));
        let window_start: DateTime<Utc> = "2026-04-26T00:00:00Z".parse().unwrap();
        let window_end: DateTime<Utc> = "2026-04-27T00:00:00Z".parse().unwrap();
        let facts = provider
            .list_by_agent_window(&agent, window_start, window_end)
            .await
            .unwrap();
        assert_eq!(facts.len(), 3);
        assert_eq!(facts[1].verdict, "fail");
    }

    #[tokio::test]
    async fn state_store_trace_provider_bridges_audit_logs_to_trace_facts() {
        // Sprint 9 (gradual landing) — bridge audit logs to TraceFact via
        // window query. Verifies severity inference from event_type as well.
        use chrono::Duration;
        use cyberclaw_store::{AuditLogRecord, ExecutionRecord, InMemoryStateStore, StateStore};
        use uuid::Uuid;

        let store = Arc::new(InMemoryStateStore::default());
        let agent = AgentId::from_string("agent-trace-bridge".into()).unwrap();
        let exec_id = Uuid::new_v4();
        let in_window: DateTime<Utc> = "2026-04-18T12:00:00Z".parse().unwrap();
        store
            .save_execution(ExecutionRecord {
                id: exec_id,
                agent_id: agent.as_str().to_string(),
                skill_id: None,
                status: "completed".to_string(),
                input: serde_json::json!({}),
                output: None,
                error: None,
                started_at: in_window,
                completed_at: Some(in_window + Duration::seconds(5)),
            })
            .await
            .unwrap();

        // Three audit log entries with different event_type categories.
        for (event_type, ts_offset) in &[
            ("policy.error.deny", Duration::seconds(1)),
            ("warn.dispatch_retry", Duration::seconds(2)),
            ("execution.complete", Duration::seconds(3)),
        ] {
            store
                .save_audit_log(AuditLogRecord {
                    id: Uuid::new_v4(),
                    execution_id: Some(exec_id),
                    event_type: event_type.to_string(),
                    actor: Some("test".to_string()),
                    action: "test".to_string(),
                    resource: None,
                    details: None,
                    timestamp: in_window + *ts_offset,
                })
                .await
                .unwrap();
        }

        let provider: Arc<dyn DigestTraceProvider> = Arc::new(StateStoreTraceProvider::new(
            store as Arc<dyn cyberclaw_store::StateStore>,
        ));

        let window_start: DateTime<Utc> = "2026-04-18T00:00:00Z".parse().unwrap();
        let window_end: DateTime<Utc> = "2026-04-19T00:00:00Z".parse().unwrap();
        let facts = provider
            .list_by_agent_window(&agent, window_start, window_end)
            .await
            .unwrap();
        assert_eq!(facts.len(), 3);

        // Severity inference from event_type keyword matching.
        let by_type: std::collections::HashMap<_, _> = facts
            .iter()
            .map(|f| (f.event_type.as_str(), f.severity.as_str()))
            .collect();
        assert_eq!(by_type.get("policy.error.deny"), Some(&"error"));
        assert_eq!(by_type.get("warn.dispatch_retry"), Some(&"warning"));
        assert_eq!(by_type.get("execution.complete"), Some(&"info"));
    }

    #[tokio::test]
    async fn collector_with_providers_populates_artifacts_traces_journals() {
        // Sprint 9 partial: when DigestArtifactProvider / DigestTraceProvider
        // / DigestJournalProvider are wired, collect() must call them in the
        // configured agent+window and surface their facts in DigestInputs.
        use crate::daily_digest::{ArtifactFact, JournalFact, TraceFact};
        use cyberclaw_core::ids::{ArtifactId, TraceId};

        struct FakeArtifactProvider {
            fact: ArtifactFact,
        }
        #[async_trait]
        impl DigestArtifactProvider for FakeArtifactProvider {
            async fn list_by_agent_window(
                &self,
                _: &AgentId,
                _: DateTime<Utc>,
                _: DateTime<Utc>,
            ) -> Result<Vec<ArtifactFact>, DigestError> {
                Ok(vec![self.fact.clone()])
            }
        }

        struct FakeTraceProvider {
            fact: TraceFact,
        }
        #[async_trait]
        impl DigestTraceProvider for FakeTraceProvider {
            async fn list_by_agent_window(
                &self,
                _: &AgentId,
                _: DateTime<Utc>,
                _: DateTime<Utc>,
            ) -> Result<Vec<TraceFact>, DigestError> {
                Ok(vec![self.fact.clone()])
            }
        }

        struct FakeJournalProvider {
            fact: JournalFact,
        }
        #[async_trait]
        impl DigestJournalProvider for FakeJournalProvider {
            async fn list_by_agent_window(
                &self,
                _: &AgentId,
                _: DateTime<Utc>,
                _: DateTime<Utc>,
            ) -> Result<Vec<JournalFact>, DigestError> {
                Ok(vec![self.fact.clone()])
            }
        }

        let agent = AgentId::from_string("agent-prov".into()).unwrap();
        let in_window: DateTime<Utc> = "2026-04-18T03:00:00Z".parse().unwrap();
        let svc = Arc::new(StubExecutionService {
            executions: vec![exec_for(&agent, in_window)],
        });

        let collector = StoreDigestCollector::new(svc)
            .with_artifact_provider(Arc::new(FakeArtifactProvider {
                fact: ArtifactFact {
                    artifact_id: ArtifactId::from_string("art-1".into()).unwrap(),
                    kind: "log".into(),
                    size_bytes: 256,
                },
            }))
            .with_trace_provider(Arc::new(FakeTraceProvider {
                fact: TraceFact {
                    trace_id: TraceId::new(),
                    event_type: "iteration_complete".into(),
                    severity: "info".into(),
                },
            }))
            .with_journal_provider(Arc::new(FakeJournalProvider {
                fact: JournalFact {
                    iteration: 3,
                    verdict: "pass".into(),
                },
            }));

        let cfg = DailyDigestConfig {
            agent_id: agent,
            window_start: "2026-04-18T00:00:00Z".parse().unwrap(),
            window_end: "2026-04-19T00:00:00Z".parse().unwrap(),
            max_rules: 5,
        };
        let inputs = collector.collect(&cfg).await.unwrap();
        assert_eq!(inputs.executions.len(), 1);
        assert_eq!(inputs.artifacts.len(), 1);
        assert_eq!(inputs.artifacts[0].kind, "log");
        assert_eq!(inputs.traces.len(), 1);
        assert_eq!(inputs.traces[0].event_type, "iteration_complete");
        assert_eq!(inputs.journal_iterations.len(), 1);
        assert_eq!(inputs.journal_iterations[0].iteration, 3);
    }

    #[tokio::test]
    async fn collector_without_providers_returns_empty_aux_facts() {
        // Sprint 9 partial: when no providers are configured the collector
        // must continue to return executions only and Vec::new() for the
        // other three fields — preserves the legacy "executions only" mode
        // for backward compatibility with existing tests/deployments.
        let agent = AgentId::from_string("agent-noprov".into()).unwrap();
        let in_window: DateTime<Utc> = "2026-04-18T03:00:00Z".parse().unwrap();
        let svc = Arc::new(StubExecutionService {
            executions: vec![exec_for(&agent, in_window)],
        });
        let collector = StoreDigestCollector::new(svc);
        let cfg = DailyDigestConfig {
            agent_id: agent,
            window_start: "2026-04-18T00:00:00Z".parse().unwrap(),
            window_end: "2026-04-19T00:00:00Z".parse().unwrap(),
            max_rules: 5,
        };
        let inputs = collector.collect(&cfg).await.unwrap();
        assert_eq!(inputs.executions.len(), 1);
        assert!(inputs.artifacts.is_empty());
        assert!(inputs.traces.is_empty());
        assert!(inputs.journal_iterations.is_empty());
    }

    #[tokio::test]
    async fn collector_skips_executions_without_started_at() {
        let agent = AgentId::from_string("agent-x".into()).unwrap();
        let mut e = exec_for(
            &agent,
            "2026-04-18T01:00:00Z".parse::<DateTime<Utc>>().unwrap(),
        );
        e.started_at = None;
        let svc = Arc::new(StubExecutionService {
            executions: vec![e],
        });
        let collector = StoreDigestCollector::new(svc);
        let cfg = DailyDigestConfig {
            agent_id: agent,
            window_start: "2026-04-18T00:00:00Z".parse().unwrap(),
            window_end: "2026-04-19T00:00:00Z".parse().unwrap(),
            max_rules: 5,
        };
        let inputs = collector.collect(&cfg).await.unwrap();
        assert!(inputs.executions.is_empty());
    }

    // -- FileDigestRepository -----------------------------------------

    fn sample_entry(agent_id: &AgentId, day: &str) -> DailyDigestEntry {
        DailyDigestEntry {
            agent_id: agent_id.clone(),
            window_start: format!("{day}T00:00:00Z").parse().unwrap(),
            window_end: format!("{day}T23:59:59Z").parse().unwrap(),
            summary: DigestSummary {
                facts_md: "facts".into(),
                problems_md: "problems".into(),
                learnings_md: "learnings".into(),
            },
            rules: vec![RuleCandidate {
                rule: "placeholder rule".into(),
                source_executions: vec![],
            }],
            source_executions: vec![],
            source_artifacts: vec![],
            reflection_trace_id: None,
            created_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn file_repo_roundtrips_and_filters_by_since() {
        let dir = tempdir().unwrap();
        let repo = FileDigestRepository::new(dir.path());
        let agent = AgentId::from_string("alpha".into()).unwrap();
        repo.save(&sample_entry(&agent, "2026-04-10"))
            .await
            .unwrap();
        repo.save(&sample_entry(&agent, "2026-04-18"))
            .await
            .unwrap();

        // since = 2026-04-15 keeps only the newer one
        let since: DateTime<Utc> = "2026-04-15T00:00:00Z".parse().unwrap();
        let list = repo.list_for_agent(&agent, since).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(
            list[0].window_end.format("%Y-%m-%d").to_string(),
            "2026-04-18"
        );
    }

    #[tokio::test]
    async fn file_repo_returns_empty_for_unknown_agent() {
        let dir = tempdir().unwrap();
        let repo = FileDigestRepository::new(dir.path());
        let agent = AgentId::from_string("nobody".into()).unwrap();
        let since = Utc::now() - chrono::Duration::days(30);
        let list = repo.list_for_agent(&agent, since).await.unwrap();
        assert!(list.is_empty());
    }

    // -- RepositoryPersister + coordinator integration ---------------

    struct EchoSummarizer;
    #[async_trait]
    impl DigestSummarizer for EchoSummarizer {
        async fn summarize(
            &self,
            _c: &DailyDigestConfig,
            _i: &DigestInputs,
        ) -> Result<(DigestSummary, Vec<RuleCandidate>), DigestError> {
            Ok((
                DigestSummary {
                    facts_md: "f".into(),
                    problems_md: "p".into(),
                    learnings_md: "l".into(),
                },
                vec![],
            ))
        }
    }

    #[tokio::test]
    async fn coordinator_persists_through_repository() {
        let agent = AgentId::from_string("rbot".into()).unwrap();
        let in_window: DateTime<Utc> = "2026-04-18T03:00:00Z".parse().unwrap();
        let svc = Arc::new(StubExecutionService {
            executions: vec![exec_for(&agent, in_window)],
        });
        let repo: Arc<dyn DigestRepository> = Arc::new(InMemoryDigestRepository::new());
        let coord = DefaultDailyDigestCoordinator::new(
            Box::new(StoreDigestCollector::new(svc)),
            Box::new(EchoSummarizer),
            Box::new(RepositoryPersister::new(repo.clone())),
        );
        let cfg = DailyDigestConfig {
            agent_id: agent.clone(),
            window_start: "2026-04-18T00:00:00Z".parse().unwrap(),
            window_end: "2026-04-19T00:00:00Z".parse().unwrap(),
            max_rules: 5,
        };
        let out = coord.run(cfg).await.unwrap();
        assert!(!out.skipped_empty_day);
        let since: DateTime<Utc> = "2026-04-01T00:00:00Z".parse().unwrap();
        let list = repo.list_for_agent(&agent, since).await.unwrap();
        assert_eq!(list.len(), 1);
    }

    // Sanity: the helpers stay wired to the real types.
    #[test]
    fn type_re_exports_stay_in_sync() {
        fn _assert<F>(_f: F)
        where
            F: Fn(&_ExecutionFact) -> DateTime<Utc>,
        {
        }
        _assert(|f| f.started_at);
    }

    // -- SemanticMemoryDigestRepository (Sprint 10 L1) ----------------

    #[tokio::test]
    async fn semantic_memory_digest_repo_save_then_list() {
        // Uses the sqlite-backed store end-to-end to prove migrations run,
        // the digest roundtrips through JSON, and reads come back sorted.
        let store = Arc::new(
            cyberclaw_store::SqliteSemanticMemoryStore::in_memory().expect("open in-memory sqlite"),
        );
        let repo = SemanticMemoryDigestRepository::new(store);

        let agent = AgentId::from_string("rbot".into()).unwrap();
        let older = {
            let mut e = sample_entry(&agent, "2026-04-10");
            e.created_at = "2026-04-10T00:00:00Z".parse().unwrap();
            e
        };
        let newer = {
            let mut e = sample_entry(&agent, "2026-04-18");
            e.created_at = "2026-04-18T00:00:00Z".parse().unwrap();
            e
        };
        repo.save(&older).await.unwrap();
        repo.save(&newer).await.unwrap();

        // Window that keeps both.
        let since: DateTime<Utc> = "2026-04-01T00:00:00Z".parse().unwrap();
        let list = repo.list_for_agent(&agent, since).await.unwrap();
        assert_eq!(list.len(), 2, "both digests returned");
        assert_eq!(
            list[0].window_end.format("%Y-%m-%d").to_string(),
            "2026-04-18",
            "most recent first"
        );
        assert_eq!(
            list[1].window_end.format("%Y-%m-%d").to_string(),
            "2026-04-10"
        );

        // Rules + provenance survived the JSON roundtrip.
        assert_eq!(list[0].rules.len(), 1);
        assert_eq!(list[0].rules[0].rule, "placeholder rule");

        // Window filter removes the older entry.
        let later_since: DateTime<Utc> = "2026-04-15T00:00:00Z".parse().unwrap();
        let filtered = repo.list_for_agent(&agent, later_since).await.unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(
            filtered[0].window_end.format("%Y-%m-%d").to_string(),
            "2026-04-18"
        );
    }

    #[tokio::test]
    async fn semantic_memory_digest_repo_save_is_idempotent_on_same_day() {
        // Sprint 10 L1 guarantee: re-running the digest on the same day
        // upserts instead of creating a second row (stable id scheme).
        let store = Arc::new(
            cyberclaw_store::SqliteSemanticMemoryStore::in_memory().expect("open in-memory sqlite"),
        );
        let repo = SemanticMemoryDigestRepository::new(store);
        let agent = AgentId::from_string("rbot".into()).unwrap();

        let mut e = sample_entry(&agent, "2026-04-18");
        e.created_at = "2026-04-18T00:00:00Z".parse().unwrap();
        repo.save(&e).await.unwrap();

        // Save again with modified summary — same id (agent + date), so it upserts.
        e.summary.facts_md = "rewritten".into();
        repo.save(&e).await.unwrap();

        let since: DateTime<Utc> = "2026-04-01T00:00:00Z".parse().unwrap();
        let list = repo.list_for_agent(&agent, since).await.unwrap();
        assert_eq!(list.len(), 1, "upsert, not duplicate");
        assert_eq!(list[0].summary.facts_md, "rewritten");
    }
}
