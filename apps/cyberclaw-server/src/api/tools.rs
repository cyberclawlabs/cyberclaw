//! Tools API — surface the built-in tool set from [`BuiltinToolRegistry`].
//!
//! Sprint 8 Lane 1. Lists every [`CapabilityFacade`] the host exposes to
//! agents as a tool. Admin UI reads this to render the Tools tab.
//!
//! # Shape
//!
//! Each entry is a `ToolInfo` with `tool_id`, `connector_id`, `risk_level`,
//! `effects`, and the input/output schemas. `output_schema` is left empty
//! (`{}`) because `CapabilityFacade` is a read-only LLM projection that
//! does not carry an output schema — per EVOLUTION_IDIOMS §4 the schema
//! owner is the connector contract, not the facade.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;
use tracing::info;

use cyberclaw_agent_runtime::{
    builtin_tools::{BuiltinToolRegistry, ToolsetConfig},
    tool_description::CapabilityFacade,
};
use cyberclaw_core::capability::RiskLevel;

use crate::error::ApiError;
use crate::state::AppState;

/// Admin-visible projection of one built-in tool.
#[derive(Debug, Clone, Serialize)]
pub struct ToolInfo {
    /// Facade name, e.g. `"file_read"`.
    pub tool_id: String,
    /// Human-friendly name — same as `tool_id` today, kept separate so
    /// future renames don't break the UI.
    pub name: String,
    /// Parent connector id (e.g. `"local"`, `"http"`, `"internal"`).
    pub connector_id: String,
    pub description: String,
    pub risk_level: String,
    pub effects: Vec<String>,
    pub input_schema: serde_json::Value,
    pub output_schema: serde_json::Value,
}

/// Response wrapper.
#[derive(Debug, Serialize)]
pub struct ToolListResponse {
    pub tools: Vec<ToolInfo>,
    pub total: usize,
}

/// Build the `/api/v1/tools` router.
pub fn create_tools_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/tools", get(list_tools))
        // F7 — DeferredToolRegistry admin surface.
        .route("/api/v1/tools/state", get(deferred_tool_state))
        .route("/api/v1/tools/promote/:name", post(deferred_tool_promote))
        .route("/api/v1/tools/demote/:name", post(deferred_tool_demote))
}

/// F7 — `GET /api/v1/tools/state` — list every tool in the
/// `DeferredToolRegistry` plus its load state (`active` / `deferred`).
///
/// Was 0-prod-reference before this commit; now production-reachable so
/// admin UIs can see exactly what schema the LLM is currently being shown
/// (`active`) vs what's name-only (`deferred`).
async fn deferred_tool_state(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let registry = state.deferred_tool_registry.read().await;
    let active: Vec<String> = registry
        .active_facades()
        .into_iter()
        .map(|f| f.name.clone())
        .collect();
    let deferred: Vec<String> = registry
        .deferred_names()
        .into_iter()
        .map(|s| s.to_string())
        .collect();
    Ok(Json(serde_json::json!({
        "active": active,
        "deferred": deferred,
        "active_count": active.len(),
        "deferred_count": deferred.len(),
    })))
}

/// F7 — `POST /api/v1/tools/promote/:name` — flip a deferred tool to
/// active so the LLM gets its full schema in the next prompt.
async fn deferred_tool_promote(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    claims: Option<axum::Extension<crate::middleware::auth::Claims>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut registry = state.deferred_tool_registry.write().await;
    let ok = registry.promote(&name);
    drop(registry);

    // 2026-05-17 (v1.2.8): persist the promotion so restart preserves it.
    // Best-effort — log warn but don't block the API success (the runtime
    // state already changed; the persistence is the operator's safety net).
    if ok {
        if let Err(e) = crate::tools_overrides::save_override(
            &name,
            crate::tools_overrides::OverrideState::Active,
        ) {
            tracing::warn!(tool = %name, error = %e, "failed to persist promote to tools.json");
        }
    }

    if let Some(sink) = state.audit.as_ref() {
        let actor = claims
            .as_ref()
            .map(|c| c.sub.to_string())
            .unwrap_or_else(|| "system".to_string());
        sink.record(crate::audit::AuditEntry::now(
            actor,
            crate::audit::AuditKind::Mutation,
            "tools.promote".to_string(),
            Some(format!("tool:{name}")),
            serde_json::json!({ "tool": name, "succeeded": ok }),
            if ok {
                crate::audit::AuditResult::Success
            } else {
                crate::audit::AuditResult::Failure {
                    reason: "unknown tool".to_string(),
                }
            },
        ))
        .await;
    }

    Ok(Json(serde_json::json!({
        "name": name,
        "promoted": ok,
    })))
}

