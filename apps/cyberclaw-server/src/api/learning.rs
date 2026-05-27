//! Learning / Org Memory / Evolution Timeline aggregate endpoints.
//!
//! Sprint 11 W2 L1. Replaces the frontend mock TODOs in
//! `web/src/pages_learning.jsx` §OrgMemoryTab and §EvolutionTimelineTab.
//!
//! # Routes
//!
//! | Method | Route | Auth | Purpose |
//! |---|---|---|---|
//! | GET | `/api/v1/learning/org-memory?limit=50` | JWT | List `MemoryScope::Global` entries |
//! | POST | `/api/v1/learning/org-memory` | JWT + admin | Write a Global-scope entry |
//! | GET | `/api/v1/learning/evolution/timeline?days=30` | JWT | Recent evolution cycles |
//!
//! # Data sources
//!
//! - Org memory: Sprint 10 `SemanticMemoryStore` — Sprint 11 does not yet
//!   plumb a concrete SQLite instance into `AppState`, so this endpoint
//!   reads and writes against an in-process `OrgMemoryStore` lock (same
//!   pattern as `admin_store`). When the real store is wired, swap the
//!   body of [`list_org_memory`] / [`post_org_memory`] to call
//!   `store.query_by_scope(&MemoryScope::Global, limit)`.
//! - Evolution timeline: reads the evolution daemon JSONL log at
//!   `$HOME/.cyberclaw/evolution.log` (falls back to `CYBERCLAW_EVOLUTION_LOG`).
//!   Missing log returns an empty list — the frontend already handles
//!   the empty case.

use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    extract::{Extension, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use cyberclaw_control_plane::evolution_orchestrator::{EvolutionConfig, EvolutionOrchestrator};
use cyberclaw_control_plane::fitness_evaluator::FitnessEvaluator;
use cyberclaw_control_plane::llm_evolution_dispatcher::LlmEvolutionDispatcher;
use cyberclaw_control_plane::mutation_engine::MutationEngine;
use cyberclaw_control_plane::skill_archive::{SelectionMethod, SkillArchive, SkillVariant};
use cyberclaw_control_plane::skill_creator::{
    SkillCreator, SkillCreatorConfig, SkillCreatorOutcome, TriggerQuery,
};
use cyberclaw_control_plane::skill_evolution::CaseTrack;
use cyberclaw_control_plane::verification_gate::{
    CriteriaBasedGate, RegressionSpec, StagedVerificationGate,
};
use cyberclaw_core::capability::{CapabilityEffect, CapabilityRef, RiskLevel};
use cyberclaw_core::ids::{CapabilityId, ConnectorId, SkillId, VariantId};
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

use crate::audit::{AuditEntry, AuditKind, AuditQuery, AuditResult};
use crate::error::ApiError;
use crate::middleware::auth::{require_admin, Claims};
use crate::state::AppState;

/// Build the learning router.
pub fn create_learning_router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/v1/learning/org-memory",
            get(list_org_memory).post(post_org_memory),
        )
        .route(
            "/api/v1/learning/evolution/timeline",
            get(evolution_timeline),
        )
        .route("/api/v1/learning/evolution/run", post(run_evolution))
        .route("/api/v1/learning/daily-digest", get(get_daily_digest))
        .route(
            "/api/v1/learning/failure-clusters",
            get(get_failure_clusters),
        )
        .route("/api/v1/learning/skill-telemetry", get(get_skill_telemetry))
}

// ---------------------------------------------------------------------------
// v1.1 — Failure clusters (Reflexion store) + skill telemetry (Voyager)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct FailureClustersQuery {
    /// Look-back window in days (default 7, max 90).
    #[serde(default)]
    pub days: Option<u32>,
    /// Minimum occurrence count to surface a cluster (default 2).
    #[serde(default)]
    pub min_count: Option<u32>,
}

#[derive(Debug, Serialize)]
struct FailureClusterDto {
    tool_name: String,
    capability_id: Option<String>,
    error_signature: String,
    occurrences: u32,
    first_seen: i64,
    last_seen: i64,
}

#[derive(Debug, Serialize)]
struct FailureClustersResponse {
    window_days: u32,
    min_count: u32,
    total: usize,
    clusters: Vec<FailureClusterDto>,
}

/// `GET /api/v1/learning/failure-clusters`
///
/// Reflexion-style failure memory: groups tool failures by (tool, error
/// signature) so the agent (and operator) can spot recurring problems and
/// avoid retrying broken paths.
async fn get_failure_clusters(
    State(state): State<Arc<AppState>>,
    Query(q): Query<FailureClustersQuery>,
) -> Result<Json<FailureClustersResponse>, ApiError> {
    let days = q.days.unwrap_or(7).clamp(1, 90);
    let min_count = q.min_count.unwrap_or(2).max(1);
    let since = Utc::now().timestamp() - (days as i64) * 86_400;

    let clusters = match state.audit.as_ref() {
        Some(sink) => sink
            .aggregate_capability_failures(since, min_count)
            .await
            .map_err(|e| ApiError::InternalError(format!("audit failure aggregate: {e}")))?,
        None => Vec::new(),
    };

    let dtos: Vec<FailureClusterDto> = clusters
        .into_iter()
        .map(|c| FailureClusterDto {
            tool_name: c.tool_name,
            capability_id: c.capability_id,
            error_signature: c.error_signature,
            occurrences: c.occurrences,
            first_seen: c.first_seen,
            last_seen: c.last_seen,
        })
        .collect();

    Ok(Json(FailureClustersResponse {
        window_days: days,
        min_count,
        total: dtos.len(),
        clusters: dtos,
    }))
}

#[derive(Debug, Deserialize)]
pub struct SkillTelemetryQuery {
    /// Look-back window in days (default 30, max 365).
    #[serde(default)]
    pub days: Option<u32>,
}

#[derive(Debug, Serialize)]
struct SkillTelemetryEntry {
    skill_id: String,
    invocations: u32,
    /// Successful invocations (audit kind=Success or no failure marker).
    successes: u32,
    success_rate: f64,
    last_used: Option<i64>,
}

