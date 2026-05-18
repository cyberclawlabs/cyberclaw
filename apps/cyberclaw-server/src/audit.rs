//! Append-only audit log backed by SQLite (WAL) with a SHA-256 hash chain.
//!
//! Sprint 8 Lane 1. Records every login, every mutation, every governance
//! event into a single SQLite database. The sink is append-only by
//! construction:
//!
//! - Only [`AuditSink::record`] writes. There is no `clear` / `delete`
//!   on this type.
//! - The HTTP surface (see [`crate::api::audit`]) exposes only `GET`
//!   routes; `DELETE` / `PUT` / `PATCH` on `/api/v1/audit/*` return
//!   `405 Method Not Allowed`.
//! - Every row embeds `this_hash = sha256(id || ts || actor || kind ||
//!   action || target || detail_json || result || failure_reason ||
//!   prev_hash)`. Tampering (row delete, row update, row re-insert)
//!   therefore breaks the chain; [`AuditSink::verify_chain`] walks the
//!   table and reports the first mismatch.
//! - SQLite is opened in `journal_mode=WAL` / `synchronous=NORMAL` so
//!   writes are durable without a per-write fsync storm.
//!
//! # Database location
//!
//! Defaults to `$HOME/.cyberclaw/audit.db`. Override via `CYBERCLAW_AUDIT_DB`.
//! The parent directory is created with the default umask; on unix the
//! file itself is chmod-ed to `0600` on first creation so other local
//! users cannot read it.
//!
//! # Architectural compliance (EVOLUTION_IDIOMS)
//!
//! - §1 Four-object model: [`AuditSink`] is a server module, not a new
//!   platform object. Not a `Connector`, `Skill`, `Agent`, `Capability`,
//!   or `Platform Plugin`.
//! - §2 Event sink: concrete type rather than a trait because the
//!   append-only + hash-chain contract is inseparable from the storage
//!   backend. A trait would invite pluggable sinks that silently break
//!   the invariant.
//! - §5 No covert sixth object: no new kind of runtime object is
//!   introduced.
//! - §6 Small surface: a single sink + four async methods (`record`,
//!   `tail`, `export`, `verify_chain`).

use chrono::{DateTime, Utc};
use cyberclaw_core::ids::{AgentId, ClarifyId};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Structured category for an audit entry.
///
/// Flat variants (Auth/Mutation/Config/Security) serialize to a plain
/// string via `rename_all = "snake_case"`. The Clarify variants carry
/// additional fields; they serialize as `{"ClarifyRequested":{...}}` in
/// JSON contexts, but the DB stores only the `as_str()` tag in the `kind`
/// column — the full field set is stored separately in `detail_json` by
/// the caller.
// NOTE: Serialize is implemented manually below so struct variants serialize
// as flat discriminator strings (matching `as_str()`). Without this, struct
// variants produce `{"clarify_requested":{...}}` on the wire, crashing the
// admin AuditPage Badge which expects `kind` to be a string. Deserialize
// stays derived (reconstruction handled by `from_str` for DB reads).
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuditKind {
    /// Login, logout, failed-login.
    Auth,
    /// Any POST/PUT/DELETE on a platform resource.
    Mutation,
    /// Settings / policy / env changes.
    Config,
    /// Dangerous capability invocations and governance denials.
    Security,
    /// A clarify request was raised by an agent — question text is NOT
    /// recorded here; only the opaque length is stored to prevent leaking
    /// potentially sensitive question wording into the audit trail.
    ClarifyRequested {
        clarify_id: ClarifyId,
        conversation_id: String,
        agent_id: AgentId,
        /// Length of the question text in bytes. The question itself is not
        /// stored (may contain sensitive user context).
        question_len: usize,
    },
    /// A clarify request was resolved (answered or explicitly dismissed).
    ClarifyResolved {
        clarify_id: ClarifyId,
        /// Label of the option the user selected, or `None` for freeform-only
        /// responses.
        picked_option: Option<String>,
        /// Byte length of the freeform answer (not the text itself).
        freeform_len: usize,
    },
    /// A clarify request expired before the user responded.
    ClarifyTimedOut {
        clarify_id: ClarifyId,
        elapsed_seconds: u64,
    },
}

// Manual Serialize: always emit the flat discriminator string (matching
// `as_str()`). Fixes admin UI crash where AuditPage Badge receives object
// instead of string for struct variants (ClarifyRequested / ClarifyResolved /
// ClarifyTimedOut). Struct fields are redundant on the wire because they
// already live in AuditEntry.detail (JSON payload).
impl Serialize for AuditKind {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl AuditKind {
    fn as_str(&self) -> &'static str {
        match self {
            AuditKind::Auth => "auth",
            AuditKind::Mutation => "mutation",
            AuditKind::Config => "config",
            AuditKind::Security => "security",
            AuditKind::ClarifyRequested { .. } => "clarify_requested",
            AuditKind::ClarifyResolved { .. } => "clarify_resolved",
            AuditKind::ClarifyTimedOut { .. } => "clarify_timed_out",
        }
    }

    /// Reconstruct an `AuditKind` from a DB string. For the flat variants
    /// (Auth/Mutation/Config/Security) the string is sufficient. For the
    /// Clarify variants the struct fields are stored in `detail_json` and
    /// are not reconstructed here — callers that need them should parse
    /// `detail_json` separately. We return a field-less sentinel so that
    /// `tail` / `export` queries can still deserialize the `kind` column
    /// without failing on unknown strings.
    fn from_str(s: &str) -> Option<Self> {
        match s {
            "auth" => Some(AuditKind::Auth),
            "mutation" => Some(AuditKind::Mutation),
            "config" => Some(AuditKind::Config),
            "security" => Some(AuditKind::Security),
            // Clarify variants carry fields; when reading back from DB the
            // full detail is in `detail_json`. Return placeholder values so
            // the row is not silently dropped by `row_to_audit`.
            "clarify_requested" => Some(AuditKind::ClarifyRequested {
                clarify_id: ClarifyId::new(),
                conversation_id: String::new(),
                agent_id: AgentId::new(),
                question_len: 0,
            }),
            "clarify_resolved" => Some(AuditKind::ClarifyResolved {
                clarify_id: ClarifyId::new(),
                picked_option: None,
                freeform_len: 0,
            }),
            "clarify_timed_out" => Some(AuditKind::ClarifyTimedOut {
                clarify_id: ClarifyId::new(),
                elapsed_seconds: 0,
            }),
            _ => None,
        }
    }
}

/// Outcome of the audited action.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuditResult {
    Success,
    Failure { reason: String },
}

impl AuditResult {
    fn label(&self) -> &'static str {
        match self {
            AuditResult::Success => "success",
            AuditResult::Failure { .. } => "failure",
        }
    }

    fn failure_reason(&self) -> Option<&str> {
        match self {
            AuditResult::Success => None,
            AuditResult::Failure { reason } => Some(reason.as_str()),
        }
    }
}

/// One audit entry — the structured payload accepted by [`AuditSink::record`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub ts: DateTime<Utc>,
    /// user_id or "system"
    pub actor: String,
    pub kind: AuditKind,
    /// Optional resource target (e.g. `"task:t_01JX4A"`).
    #[serde(default)]
    pub target: Option<String>,
    /// Verb identifier (e.g. `"login.success"`, `"task.create"`).
    pub action: String,
    #[serde(default)]
    pub ip: Option<String>,
    #[serde(default)]
    pub user_agent: Option<String>,
    /// Structured context — arbitrary JSON payload.
    #[serde(default)]
    pub detail: serde_json::Value,
    pub result: AuditResult,
}

impl AuditEntry {
    /// Build a new audit entry with the current timestamp.
    pub fn now(
        actor: impl Into<String>,
        kind: AuditKind,
        action: impl Into<String>,
        target: Option<String>,
        detail: serde_json::Value,
        result: AuditResult,
    ) -> Self {
        Self {
            ts: Utc::now(),
            actor: actor.into(),
            kind,
            target,
            action: action.into(),
            ip: None,
            user_agent: None,
            detail,
            result,
        }
    }
}

