//! Govern Memory Console HTTP surface (Sprint 17 Lane B / S18 R3).
//!
//! S18 R3: migrated from server-local `InMemoryMemoryStore` to the
//! `cyberclaw_store::LeveledMemoryStore` trait.  All handler logic is
//! preserved; field names on the wire are unchanged so the frontend
//! (pages_memory_console.jsx) does not need updating.
//!
//! # Routes
//!
//! | Method | Path | Purpose |
//! |--------|------|---------|
//! | GET    | `/api/v1/memory` | List with optional filters (level/agent_id/keyword/limit) |
//! | GET    | `/api/v1/memory/:id` | Full record + metadata |
//! | POST   | `/api/v1/memory/:id/edit` | Admin edit content, emits Mutation audit entry |
//! | GET    | `/api/v1/memory/:id/trace` | Provenance (v1: stub) |
//! | DELETE | `/api/v1/memory/:id` | Not supported in v1 (returns 400) |

use std::sync::Arc;

use axum::{
    extract::{Extension, Path, Query, State},
    routing::{delete, get, post},
    Json, Router,
};
use chrono::Utc;
use cyberclaw_governance::tool_output_sanitizer::WarningCategory;
use cyberclaw_store::{LeveledMemoryRecord, LeveledMemoryStore, MemoryLevel};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tracing::info;

use crate::audit::{AuditEntry, AuditKind, AuditResult};
use crate::error::ApiError;
use crate::middleware::auth::{require_admin, Claims};
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Level mapping helpers (S17 wire strings ↔ store enum)
// ---------------------------------------------------------------------------

/// Parse S17 wire level string ("L0", "L1", "L2") into store `MemoryLevel`.
fn parse_level(s: &str) -> Option<MemoryLevel> {
    match s {
        "L0" => Some(MemoryLevel::L0Full),
        "L1" => Some(MemoryLevel::L1Summary),
        "L2" => Some(MemoryLevel::L2Metadata),
        _ => None,
    }
}

/// Render store `MemoryLevel` as S17 wire string ("L0", "L1", "L2").
fn level_to_str(level: MemoryLevel) -> &'static str {
    match level {
        MemoryLevel::L0Full => "L0",
        MemoryLevel::L1Summary => "L1",
        MemoryLevel::L2Metadata => "L2",
    }
}

// ---------------------------------------------------------------------------
// DTO: frontend-compatible summary (matches S17 MemorySummary shape)
// ---------------------------------------------------------------------------

/// Lightweight projection returned by `GET /api/v1/memory`.
/// Shape matches S17 `MemorySummary` so pages_memory_console.jsx needs no changes.
#[derive(Debug, Serialize)]
pub struct MemorySummary {
    pub id: String,
    pub agent_id: String,
    pub level: String,
    pub created_at: chrono::DateTime<Utc>,
    pub updated_at: chrono::DateTime<Utc>,
    pub size_bytes: usize,
    pub preview: String,
}

/// Full record returned by `GET /api/v1/memory/:id`.
/// Shape matches S17 `MemoryRecord` so pages_memory_console.jsx needs no changes.
///
/// S20 E2: added `metadata` field carrying sanitization flags when credential
/// redaction occurred during create/edit. `None` for records that required no
/// redaction.
#[derive(Debug, Serialize)]
pub struct MemoryRecordDto {
    pub id: String,
    pub agent_id: String,
    pub level: String,
    pub content: String,
    pub created_at: chrono::DateTime<Utc>,
    pub updated_at: chrono::DateTime<Utc>,
    pub size_bytes: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Map<String, Value>>,
}

fn to_content_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn record_to_full(r: &LeveledMemoryRecord) -> MemoryRecordDto {
    let content = to_content_string(&r.content);
    let size_bytes = content.len();
    MemoryRecordDto {
        id: r.id.clone(),
        agent_id: r.agent_id.clone(),
        level: level_to_str(r.level).to_string(),
        content,
        size_bytes,
        created_at: r.created_at,
        updated_at: r.updated_at,
        metadata: None,
    }
}

fn record_to_summary(r: &LeveledMemoryRecord) -> MemorySummary {
    let content = to_content_string(&r.content);
    let size_bytes = content.len();
    let preview: String = content.chars().take(80).collect();
    MemorySummary {
        id: r.id.clone(),
        agent_id: r.agent_id.clone(),
        level: level_to_str(r.level).to_string(),
        created_at: r.created_at,
        updated_at: r.updated_at,
        size_bytes,
        preview,
    }
}

// ---------------------------------------------------------------------------
// seed_demo helper — called from tests and optionally from server startup
// ---------------------------------------------------------------------------