#[derive(Debug, Serialize)]
struct SkillTelemetryResponse {
    window_days: u32,
    total: usize,
    skills: Vec<SkillTelemetryEntry>,
}

/// `GET /api/v1/learning/skill-telemetry`
///
/// Voyager-style skill usage telemetry. Aggregates `skill.invoke` /
/// `skill.run` audit entries to surface invocation counts + success rates
/// per skill. Used by the Skill Hub UI to sort by relevance and by future
/// curriculum logic to deprioritize broken skills.
async fn get_skill_telemetry(
    State(state): State<Arc<AppState>>,
    Query(q): Query<SkillTelemetryQuery>,
) -> Result<Json<SkillTelemetryResponse>, ApiError> {
    use std::collections::HashMap;

    let days = q.days.unwrap_or(30).clamp(1, 365);
    let since = Utc::now().timestamp() - (days as i64) * 86_400;

    let filters = AuditQuery {
        action_prefix: Some("skill.".to_string()),
        ..Default::default()
    };
    let entries = match state.audit.as_ref() {
        Some(sink) => sink.tail(5000, &filters).await.unwrap_or_default(),
        None => Vec::new(),
    };

    let mut agg: HashMap<String, (u32, u32, Option<i64>)> = HashMap::new();
    for e in entries.iter().filter(|e| e.ts.timestamp() >= since) {
        // target shape: "skill:<id>" — see audit emit sites in chat_handler.rs
        // skill_search / skill_create intercepts.
        let skill_id = e
            .target
            .as_deref()
            .and_then(|t| t.strip_prefix("skill:"))
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                e.detail
                    .get("skill_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "unknown".to_string())
            });
        let slot = agg.entry(skill_id).or_insert((0, 0, None));
        slot.0 += 1;
        if matches!(e.result, AuditResult::Success) {
            slot.1 += 1;
        }
        let prev_last = slot.2.unwrap_or(0);
        let ts = e.ts.timestamp();
        if ts > prev_last {
            slot.2 = Some(ts);
        }
    }

    let mut skills: Vec<SkillTelemetryEntry> = agg
        .into_iter()
        .map(|(skill_id, (invocations, successes, last_used))| {
            let success_rate = if invocations > 0 {
                successes as f64 / invocations as f64
            } else {
                0.0
            };
            SkillTelemetryEntry {
                skill_id,
                invocations,
                successes,
                success_rate,
                last_used,
            }
        })
        .collect();
    skills.sort_by_key(|s| std::cmp::Reverse(s.invocations));

    Ok(Json(SkillTelemetryResponse {
        window_days: days,
        total: skills.len(),
        skills,
    }))
}

// ---------------------------------------------------------------------------
// Org memory
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ListOrgMemoryQuery {
    pub limit: Option<usize>,
}

/// One serialized `SemanticMemoryEntry`-shaped row. Field names mirror
/// `SemanticMemoryEntry` in `cyberclaw-store` so the admin SPA can switch
/// to the real store without changing its data layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrgMemoryEntry {
    pub id: String,
    pub scope: serde_json::Value,
    pub kind: String,
    pub content: String,
    #[serde(default)]
    pub rules: Vec<serde_json::Value>,
    #[serde(default)]
    pub provenance: serde_json::Value,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl_secs: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct ListOrgMemoryResponse {
    pub entries: Vec<OrgMemoryEntry>,
}

async fn list_org_memory(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListOrgMemoryQuery>,
) -> Result<Json<ListOrgMemoryResponse>, ApiError> {
    let limit = query.limit.unwrap_or(50).min(500);
    let guard = state.org_memory_store.read().await;
    let entries: Vec<OrgMemoryEntry> = guard.iter().take(limit).cloned().collect();
    info!(count = entries.len(), "learning: listed org memory entries");
    Ok(Json(ListOrgMemoryResponse { entries }))
}

#[derive(Debug, Deserialize)]
pub struct PostOrgMemoryRequest {
    /// Optional client-provided id (for idempotent upsert). Server mints one
    /// when absent.
    #[serde(default)]
    pub id: Option<String>,
    /// `"daily_digest" | "reflection" | "rule" | "free_text"`.
    #[serde(default = "default_kind")]
    pub kind: String,
    pub content: String,
    #[serde(default)]
    pub rules: Vec<serde_json::Value>,
    #[serde(default)]
    pub provenance: Option<serde_json::Value>,
    #[serde(default)]
    pub ttl_secs: Option<i64>,
}

fn default_kind() -> String {
    "free_text".to_string()
}

async fn post_org_memory(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<PostOrgMemoryRequest>,
) -> Result<(StatusCode, Json<OrgMemoryEntry>), ApiError> {
    if let Err(err) = require_admin(&claims).await {
        if let Some(sink) = state.audit.as_ref() {
            sink.record(AuditEntry::now(
                claims.sub.as_str().to_string(),
                AuditKind::Auth,
                "learning.org_memory.create:role_denied".to_string(),
                None,
                serde_json::json!({}),
                AuditResult::Failure {
                    reason: "admin role required".to_string(),
                },
            ))
            .await;
        }
        return Err(err);
    }

    if req.content.trim().is_empty() {
        return Err(ApiError::InvalidInput(
            "content must not be empty".to_string(),
        ));
    }
    if !matches!(
        req.kind.as_str(),
        "daily_digest" | "reflection" | "rule" | "free_text"
    ) {
        return Err(ApiError::InvalidInput(format!(
            "unknown memory kind '{}'. expected one of daily_digest|reflection|rule|free_text",
            req.kind
        )));
    }

    let id = req
        .id
        .unwrap_or_else(|| format!("mem_{}", uuid::Uuid::new_v4().simple()));

    let entry = OrgMemoryEntry {
        id: id.clone(),
        scope: serde_json::json!({ "kind": "global" }),
        kind: req.kind.clone(),
        content: req.content.clone(),
        rules: req.rules,
        provenance: req.provenance.unwrap_or_else(|| {
            serde_json::json!({
                "source_executions": [],
                "source_artifacts": [],
                "reflection_trace_id": null,
            })
        }),
        created_at: Utc::now().to_rfc3339(),
        ttl_secs: req.ttl_secs,
    };

    // Upsert: remove prior entry with same id, push new at the front
    // (newest-first ordering mirrors SemanticMemoryStore::query_by_scope).
    {
        let mut guard = state.org_memory_store.write().await;
        guard.retain(|e| e.id != id);
        guard.insert(0, entry.clone());
    }

    if let Some(sink) = state.audit.as_ref() {
        sink.record(AuditEntry::now(
            claims.sub.as_str().to_string(),
            AuditKind::Mutation,
            "learning.org_memory.create".to_string(),
            Some(format!("memory:{}", entry.id)),
            serde_json::json!({ "kind": entry.kind }),
            AuditResult::Success,
        ))
        .await;
    }

    info!(
        memory_id = %entry.id,
        kind = %entry.kind,
        "learning: org memory entry written"
    );
    Ok((StatusCode::CREATED, Json(entry)))
}

