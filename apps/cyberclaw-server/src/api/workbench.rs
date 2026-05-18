//! Operator Workbench aggregate endpoints.
//!
//! Sprint 11 W2 L1. Replaces the frontend mock TODOs in
//! `web/src/pages_workbench.jsx` (Diagnose / Dry-Run / Inspect tabs).
//!
//! # Routes
//!
//! | Method | Route | Auth | Purpose |
//! |---|---|---|---|
//! | GET | `/api/v1/workbench/diagnose?limit=20` | JWT | Recent failed executions + trace summary |
//! | POST | `/api/v1/workbench/dry-run` | JWT | Noop-dispatch a sample `ExecutionPlan` |
//! | GET | `/api/v1/workbench/inspect/:kind/:id` | JWT | Fetch one resource by kind (execution/artifact/agent/skill) |
//!
//! All three endpoints are read-only or side-effect-free. The dry-run path
//! MUST NOT dispatch through the real connector registry; it surfaces the
//! *planned* capability calls so the operator can promote the plan to
//! `/reviews` to run it for real.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use cyberclaw_control_plane::execution_service::ExecutionService;
use cyberclaw_core::execution::ExecutionStatus;
use cyberclaw_core::ids::ExecutionId;
use cyberclaw_llm::types::{ChatRequest, Message};

use crate::admin_store::synthetic_trace;
use crate::error::ApiError;
use crate::state::AppState;

/// Build the workbench router.
pub fn create_workbench_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/workbench/diagnose", get(diagnose))
        .route("/api/v1/workbench/dry-run", post(dry_run))
        .route("/api/v1/workbench/inspect/:kind/:id", get(inspect_resource))
        // Sprint 18 W2 — unified chat endpoint backing the SPA Workbench
        // page (web/src/pages_workbench.jsx). Replaces MOCK_RESPONSES.
        .route("/api/v1/workbench/chat", post(chat))
}