/// Seed 18 demo records (3 levels × 3 snippets × 2 agents) into the store.
///
/// Preserves the S17 seed_demo() UX so the admin console shows meaningful
/// data immediately after boot.
pub async fn seed_memory_demo(store: &dyn LeveledMemoryStore) {
    let agents = ["agent-ada", "agent-turing"];
    let levels = [
        (
            MemoryLevel::L0Full,
            vec![
                "Current task: analysing security audit log entries for anomalies.",
                "Loaded 512 tokens of context from execution-001.",
                "Pending tool call: read_file('/etc/hosts').",
            ],
        ),
        (
            MemoryLevel::L1Summary,
            vec![
                "Session 2026-04-24: successfully refactored auth module, 3 tests added.",
                "User prefers concise summaries; avoid markdown headers in plain-text replies.",
                "Previous run failed at step 4 due to missing CYBERCLAW_LLM_API_KEY.",
            ],
        ),
        (
            MemoryLevel::L2Metadata,
            vec![
                "Rule: always run `cargo clippy -- -D warnings` before committing.",
                "Preferred LLM: claude-sonnet-4-6 for code tasks, haiku for lookup.",
                "Organisation policy: no PII in audit detail fields — store byte lengths only.",
            ],
        ),
    ];

    let base_ts = Utc::now();
    for (level, snippets) in &levels {
        for (i, snippet) in snippets.iter().enumerate() {
            for agent in &agents {
                let id = uuid::Uuid::new_v4().to_string();
                let created_at = base_ts
                    - chrono::Duration::hours(((i as i64) * 3 + 1) * (agent.len() as i64 % 5 + 1));
                let record = LeveledMemoryRecord {
                    id: id.clone(),
                    // Use the global demo session so list/get/edit handlers
                    // can query by GLOBAL_DEMO_SESSION. agent_id retains the
                    // actual agent name for frontend filtering.
                    session_id: GLOBAL_DEMO_SESSION.to_string(),
                    agent_id: agent.to_string(),
                    level: *level,
                    key: id.clone(),
                    content: json!(format!("[{}] {}", agent, snippet)),
                    created_at,
                    updated_at: created_at,
                    ttl_seconds: level.default_ttl_seconds(),
                    // Seed data has no source execution; trace returns written_by: null
                    source_execution_id: None,
                    embedding: None,
                    tags: Vec::new(),
                };
                let _ = store.store_leveled(record).await;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

/// `GET /api/v1/memory` query params.
#[derive(Debug, Deserialize, Default)]
pub struct ListMemoryQuery {
    pub level: Option<String>,
    pub agent_id: Option<String>,
    /// Substring keyword filter. 2026-05-18 (v1.2.14): accepts both
    /// `keyword=` (canonical) AND `q=` (alias) — the CLI was passing `q=`
    /// while server only matched `keyword=`, leading to unfiltered "search"
    /// results that returned ALL records instead of the matching ones.
    #[serde(alias = "q")]
    pub keyword: Option<String>,
    pub limit: Option<usize>,
}

/// `POST /api/v1/memory/:id/edit` body.
#[derive(Debug, Deserialize)]
pub struct EditMemoryBody {
    pub content: String,
}

/// `POST /api/v1/memory` body (S19 F: admin create).
#[derive(Debug, Deserialize)]
pub struct CreateMemoryRequest {
    pub agent_id: String,
    /// Wire level string: "L0", "L1", or "L2".
    pub level: String,
    pub content: String,
    /// Optional tags for filtered retrieval (Hermes BT-09).
    #[serde(default)]
    pub tags: Option<Vec<String>>,
}

/// `GET /api/v1/memory/:id/trace` response (S18 R4).
#[derive(Debug, Serialize)]
pub struct TraceResponse {
    /// 写入该记录的执行信息；seed data 或旧记录返回 null。
    pub written_by: Option<WrittenByDto>,
    /// 读取该记录的执行列表。
    pub read_by: Vec<ReadByDto>,
}

/// 写入溯源信息
#[derive(Debug, Serialize)]
pub struct WrittenByDto {
    pub execution_id: String,
    pub written_at: chrono::DateTime<Utc>,
}

/// 单次读取记录
#[derive(Debug, Serialize)]
pub struct ReadByDto {
    pub execution_id: String,
    pub read_at: chrono::DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the `/api/v1/memory/*` router.
pub fn create_memory_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/memory", get(list_memory))
        .route("/api/v1/memory", post(create_memory))
        .route("/api/v1/memory/:id", get(get_memory))
        .route("/api/v1/memory/:id/edit", post(edit_memory))
        .route("/api/v1/memory/:id/promote", post(promote_memory))
        .route("/api/v1/memory/:id/trace", get(trace_memory))
        .route("/api/v1/memory/:id", delete(delete_memory))
        .route("/api/v1/memory/search", get(search_memory))
}

#[derive(Debug, Deserialize)]
struct PromoteMemoryBody {
    /// Target level: "L0", "L1", or "L2".
    to: String,
}

/// `POST /api/v1/memory/:id/promote` — change a memory's level. Admin only.
/// Exposes the trait-level `LeveledMemoryStore::promote` to HTTP so operators
/// (and LLM-driven curation agents) can move L0→L1 / L1→L2 explicitly without
/// going through edit (which preserves the level).
async fn promote_memory(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<String>,
    Json(body): Json<PromoteMemoryBody>,
) -> Result<Json<MemoryRecordDto>, ApiError> {
    require_admin(&claims).await?;

    let new_level = parse_level(&body.to)
        .ok_or_else(|| ApiError::InvalidRequest(format!("unknown level: {}", body.to)))?;

    // Confirm the record exists first so we 404 cleanly instead of bubbling
    // a store error.
    let existing = find_record_by_id(state.memory_store.as_ref(), &id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("memory record not found: {id}")))?;

    let from_level = existing.level;
    if from_level == new_level {
        // No-op; return current shape.
        return Ok(Json(record_to_full(&existing)));
    }

    state
        .memory_store
        .promote(&id, new_level)
        .await
        .map_err(|e| {
            tracing::error!("memory promote failed: {e}");
            ApiError::InternalError("failed to promote memory record".to_string())
        })?;

    if let Some(audit) = state.audit.as_ref() {
        audit
            .record(AuditEntry::now(
                claims.sub.to_string(),
                AuditKind::Mutation,
                "memory.promote".to_string(),
                Some(format!("memory:{id}")),
                json!({
                    "memory_id": id,
                    "from": level_to_str(from_level),
                    "to": body.to,
                }),
                AuditResult::Success,
            ))
            .await;
    }

    // Re-fetch to get the updated TTL etc.
    let updated = find_record_by_id(state.memory_store.as_ref(), &id)
        .await
        .ok_or_else(|| {
            ApiError::InternalError("record disappeared after promote (race?)".to_string())
        })?;
    Ok(Json(record_to_full(&updated)))
}

// ---------------------------------------------------------------------------
// Internal: find a record by id across all levels for a given session_id.
//
// The store has no "get by id" — iterate all three levels and find by id.
// session_id is stored equal to agent_id in the seed (v1 simplification).
// ---------------------------------------------------------------------------

/// Find a record by id across all levels under `GLOBAL_DEMO_SESSION`.
///
/// The `LeveledMemoryStore` trait has no get-by-id method, so we query each
/// level under the global session id and scan for the matching record. This
/// is correct for v1 because all demo records (and R1/R2 agent records) are
/// stored with `session_id = GLOBAL_DEMO_SESSION`.
async fn find_record_by_id(
    store: &dyn LeveledMemoryStore,
    id: &str,
) -> Option<LeveledMemoryRecord> {
    for level in [
        MemoryLevel::L0Full,
        MemoryLevel::L1Summary,
        MemoryLevel::L2Metadata,
    ] {
        if let Ok(records) = store.query_by_level(GLOBAL_DEMO_SESSION, level).await {
            if let Some(r) = records.into_iter().find(|r| r.id == id) {
                return Some(r);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /api/v1/memory` — list memory records (admin only).
async fn list_memory(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Query(q): Query<ListMemoryQuery>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&claims).await?;

    let filter_level = q
        .level
        .as_deref()
        .map(|s| {
            parse_level(s).ok_or_else(|| ApiError::InvalidRequest(format!("unknown level: {s}")))
        })
        .transpose()?;

    let filter_agent = q.agent_id.filter(|s| !s.is_empty());
    let filter_keyword = q
        .keyword
        .filter(|s| !s.is_empty())
        .map(|s| s.to_lowercase());
    let limit = q.limit.unwrap_or(100).min(1000);

    // Query levels: if a specific level is requested, query only that; otherwise all three.
    let levels_to_query: Vec<MemoryLevel> = match filter_level {
        Some(lv) => vec![lv],
        None => vec![
            MemoryLevel::L0Full,
            MemoryLevel::L1Summary,
            MemoryLevel::L2Metadata,
        ],
    };

    let mut all_records: Vec<LeveledMemoryRecord> = Vec::new();
    for level in levels_to_query {
        match state
            .memory_store
            .query_by_level(GLOBAL_DEMO_SESSION, level)
            .await
        {
            Ok(recs) => all_records.extend(recs),
            Err(e) => {
                tracing::warn!("memory list query_by_level error: {e}");
            }
        }
    }

    // Local filters: agent_id and keyword
    let filtered: Vec<MemorySummary> = all_records
        .iter()
        .filter(|r| filter_agent.as_deref().is_none_or(|a| r.agent_id == a))
        .filter(|r| {
            filter_keyword
                .as_deref()
                .is_none_or(|kw| to_content_string(&r.content).to_lowercase().contains(kw))
        })
        .map(record_to_summary)
        .collect();

    // Stable order: newest first, then truncate.
    let mut sorted = filtered;
    sorted.sort_by_key(|r| std::cmp::Reverse(r.created_at));
    sorted.truncate(limit);

    Ok(Json(json!({ "records": sorted })))
}

/// Search mode for `GET /api/v1/memory/search`.
#[derive(Debug, Deserialize, Default, Clone)]
#[serde(rename_all = "snake_case")]
enum SearchMode {
    #[default]
    Keyword,
    Semantic,
    /// BM25 top-100 candidates → cosine rerank → top {limit} (S26 T1).
    Hybrid,
}

/// Query parameters for `GET /api/v1/memory/search`.
#[derive(Debug, Deserialize)]
struct SearchMemoryQuery {
    /// The search query string (required, non-empty after trim).
    q: String,
    /// Optional level filter (L0 / L1 / L2).
    level: Option<String>,
    /// Optional agent_id filter.
    agent_id: Option<String>,
    /// Max results (default 20, capped at 100).
    limit: Option<usize>,
    /// Search mode: "keyword" (default, BM25) or "semantic" (cosine over embeddings).
    #[serde(default)]
    mode: SearchMode,
}

/// `GET /api/v1/memory/search?q=...&level=L1&limit=20[&mode=semantic|hybrid]` — ranked
/// search across memory records (admin only).
///
/// - `mode=keyword` (default): BM25 keyword ranking (S20, unchanged).
/// - `mode=semantic`: cosine similarity over embedding vectors (S25 T4).
/// - `mode=hybrid`: BM25 top-100 → cosine rerank → top {limit} (S26 T1).
async fn search_memory(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Query(params): Query<SearchMemoryQuery>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&claims).await?;

    if params.q.trim().is_empty() {
        return Err(ApiError::InvalidRequest(
            "search query `q` cannot be empty".to_string(),
        ));
    }

    match params.mode {
        SearchMode::Keyword => keyword_search(state, params).await,
        SearchMode::Semantic => semantic_search(state, params).await,
        SearchMode::Hybrid => hybrid_search(state, params).await,
    }
}

/// Retrieve all candidates matching optional level/agent_id filters.
async fn list_filtered_candidates(
    state: &Arc<AppState>,
    level: Option<&str>,
    agent_id: Option<&str>,
) -> Result<Vec<LeveledMemoryRecord>, ApiError> {
    let filter_level = level
        .map(|s| {
            parse_level(s).ok_or_else(|| ApiError::InvalidRequest(format!("unknown level: {s}")))
        })
        .transpose()?;

    let levels_to_query: Vec<MemoryLevel> = match filter_level {
        Some(lv) => vec![lv],
        None => vec![
            MemoryLevel::L0Full,
            MemoryLevel::L1Summary,
            MemoryLevel::L2Metadata,
        ],
    };

    let mut all_records: Vec<LeveledMemoryRecord> = Vec::new();
    for lv in levels_to_query {
        match state
            .memory_store
            .query_by_level(GLOBAL_DEMO_SESSION, lv)
            .await
        {
            Ok(recs) => all_records.extend(recs),
            Err(e) => tracing::warn!("memory search query_by_level error: {e}"),
        }
    }

    // Apply agent_id filter if provided.
    if let Some(aid) = agent_id.filter(|s| !s.is_empty()) {
        all_records.retain(|r| r.agent_id == aid);
    }

    Ok(all_records)
}

/// BM25 keyword search — original S20 logic, extracted into a helper.
async fn keyword_search(
    state: Arc<AppState>,
    params: SearchMemoryQuery,
) -> Result<Json<Value>, ApiError> {
    let query = params.q.trim().to_string();

    let filter_level = params
        .level
        .as_deref()
        .map(|s| {
            parse_level(s).ok_or_else(|| ApiError::InvalidRequest(format!("unknown level: {s}")))
        })
        .transpose()?;

    let limit = params.limit.unwrap_or(20).min(100);

    let levels_to_query: Vec<MemoryLevel> = match filter_level {
        Some(lv) => vec![lv],
        None => vec![
            MemoryLevel::L0Full,
            MemoryLevel::L1Summary,
            MemoryLevel::L2Metadata,
        ],
    };

    let mut all_records: Vec<LeveledMemoryRecord> = Vec::new();
    for level in levels_to_query {
        match state
            .memory_store
            .query_by_level(GLOBAL_DEMO_SESSION, level)
            .await
        {
            Ok(recs) => all_records.extend(recs),
            Err(e) => tracing::warn!("memory search query_by_level error: {e}"),
        }
    }

    let query_tokens = crate::search::bm25::tokenize(&query);
    let mut scored: Vec<(MemorySummary, f64, Vec<String>)> = all_records
        .iter()
        .filter_map(|r| {
            let content_str = to_content_string(&r.content);
            let score = crate::search::bm25::bm25_score(&query_tokens, &content_str);
            if score > 0.0 {
                let matches = crate::search::bm25::find_matches(&query_tokens, &content_str);
                Some((record_to_summary(r), score, matches))
            } else {
                None
            }
        })
        .collect();

    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(limit);

    let results: Vec<Value> = scored
        .into_iter()
        .map(|(summary, score, matched)| {
            json!({
                "memory": summary,
                "score": score,
                "matched_tokens": matched,
            })
        })
        .collect();

    Ok(Json(json!({
        "query": query,
        "total": results.len(),
        "results": results,
    })))
}

/// Cosine-ranked semantic search using embedding vectors (S25 T4).
async fn semantic_search(
    state: Arc<AppState>,
    params: SearchMemoryQuery,
) -> Result<Json<Value>, ApiError> {
    if state.embed_client.dimension() == 0 {
        return Err(ApiError::InvalidRequest(
            "semantic search not configured — set CYBERCLAW_EMBED_ENABLED=true".to_string(),
        ));
    }

    let query_vec = state
        .embed_client
        .embed(&params.q)
        .await
        .map_err(|e| ApiError::InternalError(format!("embed query failed: {e}")))?;

    if query_vec.is_empty() {
        return Err(ApiError::InternalError(
            "embed returned empty vector".to_string(),
        ));
    }

    let candidates =
        list_filtered_candidates(&state, params.level.as_deref(), params.agent_id.as_deref())
            .await?;

    let mut scored: Vec<(LeveledMemoryRecord, f32)> = candidates
        .into_iter()
        .filter_map(|rec| {
            let emb = rec.embedding.as_ref()?;
            if emb.len() != query_vec.len() {
                return None; // dimension mismatch — skip
            }
            let score = cosine_similarity(&query_vec, emb);
            Some((rec, score))
        })
        .collect();

    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let limit = params.limit.unwrap_or(20).min(100);
    scored.truncate(limit);

    let total = scored.len();
    let results: Vec<Value> = scored
        .into_iter()
        .map(|(rec, score)| {
            json!({
                "memory": record_to_summary(&rec),
                "score": score,
                "matched_tokens": [],
            })
        })
        .collect();

    Ok(Json(json!({
        "query": params.q,
        "mode": "semantic",
        "total": total,
        "results": results,
    })))
}

/// Compute cosine similarity between two equal-length f32 slices.
///
/// Returns 0.0 when either vector is the zero vector.
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot / (norm_a * norm_b)
    }
}

/// Hybrid search: BM25 top-100 candidates → cosine rerank → top {limit} (S26 T1).
///
/// Falls back transparently to keyword-only results when the embed client is
/// disabled (`dimension() == 0`), returning `mode: "hybrid_fallback_keyword"`.
async fn hybrid_search(
    state: Arc<AppState>,
    params: SearchMemoryQuery,
) -> Result<Json<Value>, ApiError> {
    let candidates =
        list_filtered_candidates(&state, params.level.as_deref(), params.agent_id.as_deref())
            .await?;

    // BM25 score every candidate; keep only those with score > 0.
    let query_tokens = crate::search::bm25::tokenize(&params.q);
    let mut bm25_scored: Vec<(LeveledMemoryRecord, f64)> = candidates
        .into_iter()
        .filter_map(|rec| {
            let content_str = to_content_string(&rec.content);
            let score = crate::search::bm25::bm25_score(&query_tokens, &content_str);
            if score > 0.0 {
                Some((rec, score))
            } else {
                None
            }
        })
        .collect();

    bm25_scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    bm25_scored.truncate(100);

    // Fallback: embed client disabled → return keyword results transparently.
    if state.embed_client.dimension() == 0 {
        let limit = params.limit.unwrap_or(20).min(100);
        bm25_scored.truncate(limit);
        let results: Vec<Value> = bm25_scored
            .into_iter()
            .map(|(rec, score)| {
                let matched = crate::search::bm25::find_matches(
                    &query_tokens,
                    &to_content_string(&rec.content),
                );
                json!({
                    "memory": record_to_summary(&rec),
                    "score": score,
                    "matched_tokens": matched,
                })
            })
            .collect();
        let total = results.len();
        return Ok(Json(json!({
            "query": params.q,
            "mode": "hybrid_fallback_keyword",
            "total": total,
            "results": results,
        })));
    }

    // Embed the query vector.
    let query_vec = state
        .embed_client
        .embed(&params.q)
        .await
        .map_err(|e| ApiError::InternalError(format!("embed query: {e}")))?;

    // Normalize BM25 scores and combine 50/50 with cosine similarity.
    let max_bm25 = bm25_scored.iter().map(|(_, s)| *s).fold(0.0_f64, f64::max);

    let mut hybrid_scored: Vec<(LeveledMemoryRecord, f64)> = bm25_scored
        .into_iter()
        .map(|(rec, bm25)| {
            let bm25_norm = if max_bm25 == 0.0 {
                0.0
            } else {
                bm25 / max_bm25
            };
            let cosine = rec
                .embedding
                .as_ref()
                .filter(|e| e.len() == query_vec.len())
                .map(|e| cosine_similarity(&query_vec, e) as f64)
                .unwrap_or(0.0);
            let score = 0.5 * bm25_norm + 0.5 * cosine;
            (rec, score)
        })
        .collect();

    hybrid_scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let limit = params.limit.unwrap_or(20).min(100);
    hybrid_scored.truncate(limit);

    let total = hybrid_scored.len();
    let results: Vec<Value> = hybrid_scored
        .into_iter()
        .map(|(rec, score)| {
            json!({
                "memory": record_to_summary(&rec),
                "score": score,
                "matched_tokens": [],
            })
        })
        .collect();

    Ok(Json(json!({
        "query": params.q,
        "mode": "hybrid",
        "total": total,
        "results": results,
    })))
}

/// `GET /api/v1/memory/:id` — fetch full record (admin only).
async fn get_memory(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<String>,
) -> Result<Json<MemoryRecordDto>, ApiError> {
    require_admin(&claims).await?;

    find_record_by_id(state.memory_store.as_ref(), &id)
        .await
        .map(|r| Json(record_to_full(&r)))
        .ok_or_else(|| ApiError::NotFound(format!("memory record not found: {id}")))
}

/// `POST /api/v1/memory` — manually create a new memory record (admin only, S19 F).
///
/// v1.6 WP-1 #1: accepts raw JSON Value to collect ALL missing/invalid fields in one
/// 422 response, so clients don't iteratively discover schema via 4+ round trips
/// (memory_smoke_v1.5 found `agent_id` then `level` then `content` step-by-step).
async fn create_memory(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Json(raw): Json<serde_json::Value>,
) -> Result<Json<MemoryRecordDto>, ApiError> {
    require_admin(&claims).await?;

    // v1.6 multi-error validation: collect every problem, return all at once.
    let mut errors: Vec<String> = Vec::new();

    let agent_id = raw
        .get("agent_id")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string());
    if agent_id.as_deref().is_none_or(str::is_empty) {
        errors.push("agent_id (string, non-empty) is required".to_string());
    }

    let level_str = raw
        .get("level")
        .and_then(|v| v.as_str())
        .map(String::from);
    let level = match level_str.as_deref() {
        Some(s) => match parse_level(s) {
            Some(l) => Some(l),
            None => {
                errors.push(format!("level must be 'L0', 'L1', or 'L2' (got '{s}')"));
                None
            }
        },
        None => {
            errors.push("level (string, one of L0/L1/L2) is required".to_string());
            None
        }
    };

    let content = raw
        .get("content")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string());
    if content.as_deref().is_none_or(str::is_empty) {
        errors.push("content (string, non-empty) is required".to_string());
    }

    let tags: Option<Vec<String>> = raw.get("tags").and_then(|v| v.as_array()).map(|arr| {
        arr.iter()
            .filter_map(|x| x.as_str().map(String::from))
            .collect()
    });

    if !errors.is_empty() {
        return Err(ApiError::InvalidRequest(format!(
            "validation failed ({} error{}): {}",
            errors.len(),
            if errors.len() == 1 { "" } else { "s" },
            errors.join("; ")
        )));
    }

    // All fields validated; safe to unwrap.
    let req = CreateMemoryRequest {
        agent_id: agent_id.unwrap(),
        level: level_str.unwrap(),
        content: content.unwrap(),
        tags,
    };
    let level = level.unwrap();

    let now = Utc::now();
    let memory_id = uuid::Uuid::new_v4().to_string();

    // S20 E2: sanitize content before storing — redact any detected credentials.
    let sanitized = state
        .memory_sanitizer
        .sanitize("memory.create", &req.content);
    let final_content = sanitized.content.clone();
    let redacted_count = sanitized
        .warnings
        .iter()
        .filter(|w| w.category == WarningCategory::CredentialLeak)
        .count();
    if redacted_count > 0 {
        tracing::info!(
            memory_id = %memory_id,
            redacted = redacted_count,
            "S20 E2: sanitizer redacted credentials from memory content (create)"
        );
    }
    if let Some(audit) = state.audit.as_ref() {
        audit
            .record_sanitizer_warnings(
                claims.sub.to_string(),
                "memory.create",
                Some(format!("memory:{memory_id}")),
                &sanitized.warnings,
            )
            .await;
    }

    let record = LeveledMemoryRecord {
        id: memory_id.clone(),
        session_id: GLOBAL_DEMO_SESSION.to_string(),
        agent_id: req.agent_id.clone(),
        level,
        key: memory_id.clone(),
        content: serde_json::Value::String(final_content),
        created_at: now,
        updated_at: now,
        ttl_seconds: level.default_ttl_seconds(),
        source_execution_id: None,
        embedding: None,
        tags: req.tags.clone().unwrap_or_default(),
    };

    state
        .memory_store
        .store_leveled(record.clone())
        .await
        .map_err(|e| {
            tracing::error!("memory create store_leveled failed: {e}");
            ApiError::InternalError("failed to create memory record".to_string())
        })?;

    let mut dto = record_to_full(&record);
    if redacted_count > 0 {
        let mut meta = Map::new();
        meta.insert("sanitized".to_string(), Value::String("true".to_string()));
        meta.insert(
            "redacted_count".to_string(),
            Value::String(redacted_count.to_string()),
        );
        dto.metadata = Some(meta);
    }

    // Emit audit entry for the mutation.
    if let Some(audit) = state.audit.as_ref() {
        let actor = claims.sub.to_string();
        audit
            .record(AuditEntry::now(
                actor.clone(),
                AuditKind::Mutation,
                "memory.create".to_string(),
                Some(format!("memory:{memory_id}")),
                json!({
                    "memory_id": memory_id,
                    "agent_id": req.agent_id,
                    "level": req.level,
                    "content_len": req.content.len(),
                    "sanitized": redacted_count > 0,
                }),
                AuditResult::Success,
            ))
            .await;
        info!(actor, memory_id = %memory_id, "memory.create audit logged");
    }

    Ok(Json(dto))
}

/// `POST /api/v1/memory/:id/edit` — update content, emit audit entry (admin only).
async fn edit_memory(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<String>,
    Json(body): Json<EditMemoryBody>,
) -> Result<Json<MemoryRecordDto>, ApiError> {
    require_admin(&claims).await?;

    if body.content.is_empty() {
        return Err(ApiError::InvalidRequest(
            "content must not be empty".to_string(),
        ));
    }

    let existing = find_record_by_id(state.memory_store.as_ref(), &id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("memory record not found: {id}")))?;

    // S20 E2: sanitize content before storing — redact any detected credentials.
    let sanitized = state
        .memory_sanitizer
        .sanitize("memory.edit", &body.content);
    let final_content = sanitized.content.clone();
    let redacted_count = sanitized
        .warnings
        .iter()
        .filter(|w| w.category == WarningCategory::CredentialLeak)
        .count();
    if redacted_count > 0 {
        tracing::info!(
            memory_id = %id,
            redacted = redacted_count,
            "S20 E2: sanitizer redacted credentials from memory content (edit)"
        );
    }
    if let Some(audit) = state.audit.as_ref() {
        audit
            .record_sanitizer_warnings(
                claims.sub.to_string(),
                "memory.edit",
                Some(format!("memory:{id}")),
                &sanitized.warnings,
            )
            .await;
    }

    let now = Utc::now();
    let updated = LeveledMemoryRecord {
        content: json!(final_content),
        updated_at: now,
        ..existing
    };

    state
        .memory_store
        .store_leveled(updated.clone())
        .await
        .map_err(|e| {
            tracing::error!("memory edit store_leveled failed: {e}");
            ApiError::InternalError("failed to update memory record".to_string())
        })?;

    let mut dto = record_to_full(&updated);
    if redacted_count > 0 {
        let mut meta = Map::new();
        meta.insert("sanitized".to_string(), Value::String("true".to_string()));
        meta.insert(
            "redacted_count".to_string(),
            Value::String(redacted_count.to_string()),
        );
        dto.metadata = Some(meta);
    }

    // Emit audit entry for the mutation.
    if let Some(audit) = state.audit.as_ref() {
        let actor = claims.sub.to_string();
        audit
            .record(AuditEntry::now(
                actor.clone(),
                AuditKind::Mutation,
                "memory.edit".to_string(),
                Some(format!("memory:{id}")),
                json!({
                    "memory_id": id,
                    "content_len": dto.content.len(),
                    "level": dto.level,
                    "agent_id": dto.agent_id,
                    "sanitized": redacted_count > 0,
                }),
                AuditResult::Success,
            ))
            .await;
        info!(actor, memory_id = %id, "memory.edit audit logged");
    }

    Ok(Json(dto))
}

/// `GET /api/v1/memory/:id/trace` — provenance (S18 R4: real data).
async fn trace_memory(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<String>,
) -> Result<Json<TraceResponse>, ApiError> {
    require_admin(&claims).await?;

    let record = find_record_by_id(state.memory_store.as_ref(), &id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("memory record not found: {id}")))?;

    // written_by: populated when R1 write hook set source_execution_id
    let written_by = record.source_execution_id.as_ref().map(|eid| WrittenByDto {
        execution_id: eid.clone(),
        written_at: record.created_at,
    });

    // read_by: query reads log from the store
    let read_records = state.memory_store.get_reads(&id).await.unwrap_or_default();

    let read_by: Vec<ReadByDto> = read_records
        .into_iter()
        .map(|r| ReadByDto {
            execution_id: r.execution_id,
            read_at: r.read_at,
        })
        .collect();

    Ok(Json(TraceResponse {
        written_by,
        read_by,
    }))
}

/// `DELETE /api/v1/memory/:id` — permanently delete a memory record (admin only, S19 F).
async fn delete_memory(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&claims).await?;

    // Verify the record exists first (return 404 for unknown ids).
    find_record_by_id(state.memory_store.as_ref(), &id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("memory record not found: {id}")))?;

    state.memory_store.delete(&id).await.map_err(|e| {
        tracing::error!("memory delete failed: {e}");
        ApiError::InternalError("failed to delete memory record".to_string())
    })?;

    // Emit audit entry.
    if let Some(audit) = state.audit.as_ref() {
        let actor = claims.sub.to_string();
        audit
            .record(AuditEntry::now(
                actor.clone(),
                AuditKind::Mutation,
                "memory.delete".to_string(),
                Some(format!("memory:{id}")),
                json!({ "memory_id": id }),
                AuditResult::Success,
            ))
            .await;
        info!(actor, memory_id = %id, "memory.delete audit logged");
    }

    Ok(Json(json!({ "deleted": id })))
}

// ---------------------------------------------------------------------------
// Global session ID used for demo records and v1 single-tenant operation.
// All seed records and agent executions (R1/R2) use this session_id until
// multi-tenant session isolation is wired up.
// ---------------------------------------------------------------------------
pub const GLOBAL_DEMO_SESSION: &str = "_cyberclaw_demo_";

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::test_helpers::{build_test_state, jwt_for, seed_users_with_role};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::middleware as axum_middleware;
    use serial_test::serial;
    use tower::ServiceExt;

    fn app_with_memory(state: Arc<AppState>) -> axum::Router {
        create_memory_router()
            .layer(axum_middleware::from_fn_with_state(
                state.clone(),
                crate::middleware::jwt_auth,
            ))
            .with_state(state)
    }

    async fn build_seeded_state() -> Arc<AppState> {
        let state = build_test_state();
        seed_memory_demo(state.memory_store.as_ref()).await;
        state
    }

    // ------------------------------------------------------------------
    // test_list_admin_only: viewer → 403
    // ------------------------------------------------------------------
    #[tokio::test]
    #[serial]
    async fn test_list_admin_only() {
        let state = build_seeded_state().await;
        let (_tmp, _restore) = seed_users_with_role("viewer-user", "viewer");
        let token = jwt_for(&state, "viewer-user");

        let app = app_with_memory(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/memory")
                    .header("Authorization", format!("Bearer {}", token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "viewer must get 403");
    }

    // ------------------------------------------------------------------
    // test_list_filters_by_level
    // ------------------------------------------------------------------
    #[tokio::test]
    #[serial]
    async fn test_list_filters_by_level() {
        let state = build_seeded_state().await;
        let (_tmp, _restore) = seed_users_with_role("admin-user", "admin");
        let token = jwt_for(&state, "admin-user");

        let app = app_with_memory(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/memory?level=L0")
                    .header("Authorization", format!("Bearer {}", token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let records = json["records"].as_array().unwrap();
        // Seeded: 3 snippets × 2 agents = 6 L0 records
        assert_eq!(records.len(), 6, "expected 6 L0 records from seed");
        for rec in records {
            assert_eq!(rec["level"].as_str().unwrap(), "L0");
        }
    }

    // ------------------------------------------------------------------
    // test_list_filters_by_agent
    // ------------------------------------------------------------------
    #[tokio::test]
    #[serial]
    async fn test_list_filters_by_agent() {
        let state = build_seeded_state().await;
        let (_tmp, _restore) = seed_users_with_role("admin-user2", "admin");
        let token = jwt_for(&state, "admin-user2");

        let app = app_with_memory(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/memory?agent_id=agent-ada")
                    .header("Authorization", format!("Bearer {}", token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let records = json["records"].as_array().unwrap();
        // Seeded: 3 levels × 3 snippets = 9 records for agent-ada
        assert_eq!(records.len(), 9, "expected 9 records for agent-ada");
        for rec in records {
            assert_eq!(rec["agent_id"].as_str().unwrap(), "agent-ada");
        }
    }

    // ------------------------------------------------------------------
    // test_edit_emits_audit_entry
    // ------------------------------------------------------------------
    #[tokio::test]
    #[serial]
    async fn test_edit_emits_audit_entry() {
        use tempfile::TempDir;

        let state = build_seeded_state().await;
        let (_tmp, _restore) = seed_users_with_role("admin-edit", "admin");
        let token = jwt_for(&state, "admin-edit");

        // Inject an audit sink so we can verify the entry was recorded.
        let tmp_audit = TempDir::new().unwrap();
        let audit_path = tmp_audit.path().join("audit.db");
        let sink = Arc::new(crate::audit::AuditSink::new(audit_path).await.unwrap());
        let state_with_audit = {
            let mut s = (*state).clone();
            s.audit = Some(sink.clone());
            Arc::new(s)
        };

        // Pick any L0 record id from the seeded store.
        let l0_records = state_with_audit
            .memory_store
            .query_by_level(GLOBAL_DEMO_SESSION, MemoryLevel::L0Full)
            .await
            .unwrap();
        assert!(!l0_records.is_empty(), "need at least one L0 record");
        let id = l0_records[0].id.clone();

        let app = app_with_memory(state_with_audit);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/memory/{}/edit", id))
                    .header("Authorization", format!("Bearer {}", token))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({ "content": "updated content by admin" }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);

        // Verify audit sink contains the memory.edit entry.
        let entries = sink
            .tail(10, &crate::audit::AuditQuery::default())
            .await
            .unwrap();
        assert!(
            entries.iter().any(|e| e.action == "memory.edit"),
            "expected memory.edit audit entry"
        );
    }

    // ------------------------------------------------------------------
    // test_trace_returns_404_for_unknown (S18 R4: no longer stub-200)
    // ------------------------------------------------------------------
    #[tokio::test]
    #[serial]
    async fn test_trace_returns_404_for_unknown() {
        let state = build_test_state();
        let (_tmp, _restore) = seed_users_with_role("admin-trace", "admin");
        let token = jwt_for(&state, "admin-trace");

        let app = app_with_memory(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/memory/nonexistent-id/trace")
                    .header("Authorization", format!("Bearer {}", token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        // S18 R4: non-existent memory id → 404
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // ------------------------------------------------------------------
    // test_trace_returns_written_by_after_memory_write (S18 R4)
    // ------------------------------------------------------------------
    #[tokio::test]
    #[serial]
    async fn test_trace_returns_written_by_after_memory_write() {
        use cyberclaw_store::{LeveledMemoryRecord, MemoryLevel};

        let state = build_test_state();
        let (_tmp, _restore) = seed_users_with_role("admin-trace-w", "admin");
        let token = jwt_for(&state, "admin-trace-w");

        // Manually insert a record with source_execution_id set (simulates R1 write hook)
        let exec_id = "test-exec-001".to_string();
        let memory_id = "mem-trace-test-001".to_string();
        let now = chrono::Utc::now();
        let record = LeveledMemoryRecord {
            id: memory_id.clone(),
            session_id: GLOBAL_DEMO_SESSION.to_string(),
            agent_id: "agent-test".to_string(),
            level: MemoryLevel::L1Summary,
            key: memory_id.clone(),
            content: serde_json::json!({ "summary": "test execution summary" }),
            created_at: now,
            updated_at: now,
            ttl_seconds: MemoryLevel::L1Summary.default_ttl_seconds(),
            source_execution_id: Some(exec_id.clone()),
            embedding: None,
            tags: Vec::new(),
        };
        state.memory_store.store_leveled(record).await.unwrap();

        let app = app_with_memory(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/v1/memory/{}/trace", memory_id))
                    .header("Authorization", format!("Bearer {}", token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let written_by = &json["written_by"];
        assert!(
            !written_by.is_null(),
            "written_by should be non-null when source_execution_id is set"
        );
        assert_eq!(
            written_by["execution_id"].as_str().unwrap(),
            exec_id,
            "written_by.execution_id must match"
        );
        assert!(
            written_by["written_at"].as_str().is_some(),
            "written_at must be present"
        );
    }

    // ------------------------------------------------------------------
    // test_trace_returns_read_by_after_memory_read (S18 R4)
    // ------------------------------------------------------------------
    #[tokio::test]
    #[serial]
    async fn test_trace_returns_read_by_after_memory_read() {
        use cyberclaw_store::{LeveledMemoryRecord, MemoryLevel};

        let state = build_test_state();
        let (_tmp, _restore) = seed_users_with_role("admin-trace-r", "admin");
        let token = jwt_for(&state, "admin-trace-r");

        // Seed a record
        let memory_id = "mem-trace-test-002".to_string();
        let now = chrono::Utc::now();
        let record = LeveledMemoryRecord {
            id: memory_id.clone(),
            session_id: GLOBAL_DEMO_SESSION.to_string(),
            agent_id: "agent-test".to_string(),
            level: MemoryLevel::L1Summary,
            key: memory_id.clone(),
            content: serde_json::json!({ "summary": "prior context" }),
            created_at: now,
            updated_at: now,
            ttl_seconds: MemoryLevel::L1Summary.default_ttl_seconds(),
            source_execution_id: None,
            embedding: None,
            tags: Vec::new(),
        };
        state.memory_store.store_leveled(record).await.unwrap();

        // Simulate R2 read hook recording reads
        let exec_a = "exec-reader-A";
        let exec_b = "exec-reader-B";
        state
            .memory_store
            .record_read(&memory_id, exec_a)
            .await
            .unwrap();
        state
            .memory_store
            .record_read(&memory_id, exec_b)
            .await
            .unwrap();

        let app = app_with_memory(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/v1/memory/{}/trace", memory_id))
                    .header("Authorization", format!("Bearer {}", token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let read_by = json["read_by"].as_array().unwrap();
        assert_eq!(read_by.len(), 2, "expected 2 read_by entries");
        let ids: Vec<&str> = read_by
            .iter()
            .map(|e| e["execution_id"].as_str().unwrap())
            .collect();
        assert!(ids.contains(&exec_a), "exec-reader-A must be in read_by");
        assert!(ids.contains(&exec_b), "exec-reader-B must be in read_by");
    }

    // ------------------------------------------------------------------
    // test_trace_admin_only (S18 R4: guard unchanged)
    // ------------------------------------------------------------------
    #[tokio::test]
    #[serial]
    async fn test_trace_admin_only() {
        let state = build_seeded_state().await;
        let (_tmp, _restore) = seed_users_with_role("viewer-trace", "viewer");
        let token = jwt_for(&state, "viewer-trace");

        let all_records = state
            .memory_store
            .query_by_level(GLOBAL_DEMO_SESSION, MemoryLevel::L0Full)
            .await
            .unwrap();
        let id = all_records[0].id.clone();

        let app = app_with_memory(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/v1/memory/{}/trace", id))
                    .header("Authorization", format!("Bearer {}", token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "viewer must get 403 on trace"
        );
    }

    // ------------------------------------------------------------------
    // test_trace_seed_data_has_null_written_by (S18 R4)
    // Seed records have source_execution_id: None → written_by: null
    // ------------------------------------------------------------------
    #[tokio::test]
    #[serial]
    async fn test_trace_seed_data_has_null_written_by() {
        let state = build_seeded_state().await;
        let (_tmp, _restore) = seed_users_with_role("admin-trace3", "admin");
        let token = jwt_for(&state, "admin-trace3");

        let all_records = state
            .memory_store
            .query_by_level(GLOBAL_DEMO_SESSION, MemoryLevel::L0Full)
            .await
            .unwrap();
        let id = all_records[0].id.clone();

        let app = app_with_memory(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/v1/memory/{}/trace", id))
                    .header("Authorization", format!("Bearer {}", token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(
            json["written_by"].is_null(),
            "seed data: written_by must be null"
        );
        assert_eq!(json["read_by"].as_array().unwrap().len(), 0);
    }

    // ------------------------------------------------------------------
    // S19 F: test_create_admin_only (viewer → 403)
    // ------------------------------------------------------------------
    #[tokio::test]
    #[serial]
    async fn test_create_admin_only() {
        let state = build_test_state();
        let (_tmp, _restore) = seed_users_with_role("viewer-create", "viewer");
        let token = jwt_for(&state, "viewer-create");

        let app = app_with_memory(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/memory")
                    .header("Authorization", format!("Bearer {}", token))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "agent_id": "agent-test",
                            "level": "L1",
                            "content": "some content"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "viewer must get 403 on create"
        );
    }

    // ------------------------------------------------------------------
    // S19 F: test_create_happy_path (returns 200 + record in store)
    // ------------------------------------------------------------------
    #[tokio::test]
    #[serial]
    async fn test_create_happy_path() {
        let state = build_test_state();
        let (_tmp, _restore) = seed_users_with_role("admin-create-happy", "admin");
        let token = jwt_for(&state, "admin-create-happy");

        let app = app_with_memory(state.clone());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/memory")
                    .header("Authorization", format!("Bearer {}", token))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "agent_id": "agent-test",
                            "level": "L1",
                            "content": "admin-seeded content"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
            .await
            .unwrap();
        let dto: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(dto["level"].as_str().unwrap(), "L1");
        assert_eq!(dto["agent_id"].as_str().unwrap(), "agent-test");
        let memory_id = dto["id"].as_str().unwrap().to_string();

        // Verify the record is retrievable from store.
        let record = find_record_by_id(state.memory_store.as_ref(), &memory_id).await;
        assert!(record.is_some(), "created record must be in store");
    }

    // ------------------------------------------------------------------
    // S19 F: test_create_validates_level (level=X → 400)
    // ------------------------------------------------------------------
    #[tokio::test]
    #[serial]
    async fn test_create_validates_level() {
        let state = build_test_state();
        let (_tmp, _restore) = seed_users_with_role("admin-create-lvl", "admin");
        let token = jwt_for(&state, "admin-create-lvl");

        let app = app_with_memory(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/memory")
                    .header("Authorization", format!("Bearer {}", token))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "agent_id": "agent-test",
                            "level": "X",
                            "content": "some content"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "unknown level must get 400"
        );
    }

    // ------------------------------------------------------------------
    // S19 F: test_create_validates_nonempty_content (empty content → 400)
    // ------------------------------------------------------------------
    #[tokio::test]
    #[serial]
    async fn test_create_validates_nonempty_content() {
        let state = build_test_state();
        let (_tmp, _restore) = seed_users_with_role("admin-create-empty", "admin");
        let token = jwt_for(&state, "admin-create-empty");

        let app = app_with_memory(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/memory")
                    .header("Authorization", format!("Bearer {}", token))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "agent_id": "agent-test",
                            "level": "L0",
                            "content": ""
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "empty content must get 400"
        );
    }

    // ------------------------------------------------------------------
    // S19 F: test_delete_admin_only (viewer → 403)
    // ------------------------------------------------------------------
    #[tokio::test]
    #[serial]
    async fn test_delete_admin_only() {
        let state = build_seeded_state().await;
        let (_tmp, _restore) = seed_users_with_role("viewer-delete", "viewer");
        let token = jwt_for(&state, "viewer-delete");

        let l0 = state
            .memory_store
            .query_by_level(GLOBAL_DEMO_SESSION, MemoryLevel::L0Full)
            .await
            .unwrap();
        let id = l0[0].id.clone();

        let app = app_with_memory(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/v1/memory/{}", id))
                    .header("Authorization", format!("Bearer {}", token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "viewer must get 403 on delete"
        );
    }

    // ------------------------------------------------------------------
    // S19 F: test_delete_happy_path (returns 200 + record gone from list)
    // ------------------------------------------------------------------
    #[tokio::test]
    #[serial]
    async fn test_delete_happy_path() {
        let state = build_seeded_state().await;
        let (_tmp, _restore) = seed_users_with_role("admin-delete-happy", "admin");
        let token = jwt_for(&state, "admin-delete-happy");

        let l0 = state
            .memory_store
            .query_by_level(GLOBAL_DEMO_SESSION, MemoryLevel::L0Full)
            .await
            .unwrap();
        assert!(!l0.is_empty(), "need at least one L0 record to delete");
        let id = l0[0].id.clone();

        let app = app_with_memory(state.clone());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/v1/memory/{}", id))
                    .header("Authorization", format!("Bearer {}", token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["deleted"].as_str().unwrap(), id);

        // Record must no longer be in store.
        let record = find_record_by_id(state.memory_store.as_ref(), &id).await;
        assert!(record.is_none(), "record must be gone after delete");
    }

    // ------------------------------------------------------------------
    // S19 F: test_delete_404_for_unknown
    // ------------------------------------------------------------------
    #[tokio::test]
    #[serial]
    async fn test_delete_404_for_unknown() {
        let state = build_test_state();
        let (_tmp, _restore) = seed_users_with_role("admin-delete-404", "admin");
        let token = jwt_for(&state, "admin-delete-404");

        let app = app_with_memory(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/v1/memory/nonexistent-memory-id")
                    .header("Authorization", format!("Bearer {}", token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "unknown id must get 404"
        );
    }

    // ==================================================================
    // S25 T4 — semantic search tests
    // ==================================================================

    /// Fake embed client for tests: always returns the same vector.
    #[derive(Debug)]
    struct FakeEmbedClient {
        vec: Vec<f32>,
    }

    #[async_trait::async_trait]
    impl cyberclaw_llm::EmbedClient for FakeEmbedClient {
        async fn embed(&self, _input: &str) -> cyberclaw_llm::LlmResult<Vec<f32>> {
            Ok(self.vec.clone())
        }
        fn dimension(&self) -> usize {
            self.vec.len()
        }
    }

    // ------------------------------------------------------------------
    // S25 T4: NoopEmbedClient (dim=0) → mode=semantic → 400
    // ------------------------------------------------------------------
    #[tokio::test]
    #[serial]
    async fn search_with_semantic_mode_disabled_returns_400() {
        // Default test state uses NoopEmbedClient (dimension = 0).
        let state = build_test_state();
        let (_tmp, _restore) = seed_users_with_role("admin-sem-disabled", "admin");
        let token = jwt_for(&state, "admin-sem-disabled");

        let app = app_with_memory(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/memory/search?q=hello&mode=semantic")
                    .header("Authorization", format!("Bearer {}", token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "NoopEmbedClient must reject semantic mode with 400"
        );
    }

    // ------------------------------------------------------------------
    // S25 T4: semantic mode with FakeEmbedClient returns cosine-ranked results
    // ------------------------------------------------------------------
    #[tokio::test]
    #[serial]
    async fn search_with_semantic_mode_returns_ranked() {
        use chrono::Utc;

        // Build state with a FakeEmbedClient that always returns [1.0, 0.0, 0.0].
        let mut state = (*build_test_state()).clone();
        state.embed_client = std::sync::Arc::new(FakeEmbedClient {
            vec: vec![1.0_f32, 0.0, 0.0],
        });
        let state = std::sync::Arc::new(state);

        let (_tmp, _restore) = seed_users_with_role("admin-sem-ranked", "admin");
        let token = jwt_for(&state, "admin-sem-ranked");

        // Seed 3 records with distinct embeddings:
        //   matching   → [1.0, 0.0, 0.0]  cosine = 1.0
        //   orthogonal → [0.0, 1.0, 0.0]  cosine = 0.0
        //   opposite   → [-1.0, 0.0, 0.0] cosine = -1.0
        let now = Utc::now();
        let seed = |id: &str, emb: Vec<f32>| LeveledMemoryRecord {
            id: id.to_string(),
            session_id: GLOBAL_DEMO_SESSION.to_string(),
            agent_id: "agent-sem-test".to_string(),
            level: MemoryLevel::L1Summary,
            key: id.to_string(),
            content: serde_json::json!(format!("content-{}", id)),
            created_at: now,
            updated_at: now,
            ttl_seconds: MemoryLevel::L1Summary.default_ttl_seconds(),
            source_execution_id: None,
            embedding: Some(emb),
            tags: Vec::new(),
        };

        state
            .memory_store
            .store_leveled(seed("sem-match", vec![1.0, 0.0, 0.0]))
            .await
            .unwrap();
        state
            .memory_store
            .store_leveled(seed("sem-ortho", vec![0.0, 1.0, 0.0]))
            .await
            .unwrap();
        state
            .memory_store
            .store_leveled(seed("sem-oppos", vec![-1.0, 0.0, 0.0]))
            .await
            .unwrap();

        let app = app_with_memory(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/memory/search?q=anything&mode=semantic&limit=10")
                    .header("Authorization", format!("Bearer {}", token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(body["mode"].as_str().unwrap(), "semantic");
        let results = body["results"].as_array().unwrap();
        // Only records with embeddings of matching dimension (3) appear; seed_demo
        // records have embedding=None and are skipped by the semantic path.
        // Among our 3 seeded records, all 3 should appear.
        assert!(results.len() >= 3, "expected at least 3 semantic results");

        // First result must be sem-match (cosine = 1.0).
        let top_id = results[0]["memory"]["id"].as_str().unwrap();
        assert_eq!(
            top_id, "sem-match",
            "top result must be the matching vector"
        );

        // Score of top must be strictly greater than second.
        let top_score = results[0]["score"].as_f64().unwrap();
        let second_score = results[1]["score"].as_f64().unwrap();
        assert!(
            top_score > second_score,
            "cosine scores must be in descending order"
        );
    }

    // ==================================================================
    // S26 T1 — hybrid search tests
    // ==================================================================

    // ------------------------------------------------------------------
    // S26 T1: hybrid mode falls back to keyword when embed disabled (dim=0)
    // ------------------------------------------------------------------
    #[tokio::test]
    #[serial]
    async fn search_with_hybrid_mode_falls_back_to_keyword_when_embed_disabled() {
        // Default test state uses NoopEmbedClient (dimension = 0).
        let state = build_seeded_state().await;
        let (_tmp, _restore) = seed_users_with_role("admin-hybrid-fallback", "admin");
        let token = jwt_for(&state, "admin-hybrid-fallback");

        let app = app_with_memory(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/memory/search?q=security&mode=hybrid")
                    .header("Authorization", format!("Bearer {}", token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

        // Fallback mode string must indicate keyword path was used.
        assert_eq!(
            body["mode"].as_str().unwrap(),
            "hybrid_fallback_keyword",
            "embed disabled must produce hybrid_fallback_keyword mode"
        );
        // Seed data contains "security" in L0 content; at least 1 result expected.
        let results = body["results"].as_array().unwrap();
        assert!(
            !results.is_empty(),
            "hybrid fallback must return at least one BM25 result"
        );
    }

    // ------------------------------------------------------------------
    // S26 T1: hybrid mode combines BM25 and cosine correctly
    //   record_a: "rust async runtime", embedding [1.0,0.0,0.0,0.0]  → high BM25 + high cosine
    //   record_b: "no keyword match",   embedding [0.99,0.0,0.0,0.0] → BM25=0 (filtered out)
    //   record_c: "rust async helper",  embedding None               → high BM25, cosine=0
    //
    // record_b has BM25=0 so it's excluded from candidates entirely.
    // Expected order: record_a (both high) > record_c (BM25 only).
    // ------------------------------------------------------------------
    #[tokio::test]
    #[serial]
    async fn search_with_hybrid_mode_combines_bm25_and_cosine() {
        use chrono::Utc;

        // Build state with FakeEmbedClient dim=4, always returns [1.0,0.0,0.0,0.0].
        let mut state = (*build_test_state()).clone();
        state.embed_client = std::sync::Arc::new(FakeEmbedClient {
            vec: vec![1.0_f32, 0.0, 0.0, 0.0],
        });
        let state = std::sync::Arc::new(state);

        let (_tmp, _restore) = seed_users_with_role("admin-hybrid-combo", "admin");
        let token = jwt_for(&state, "admin-hybrid-combo");

        let now = Utc::now();
        let make_rec = |id: &str, content: &str, emb: Option<Vec<f32>>| LeveledMemoryRecord {
            id: id.to_string(),
            session_id: GLOBAL_DEMO_SESSION.to_string(),
            agent_id: "agent-hybrid-test".to_string(),
            level: MemoryLevel::L1Summary,
            key: id.to_string(),
            content: serde_json::json!(content),
            created_at: now,
            updated_at: now,
            ttl_seconds: MemoryLevel::L1Summary.default_ttl_seconds(),
            source_execution_id: None,
            embedding: emb,
            tags: Vec::new(),
        };

        // record_a: high BM25 ("rust" matches) + high cosine ([1,0,0,0] aligns perfectly)
        state
            .memory_store
            .store_leveled(make_rec(
                "hybrid-a",
                "rust async runtime",
                Some(vec![1.0, 0.0, 0.0, 0.0]),
            ))
            .await
            .unwrap();

        // record_b: "no keyword match" → BM25 = 0 for query "rust" → excluded from candidates
        state
            .memory_store
            .store_leveled(make_rec(
                "hybrid-b",
                "no keyword match",
                Some(vec![0.99, 0.0, 0.0, 0.0]),
            ))
            .await
            .unwrap();

        // record_c: high BM25 ("rust" matches) but no embedding → cosine contribution = 0
        state
            .memory_store
            .store_leveled(make_rec("hybrid-c", "rust async helper", None))
            .await
            .unwrap();

        let app = app_with_memory(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/memory/search?q=rust&mode=hybrid&limit=10")
                    .header("Authorization", format!("Bearer {}", token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(body["mode"].as_str().unwrap(), "hybrid");

        let results = body["results"].as_array().unwrap();
        // record_b has BM25=0 for "rust" → excluded; only a and c remain.
        assert!(
            results.len() >= 2,
            "expected at least hybrid-a and hybrid-c"
        );

        // record_a must be top-1: bm25_norm=1.0, cosine=1.0 → score=1.0
        let top_id = results[0]["memory"]["id"].as_str().unwrap();
        assert_eq!(top_id, "hybrid-a", "hybrid-a must be top result");

        // record_c must be second: bm25_norm≈1.0 (same BM25 as a), cosine=0 → score≈0.5
        // record_a score=1.0 > record_c score≈0.5
        let top_score = results[0]["score"].as_f64().unwrap();
        let second_score = results[1]["score"].as_f64().unwrap();
        assert!(
            top_score > second_score,
            "hybrid-a score ({top_score}) must exceed hybrid-c score ({second_score})"
        );

        // record_b must NOT appear (BM25 = 0 → excluded from candidates).
        let ids: Vec<&str> = results
            .iter()
            .map(|r| r["memory"]["id"].as_str().unwrap())
            .collect();
        assert!(
            !ids.contains(&"hybrid-b"),
            "hybrid-b must be excluded (BM25=0)"
        );
    }

    // ------------------------------------------------------------------
    // S25 T4: no mode param → defaults to Keyword (BM25 backward compat)
    // ------------------------------------------------------------------
    #[tokio::test]
    #[serial]
    async fn search_with_keyword_mode_unchanged_backward_compat() {
        let state = build_seeded_state().await;
        let (_tmp, _restore) = seed_users_with_role("admin-bm25-compat", "admin");
        let token = jwt_for(&state, "admin-bm25-compat");

        let app = app_with_memory(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    // No mode param → defaults to keyword
                    .uri("/api/v1/memory/search?q=security")
                    .header("Authorization", format!("Bearer {}", token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

        // Response must have standard BM25 shape (query + total + results).
        assert!(body["query"].is_string(), "must have query field");
        assert!(body["total"].is_number(), "must have total field");
        assert!(body["results"].is_array(), "must have results array");
        // No "mode" field in the keyword response (backward compat).
        assert!(
            body["mode"].is_null(),
            "keyword response must not include mode field"
        );
    }
}