// ---------------------------------------------------------------------------
// Evolution timeline
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct EvolutionTimelineQuery {
    pub days: Option<u32>,
}

/// One cycle summary row. Matches the shape expected by the UI — missing
/// fields render as "—" so we keep the contract lenient.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvolutionCycle {
    pub id: String,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
    pub outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gene_changed: Option<String>,
    #[serde(default)]
    pub meta: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct EvolutionTimelineResponse {
    pub cycles: Vec<EvolutionCycle>,
}

fn evolution_log_path() -> PathBuf {
    if let Ok(p) = std::env::var("CYBERCLAW_EVOLUTION_LOG") {
        return PathBuf::from(p);
    }
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".cyberclaw").join("evolution.log"))
        .unwrap_or_else(|| PathBuf::from(".cyberclaw").join("evolution.log"))
}

async fn evolution_timeline(
    Query(query): Query<EvolutionTimelineQuery>,
) -> Result<Json<EvolutionTimelineResponse>, ApiError> {
    let days = query.days.unwrap_or(30).clamp(1, 365) as i64;
    let cutoff = Utc::now() - ChronoDuration::days(days);
    let path = evolution_log_path();

    let content = match tokio::fs::read_to_string(&path).await {
        Ok(s) => s,
        Err(err) => {
            warn!(
                path = %path.display(),
                %err,
                "evolution log unreadable; returning empty timeline"
            );
            return Ok(Json(EvolutionTimelineResponse { cycles: Vec::new() }));
        }
    };

    let mut cycles: Vec<EvolutionCycle> = Vec::new();
    for (lineno, line) in content.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<EvolutionCycle>(line) {
            Ok(cycle) => {
                // Filter by window — skip rows older than the cutoff.
                if let Ok(ts) = DateTime::parse_from_rfc3339(&cycle.started_at) {
                    if ts.with_timezone(&Utc) < cutoff {
                        continue;
                    }
                }
                cycles.push(cycle);
            }
            Err(err) => {
                warn!(
                    path = %path.display(),
                    lineno = lineno + 1,
                    %err,
                    "skipping malformed evolution log line"
                );
            }
        }
    }
    // Newest first.
    cycles.sort_by(|a, b| b.started_at.cmp(&a.started_at));

    Ok(Json(EvolutionTimelineResponse { cycles }))
}

// ---------------------------------------------------------------------------
// POST /evolution/run — Sprint 21 path #2 closure
// ---------------------------------------------------------------------------

/// Request body for `POST /api/v1/learning/evolution/run`.
///
/// Triggers one description-optimization cycle through the
/// [`EvolutionOrchestrator`] backed by [`LlmEvolutionDispatcher`]. The cycle
/// runs asynchronously — the endpoint returns `202 Accepted` immediately and
/// appends a final `EvolutionCycle` row to `~/.cyberclaw/evolution.log` once
/// the optimization terminates (converged / max-iter / failed).
#[derive(Debug, Deserialize)]
pub struct RunEvolutionRequest {
    /// Skill identifier — used as the audit/target key. Mirrors the
    /// `SkillId` written into the seed [`SkillVariant`].
    pub skill_id: String,
    /// Filesystem path to the skill directory containing `SKILL.md`.
    /// Must be readable by the server process.
    pub target_skill_path: String,
    /// Trigger queries (60/40 train/holdout split per `SkillCreator`).
    pub trigger_dataset: Vec<TriggerQuery>,
    /// Override `SkillCreatorConfig::max_iterations`. Default 5.
    #[serde(default)]
    pub max_iterations: Option<u32>,
    /// Override `SkillCreatorConfig::min_pass_rate`. Default 0.9.
    #[serde(default)]
    pub min_pass_rate: Option<f32>,
    /// LLM model identifier passed to the dispatcher. Falls back to
    /// `CYBERCLAW_EVOLUTION_MODEL` env var, then `gpt-4o-mini`.
    #[serde(default)]
    pub model: Option<String>,
}

/// Response body for `POST /api/v1/learning/evolution/run`.
#[derive(Debug, Serialize)]
pub struct RunEvolutionResponse {
    /// Cycle identifier — caller can poll `GET /timeline` to find the
    /// terminal record by this id.
    pub cycle_id: String,
    /// Always `"running"` on initial response. The terminal status is
    /// written to the evolution log when the spawned task finishes.
    pub status: String,
}

/// Resolve the LLM model identifier from request → env → built-in default.
fn resolve_model(req_model: Option<String>) -> String {
    req_model
        .or_else(|| std::env::var("CYBERCLAW_EVOLUTION_MODEL").ok())
        .unwrap_or_else(|| "gpt-4o-mini".to_string())
}