/// Row as read back from the database — entry fields plus chain metadata.
#[derive(Debug, Clone, Serialize)]
pub struct AuditRow {
    pub id: i64,
    #[serde(flatten)]
    pub entry: AuditEntry,
    pub prev_hash: Option<String>,
    pub this_hash: String,
}

/// Optional query filters for [`AuditSink::tail`].
#[derive(Debug, Default, Deserialize)]
pub struct AuditQuery {
    #[serde(default)]
    pub actor: Option<String>,
    #[serde(default)]
    pub kind: Option<AuditKind>,
    #[serde(default)]
    pub action_prefix: Option<String>,
    #[serde(default)]
    pub since: Option<DateTime<Utc>>,
}

/// A-4 (2026-05-05) — Failure mode recorded for a capability gap request.
///
/// Surfaced via `/api/v1/admin/capability-requests` so operators know
/// whether the gap is "we never built this capability" (NotFound) versus
/// "policy refused" (GovernanceDenied) versus "we tried but blew up"
/// (ExecutionError).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityRequestReason {
    NotFound,
    GovernanceDenied,
    ExecutionError,
}

impl CapabilityRequestReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            CapabilityRequestReason::NotFound => "not_found",
            CapabilityRequestReason::GovernanceDenied => "governance_denied",
            CapabilityRequestReason::ExecutionError => "execution_error",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "not_found" => Some(CapabilityRequestReason::NotFound),
            "governance_denied" => Some(CapabilityRequestReason::GovernanceDenied),
            "execution_error" => Some(CapabilityRequestReason::ExecutionError),
            _ => None,
        }
    }
}

/// A-4 (2026-05-05) — Status of a capability gap entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityRequestStatus {
    Pending,
    Implemented,
    Rejected,
}

impl CapabilityRequestStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            CapabilityRequestStatus::Pending => "pending",
            CapabilityRequestStatus::Implemented => "implemented",
            CapabilityRequestStatus::Rejected => "rejected",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(CapabilityRequestStatus::Pending),
            "implemented" => Some(CapabilityRequestStatus::Implemented),
            "rejected" => Some(CapabilityRequestStatus::Rejected),
            _ => None,
        }
    }
}

/// D-4 (2026-05-05) — Aggregated failure cluster computed from `audit_log`.
///
/// Output of [`AuditSink::aggregate_capability_failures`]. Each cluster
/// represents N occurrences of the same `(tool_name, capability_id,
/// error_signature)` triple inside the scan window. The feedback loop
/// folds clusters back into `capability_requests` via
/// [`AuditSink::record_capability_request`] so historical / batch
/// failures stay reachable from the admin queue, not just live
/// per-call writes from `chat_handler`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapabilityFailureCluster {
    pub tool_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability_id: Option<String>,
    /// First 80 chars of the error message — used as a coarse signature
    /// so unrelated `latency_ms=...` chatter doesn't shatter clusters.
    pub error_signature: String,
    pub occurrences: u32,
    pub first_seen: i64,
    pub last_seen: i64,
}

/// A-4 (2026-05-05) — Row materialised from the `capability_requests` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityRequestRow {
    pub id: String,
    /// Unix-epoch seconds (UTC) when the gap was first recorded.
    pub requested_at: i64,
    /// Last-seen timestamp; refreshed every time the same gap is hit again.
    pub last_seen_at: i64,
    pub tool_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempted_capability_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempted_connector_id: Option<String>,
    pub failure_reason: CapabilityRequestReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    pub count: i64,
    pub status: CapabilityRequestStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// Outcome of walking the hash chain from the first row to the last.
#[derive(Debug, Clone, Serialize)]
pub struct VerifyReport {
    /// Total number of rows in the table.
    pub total: u64,
    /// Row id (1-based) of the highest row whose hash matches the
    /// re-computed value. Equal to `total` when the table is intact.
    pub ok_until: u64,
    /// First row id where the stored `this_hash` disagrees with the
    /// re-computed hash, or where the chain link `prev_hash` mismatches
    /// the previous row's `this_hash`. `None` when the table is intact.
    pub corrupted_at: Option<u64>,
}

/// Append-only audit sink backed by SQLite (WAL mode).
///
/// Construct once at server startup with [`AuditSink::new`] and share via
/// [`std::sync::Arc`]. Callers record via [`AuditSink::record`]; readers
/// use [`AuditSink::tail`], [`AuditSink::export`], or
/// [`AuditSink::verify_chain`].
///
/// Never expose `clear` / `delete` / `rewrite` through any path. The
/// append-only contract plus the hash chain is the whole point of this
/// type; a public mutator would silently break it.
pub struct AuditSink {
    path: PathBuf,
    conn: Arc<Mutex<Connection>>,
}