/// F7 — `POST /api/v1/tools/demote/:name` — flip an active tool to
/// deferred (name-only in prompt, retrievable via ToolSearch).
async fn deferred_tool_demote(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    claims: Option<axum::Extension<crate::middleware::auth::Claims>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut registry = state.deferred_tool_registry.write().await;
    let ok = registry.demote(&name);
    drop(registry);

    // 2026-05-17 (v1.2.8): persist the demote so restart preserves it.
    if ok {
        if let Err(e) = crate::tools_overrides::save_override(
            &name,
            crate::tools_overrides::OverrideState::Deferred,
        ) {
            tracing::warn!(tool = %name, error = %e, "failed to persist demote to tools.json");
        }
    }

    if let Some(sink) = state.audit.as_ref() {
        let actor = claims
            .as_ref()
            .map(|c| c.sub.to_string())
            .unwrap_or_else(|| "system".to_string());
        sink.record(crate::audit::AuditEntry::now(
            actor,
            crate::audit::AuditKind::Mutation,
            "tools.demote".to_string(),
            Some(format!("tool:{name}")),
            serde_json::json!({ "tool": name, "succeeded": ok }),
            if ok {
                crate::audit::AuditResult::Success
            } else {
                crate::audit::AuditResult::Failure {
                    reason: "unknown tool".to_string(),
                }
            },
        ))
        .await;
    }

    Ok(Json(serde_json::json!({
        "name": name,
        "demoted": ok,
    })))
}

/// `GET /api/v1/tools` — list all built-in tools.
///
/// F12 Phase C (2026-05-06) — pulls from the live `DeferredToolRegistry`
/// (seeded at server startup with default + 7 connector module facades)
/// instead of constructing a fresh `BuiltinToolRegistry::with_defaults()`
/// each call. After Phase C `default_facades()` only holds 3 chat_handler-
/// intercepted entries (skill_create / skill_search / delegate_to_sub_agent),
/// so reading the builtin alone would show 3 tools instead of the real
/// 40+ that the LLM sees. The deferred registry is the canonical
/// post-aggregation source of truth.
async fn list_tools(State(state): State<Arc<AppState>>) -> Json<ToolListResponse> {
    let registry_guard = state.deferred_tool_registry.read().await;
    let active = registry_guard.active_facades();
    let mut tools: Vec<ToolInfo> = active.iter().map(|f| to_tool_info(f)).collect();
    drop(registry_guard);

    if tools.is_empty() {
        tools = curated_fallback();
    }

    tools.sort_by(|a, b| a.tool_id.cmp(&b.tool_id));
    let total = tools.len();
    info!("tools: returning {} entries", total);
    Json(ToolListResponse { tools, total })
}

fn to_tool_info(f: &CapabilityFacade) -> ToolInfo {
    ToolInfo {
        tool_id: f.name.clone(),
        name: f.name.clone(),
        connector_id: f.connector_id.as_str().to_string(),
        description: f.description.clone(),
        risk_level: risk_label(&f.risk_level).to_string(),
        effects: f.effects.clone(),
        input_schema: f
            .input_schema
            .clone()
            .unwrap_or_else(|| serde_json::json!({})),
        output_schema: serde_json::json!({}),
    }
}

fn risk_label(r: &RiskLevel) -> &'static str {
    match r {
        RiskLevel::Low => "low",
        RiskLevel::Medium => "medium",
        RiskLevel::High => "high",
        RiskLevel::Critical => "critical",
    }
}

/// Curated fallback list — used only if `BuiltinToolRegistry::with_defaults`
/// is ever made empty. The canonical source is the builtin registry.
fn curated_fallback() -> Vec<ToolInfo> {
    BuiltinToolRegistry::with_defaults()
        .get_facades(&ToolsetConfig::default_config())
        .iter()
        .map(to_tool_info)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn risk_label_covers_all_levels() {
        assert_eq!(risk_label(&RiskLevel::Low), "low");
        assert_eq!(risk_label(&RiskLevel::Medium), "medium");
        assert_eq!(risk_label(&RiskLevel::High), "high");
        assert_eq!(risk_label(&RiskLevel::Critical), "critical");
    }

    #[tokio::test]
    async fn tools_list_returns_known_set() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt;

        // No state needed; handler is stateless. Build a fake state so
        // the `with_state` signature is satisfied.
        let state = crate::api::test_helpers::build_test_state();
        let app = create_tools_router().with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/tools")
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
        let total = json["total"].as_u64().unwrap();
        // F12 Phase C — default_facades() now holds only the 3 chat_handler-
        // intercepted facades. Connector-owned facades are seeded at server
        // startup (not in unit-test state). Verify the 3 remaining entries.
        assert!(
            total >= 3,
            "expected at least 3 builtin tools (skill_create, skill_search, delegate_to_sub_agent), got {total}"
        );
        let tools = json["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().filter_map(|t| t["tool_id"].as_str()).collect();
        assert!(names.contains(&"skill_create"));
        assert!(names.contains(&"skill_search"));
        assert!(names.contains(&"delegate_to_sub_agent"));
    }
}