/// Build a minimal capability ref for the mutation engine. The real
/// dispatcher does not consult this — it only feeds the engine's
/// validation contract — but `MutationEngine::new` requires one.
fn mutation_capability() -> CapabilityRef {
    CapabilityRef {
        id: CapabilityId::from_string("llm.mutate".to_string()).expect("static cap id"),
        connector_id: ConnectorId::from_string("llm".to_string()).expect("static connector id"),
        risk: RiskLevel::Low,
        effects: vec![CapabilityEffect::Read],
        placement: None,
    }
}

/// Construct a `SkillCreator` wired to the LLM dispatcher with a freshly
/// seeded archive. Returns the configured creator + the dispatcher (kept on
/// the stack so its lifetime outlives the optimize() call).
///
/// PRODUCTION caveat: archive seeding here is structural — the variant
/// carries no SKILL.md text or scoring history. The orchestrator treats it
/// as iteration zero and the dispatcher rewrites from scratch on the first
/// step. This matches `claude-skill/skills/skill-creator/scripts/run_loop.py`
/// but is less rigorous than seeding from a prior best variant.
fn build_skill_creator(
    skill_id: SkillId,
    target_skill_path: PathBuf,
    trigger_dataset: Vec<TriggerQuery>,
    max_iterations: Option<u32>,
    min_pass_rate: Option<f32>,
) -> SkillCreator {
    // 1. Seed the archive with one synthetic parent variant.
    let mut archive = SkillArchive::new(skill_id.clone());
    archive.add_variant(SkillVariant {
        variant_id: VariantId::new(),
        parent_variant_id: None,
        skill_id,
        score: 0.0,
        child_count: 0,
        track: CaseTrack::Success,
        patch_artifact_id: None,
    });

    // 2. Build the supporting evolution components.
    let mutation_engine = MutationEngine::new(mutation_capability());
    let evaluator = FitnessEvaluator::new();
    let smoke = RegressionSpec::minimal();
    let criteria = CriteriaBasedGate::default();
    let verification = StagedVerificationGate::new(smoke, criteria);
    let cfg = EvolutionConfig {
        selection_method: SelectionMethod::Best,
        min_score_to_archive: 0.0,
        max_iterations: max_iterations.unwrap_or(5),
        default_track: CaseTrack::Success,
        max_wall_time: None,
        max_cost_usd: None,
    };
    let orchestrator =
        EvolutionOrchestrator::new(archive, mutation_engine, evaluator, verification, cfg);

    // 3. Façade.
    let mut creator_cfg = SkillCreatorConfig::with_defaults(target_skill_path, trigger_dataset);
    if let Some(m) = max_iterations {
        creator_cfg.max_iterations = m;
    }
    if let Some(p) = min_pass_rate {
        creator_cfg.min_pass_rate = p;
    }
    SkillCreator::new(orchestrator, creator_cfg)
}

/// Project a [`SkillCreatorOutcome`] onto the [`EvolutionCycle`] log shape
/// that `GET /timeline` understands.
///
/// This is the **contract** between optimization runs and the timeline UI.
/// Field choices here drive how operators read evolution history:
/// - `outcome` is the human-facing status label
/// - `gene_changed` exposes which variant became the winner
/// - `meta` carries pass_rate / iterations / model for drill-down
fn outcome_to_cycle(
    cycle_id: String,
    started_at: DateTime<Utc>,
    skill_id: &str,
    model: &str,
    outcome: SkillCreatorOutcome,
) -> EvolutionCycle {
    let ended_at = Utc::now().to_rfc3339();
    let (outcome_label, gene_changed, mut meta) = match outcome {
        SkillCreatorOutcome::Converged {
            final_variant_id,
            pass_rate,
            iterations,
        } => (
            "converged".to_string(),
            Some(final_variant_id.as_str().to_string()),
            serde_json::json!({
                "pass_rate": pass_rate,
                "iterations": iterations,
            }),
        ),
        SkillCreatorOutcome::MaxIterationsReached {
            final_variant_id,
            pass_rate,
        } => (
            "max_iterations".to_string(),
            Some(final_variant_id.as_str().to_string()),
            serde_json::json!({
                "pass_rate": pass_rate,
            }),
        ),
        SkillCreatorOutcome::Failed { reason } => (
            "failed".to_string(),
            None,
            serde_json::json!({
                "reason": reason,
            }),
        ),
    };
    // Drill-down context every variant carries.
    if let serde_json::Value::Object(ref mut map) = meta {
        map.insert(
            "skill_id".to_string(),
            serde_json::Value::String(skill_id.to_string()),
        );
        map.insert(
            "model".to_string(),
            serde_json::Value::String(model.to_string()),
        );
    }
    EvolutionCycle {
        id: cycle_id,
        started_at: started_at.to_rfc3339(),
        ended_at: Some(ended_at),
        outcome: outcome_label,
        gene_changed,
        meta,
    }
}

/// Append one [`EvolutionCycle`] row to the evolution log. JSONL format —
/// one record per line. Failures are logged but never propagated, since the
/// optimization itself already completed.
async fn append_cycle_to_log(cycle: &EvolutionCycle) {
    let path = evolution_log_path();
    if let Some(parent) = path.parent() {
        if let Err(e) = tokio::fs::create_dir_all(parent).await {
            warn!(?path, %e, "failed to ensure evolution log parent dir");
            return;
        }
    }
    let line = match serde_json::to_string(cycle) {
        Ok(s) => s,
        Err(e) => {
            error!(%e, "failed to serialize evolution cycle for log append");
            return;
        }
    };
    let payload = format!("{line}\n");
    match tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .await
    {
        Ok(mut file) => {
            use tokio::io::AsyncWriteExt;
            if let Err(e) = file.write_all(payload.as_bytes()).await {
                error!(?path, %e, "failed to append evolution cycle to log");
            }
        }
        Err(e) => {
            error!(?path, %e, "failed to open evolution log for append");
        }
    }
}