impl AuditSink {
    /// Open (or create) the audit database at `path`. Applies WAL pragmas,
    /// runs `CREATE TABLE IF NOT EXISTS`, and on unix sets file mode `0600`.
    pub async fn new(path: PathBuf) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                tokio::fs::create_dir_all(parent).await?;
            }
        }
        let open_path = path.clone();
        let conn = tokio::task::spawn_blocking(move || -> anyhow::Result<Connection> {
            let c = Connection::open(&open_path)?;
            c.pragma_update(None, "journal_mode", "WAL")?;
            c.pragma_update(None, "synchronous", "NORMAL")?;
            c.execute_batch(
                "CREATE TABLE IF NOT EXISTS audit_log (
                    id             INTEGER PRIMARY KEY AUTOINCREMENT,
                    ts             TEXT NOT NULL,
                    actor          TEXT NOT NULL,
                    kind           TEXT NOT NULL,
                    action         TEXT NOT NULL,
                    target         TEXT,
                    ip             TEXT,
                    user_agent     TEXT,
                    detail_json    TEXT NOT NULL,
                    result         TEXT NOT NULL,
                    failure_reason TEXT,
                    prev_hash      TEXT,
                    this_hash      TEXT NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_audit_ts     ON audit_log(ts);
                CREATE INDEX IF NOT EXISTS idx_audit_actor  ON audit_log(actor);
                CREATE INDEX IF NOT EXISTS idx_audit_kind   ON audit_log(kind);

                -- A-4 (2026-05-05) — Capability Gap Queue.
                -- When an LLM tool call cannot be dispatched (capability not
                -- found, governance denial, or execution error), the gap is
                -- recorded here and surfaced via /api/v1/admin/capability-requests.
                -- Aggregated by (tool_name, failure_reason): repeat hits bump
                -- `count` instead of inserting new rows, so the queue stays
                -- short and operators see how often each gap is hit.
                CREATE TABLE IF NOT EXISTS capability_requests (
                    id                       TEXT PRIMARY KEY,
                    requested_at             INTEGER NOT NULL,
                    tool_name                TEXT NOT NULL,
                    attempted_capability_id  TEXT,
                    attempted_connector_id   TEXT,
                    failure_reason           TEXT NOT NULL,
                    actor_id                 TEXT,
                    trace_id                 TEXT,
                    count                    INTEGER NOT NULL DEFAULT 1,
                    status                   TEXT NOT NULL DEFAULT 'pending',
                    notes                    TEXT,
                    last_seen_at             INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_caprq_status ON capability_requests(status);
                CREATE INDEX IF NOT EXISTS idx_caprq_tool   ON capability_requests(tool_name);
                CREATE UNIQUE INDEX IF NOT EXISTS idx_caprq_dedupe
                    ON capability_requests(tool_name, failure_reason, status);",
            )?;
            Ok(c)
        })
        .await
        .map_err(|e| anyhow::anyhow!("audit: spawn_blocking join failed: {e}"))??;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = std::fs::metadata(&path) {
                let mut perm = meta.permissions();
                if perm.mode() & 0o777 != 0o600 {
                    perm.set_mode(0o600);
                    let _ = std::fs::set_permissions(&path, perm);
                }
            }
        }

        Ok(Self {
            path,
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Resolve the default database path: `$CYBERCLAW_AUDIT_DB` overrides
    /// `$HOME/.cyberclaw/audit.db`.
    pub fn default_path() -> PathBuf {
        if let Ok(p) = std::env::var("CYBERCLAW_AUDIT_DB") {
            return PathBuf::from(p);
        }
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(".cyberclaw").join("audit.db"))
            .unwrap_or_else(|| PathBuf::from(".cyberclaw").join("audit.db"))
    }

    /// Absolute path of the backing database file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Write one entry. Errors are logged but swallowed — audit failure
    /// must not break the caller's control flow.
    pub async fn record(&self, entry: AuditEntry) {
        if let Err(err) = self.record_inner(entry).await {
            tracing::error!(path = %self.path.display(), %err, "audit: write failed");
        }
    }

    async fn record_inner(&self, entry: AuditEntry) -> anyhow::Result<()> {
        let detail_json = serde_json::to_string(&entry.detail)?;
        let ts_str = entry
            .ts
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let kind_str = entry.kind.as_str().to_string();
        let result_label = entry.result.label().to_string();
        let failure_reason = entry.result.failure_reason().map(|s| s.to_string());
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let mut guard = conn.blocking_lock();
            let tx = guard.transaction()?;
            // `prev_hash` is the chain head before this insert.
            let prev_hash: Option<String> = tx
                .query_row(
                    "SELECT this_hash FROM audit_log ORDER BY id DESC LIMIT 1",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;

            // Reserve the next id by inserting a placeholder, then update
            // with the computed hash. rusqlite's last_insert_rowid is
            // deterministic inside a transaction.
            tx.execute(
                "INSERT INTO audit_log
                    (ts, actor, kind, action, target, ip, user_agent,
                     detail_json, result, failure_reason, prev_hash, this_hash)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    ts_str,
                    entry.actor,
                    kind_str,
                    entry.action,
                    entry.target,
                    entry.ip,
                    entry.user_agent,
                    detail_json,
                    result_label,
                    failure_reason,
                    prev_hash,
                    "", // placeholder, rewritten below
                ],
            )?;
            let id = tx.last_insert_rowid();
            let this_hash = compute_hash(
                id,
                &ts_str,
                &entry.actor,
                &kind_str,
                &entry.action,
                entry.target.as_deref(),
                &detail_json,
                &result_label,
                failure_reason.as_deref(),
                prev_hash.as_deref(),
            );
            tx.execute(
                "UPDATE audit_log SET this_hash = ?1 WHERE id = ?2",
                params![this_hash, id],
            )?;
            tx.commit()?;
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("audit: spawn_blocking join failed: {e}"))??;
        Ok(())
    }

    /// Convenience helper for the common mutation case.
    pub async fn record_mutation(
        &self,
        actor: impl Into<String>,
        action: impl Into<String>,
        target: Option<String>,
        detail: serde_json::Value,
        result: AuditResult,
    ) {
        self.record(AuditEntry::now(
            actor,
            AuditKind::Mutation,
            action,
            target,
            detail,
            result,
        ))
        .await
    }

    /// R-3 (2026-05-05) — record one row per capability dispatch.
    ///
    /// Called twice per agent tool call: once before dispatch (`status =
    /// "started"`), once after (`status = "success" | "failed"`). The two
    /// rows are linked via `execution_id` (which is also propagated as the
    /// `target` field) so downstream consumers can fold them into a single
    /// span.
    ///
    /// Action prefix is `capability.` so `/api/v1/audit/logs?action_prefix=capability.`
    /// surfaces the granular trail in the admin UI without further wiring.
    #[allow(clippy::too_many_arguments)]
    pub async fn record_capability_invoke(
        &self,
        actor: impl Into<String>,
        execution_id: &str,
        capability_id: &str,
        connector_id: &str,
        tool_name: &str,
        request_hash: Option<String>,
    ) {
        self.record(AuditEntry::now(
            actor,
            AuditKind::Mutation,
            "capability.invoke",
            Some(format!("execution:{}", execution_id)),
            serde_json::json!({
                "capability_id": capability_id,
                "connector_id": connector_id,
                "tool_name": tool_name,
                "request_hash": request_hash,
                "status": "started",
            }),
            AuditResult::Success,
        ))
        .await
    }

    /// Companion to [`record_capability_invoke`]: capture the dispatch
    /// outcome (latency, error, output size) once the connector returned.
    #[allow(clippy::too_many_arguments)]
    pub async fn record_capability_complete(
        &self,
        actor: impl Into<String>,
        execution_id: &str,
        capability_id: &str,
        connector_id: &str,
        tool_name: &str,
        latency_ms: u64,
        output_bytes: usize,
        error: Option<&str>,
    ) {
        let result = match error {
            None => AuditResult::Success,
            Some(reason) => AuditResult::Failure {
                reason: reason.to_string(),
            },
        };
        self.record(AuditEntry::now(
            actor,
            AuditKind::Mutation,
            "capability.complete",
            Some(format!("execution:{}", execution_id)),
            serde_json::json!({
                "capability_id": capability_id,
                "connector_id": connector_id,
                "tool_name": tool_name,
                "latency_ms": latency_ms,
                "output_bytes": output_bytes,
                "status": if error.is_some() { "failed" } else { "success" },
            }),
            result,
        ))
        .await
    }

    /// Convenience helper for the auth category.
    pub async fn record_auth(
        &self,
        actor: impl Into<String>,
        action: impl Into<String>,
        detail: serde_json::Value,
        result: AuditResult,
    ) {
        self.record(AuditEntry::now(
            actor,
            AuditKind::Auth,
            action,
            None,
            detail,
            result,
        ))
        .await
    }

    /// Record one `Security` row per [`SanitizationWarning`].
    ///
    /// `tool_name` is the value the caller passed to
    /// `ToolOutputSanitizer::sanitize` (e.g. `"memory.create"`,
    /// `"clarify_response"`); it is preserved in the audit `detail` so
    /// the frontend can show *where* the hit occurred. The `action`
    /// field uses the form `"sanitizer.<category>"` — this is the same
    /// prefix `/api/v1/security/injection/hits` filters on, so adding
    /// rows here makes them surface in the SPA without further wiring.
    pub async fn record_sanitizer_warnings(
        &self,
        actor: impl Into<String>,
        tool_name: &str,
        target: Option<String>,
        warnings: &[cyberclaw_governance::tool_output_sanitizer::SanitizationWarning],
    ) {
        if warnings.is_empty() {
            return;
        }
        let actor_str = actor.into();
        for w in warnings {
            let category = match w.category {
                cyberclaw_governance::tool_output_sanitizer::WarningCategory::PromptInjection => {
                    "prompt_injection"
                }
                cyberclaw_governance::tool_output_sanitizer::WarningCategory::CredentialLeak => {
                    "credential_leak"
                }
                cyberclaw_governance::tool_output_sanitizer::WarningCategory::SuspiciousUrl => {
                    "suspicious_url"
                }
                cyberclaw_governance::tool_output_sanitizer::WarningCategory::OversizedOutput => {
                    "oversized_output"
                }
            };
            let severity = match w.severity {
                cyberclaw_governance::tool_output_sanitizer::SanitizationSeverity::Info => "INFO",
                cyberclaw_governance::tool_output_sanitizer::SanitizationSeverity::Low => "LOW",
                cyberclaw_governance::tool_output_sanitizer::SanitizationSeverity::Medium => "MED",
                cyberclaw_governance::tool_output_sanitizer::SanitizationSeverity::High => "HIGH",
                cyberclaw_governance::tool_output_sanitizer::SanitizationSeverity::Critical => {
                    "CRIT"
                }
            };
            self.record(AuditEntry::now(
                actor_str.clone(),
                AuditKind::Security,
                format!("sanitizer.{category}"),
                target.clone(),
                serde_json::json!({
                    "tool": tool_name,
                    "pattern": w.message,
                    "severity": severity,
                }),
                AuditResult::Success,
            ))
            .await;
        }
    }

    /// Return up to `limit` most-recent rows that match `filters`, ordered
    /// oldest-first. Returns [`AuditEntry`] to keep the existing API stable;
    /// use [`AuditSink::tail_rows`] when the caller needs id + chain hashes.
    pub async fn tail(
        &self,
        limit: usize,
        filters: &AuditQuery,
    ) -> anyhow::Result<Vec<AuditEntry>> {
        let rows = self.tail_rows(limit, filters).await?;
        Ok(rows.into_iter().map(|r| r.entry).collect())
    }

    /// Like [`AuditSink::tail`] but exposes id + hash-chain metadata.
    pub async fn tail_rows(
        &self,
        limit: usize,
        filters: &AuditQuery,
    ) -> anyhow::Result<Vec<AuditRow>> {
        let conn = self.conn.clone();
        let actor = filters.actor.clone();
        let kind = filters.kind.as_ref().map(|k| k.as_str().to_string());
        let action_prefix = filters.action_prefix.clone();
        let since = filters
            .since
            .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
        let limit = limit.max(1) as i64;

        tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<AuditRow>> {
            let guard = conn.blocking_lock();
            // Build predicates dynamically to keep the SQL + bindings in lockstep.
            let mut sql = String::from(
                "SELECT id, ts, actor, kind, action, target, ip, user_agent,
                        detail_json, result, failure_reason, prev_hash, this_hash
                 FROM audit_log WHERE 1=1",
            );
            let mut binds: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
            if let Some(a) = actor.as_ref() {
                sql.push_str(" AND actor = ?");
                binds.push(Box::new(a.clone()));
            }
            if let Some(k) = kind.as_ref() {
                sql.push_str(" AND kind = ?");
                binds.push(Box::new(k.clone()));
            }
            if let Some(p) = action_prefix.as_ref() {
                sql.push_str(" AND action LIKE ?");
                binds.push(Box::new(format!("{p}%")));
            }
            if let Some(s) = since.as_ref() {
                sql.push_str(" AND ts >= ?");
                binds.push(Box::new(s.clone()));
            }
            sql.push_str(" ORDER BY id DESC LIMIT ?");
            binds.push(Box::new(limit));

            let mut stmt = guard.prepare(&sql)?;
            let bind_refs: Vec<&dyn rusqlite::ToSql> = binds
                .iter()
                .map(|b| b.as_ref() as &dyn rusqlite::ToSql)
                .collect();
            let mut out = Vec::new();
            let rows =
                stmt.query_map(rusqlite::params_from_iter(bind_refs.iter()), row_to_audit)?;
            for r in rows {
                out.push(r?);
            }
            // Return oldest-first for UI stability.
            out.reverse();
            Ok(out)
        })
        .await
        .map_err(|e| anyhow::anyhow!("audit: spawn_blocking join failed: {e}"))?
    }

    /// Sprint 20 W1 — RB-11 backup primitive.
    ///
    /// Snapshot the entire audit DB to `dest` via SQLite's `VACUUM INTO`,
    /// the only safe backup primitive under WAL mode. Unlike `cp`,
    /// `VACUUM INTO` produces a chain-consistent file even when the
    /// main DB is being concurrently written: SQLite handles the WAL
    /// drain internally.
    ///
    /// The destination must NOT already exist; SQLite refuses to
    /// overwrite. Caller is responsible for unique naming (typically
    /// `audit-<UTC-iso8601>.db`).
    ///
    /// After this returns Ok, run [`AuditSink::verify_chain_at`] on
    /// `dest` to confirm the snapshot is intact before signing /
    /// uploading.
    pub async fn vacuum_into(&self, dest: PathBuf) -> anyhow::Result<()> {
        let conn = self.conn.clone();
        let dest_str = dest
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("dest path is not valid UTF-8"))?
            .to_string();
        // Reject paths with single-quotes — SQLite literal injection.
        if dest_str.contains('\'') {
            anyhow::bail!("dest path contains single-quote, refusing to VACUUM INTO");
        }
        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let guard = conn.blocking_lock();
            // VACUUM INTO is parameter-safe but rusqlite's prepare()
            // does not accept VACUUM as a prepared statement; use
            // `execute_batch` with the path inline (we sanitised
            // single-quotes above).
            guard.execute_batch(&format!("VACUUM INTO '{}'", dest_str))?;
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("audit: spawn_blocking join failed: {e}"))?
    }

    /// Verify the hash chain on a separate audit DB file (a backup
    /// produced by [`AuditSink::vacuum_into`]). Returns the same
    /// [`VerifyReport`] as [`AuditSink::verify_chain`].
    pub async fn verify_chain_at(path: PathBuf) -> anyhow::Result<VerifyReport> {
        let sink = Self::new(path).await?;
        sink.verify_chain().await
    }

    /// Dump the full table as JSON Lines bytes for off-line archival.
    /// Never truncates or mutates the database.
    pub async fn export(&self) -> anyhow::Result<Vec<u8>> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<u8>> {
            let guard = conn.blocking_lock();
            let mut stmt = guard.prepare(
                "SELECT id, ts, actor, kind, action, target, ip, user_agent,
                        detail_json, result, failure_reason, prev_hash, this_hash
                 FROM audit_log ORDER BY id ASC",
            )?;
            let rows = stmt.query_map([], row_to_audit)?;
            let mut buf = Vec::new();
            for r in rows {
                let row = r?;
                let line = serde_json::to_string(&row)?;
                buf.extend_from_slice(line.as_bytes());
                buf.push(b'\n');
            }
            Ok(buf)
        })
        .await
        .map_err(|e| anyhow::anyhow!("audit: spawn_blocking join failed: {e}"))?
    }

    // ---------------------------------------------------------------------
    // A-4 (2026-05-05) — Capability Gap Queue
    // ---------------------------------------------------------------------

    /// Record a capability gap.
    ///
    /// Aggregation rule: when a row already exists with the same
    /// `(tool_name, failure_reason, status='pending')` triple, its `count`
    /// is incremented and `last_seen_at` is refreshed; otherwise a new
    /// row is inserted. Once an operator marks a row `implemented` or
    /// `rejected`, future hits of the same `(tool_name, failure_reason)`
    /// open a fresh `pending` row — operators stay informed when a
    /// supposedly-fixed gap regresses.
    ///
    /// Errors are logged but swallowed — gap recording must never break
    /// the caller's control flow.
    #[allow(clippy::too_many_arguments)]
    pub async fn record_capability_request(
        &self,
        tool_name: &str,
        attempted_capability_id: Option<&str>,
        attempted_connector_id: Option<&str>,
        failure_reason: CapabilityRequestReason,
        actor_id: Option<&str>,
        trace_id: Option<&str>,
    ) {
        self.record_capability_request_with_notes(
            tool_name,
            attempted_capability_id,
            attempted_connector_id,
            failure_reason,
            actor_id,
            trace_id,
            None,
        )
        .await;
    }

    /// D-4 (2026-05-05) — same as [`record_capability_request`] but also
    /// accepts an operator-facing `notes` blurb. Used by the feedback
    /// loop to attach an "auto-feedback: N occurrences..." trail so the
    /// admin queue distinguishes batch-detected gaps from live ones.
    ///
    /// Notes only overwrite when non-null; existing operator notes on a
    /// pending row are preserved (`COALESCE(?2, notes)` semantics).
    #[allow(clippy::too_many_arguments)]
    pub async fn record_capability_request_with_notes(
        &self,
        tool_name: &str,
        attempted_capability_id: Option<&str>,
        attempted_connector_id: Option<&str>,
        failure_reason: CapabilityRequestReason,
        actor_id: Option<&str>,
        trace_id: Option<&str>,
        notes: Option<&str>,
    ) {
        let conn = self.conn.clone();
        let tool_name = tool_name.to_string();
        let attempted_capability_id = attempted_capability_id.map(|s| s.to_string());
        let attempted_connector_id = attempted_connector_id.map(|s| s.to_string());
        let actor_id = actor_id.map(|s| s.to_string());
        let trace_id = trace_id.map(|s| s.to_string());
        let notes = notes.map(|s| s.to_string());
        let reason_str = failure_reason.as_str().to_string();
        let now = chrono::Utc::now().timestamp();

        let res = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let mut guard = conn.blocking_lock();
            let tx = guard.transaction()?;

            // Look up an existing pending row for this (tool, reason).
            let existing_id: Option<String> = tx
                .query_row(
                    "SELECT id FROM capability_requests
                     WHERE tool_name = ?1 AND failure_reason = ?2 AND status = 'pending'
                     LIMIT 1",
                    params![tool_name, reason_str],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;

            if let Some(id) = existing_id {
                tx.execute(
                    "UPDATE capability_requests
                       SET count = count + 1,
                           last_seen_at = ?1,
                           actor_id = COALESCE(?2, actor_id),
                           trace_id = COALESCE(?3, trace_id),
                           attempted_capability_id = COALESCE(?4, attempted_capability_id),
                           attempted_connector_id = COALESCE(?5, attempted_connector_id),
                           notes = COALESCE(notes, ?6)
                     WHERE id = ?7",
                    params![
                        now,
                        actor_id,
                        trace_id,
                        attempted_capability_id,
                        attempted_connector_id,
                        notes,
                        id
                    ],
                )?;
            } else {
                let id = uuid::Uuid::new_v4().simple().to_string();
                tx.execute(
                    "INSERT INTO capability_requests
                        (id, requested_at, tool_name, attempted_capability_id,
                         attempted_connector_id, failure_reason, actor_id, trace_id,
                         count, status, notes, last_seen_at)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, 1, 'pending', ?, ?)",
                    params![
                        id,
                        now,
                        tool_name,
                        attempted_capability_id,
                        attempted_connector_id,
                        reason_str,
                        actor_id,
                        trace_id,
                        notes,
                        now
                    ],
                )?;
            }
            tx.commit()?;
            Ok(())
        })
        .await;

        match res {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::error!(%e, "audit: record_capability_request failed"),
            Err(e) => tracing::error!(%e, "audit: record_capability_request join failed"),
        }
    }

    /// List capability gap rows, optionally filtered by status. Newest first
    /// (most recent `last_seen_at`). `limit` caps the response size.
    pub async fn list_capability_requests(
        &self,
        status: Option<CapabilityRequestStatus>,
        limit: usize,
    ) -> anyhow::Result<Vec<CapabilityRequestRow>> {
        let conn = self.conn.clone();
        let status_str = status.map(|s| s.as_str().to_string());
        let limit = limit.max(1) as i64;

        tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<CapabilityRequestRow>> {
            let guard = conn.blocking_lock();
            let mut sql = String::from(
                "SELECT id, requested_at, tool_name, attempted_capability_id,
                        attempted_connector_id, failure_reason, actor_id, trace_id,
                        count, status, notes, last_seen_at
                 FROM capability_requests",
            );
            let mut binds: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
            if let Some(s) = status_str.as_ref() {
                sql.push_str(" WHERE status = ?");
                binds.push(Box::new(s.clone()));
            }
            sql.push_str(" ORDER BY last_seen_at DESC LIMIT ?");
            binds.push(Box::new(limit));

            let mut stmt = guard.prepare(&sql)?;
            let bind_refs: Vec<&dyn rusqlite::ToSql> = binds
                .iter()
                .map(|b| b.as_ref() as &dyn rusqlite::ToSql)
                .collect();
            let rows = stmt.query_map(rusqlite::params_from_iter(bind_refs.iter()), |row| {
                let reason_str: String = row.get(5)?;
                let status_str: String = row.get(9)?;
                Ok(CapabilityRequestRow {
                    id: row.get(0)?,
                    requested_at: row.get(1)?,
                    tool_name: row.get(2)?,
                    attempted_capability_id: row.get(3)?,
                    attempted_connector_id: row.get(4)?,
                    failure_reason: CapabilityRequestReason::parse(&reason_str)
                        .unwrap_or(CapabilityRequestReason::ExecutionError),
                    actor_id: row.get(6)?,
                    trace_id: row.get(7)?,
                    count: row.get(8)?,
                    status: CapabilityRequestStatus::parse(&status_str)
                        .unwrap_or(CapabilityRequestStatus::Pending),
                    notes: row.get(10)?,
                    last_seen_at: row.get(11)?,
                })
            })?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r?);
            }
            Ok(out)
        })
        .await
        .map_err(|e| anyhow::anyhow!("audit: spawn_blocking join failed: {e}"))?
    }

    /// Update the `status` (and optionally `notes`) on a capability gap row.
    /// Returns `true` when the row exists and was mutated.
    pub async fn update_capability_request_status(
        &self,
        id: &str,
        status: CapabilityRequestStatus,
        notes: Option<&str>,
    ) -> anyhow::Result<bool> {
        let conn = self.conn.clone();
        let id = id.to_string();
        let status_str = status.as_str().to_string();
        let notes = notes.map(|s| s.to_string());
        tokio::task::spawn_blocking(move || -> anyhow::Result<bool> {
            let guard = conn.blocking_lock();
            let n = guard.execute(
                "UPDATE capability_requests
                    SET status = ?1,
                        notes  = COALESCE(?2, notes)
                  WHERE id = ?3",
                params![status_str, notes, id],
            )?;
            Ok(n > 0)
        })
        .await
        .map_err(|e| anyhow::anyhow!("audit: spawn_blocking join failed: {e}"))?
    }

    /// D-4 (2026-05-05) — Aggregate failed `capability.complete` rows.
    ///
    /// Scans `audit_log` for rows where:
    ///   - `action = 'capability.complete'`
    ///   - `timestamp >= since` (RFC3339; converted from unix-epoch seconds)
    ///   - `json_extract(detail, '$.status') = 'error'` OR
    ///     `json_extract(detail, '$.status') = 'failed'` (legacy spelling
    ///     used by [`record_capability_complete`] before D-4)
    ///
    /// Groups by `(tool_name, capability_id, substr(error, 0, 80))` and
    /// returns clusters with `occurrences >= min_count`. The feedback
    /// loop folds these back into `capability_requests` so the admin
    /// queue eventually surfaces gaps that only appear in batch
    /// `capability.complete` rows (e.g. a fleet-wide tool regression
    /// that never hit the live `chat_handler` path).
    ///
    /// Read-only — never writes to either table. The companion writer
    /// is [`record_capability_request`].
    pub async fn aggregate_capability_failures(
        &self,
        since: i64,
        min_count: u32,
    ) -> anyhow::Result<Vec<CapabilityFailureCluster>> {
        let conn = self.conn.clone();
        // Convert the unix-epoch cutoff to the RFC3339 form actually stored
        // in `audit_log.ts` (see `record_inner`). String comparison on the
        // ISO-8601 form is monotonic in real time, so `ts >= ?1` is correct.
        let since_str = chrono::DateTime::<chrono::Utc>::from_timestamp(since, 0)
            .unwrap_or_else(chrono::Utc::now)
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let min_count_i64 = min_count.max(1) as i64;

        tokio::task::spawn_blocking(
            move || -> anyhow::Result<Vec<CapabilityFailureCluster>> {
                let guard = conn.blocking_lock();
                // The failure_reason column already carries the error string for
                // AuditResult::Failure rows (see `record_inner`); use it as the
                // canonical signature source rather than re-parsing detail_json.
                // We still gate on detail.status to skip stale `success` rows
                // that share the same action.
                let mut stmt = guard.prepare(
                    "SELECT
                        json_extract(detail_json, '$.tool_name')     AS tool_name,
                        json_extract(detail_json, '$.capability_id') AS capability_id,
                        substr(COALESCE(failure_reason, json_extract(detail_json, '$.error'), ''), 1, 80) AS error_signature,
                        COUNT(*)                                     AS occurrences,
                        MIN(ts)                                      AS first_seen,
                        MAX(ts)                                      AS last_seen
                     FROM audit_log
                     WHERE action = 'capability.complete'
                       AND ts >= ?1
                       AND (json_extract(detail_json, '$.status') = 'error'
                            OR json_extract(detail_json, '$.status') = 'failed'
                            OR result = 'failure')
                       AND tool_name IS NOT NULL
                     GROUP BY tool_name, capability_id, error_signature
                     HAVING COUNT(*) >= ?2
                     ORDER BY occurrences DESC, last_seen DESC",
                )?;
                let rows = stmt.query_map(params![since_str, min_count_i64], |row| {
                    let tool_name: String = row.get(0)?;
                    let capability_id: Option<String> = row.get(1)?;
                    let error_signature: String = row.get::<_, Option<String>>(2)?.unwrap_or_default();
                    let occurrences: i64 = row.get(3)?;
                    let first_seen_ts: String = row.get(4)?;
                    let last_seen_ts: String = row.get(5)?;
                    let parse_ts = |s: &str| -> i64 {
                        chrono::DateTime::parse_from_rfc3339(s)
                            .map(|t| t.timestamp())
                            .unwrap_or(0)
                    };
                    Ok(CapabilityFailureCluster {
                        tool_name,
                        capability_id,
                        error_signature,
                        occurrences: occurrences.max(0) as u32,
                        first_seen: parse_ts(&first_seen_ts),
                        last_seen: parse_ts(&last_seen_ts),
                    })
                })?;
                let mut out = Vec::new();
                for r in rows {
                    out.push(r?);
                }
                Ok(out)
            },
        )
        .await
        .map_err(|e| anyhow::anyhow!("audit: spawn_blocking join failed: {e}"))?
    }

    /// Walk the entire table and re-compute each row's `this_hash`. Returns
    /// a [`VerifyReport`] describing the first mismatch or confirming the
    /// whole chain is intact.
    pub async fn verify_chain(&self) -> anyhow::Result<VerifyReport> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<VerifyReport> {
            let guard = conn.blocking_lock();
            let total: u64 = guard.query_row("SELECT COUNT(*) FROM audit_log", [], |row| {
                row.get::<_, i64>(0)
            })? as u64;
            let mut stmt = guard.prepare(
                "SELECT id, ts, actor, kind, action, target,
                        detail_json, result, failure_reason, prev_hash, this_hash
                 FROM audit_log ORDER BY id ASC",
            )?;
            let mut rows = stmt.query([])?;
            let mut ok_until: u64 = 0;
            let mut expected_prev: Option<String> = None;
            let mut corrupted_at: Option<u64> = None;
            while let Some(row) = rows.next()? {
                let id: i64 = row.get(0)?;
                let ts: String = row.get(1)?;
                let actor: String = row.get(2)?;
                let kind: String = row.get(3)?;
                let action: String = row.get(4)?;
                let target: Option<String> = row.get(5)?;
                let detail_json: String = row.get(6)?;
                let result: String = row.get(7)?;
                let failure_reason: Option<String> = row.get(8)?;
                let prev_hash: Option<String> = row.get(9)?;
                let stored_hash: String = row.get(10)?;

                // Chain link check.
                if prev_hash != expected_prev {
                    corrupted_at = Some(id as u64);
                    break;
                }
                let recomputed = compute_hash(
                    id,
                    &ts,
                    &actor,
                    &kind,
                    &action,
                    target.as_deref(),
                    &detail_json,
                    &result,
                    failure_reason.as_deref(),
                    prev_hash.as_deref(),
                );
                if recomputed != stored_hash {
                    corrupted_at = Some(id as u64);
                    break;
                }
                ok_until = id as u64;
                expected_prev = Some(stored_hash);
            }
            Ok(VerifyReport {
                total,
                ok_until,
                corrupted_at,
            })
        })
        .await
        .map_err(|e| anyhow::anyhow!("audit: spawn_blocking join failed: {e}"))?
    }
}