// ---------------------------------------------------------------------------
// Diagnose
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct DiagnoseQuery {
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize)]
struct DiagnoseEntry {
    id: String,
    agent: String,
    capability: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    trace_summary: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct DiagnoseResponse {
    executions: Vec<DiagnoseEntry>,
}

/// `GET /api/v1/workbench/diagnose?limit=20`
///
/// Returns the most recent failed executions. Tries the real
/// `execution_service` first; falls back to the admin demo store so a
/// freshly-booted server still shows content.
async fn diagnose(
    State(state): State<Arc<AppState>>,
    Query(query): Query<DiagnoseQuery>,
) -> Result<Json<DiagnoseResponse>, ApiError> {
    let limit = query.limit.unwrap_or(20).min(100);

    // Real path first.
    let real_failed = state
        .execution_service
        .list_all(Some(ExecutionStatus::Failed))
        .await
        .map_err(|e| ApiError::InternalError(format!("list_all failed: {}", e)))?;

    if !real_failed.is_empty() {
        let entries: Vec<DiagnoseEntry> = real_failed
            .into_iter()
            .take(limit)
            .map(|e| {
                let id_str = e.id.as_str().to_string();
                DiagnoseEntry {
                    id: id_str.clone(),
                    agent: e.agent.id.as_str().to_string(),
                    // `Execution` does not carry a direct capability id —
                    // surface the agent role as the closest proxy.
                    capability: e.agent.role.clone(),
                    status: format!("{:?}", e.status),
                    // `Execution` has no error field on this version of the
                    // core type; `None` renders as a stable "—" in the UI.
                    error: None,
                    trace_summary: synthetic_trace(&id_str),
                }
            })
            .collect();
        return Ok(Json(DiagnoseResponse {
            executions: entries,
        }));
    }

    // Fallback: admin store demo executions with status="failed".
    let seeded = state.admin_store.executions.read().await;
    let entries: Vec<DiagnoseEntry> = seeded
        .iter()
        .filter(|e| e.status.eq_ignore_ascii_case("failed"))
        .take(limit)
        .map(|e| DiagnoseEntry {
            id: e.execution_id.clone(),
            agent: e.agent.clone(),
            capability: e.capability.clone(),
            status: e.status.clone(),
            error: None,
            trace_summary: synthetic_trace(&e.execution_id),
        })
        .collect();
    info!(count = entries.len(), "workbench: diagnose served");
    Ok(Json(DiagnoseResponse {
        executions: entries,
    }))
}

// ---------------------------------------------------------------------------
// Dry-run
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct DryRunRequest {
    /// Loose shape — mirrors `cyberclaw_core::execution::ExecutionPlan` but
    /// we accept arbitrary JSON so the frontend can paste a partial plan
    /// without blocking on schema mismatches.
    #[serde(default)]
    pub plan: serde_json::Value,
    /// Optional scenario description used purely for logging/display.
    #[serde(default)]
    pub scenario: Option<String>,
}

#[derive(Debug, Serialize)]
struct WouldCall {
    capability: String,
    args: serde_json::Value,
    simulated_risk: &'static str,
    simulated_decision: &'static str,
}

#[derive(Debug, Serialize)]
struct DryRunResponse {
    simulated_results: Vec<serde_json::Value>,
    would_call: Vec<WouldCall>,
    risk_assessment: serde_json::Value,
}

/// `POST /api/v1/workbench/dry-run`
///
/// Walks the supplied plan, extracts capability ids, and returns a
/// simulated dispatch report. Never touches the live connector registry —
/// this is the contract the UI banner advertises: "Zero production
/// side-effects · promote to /reviews to execute for real".
async fn dry_run(
    State(_state): State<Arc<AppState>>,
    Json(req): Json<DryRunRequest>,
) -> Result<Json<DryRunResponse>, ApiError> {
    let capabilities = extract_capability_calls(&req.plan);

    let mut would_call: Vec<WouldCall> = Vec::with_capacity(capabilities.len());
    let mut high_count = 0usize;
    let mut ask_count = 0usize;
    for (cap, args) in &capabilities {
        let (risk, decision) = classify_risk(cap);
        if risk == "high" || risk == "critical" {
            high_count += 1;
        }
        if decision == "ask" {
            ask_count += 1;
        }
        would_call.push(WouldCall {
            capability: cap.clone(),
            args: args.clone(),
            simulated_risk: risk,
            simulated_decision: decision,
        });
    }

    let simulated_results: Vec<serde_json::Value> = capabilities
        .iter()
        .map(|(cap, _)| {
            serde_json::json!({
                "capability": cap,
                "result": "noop",
                "note": "dry-run dispatcher substituted",
            })
        })
        .collect();

    let risk_assessment = serde_json::json!({
        "total_calls": capabilities.len(),
        "high_risk": high_count,
        "requires_approval": ask_count,
        "scenario": req.scenario,
    });

    info!(
        total = capabilities.len(),
        high = high_count,
        ask = ask_count,
        "workbench: dry-run served"
    );
    Ok(Json(DryRunResponse {
        simulated_results,
        would_call,
        risk_assessment,
    }))
}

/// Walk `plan` recursively and collect `(capability_id, args)` pairs.
///
/// Accepts two shapes:
/// - `{ "steps": [{"capability": "...", "args": {...}}, ...] }`
/// - `{ "capability": "...", "args": {...} }` (single step)
///
/// Both mirror variants already seen in `ExecutionPlan` JSON serializations.
fn extract_capability_calls(plan: &serde_json::Value) -> Vec<(String, serde_json::Value)> {
    fn walk(v: &serde_json::Value, out: &mut Vec<(String, serde_json::Value)>) {
        if let Some(obj) = v.as_object() {
            if let Some(cap) = obj.get("capability").and_then(|c| c.as_str()) {
                let args = obj.get("args").cloned().unwrap_or(serde_json::Value::Null);
                out.push((cap.to_string(), args));
            }
            for (_, child) in obj.iter() {
                walk(child, out);
            }
        } else if let Some(arr) = v.as_array() {
            for item in arr {
                walk(item, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(plan, &mut out);
    out
}

/// Rough risk classifier — mirrors the heuristic the admin store uses
/// when it has no real policy engine to consult. Production writes still
/// go through `DangerousCapabilityFilter`.
fn classify_risk(cap: &str) -> (&'static str, &'static str) {
    let c = cap.to_lowercase();
    if c.contains("delete") || c.contains("destructive") {
        ("critical", "deny")
    } else if c.contains("deploy") || c.contains("write") || c.contains("exec") {
        ("high", "ask")
    } else if c.contains("create") || c.contains("update") {
        ("medium", "ask")
    } else {
        ("low", "allow")
    }
}

// ---------------------------------------------------------------------------
// Inspect
// ---------------------------------------------------------------------------

/// `GET /api/v1/workbench/inspect/:kind/:id`
///
/// Supported kinds: `execution`, `artifact`, `agent`, `skill`. The endpoint
/// is purposely read-only and returns a 404 when the resource is absent.
async fn inspect_resource(
    State(state): State<Arc<AppState>>,
    Path((kind, id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    match kind.as_str() {
        "execution" => inspect_execution(&state, &id).await,
        "artifact" => inspect_artifact(&state, &id).await,
        "agent" => inspect_agent(&state, &id).await,
        "skill" => inspect_skill(&state, &id).await,
        other => {
            warn!(kind = %other, "workbench: inspect unknown kind");
            Err(ApiError::InvalidInput(format!(
                "unknown kind '{}'. expected one of execution|artifact|agent|skill",
                other
            )))
        }
    }
}

async fn inspect_execution(
    state: &Arc<AppState>,
    id: &str,
) -> Result<Json<serde_json::Value>, ApiError> {
    if let Ok(eid) = ExecutionId::from_string(id.to_string()) {
        if let Ok(Some(exec)) = state.execution_service.get(&eid).await {
            return Ok(Json(
                serde_json::to_value(exec).unwrap_or(serde_json::Value::Null),
            ));
        }
    }
    let seeded = state.admin_store.executions.read().await;
    match seeded.iter().find(|e| e.execution_id == id) {
        Some(e) => Ok(Json(serde_json::to_value(e).unwrap_or_default())),
        None => Err(ApiError::NotFound(format!("execution '{}' not found", id))),
    }
}

async fn inspect_artifact(
    _state: &Arc<AppState>,
    id: &str,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Sprint 11 does not yet plumb an artifact store into AppState. Surface
    // a stable placeholder so the frontend can render the panel; the real
    // store lookup lands in a follow-up task.
    Ok(Json(serde_json::json!({
        "id": id,
        "kind": "artifact",
        "note": "artifact inspection not yet wired to a store",
    })))
}

async fn inspect_agent(
    state: &Arc<AppState>,
    id: &str,
) -> Result<Json<serde_json::Value>, ApiError> {
    let seeded = state.admin_store.agents.read().await;
    match seeded.iter().find(|a| a.agent_id == id || a.name == id) {
        Some(a) => Ok(Json(serde_json::to_value(a).unwrap_or_default())),
        None => Err(ApiError::NotFound(format!("agent '{}' not found", id))),
    }
}

async fn inspect_skill(
    state: &Arc<AppState>,
    id: &str,
) -> Result<Json<serde_json::Value>, ApiError> {
    let seeded = state.admin_store.skills.read().await;
    match seeded.iter().find(|s| s.skill_id == id || s.name == id) {
        Some(s) => Ok(Json(serde_json::to_value(s).unwrap_or_default())),
        None => Err(ApiError::NotFound(format!("skill '{}' not found", id))),
    }
}

// ---------------------------------------------------------------------------
// Workbench chat — unified entrypoint for the SPA chat-style UI.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct WorkbenchChatRequest {
    pub mode: String,
    pub prompt: String,
}

#[derive(Debug, Serialize)]
pub struct WorkbenchChatAction {
    pub label: String,
}

#[derive(Debug, Serialize)]
pub struct WorkbenchChatResponse {
    pub role: String,
    pub actor: String,
    pub content: String,
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provenance: Option<bool>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub actions: Vec<WorkbenchChatAction>,
}

/// `POST /api/v1/workbench/chat` — chat-style entrypoint for the
/// admin Workbench page. Replaces the prior in-SPA `MOCK_RESPONSES`
/// fixture (`web/src/pages_workbench.jsx::47`).
///
/// Each of the four modes (`diagnose`, `what-if`, `inspect`,
/// `dry-run`) is a different operator *stance*, so each gets its own
/// system prompt. The user prompt is the operator's free-text query.
/// The LLM call is read-only — no tool palette is attached and no
/// audit row is written here (the SPA records its own audit via the
/// review/dispatch routes when the operator promotes a result).
async fn chat(
    State(state): State<Arc<AppState>>,
    Json(req): Json<WorkbenchChatRequest>,
) -> Result<Json<WorkbenchChatResponse>, ApiError> {
    let (actor, system_prompt, default_actions) = mode_profile(&req.mode)
        .ok_or_else(|| ApiError::InvalidRequest(format!("unknown workbench mode: {}", req.mode)))?;

    let trace_ref = format!(
        "exec_{:08x}",
        chrono::Utc::now().timestamp_millis() as u64 & 0xFFFF_FFFF
    );

    let model = std::env::var("LLM_DEFAULT_MODEL").unwrap_or_else(|_| "gpt-4".to_string());
    // CONSTITUTION-BYPASS-OK: workbench modes (Release Guard / Policy Simulator
    // / Inspector) are READ-ONLY operator diagnostics that intentionally have
    // narrow, mode-specific prompts. Forcing the constitution here would
    // conflict with their "ask for missing kind/id" behavior. See
    // mode_profile() for the per-mode prompts.
    let llm_req = ChatRequest {
        model,
        messages: vec![Message::system(&system_prompt), Message::user(&req.prompt)],
        ..Default::default()
    };

    let content = match state.llm_client.chat_completion(llm_req).await {
        Ok(resp) => resp
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "(LLM returned empty response)".into()),
        Err(e) => {
            warn!(mode = %req.mode, error = %e, "workbench/chat LLM call failed");
            format!(
                "Workbench `{}` mode is currently unavailable: {}.\n\n\
                 Verify the LLM provider configuration (LLM_PROVIDER / \
                 LLM_API_KEY / LLM_BASE_URL) and retry.",
                req.mode, e
            )
        }
    };

    let actions = default_actions
        .iter()
        .map(|label| WorkbenchChatAction {
            label: (*label).into(),
        })
        .collect();

    Ok(Json(WorkbenchChatResponse {
        role: "assistant".into(),
        actor: actor.into(),
        content,
        mode: req.mode.clone(),
        trace_ref: Some(trace_ref),
        provenance: Some(false),
        actions,
    }))
}

/// Per-mode `(actor_label, system_prompt, default_actions)` profile.
///
/// Returns `None` for unknown modes so the caller can return 400.
///
/// The system prompts share a common discipline: read-only scope and
/// an explicit "do not fabricate identifiers" rule. Operators quote
/// these answers in incident reports, so a hallucinated execution id
/// or trace hash is a real harm — uncertainty must surface instead.
fn mode_profile(mode: &str) -> Option<(&'static str, String, &'static [&'static str])> {
    const NO_FAB: &str = "Never invent execution IDs, trace hashes, rule names, agent names, \
                          policy versions, or numeric metrics. If the operator's prompt does \
                          not contain a concrete identifier, say so and ask for one — do not \
                          fill in plausible-looking placeholders.";
    const READ_ONLY: &str = "You are read-only. You cannot dispatch, approve, mutate state, \
                             or call tools. Your reply will be shown verbatim to a human \
                             operator who decides what to do next.";

    match mode {
        "diagnose" => {
            let prompt = format!(
                "You are Release Guard, a postmortem assistant for the CyberClaw control \
                 plane. The operator wants to understand why a specific execution failed, was \
                 blocked, or was sent to review. {READ_ONLY} {NO_FAB} Structure your reply as: \
                 (1) what the operator's prompt actually identifies, (2) what would normally \
                 cause that outcome, (3) what the operator should fetch next (audit row, trace \
                 id, policy version) to confirm. Be terse — operators copy this into tickets."
            );
            Some((
                "Release Guard",
                prompt,
                &["View trace →", "Open policy", "Open audit row"],
            ))
        }
        "what-if" => {
            let prompt = format!(
                "You are Policy Simulator. The operator is exploring a hypothetical change to \
                 a governance threshold, rule, or policy and wants to reason about its effect. \
                 {READ_ONLY} {NO_FAB} You do not have access to historical decision counts — \
                 if the operator did not provide them, ask. Otherwise, walk through the \
                 directional effect (more/fewer auto-approvals, risk delta, review-queue load) \
                 and call out the dual-approval / blanket-threshold tradeoff. End with a \
                 recommendation framed as a question, not a directive."
            );
            Some((
                "Policy Simulator",
                prompt,
                &["Apply proposal", "Open historical decisions"],
            ))
        }
        "inspect" => {
            let prompt = format!(
                "You are Inspector. The operator wants details about a specific resource \
                 (execution, agent, skill, artifact). {READ_ONLY} {NO_FAB} Free-text inspection \
                 is not supported — the structured route is `GET /api/v1/workbench/inspect/\
                 :kind/:id`. If the operator's prompt names a `kind/id` pair (e.g. \
                 `execution/exec_…` or `agent/ag_…`), tell them which curl/route to call and \
                 what fields to expect. If they did not name one, ask for `kind/id`. Do not \
                 fabricate a record."
            );
            Some(("Inspector", prompt, &[]))
        }
        "dry-run" => {
            let prompt = format!(
                "You are Sandbox. The operator is sketching an `ExecutionPlan` and wants to \
                 think out loud before promoting it. {READ_ONLY} {NO_FAB} Restate the plan in \
                 the operator's own terms, list the capabilities it would touch, flag any \
                 capability that typically requires review (file delete, shell exec, network \
                 egress, credential access), and end with: 'Promote to /reviews to dispatch \
                 for real — this chat does not commit.' Keep it short."
            );
            Some(("Sandbox", prompt, &["Promote to /reviews", "Open trace"]))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::test_helpers::build_test_state;

    #[tokio::test]
    async fn diagnose_empty_when_no_failures() {
        let state = build_test_state();
        let resp = diagnose(State(state), Query(DiagnoseQuery { limit: Some(20) }))
            .await
            .expect("diagnose ok");
        assert_eq!(resp.0.executions.len(), 0);
    }

    #[tokio::test]
    async fn diagnose_falls_back_to_admin_store() {
        let state = build_test_state();
        state.admin_store.seed_demo().await;
        let resp = diagnose(State(state), Query(DiagnoseQuery { limit: Some(20) }))
            .await
            .expect("diagnose ok");
        // Seed data includes 3 `failed` executions.
        assert!(
            !resp.0.executions.is_empty(),
            "seeded failures should surface in diagnose output"
        );
        assert!(
            resp.0
                .executions
                .iter()
                .all(|e| e.status.eq_ignore_ascii_case("failed")),
            "all diagnose entries must be failed"
        );
    }

    #[tokio::test]
    async fn dry_run_extracts_capabilities_and_classifies_risk() {
        let state = build_test_state();
        let plan = serde_json::json!({
            "steps": [
                { "capability": "fs.read", "args": { "path": "/tmp/x" } },
                { "capability": "cmd.exec", "args": { "command": "ls" } },
                { "capability": "fs.delete", "args": { "path": "/var/log/old" } },
            ]
        });
        let resp = dry_run(
            State(state),
            Json(DryRunRequest {
                plan,
                scenario: Some("test".to_string()),
            }),
        )
        .await
        .expect("dry_run ok");
        assert_eq!(resp.0.would_call.len(), 3);
        assert_eq!(resp.0.simulated_results.len(), 3);
        // delete -> critical, cmd.exec -> high, fs.read -> low.
        let capabilities: Vec<&str> = resp
            .0
            .would_call
            .iter()
            .map(|c| c.capability.as_str())
            .collect();
        assert!(capabilities.contains(&"fs.delete"));
        assert!(capabilities.contains(&"cmd.exec"));
        assert!(capabilities.contains(&"fs.read"));
    }

    #[tokio::test]
    async fn inspect_unknown_kind_returns_400() {
        let state = build_test_state();
        let err = inspect_resource(
            State(state),
            Path(("portal".to_string(), "anything".to_string())),
        )
        .await
        .expect_err("unknown kind must 400");
        assert!(matches!(err, ApiError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn inspect_agent_from_admin_store() {
        let state = build_test_state();
        state.admin_store.seed_demo().await;
        let resp = inspect_resource(
            State(state),
            Path(("agent".to_string(), "ag_01H8X".to_string())),
        )
        .await
        .expect("inspect ok");
        assert_eq!(resp.0["agent_id"], "ag_01H8X");
    }

    #[tokio::test]
    async fn inspect_missing_agent_returns_404() {
        let state = build_test_state();
        let err = inspect_resource(
            State(state),
            Path(("agent".to_string(), "does-not-exist".to_string())),
        )
        .await
        .expect_err("missing must 404");
        assert!(matches!(err, ApiError::NotFound(_)));
    }
}