/// v1.6 WP-1 #8: accepts raw JSON Value so we can collect ALL missing/invalid
/// fields at once instead of serde returning the first error sequentially.
async fn run_evolution(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Json(raw): Json<serde_json::Value>,
) -> Result<(StatusCode, Json<RunEvolutionResponse>), ApiError> {
    if let Err(err) = require_admin(&claims).await {
        if let Some(sink) = state.audit.as_ref() {
            sink.record(AuditEntry::now(
                claims.sub.as_str().to_string(),
                AuditKind::Auth,
                "learning.evolution.run:role_denied".to_string(),
                None,
                serde_json::json!({}),
                AuditResult::Failure {
                    reason: "admin role required".to_string(),
                },
            ))
            .await;
        }
        return Err(err);
    }

    // v1.6 multi-error validation: collect every problem before returning.
    let mut errors: Vec<String> = Vec::new();

    let skill_id = raw
        .get("skill_id")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string());
    if skill_id.as_deref().is_none_or(str::is_empty) {
        errors.push("skill_id (string, non-empty) is required".to_string());
    }

    let target_skill_path = raw
        .get("target_skill_path")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string());
    if target_skill_path.as_deref().is_none_or(str::is_empty) {
        errors.push("target_skill_path (string, non-empty) is required".to_string());
    }

    // trigger_dataset: each entry needs `query` (string) + `should_trigger` (bool).
    let trigger_dataset_value = raw.get("trigger_dataset").and_then(|v| v.as_array());
    let mut trigger_dataset: Vec<TriggerQuery> = Vec::new();
    match trigger_dataset_value {
        None => errors.push(
            "trigger_dataset (array of {query: string, should_trigger: bool}) is required"
                .to_string(),
        ),
        Some(arr) if arr.is_empty() => {
            errors.push("trigger_dataset must contain at least one query".to_string());
        }
        Some(arr) => {
            for (i, item) in arr.iter().enumerate() {
                let q = item.get("query").and_then(|v| v.as_str());
                let st = item.get("should_trigger").and_then(|v| v.as_bool());
                match (q, st) {
                    (Some(q), Some(st)) => {
                        trigger_dataset.push(TriggerQuery {
                            query: q.to_string(),
                            should_trigger: st,
                        });
                    }
                    _ => {
                        errors.push(format!(
                            "trigger_dataset[{i}] needs both 'query' (string) and 'should_trigger' (bool)"
                        ));
                    }
                }
            }
        }
    }

    if !errors.is_empty() {
        return Err(ApiError::InvalidInput(format!(
            "validation failed ({} error{}): {}",
            errors.len(),
            if errors.len() == 1 { "" } else { "s" },
            errors.join("; ")
        )));
    }

    let max_iterations = raw
        .get("max_iterations")
        .and_then(|v| v.as_u64())
        .map(|n| n as u32);
    let min_pass_rate = raw
        .get("min_pass_rate")
        .and_then(|v| v.as_f64())
        .map(|f| f as f32);
    let model = raw
        .get("model")
        .and_then(|v| v.as_str())
        .map(String::from);

    let req = RunEvolutionRequest {
        skill_id: skill_id.unwrap(),
        target_skill_path: target_skill_path.unwrap(),
        trigger_dataset,
        max_iterations,
        min_pass_rate,
        model,
    };

    let cycle_id = format!("evo_{}", uuid::Uuid::new_v4().simple());
    let started_at = Utc::now();
    let model = resolve_model(req.model);
    let skill_id_str = req.skill_id.clone();

    // Snapshot for the audit row.
    let dataset_size = req.trigger_dataset.len();
    if let Some(sink) = state.audit.as_ref() {
        sink.record(AuditEntry::now(
            claims.sub.as_str().to_string(),
            AuditKind::Mutation,
            "learning.evolution.run.start".to_string(),
            Some(format!("skill:{skill_id_str}")),
            serde_json::json!({
                "cycle_id": cycle_id,
                "model": model,
                "trigger_count": dataset_size,
                "max_iterations": req.max_iterations,
                "min_pass_rate": req.min_pass_rate,
            }),
            AuditResult::Success,
        ))
        .await;
    }

    // Build the dispatcher. The orchestrator + creator are constructed inside
    // the spawned task so the &mut self requirement on optimize() is local.
    let llm_client = state.llm_client.clone();
    let target_path = PathBuf::from(req.target_skill_path);
    let trigger_dataset = req.trigger_dataset;
    let max_iter = req.max_iterations;
    let min_pass = req.min_pass_rate;
    let cycle_id_for_task = cycle_id.clone();
    let skill_id_for_task = skill_id_str.clone();
    let model_for_task = model.clone();

    tokio::spawn(async move {
        let dispatcher = LlmEvolutionDispatcher::new(llm_client, model_for_task.clone());
        let skill_id = match SkillId::from_string(skill_id_for_task.clone()) {
            Ok(id) => id,
            Err(e) => {
                error!(skill_id = %skill_id_for_task, %e, "invalid skill_id, aborting cycle");
                let cycle = outcome_to_cycle(
                    cycle_id_for_task,
                    started_at,
                    &skill_id_for_task,
                    &model_for_task,
                    SkillCreatorOutcome::Failed {
                        reason: format!("invalid skill_id: {e}"),
                    },
                );
                append_cycle_to_log(&cycle).await;
                return;
            }
        };
        let mut creator =
            build_skill_creator(skill_id, target_path, trigger_dataset, max_iter, min_pass);
        let outcome = creator.optimize(&dispatcher).await;
        info!(cycle_id = %cycle_id_for_task, ?outcome, "evolution cycle terminated");
        let cycle = outcome_to_cycle(
            cycle_id_for_task,
            started_at,
            &skill_id_for_task,
            &model_for_task,
            outcome,
        );
        append_cycle_to_log(&cycle).await;
    });

    info!(
        cycle_id = %cycle_id,
        skill_id = %skill_id_str,
        trigger_count = dataset_size,
        "learning: evolution cycle accepted"
    );
    Ok((
        StatusCode::ACCEPTED,
        Json(RunEvolutionResponse {
            cycle_id,
            status: "running".to_string(),
        }),
    ))
}