/// Materialize a DB row into an [`AuditRow`].
fn row_to_audit(row: &rusqlite::Row<'_>) -> rusqlite::Result<AuditRow> {
    let id: i64 = row.get(0)?;
    let ts_str: String = row.get(1)?;
    let actor: String = row.get(2)?;
    let kind_str: String = row.get(3)?;
    let action: String = row.get(4)?;
    let target: Option<String> = row.get(5)?;
    let ip: Option<String> = row.get(6)?;
    let user_agent: Option<String> = row.get(7)?;
    let detail_json: String = row.get(8)?;
    let result_label: String = row.get(9)?;
    let failure_reason: Option<String> = row.get(10)?;
    let prev_hash: Option<String> = row.get(11)?;
    let this_hash: String = row.get(12)?;

    let ts = DateTime::parse_from_rfc3339(&ts_str)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e))
        })?;
    let kind = AuditKind::from_str(&kind_str).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            3,
            rusqlite::types::Type::Text,
            Box::<dyn std::error::Error + Send + Sync>::from(format!("unknown kind: {kind_str}")),
        )
    })?;
    let detail: serde_json::Value =
        serde_json::from_str(&detail_json).unwrap_or(serde_json::Value::Null);
    let result = match result_label.as_str() {
        "success" => AuditResult::Success,
        "failure" => AuditResult::Failure {
            reason: failure_reason.clone().unwrap_or_default(),
        },
        other => AuditResult::Failure {
            reason: format!("unknown result label: {other}"),
        },
    };
    Ok(AuditRow {
        id,
        entry: AuditEntry {
            ts,
            actor,
            kind,
            target,
            action,
            ip,
            user_agent,
            detail,
            result,
        },
        prev_hash,
        this_hash,
    })
}