// ---------------------------------------------------------------------------
// Daily digest
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct DailyDigestQuery {
    pub date: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DigestKpi {
    pub value: i64,
    pub delta: String,
    pub sub: String,
}

#[derive(Debug, Serialize)]
pub struct DigestStats {
    pub executions: DigestKpi,
    pub approvals: DigestKpi,
    pub learning_entries: DigestKpi,
    pub incidents: DigestKpi,
}

#[derive(Debug, Serialize)]
pub struct DigestHighlight {
    pub icon: String,
    pub label: String,
    pub tone: String,
}

#[derive(Debug, Serialize)]
pub struct DigestFeedEntry {
    pub ts: String,
    pub avatar: String,
    pub name: String,
    pub body: String,
    pub tags: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct DigestFeedBucket {
    pub bucket: String,
    pub entries: Vec<DigestFeedEntry>,
}

#[derive(Debug, Serialize)]
pub struct DailyDigestResponse {
    pub date: String,
    pub stats: DigestStats,
    pub highlights: Vec<DigestHighlight>,
    pub recommendations: Vec<serde_json::Value>,
    pub feed: Vec<DigestFeedBucket>,
}

fn digest_kind_avatar(kind: &str) -> &'static str {
    match kind {
        "daily_digest" => "📋",
        "reflection" => "◆",
        "rule" => "⚖",
        _ => "◦",
    }
}

fn digest_kind_name(kind: &str) -> &'static str {
    match kind {
        "daily_digest" => "Daily Digest",
        "reflection" => "Reflection",
        "rule" => "Rule Engine",
        _ => "Operator",
    }
}