/// Compute `this_hash = sha256(id || ts || actor || kind || action || target
/// || detail_json || result || failure_reason || prev_hash)`, with `\x00`
/// separators between fields to avoid ambiguity.
#[allow(clippy::too_many_arguments)]
fn compute_hash(
    id: i64,
    ts: &str,
    actor: &str,
    kind: &str,
    action: &str,
    target: Option<&str>,
    detail_json: &str,
    result: &str,
    failure_reason: Option<&str>,
    prev_hash: Option<&str>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(id.to_le_bytes());
    hasher.update([0u8]);
    hasher.update(ts.as_bytes());
    hasher.update([0u8]);
    hasher.update(actor.as_bytes());
    hasher.update([0u8]);
    hasher.update(kind.as_bytes());
    hasher.update([0u8]);
    hasher.update(action.as_bytes());
    hasher.update([0u8]);
    hasher.update(target.unwrap_or("").as_bytes());
    hasher.update([0u8]);
    hasher.update(detail_json.as_bytes());
    hasher.update([0u8]);
    hasher.update(result.as_bytes());
    hasher.update([0u8]);
    hasher.update(failure_reason.unwrap_or("").as_bytes());
    hasher.update([0u8]);
    hasher.update(prev_hash.unwrap_or("").as_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn entry(actor: &str, action: &str, kind: AuditKind) -> AuditEntry {
        AuditEntry::now(
            actor.to_string(),
            kind,
            action.to_string(),
            None,
            serde_json::json!({}),
            AuditResult::Success,
        )
    }

    #[tokio::test]
    async fn audit_sink_insert_and_tail() {
        let tmp = TempDir::new().unwrap();
        let sink = AuditSink::new(tmp.path().join("audit.db")).await.unwrap();
        sink.record(entry("alice", "login.success", AuditKind::Auth))
            .await;
        sink.record(entry("bob", "task.create", AuditKind::Mutation))
            .await;
        let got = sink.tail(100, &AuditQuery::default()).await.unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].actor, "alice");
        assert_eq!(got[1].actor, "bob");
    }

    #[tokio::test]
    async fn record_sanitizer_warnings_roundtrips_through_filter() {
        use cyberclaw_governance::tool_output_sanitizer::{
            SanitizationSeverity, SanitizationWarning, WarningCategory,
        };

        let tmp = TempDir::new().unwrap();
        let sink = AuditSink::new(tmp.path().join("audit.db")).await.unwrap();

        let warnings = vec![
            SanitizationWarning {
                category: WarningCategory::PromptInjection,
                message: "ignore previous instructions".to_string(),
                severity: SanitizationSeverity::Critical,
            },
            SanitizationWarning {
                category: WarningCategory::CredentialLeak,
                message: "ANTHROPIC_API_KEY=sk-ant-…".to_string(),
                severity: SanitizationSeverity::High,
            },
        ];

        sink.record_sanitizer_warnings(
            "op_ada",
            "memory.create",
            Some("memory:m_01".to_string()),
            &warnings,
        )
        .await;

        // Filter mirrors what /api/v1/security/injection/hits uses.
        let rows = sink
            .tail_rows(
                10,
                &AuditQuery {
                    kind: Some(AuditKind::Security),
                    action_prefix: Some("sanitizer.".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        assert_eq!(rows.len(), 2, "both warnings must persist");

        let actions: Vec<&str> = rows.iter().map(|r| r.entry.action.as_str()).collect();
        assert!(actions.contains(&"sanitizer.prompt_injection"));
        assert!(actions.contains(&"sanitizer.credential_leak"));

        for r in &rows {
            assert_eq!(r.entry.actor, "op_ada");
            assert_eq!(r.entry.target.as_deref(), Some("memory:m_01"));
            assert_eq!(
                r.entry.detail.get("tool").and_then(|v| v.as_str()),
                Some("memory.create")
            );
            assert!(r.entry.detail.get("pattern").is_some());
            assert!(r.entry.detail.get("severity").is_some());
        }

        let cred_row = rows
            .iter()
            .find(|r| r.entry.action == "sanitizer.credential_leak")
            .unwrap();
        assert_eq!(
            cred_row
                .entry
                .detail
                .get("severity")
                .and_then(|v| v.as_str()),
            Some("HIGH")
        );
    }

    #[tokio::test]
    async fn capability_invoke_records_two_rows_per_dispatch() {
        // R-3: every tool call must produce a `capability.invoke` row plus
        // a `capability.complete` row, linked by execution id, so the
        // audit chain has tool-level granularity (not just agent.invoke).
        let tmp = TempDir::new().unwrap();
        let sink = AuditSink::new(tmp.path().join("audit.db")).await.unwrap();

        sink.record_capability_invoke(
            "agent_alpha",
            "exec_01",
            "fs.read",
            "local",
            "Read",
            Some("hash_abc".into()),
        )
        .await;
        sink.record_capability_complete(
            "agent_alpha",
            "exec_01",
            "fs.read",
            "local",
            "Read",
            42,
            128,
            None,
        )
        .await;

        let rows = sink
            .tail(
                100,
                &AuditQuery {
                    action_prefix: Some("capability.".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        assert_eq!(rows.len(), 2, "must have one start + one complete row");
        let actions: Vec<&str> = rows.iter().map(|e| e.action.as_str()).collect();
        assert!(actions.contains(&"capability.invoke"));
        assert!(actions.contains(&"capability.complete"));
        for r in &rows {
            assert_eq!(r.target.as_deref(), Some("execution:exec_01"));
            assert_eq!(
                r.detail.get("capability_id").and_then(|v| v.as_str()),
                Some("fs.read")
            );
        }
    }

    #[tokio::test]
    async fn capability_complete_records_failure() {
        let tmp = TempDir::new().unwrap();
        let sink = AuditSink::new(tmp.path().join("audit.db")).await.unwrap();
        sink.record_capability_complete(
            "agent_alpha",
            "exec_02",
            "cmd.run",
            "local",
            "Bash",
            100,
            0,
            Some("forbidden pattern"),
        )
        .await;
        let rows = sink
            .tail(
                10,
                &AuditQuery {
                    action_prefix: Some("capability.".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        match &rows[0].result {
            AuditResult::Failure { reason } => {
                assert!(reason.contains("forbidden pattern"))
            }
            other => panic!("expected Failure, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn record_sanitizer_warnings_empty_is_noop() {
        let tmp = TempDir::new().unwrap();
        let sink = AuditSink::new(tmp.path().join("audit.db")).await.unwrap();

        sink.record_sanitizer_warnings("op_x", "memory.create", None, &[])
            .await;

        let rows = sink.tail_rows(10, &AuditQuery::default()).await.unwrap();
        assert_eq!(rows.len(), 0, "empty warnings must not write rows");
    }

    #[tokio::test]
    async fn audit_hash_chain_links_correctly() {
        let tmp = TempDir::new().unwrap();
        let sink = AuditSink::new(tmp.path().join("audit.db")).await.unwrap();
        for i in 0..5 {
            sink.record(entry(&format!("u{i}"), "x.y", AuditKind::Mutation))
                .await;
        }
        let rows = sink.tail_rows(100, &AuditQuery::default()).await.unwrap();
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0].prev_hash, None, "first row has no prev_hash");
        for w in rows.windows(2) {
            assert_eq!(
                w[1].prev_hash.as_deref(),
                Some(w[0].this_hash.as_str()),
                "chain link at id={} must match predecessor",
                w[1].id
            );
        }
        let report = sink.verify_chain().await.unwrap();
        assert_eq!(report.total, 5);
        assert_eq!(report.ok_until, 5);
        assert_eq!(report.corrupted_at, None);
    }

    #[tokio::test]
    async fn audit_verify_chain_detects_tampering() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("audit.db");
        let sink = AuditSink::new(db_path.clone()).await.unwrap();
        for i in 0..4 {
            sink.record(entry(&format!("u{i}"), "a.b", AuditKind::Mutation))
                .await;
        }
        // Tamper with row 2: rewrite its `actor` field out from under us.
        // The sink holds an open WAL connection, so we go through it.
        {
            let conn = sink.conn.clone();
            tokio::task::spawn_blocking(move || {
                let guard = conn.blocking_lock();
                guard
                    .execute("UPDATE audit_log SET actor = 'mallory' WHERE id = 2", [])
                    .unwrap();
            })
            .await
            .unwrap();
        }
        let report = sink.verify_chain().await.unwrap();
        assert_eq!(report.total, 4);
        assert_eq!(report.ok_until, 1, "row 1 still verifies");
        assert_eq!(report.corrupted_at, Some(2), "row 2 is the first break");
    }

    #[tokio::test]
    async fn audit_query_by_actor() {
        let tmp = TempDir::new().unwrap();
        let sink = AuditSink::new(tmp.path().join("audit.db")).await.unwrap();
        sink.record(entry("alice", "a.b", AuditKind::Mutation))
            .await;
        sink.record(entry("bob", "a.b", AuditKind::Mutation)).await;
        sink.record(entry("alice", "c.d", AuditKind::Mutation))
            .await;
        let got = sink
            .tail(
                100,
                &AuditQuery {
                    actor: Some("alice".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(got.len(), 2);
        assert!(got.iter().all(|e| e.actor == "alice"));
    }

    #[tokio::test]
    async fn audit_query_by_kind() {
        let tmp = TempDir::new().unwrap();
        let sink = AuditSink::new(tmp.path().join("audit.db")).await.unwrap();
        sink.record(entry("alice", "login", AuditKind::Auth)).await;
        sink.record(entry("alice", "task.create", AuditKind::Mutation))
            .await;
        sink.record(entry("alice", "login.failed", AuditKind::Auth))
            .await;
        let got = sink
            .tail(
                100,
                &AuditQuery {
                    kind: Some(AuditKind::Auth),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(got.len(), 2);
        assert!(got.iter().all(|e| e.kind == AuditKind::Auth));
    }

    #[tokio::test]
    async fn audit_query_by_action_prefix() {
        let tmp = TempDir::new().unwrap();
        let sink = AuditSink::new(tmp.path().join("audit.db")).await.unwrap();
        sink.record(entry("a", "login.success", AuditKind::Auth))
            .await;
        sink.record(entry("a", "login.failed", AuditKind::Auth))
            .await;
        sink.record(entry("a", "task.create", AuditKind::Mutation))
            .await;
        let got = sink
            .tail(
                100,
                &AuditQuery {
                    action_prefix: Some("login.".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(got.len(), 2);
        assert!(got.iter().all(|e| e.action.starts_with("login.")));
    }

    #[tokio::test]
    async fn audit_export_returns_json_lines() {
        let tmp = TempDir::new().unwrap();
        let sink = AuditSink::new(tmp.path().join("audit.db")).await.unwrap();
        sink.record(entry("carol", "x.y", AuditKind::Mutation))
            .await;
        sink.record(entry("dan", "a.b", AuditKind::Auth)).await;
        let bytes = sink.export().await.unwrap();
        let text = String::from_utf8(bytes).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        // Every line parses as JSON with id + this_hash.
        for line in lines {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            assert!(v.get("id").is_some(), "each line must carry an id");
            assert!(
                v.get("this_hash").is_some(),
                "each line must carry this_hash"
            );
        }
        assert!(text.contains("carol"));
        assert!(text.contains("dan"));
    }

    // ---------------------------------------------------------------------
    // A-4 — Capability Gap Queue
    // ---------------------------------------------------------------------

    #[tokio::test]
    async fn capability_request_insert_and_list() {
        let tmp = TempDir::new().unwrap();
        let sink = AuditSink::new(tmp.path().join("audit.db")).await.unwrap();
        sink.record_capability_request(
            "verify_numeric",
            None,
            None,
            CapabilityRequestReason::NotFound,
            Some("agent_alpha"),
            Some("trace_001"),
        )
        .await;
        let rows = sink.list_capability_requests(None, 100).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].tool_name, "verify_numeric");
        assert_eq!(rows[0].count, 1);
        assert_eq!(rows[0].failure_reason, CapabilityRequestReason::NotFound);
        assert_eq!(rows[0].status, CapabilityRequestStatus::Pending);
        assert_eq!(rows[0].actor_id.as_deref(), Some("agent_alpha"));
    }

    #[tokio::test]
    async fn capability_request_aggregates_same_tool_reason() {
        let tmp = TempDir::new().unwrap();
        let sink = AuditSink::new(tmp.path().join("audit.db")).await.unwrap();
        for _ in 0..5 {
            sink.record_capability_request(
                "verify_numeric",
                None,
                None,
                CapabilityRequestReason::NotFound,
                None,
                None,
            )
            .await;
        }
        let rows = sink.list_capability_requests(None, 100).await.unwrap();
        assert_eq!(rows.len(), 1, "same tool+reason must aggregate");
        assert_eq!(rows[0].count, 5);
    }

    #[tokio::test]
    async fn capability_request_distinct_reasons_separate_rows() {
        let tmp = TempDir::new().unwrap();
        let sink = AuditSink::new(tmp.path().join("audit.db")).await.unwrap();
        sink.record_capability_request(
            "verify_numeric",
            None,
            None,
            CapabilityRequestReason::NotFound,
            None,
            None,
        )
        .await;
        sink.record_capability_request(
            "verify_numeric",
            None,
            None,
            CapabilityRequestReason::GovernanceDenied,
            None,
            None,
        )
        .await;
        let rows = sink.list_capability_requests(None, 100).await.unwrap();
        assert_eq!(rows.len(), 2);
    }

    #[tokio::test]
    async fn capability_request_filters_by_status() {
        let tmp = TempDir::new().unwrap();
        let sink = AuditSink::new(tmp.path().join("audit.db")).await.unwrap();
        sink.record_capability_request(
            "verify_numeric",
            None,
            None,
            CapabilityRequestReason::NotFound,
            None,
            None,
        )
        .await;
        let rows = sink
            .list_capability_requests(Some(CapabilityRequestStatus::Pending), 10)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        let id = rows[0].id.clone();
        let updated = sink
            .update_capability_request_status(
                &id,
                CapabilityRequestStatus::Implemented,
                Some("shipped in S37"),
            )
            .await
            .unwrap();
        assert!(updated);

        let pending = sink
            .list_capability_requests(Some(CapabilityRequestStatus::Pending), 10)
            .await
            .unwrap();
        assert_eq!(pending.len(), 0);

        let done = sink
            .list_capability_requests(Some(CapabilityRequestStatus::Implemented), 10)
            .await
            .unwrap();
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].notes.as_deref(), Some("shipped in S37"));
    }

    #[tokio::test]
    async fn capability_request_update_unknown_returns_false() {
        let tmp = TempDir::new().unwrap();
        let sink = AuditSink::new(tmp.path().join("audit.db")).await.unwrap();
        let updated = sink
            .update_capability_request_status("no_such_id", CapabilityRequestStatus::Rejected, None)
            .await
            .unwrap();
        assert!(!updated);
    }

    // ---------------------------------------------------------------------
    // D-4 — Aggregate capability failures from audit_log
    // ---------------------------------------------------------------------

    async fn record_failure(sink: &AuditSink, tool: &str, cap_id: &str, error: &str) {
        sink.record_capability_complete(
            "agent_alpha",
            "exec_test",
            cap_id,
            "local",
            tool,
            10,
            0,
            Some(error),
        )
        .await;
    }

    #[tokio::test]
    async fn aggregate_capability_failures_groups_repeated_errors() {
        let tmp = TempDir::new().unwrap();
        let sink = AuditSink::new(tmp.path().join("audit.db")).await.unwrap();
        for _ in 0..5 {
            record_failure(&sink, "verify_numeric", "verify.numeric", "boom").await;
        }
        let since = chrono::Utc::now().timestamp() - 60;
        let clusters = sink.aggregate_capability_failures(since, 3).await.unwrap();
        assert_eq!(clusters.len(), 1, "all 5 hits collapse into one cluster");
        assert_eq!(clusters[0].tool_name, "verify_numeric");
        assert_eq!(clusters[0].occurrences, 5);
        assert_eq!(clusters[0].capability_id.as_deref(), Some("verify.numeric"));
    }

    #[tokio::test]
    async fn aggregate_capability_failures_distinct_tools_separate_clusters() {
        let tmp = TempDir::new().unwrap();
        let sink = AuditSink::new(tmp.path().join("audit.db")).await.unwrap();
        for _ in 0..3 {
            record_failure(&sink, "tool_a", "cap.a", "err_a").await;
        }
        for _ in 0..3 {
            record_failure(&sink, "tool_b", "cap.b", "err_b").await;
        }
        let since = chrono::Utc::now().timestamp() - 60;
        let clusters = sink.aggregate_capability_failures(since, 3).await.unwrap();
        assert_eq!(clusters.len(), 2);
        let tools: Vec<&str> = clusters.iter().map(|c| c.tool_name.as_str()).collect();
        assert!(tools.contains(&"tool_a"));
        assert!(tools.contains(&"tool_b"));
    }

    #[tokio::test]
    async fn aggregate_capability_failures_respects_min_count() {
        let tmp = TempDir::new().unwrap();
        let sink = AuditSink::new(tmp.path().join("audit.db")).await.unwrap();
        for _ in 0..5 {
            record_failure(&sink, "verify_numeric", "verify.numeric", "boom").await;
        }
        let since = chrono::Utc::now().timestamp() - 60;
        let clusters = sink.aggregate_capability_failures(since, 10).await.unwrap();
        assert!(
            clusters.is_empty(),
            "min_count=10 must filter out a 5-row cluster"
        );
    }

    #[tokio::test]
    async fn aggregate_capability_failures_skips_success_rows() {
        let tmp = TempDir::new().unwrap();
        let sink = AuditSink::new(tmp.path().join("audit.db")).await.unwrap();
        // 3 successes — must not show up.
        for _ in 0..3 {
            sink.record_capability_complete(
                "agent_alpha",
                "exec_test",
                "verify.numeric",
                "local",
                "verify_numeric",
                10,
                10,
                None,
            )
            .await;
        }
        // 3 failures — must show up.
        for _ in 0..3 {
            record_failure(&sink, "verify_numeric", "verify.numeric", "boom").await;
        }
        let since = chrono::Utc::now().timestamp() - 60;
        let clusters = sink.aggregate_capability_failures(since, 3).await.unwrap();
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].occurrences, 3);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn audit_db_file_is_0600() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("audit.db");
        let sink = AuditSink::new(db_path.clone()).await.unwrap();
        sink.record(entry("a", "login", AuditKind::Auth)).await;
        let mode = std::fs::metadata(&db_path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "sqlite file must be 0600");
    }
}