/// `GET /api/v1/learning/daily-digest?date=YYYY-MM-DD`
///
/// Returns org-memory entries for the given UTC date grouped into the digest
/// shape expected by the admin UI's `DailyDigestTab`. `date` defaults to
/// today UTC. KPI stats (executions/approvals/incidents) are derived from
/// `audit.tail()` filtered to the requested date; degrades to zeros when no
/// audit sink is configured.
async fn get_daily_digest(
    State(state): State<Arc<AppState>>,
    Query(query): Query<DailyDigestQuery>,
) -> Result<Json<DailyDigestResponse>, ApiError> {
    let date = query
        .date
        .unwrap_or_else(|| Utc::now().format("%Y-%m-%d").to_string());

    let guard = state.org_memory_store.read().await;
    let day_entries: Vec<OrgMemoryEntry> = guard
        .iter()
        .filter(|e| e.created_at.starts_with(&date))
        .cloned()
        .collect();
    drop(guard);

    let learning_count = day_entries.len() as i64;
    let rule_count = day_entries.iter().filter(|e| e.kind == "rule").count() as i64;

    let date_start = chrono::NaiveDate::parse_from_str(&date, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|dt| dt.and_utc());

    let (exec_count, approval_count, incident_count) =
        if let (Some(sink), Some(since)) = (state.audit.as_ref(), date_start) {
            let filters = AuditQuery {
                actor: None,
                kind: None,
                action_prefix: None,
                since: Some(since),
            };
            let entries = sink.tail(1000, &filters).await.unwrap_or_default();
            let execs = entries
                .iter()
                .filter(|e| e.action.starts_with("task.") || e.action.starts_with("execution."))
                .count() as i64;
            let approvals = entries
                .iter()
                .filter(|e| e.action.starts_with("review."))
                .count() as i64;
            let incidents = entries
                .iter()
                .filter(|e| matches!(e.kind, AuditKind::Security))
                .count() as i64;
            (execs, approvals, incidents)
        } else {
            (0, 0, 0)
        };

    let stats = DigestStats {
        executions: DigestKpi {
            value: exec_count,
            delta: String::new(),
            sub: String::new(),
        },
        approvals: DigestKpi {
            value: approval_count,
            delta: String::new(),
            sub: String::new(),
        },
        learning_entries: DigestKpi {
            value: learning_count,
            delta: format!("{rule_count} rule candidates"),
            sub: String::new(),
        },
        incidents: DigestKpi {
            value: incident_count,
            delta: String::new(),
            sub: String::new(),
        },
    };

    let highlights: Vec<DigestHighlight> = day_entries
        .iter()
        .filter(|e| e.kind == "rule")
        .take(3)
        .map(|e| DigestHighlight {
            icon: "✦".to_string(),
            label: e.content.chars().take(80).collect(),
            tone: "violet".to_string(),
        })
        .collect();

    let mut morning: Vec<DigestFeedEntry> = Vec::new();
    let mut afternoon: Vec<DigestFeedEntry> = Vec::new();
    let mut evening: Vec<DigestFeedEntry> = Vec::new();

    for e in &day_entries {
        let hour: u32 = e
            .created_at
            .get(11..13)
            .and_then(|h| h.parse().ok())
            .unwrap_or(12);
        let ts = e.created_at.get(11..16).unwrap_or("--:--").to_string();
        let tags = if e.kind == "rule" {
            vec!["learned".to_string()]
        } else {
            vec![]
        };
        let feed_entry = DigestFeedEntry {
            ts,
            avatar: digest_kind_avatar(&e.kind).to_string(),
            name: digest_kind_name(&e.kind).to_string(),
            body: e.content.clone(),
            tags,
        };
        match hour {
            0..=11 => morning.push(feed_entry),
            12..=17 => afternoon.push(feed_entry),
            _ => evening.push(feed_entry),
        }
    }

    let mut feed = Vec::new();
    if !morning.is_empty() {
        feed.push(DigestFeedBucket {
            bucket: "🌅 Morning".to_string(),
            entries: morning,
        });
    }
    if !afternoon.is_empty() {
        feed.push(DigestFeedBucket {
            bucket: "☀ Afternoon".to_string(),
            entries: afternoon,
        });
    }
    if !evening.is_empty() {
        feed.push(DigestFeedBucket {
            bucket: "🌙 Evening".to_string(),
            entries: evening,
        });
    }

    info!(date = %date, learning_count, "learning: daily digest fetched");

    Ok(Json(DailyDigestResponse {
        date,
        stats,
        highlights,
        recommendations: Vec::new(),
        feed,
    }))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #[allow(unused_imports)]
    use super::*;
    use crate::api::test_helpers::{build_test_state, seed_users_with_role};
    use cyberclaw_core::ids::UserId;
    use serial_test::serial;

    fn claims_for(user_id: &str) -> Claims {
        let uid = UserId::from_string(user_id.to_string()).expect("valid user id");
        let now = Utc::now().timestamp();
        Claims {
            sub: uid,
            tenant: None,
            iat: now,
            exp: now + 3600,
        }
    }

    #[tokio::test]
    async fn list_org_memory_empty_initially() {
        let state = build_test_state();
        let resp = list_org_memory(State(state), Query(ListOrgMemoryQuery { limit: None }))
            .await
            .expect("list ok");
        assert_eq!(resp.0.entries.len(), 0);
    }

    #[tokio::test]
    #[serial]
    async fn post_org_memory_requires_admin() {
        let (_tmp, _restore) = seed_users_with_role("op-viewer-mem", "viewer");
        let state = build_test_state();
        let claims = claims_for("op-viewer-mem");
        let err = post_org_memory(
            State(state),
            Extension(claims),
            Json(PostOrgMemoryRequest {
                id: None,
                kind: "free_text".to_string(),
                content: "hello".to_string(),
                rules: Vec::new(),
                provenance: None,
                ttl_secs: None,
            }),
        )
        .await
        .expect_err("viewer must be denied");
        assert!(matches!(err, ApiError::Forbidden(_)));
    }

    #[tokio::test]
    #[serial]
    async fn post_org_memory_admin_round_trip() {
        let (_tmp, _restore) = seed_users_with_role("op-admin-mem", "admin");
        let state = build_test_state();
        let claims = claims_for("op-admin-mem");
        let (code, Json(entry)) = post_org_memory(
            State(state.clone()),
            Extension(claims),
            Json(PostOrgMemoryRequest {
                id: Some("fact-001".to_string()),
                kind: "free_text".to_string(),
                content: "Treasury transfers under $50 auto-approve".to_string(),
                rules: Vec::new(),
                provenance: None,
                ttl_secs: None,
            }),
        )
        .await
        .expect("admin must pass");
        assert_eq!(code, StatusCode::CREATED);
        assert_eq!(entry.id, "fact-001");
        assert_eq!(entry.kind, "free_text");

        let resp = list_org_memory(State(state), Query(ListOrgMemoryQuery { limit: Some(10) }))
            .await
            .expect("list ok");
        assert_eq!(resp.0.entries.len(), 1);
        assert_eq!(resp.0.entries[0].id, "fact-001");
    }

    #[tokio::test]
    #[serial]
    async fn post_org_memory_rejects_empty_content() {
        let (_tmp, _restore) = seed_users_with_role("op-admin-empty", "admin");
        let state = build_test_state();
        let claims = claims_for("op-admin-empty");
        let err = post_org_memory(
            State(state),
            Extension(claims),
            Json(PostOrgMemoryRequest {
                id: None,
                kind: "free_text".to_string(),
                content: "   ".to_string(),
                rules: Vec::new(),
                provenance: None,
                ttl_secs: None,
            }),
        )
        .await
        .expect_err("empty content must 400");
        assert!(matches!(err, ApiError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn evolution_timeline_missing_log_returns_empty() {
        // Point at a nonexistent path so the handler hits the "log unreadable"
        // branch rather than reading the real `~/.cyberclaw/evolution.log`.
        let tmp = tempfile::tempdir().expect("tempdir");
        let fake_log = tmp.path().join("nonexistent.log");
        let prev = std::env::var("CYBERCLAW_EVOLUTION_LOG").ok();
        // SAFETY: test-local mutation; `#[serial]` is not strictly needed
        // because we restore before awaiting any other env user.
        unsafe {
            std::env::set_var("CYBERCLAW_EVOLUTION_LOG", fake_log.as_os_str());
        }
        let resp = evolution_timeline(Query(EvolutionTimelineQuery { days: Some(7) }))
            .await
            .expect("timeline ok");
        unsafe {
            match prev {
                Some(v) => std::env::set_var("CYBERCLAW_EVOLUTION_LOG", v),
                None => std::env::remove_var("CYBERCLAW_EVOLUTION_LOG"),
            }
        }
        assert_eq!(resp.0.cycles.len(), 0);
    }

    // ---------------------------------------------------------------------
    // Evolution /run endpoint — Sprint 21 path #2 closure tests
    // ---------------------------------------------------------------------

    fn sample_dataset() -> Vec<TriggerQuery> {
        vec![
            TriggerQuery {
                query: "draft a python script".to_string(),
                should_trigger: true,
            },
            TriggerQuery {
                query: "what time is it".to_string(),
                should_trigger: false,
            },
        ]
    }

    #[test]
    fn outcome_to_cycle_converged_emits_pass_rate_and_iterations() {
        let v = VariantId::new();
        let started = Utc::now();
        let cycle = outcome_to_cycle(
            "evo_test".to_string(),
            started,
            "sk_demo",
            "test-model",
            SkillCreatorOutcome::Converged {
                final_variant_id: v.clone(),
                pass_rate: 0.92,
                iterations: 3,
            },
        );
        assert_eq!(cycle.id, "evo_test");
        assert_eq!(cycle.outcome, "converged");
        assert_eq!(cycle.gene_changed.as_deref(), Some(v.as_str()));
        let pr = cycle.meta["pass_rate"].as_f64().unwrap() as f32;
        assert!((pr - 0.92).abs() < 1e-5, "pass_rate ~ 0.92, got {pr}");
        assert_eq!(cycle.meta["iterations"].as_u64().unwrap(), 3);
        assert_eq!(cycle.meta["skill_id"], "sk_demo");
        assert_eq!(cycle.meta["model"], "test-model");
        assert!(cycle.ended_at.is_some());
    }

    #[test]
    fn outcome_to_cycle_max_iterations_omits_iterations_keeps_pass_rate() {
        let v = VariantId::new();
        let started = Utc::now();
        let cycle = outcome_to_cycle(
            "evo_test".to_string(),
            started,
            "sk_demo",
            "test-model",
            SkillCreatorOutcome::MaxIterationsReached {
                final_variant_id: v.clone(),
                pass_rate: 0.4,
            },
        );
        assert_eq!(cycle.outcome, "max_iterations");
        assert_eq!(cycle.gene_changed.as_deref(), Some(v.as_str()));
        let pr = cycle.meta["pass_rate"].as_f64().unwrap() as f32;
        assert!((pr - 0.4).abs() < 1e-5, "pass_rate ~ 0.4, got {pr}");
        assert!(cycle.meta.get("iterations").is_none());
    }

    #[test]
    fn outcome_to_cycle_failed_carries_reason_no_variant() {
        let started = Utc::now();
        let cycle = outcome_to_cycle(
            "evo_test".to_string(),
            started,
            "sk_demo",
            "test-model",
            SkillCreatorOutcome::Failed {
                reason: "LLM unavailable".to_string(),
            },
        );
        assert_eq!(cycle.outcome, "failed");
        assert!(cycle.gene_changed.is_none());
        assert_eq!(cycle.meta["reason"], "LLM unavailable");
        assert_eq!(cycle.meta["skill_id"], "sk_demo");
    }

    #[tokio::test]
    #[serial]
    async fn run_evolution_requires_admin() {
        let (_tmp, _restore) = seed_users_with_role("op-viewer-evo", "viewer");
        let state = build_test_state();
        let claims = claims_for("op-viewer-evo");
        let dataset_json: Vec<serde_json::Value> = sample_dataset()
            .iter()
            .map(|q| serde_json::json!({ "query": q.query, "should_trigger": q.should_trigger }))
            .collect();
        let err = run_evolution(
            State(state),
            Extension(claims),
            Json(serde_json::json!({
                "skill_id": "sk_demo",
                "target_skill_path": "/tmp/skill",
                "trigger_dataset": dataset_json,
            })),
        )
        .await
        .expect_err("viewer must be denied");
        assert!(matches!(err, ApiError::Forbidden(_)));
    }

    #[tokio::test]
    #[serial]
    async fn run_evolution_rejects_empty_dataset() {
        let (_tmp, _restore) = seed_users_with_role("op-admin-evo-empty", "admin");
        let state = build_test_state();
        let claims = claims_for("op-admin-evo-empty");
        let err = run_evolution(
            State(state),
            Extension(claims),
            Json(serde_json::json!({
                "skill_id": "sk_demo",
                "target_skill_path": "/tmp/skill",
                "trigger_dataset": Vec::<serde_json::Value>::new(),
            })),
        )
        .await
        .expect_err("empty dataset must 400");
        assert!(matches!(err, ApiError::InvalidInput(_)));
    }

    #[tokio::test]
    #[serial]
    async fn run_evolution_accepts_and_writes_log_on_termination() {
        let (_tmp_users, _restore) = seed_users_with_role("op-admin-evo-run", "admin");

        // Redirect the evolution log to a temp file so we can assert on it.
        let tmp = tempfile::tempdir().expect("tempdir");
        let fake_log = tmp.path().join("evolution.log");
        let prev = std::env::var("CYBERCLAW_EVOLUTION_LOG").ok();
        unsafe {
            std::env::set_var("CYBERCLAW_EVOLUTION_LOG", fake_log.as_os_str());
        }

        // Point at a non-existent SKILL.md path so SkillCreator::optimize
        // returns Failed quickly (no LLM call needed; deterministic).
        let missing_skill_dir = tmp.path().join("no-such-skill");

        let state = build_test_state();
        let claims = claims_for("op-admin-evo-run");
        let dataset_json: Vec<serde_json::Value> = sample_dataset()
            .iter()
            .map(|q| serde_json::json!({ "query": q.query, "should_trigger": q.should_trigger }))
            .collect();
        let (code, Json(resp)) = run_evolution(
            State(state),
            Extension(claims),
            Json(serde_json::json!({
                "skill_id": "sk_demo",
                "target_skill_path": missing_skill_dir.to_string_lossy().into_owned(),
                "trigger_dataset": dataset_json,
                "max_iterations": 1,
                "model": "test-model",
            })),
        )
        .await
        .expect("admin happy path returns 202");

        assert_eq!(code, StatusCode::ACCEPTED);
        assert_eq!(resp.status, "running");
        assert!(resp.cycle_id.starts_with("evo_"));
        let returned_cycle = resp.cycle_id.clone();

        // Wait for spawned task (missing SKILL.md → fails immediately).
        for _ in 0..50 {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            if let Ok(content) = tokio::fs::read_to_string(&fake_log).await {
                if content.contains(&returned_cycle) {
                    break;
                }
            }
        }
        let content = tokio::fs::read_to_string(&fake_log)
            .await
            .expect("log file written");
        assert!(
            content.contains(&returned_cycle),
            "expected cycle_id {returned_cycle} in log; got: {content}"
        );

        // Each line is a JSON-serialized EvolutionCycle.
        let line = content
            .lines()
            .find(|l| l.contains(&returned_cycle))
            .expect("cycle line present");
        let cycle: EvolutionCycle =
            serde_json::from_str(line).expect("cycle line is valid EvolutionCycle JSON");
        assert_eq!(cycle.outcome, "failed", "expected failure, got {cycle:?}");
        assert!(cycle.meta["reason"].is_string());
        assert_eq!(cycle.meta["skill_id"], "sk_demo");
        assert_eq!(cycle.meta["model"], "test-model");

        unsafe {
            match prev {
                Some(v) => std::env::set_var("CYBERCLAW_EVOLUTION_LOG", v),
                None => std::env::remove_var("CYBERCLAW_EVOLUTION_LOG"),
            }
        }
    }
}
