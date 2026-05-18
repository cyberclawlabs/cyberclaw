//! Agents API
//!
//! 提供 Agent 列表、调用和状态查询的 RESTful API

use axum::{
    extract::{Path, Query, State},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, error, info};

use cyberclaw_agent_runtime::builtin_tools::{BuiltinToolRegistry, ToolsetConfig};
use cyberclaw_agent_runtime::config::AgentConfig;
use cyberclaw_agent_runtime::tool_description::CapabilityFacade;
use cyberclaw_agent_runtime::types::{AgentRequest, AgentResponse};
use cyberclaw_agent_runtime::AgentRuntime;
use cyberclaw_core::ids::AgentId;
use cyberclaw_llm::types::{ChatRequest, FunctionDefinition, Message, ToolDefinition};

use crate::admin_store::AdminExecution;
use crate::api::admin::events::AdminEvent;
use crate::audit::{AuditEntry, AuditKind, AuditResult};
use crate::error::ApiError;
use crate::middleware::auth::Claims;
use crate::state::AppState;
use crate::types::{AgentExecutionStatus, AgentInfo};

/// Agent 调用请求
#[derive(Debug, Deserialize)]
pub struct InvokeAgentRequest {
    pub input: String,
    #[serde(default)]
    pub context: serde_json::Value,
}

/// Agent 调用响应
#[derive(Debug, Serialize)]
pub struct InvokeAgentResponse {
    pub agent_id: String,
    pub success: bool,
    pub output: String,
    pub metadata: Option<serde_json::Value>,
}

impl From<AgentResponse> for InvokeAgentResponse {
    fn from(resp: AgentResponse) -> Self {
        InvokeAgentResponse {
            agent_id: resp.agent_id.as_str().to_string(),
            success: resp.success,
            output: resp.output,
            metadata: Some(serde_json::json!(resp.metadata)),
        }
    }
}

/// 创建 Agents API 路由
pub fn create_agents_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/agents", get(list_agents).post(create_agent))
        .route(
            "/api/v1/agents/:id",
            axum::routing::patch(update_agent).delete(delete_agent),
        )
        .route("/api/v1/agents/:name/invoke", post(invoke_agent))
        .route("/api/v1/agents/:name/status", get(get_agent_status))
        .route("/api/v1/agents/:id/digest", get(list_agent_digests))
        .route(
            "/api/v1/agents/:id/effective-prompt",
            get(get_effective_prompt),
        )
        // F5 — SubAgentOrchestrator first production caller.
        .route("/api/v1/agents/delegate", post(delegate_subtask))
}

/// Validate and normalize a trust-level string. Accepts `high | medium | low`.
fn normalize_trust_level(input: &str) -> Result<String, ApiError> {
    let normalized = input.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "high" | "medium" | "low" => Ok(normalized),
        _ => Err(ApiError::InvalidRequest(format!(
            "trust must be one of: high, medium, low (got '{}')",
            input
        ))),
    }
}

/// F5 — request body for `POST /api/v1/agents/delegate`.
#[derive(Debug, Deserialize)]
pub struct DelegateSubtaskRequest {
    /// Parent agent id (string form). When omitted, a synthetic
    /// "delegate-root" parent id is used so the depth counter starts at 0.
    #[serde(default)]
    pub parent_agent_id: Option<String>,
    /// Plain-language subtask the child agent must complete.
    pub task_description: String,
    /// Iteration budget for the child (default 8).
    #[serde(default)]
    pub max_iterations: Option<u32>,
}

/// F5 — first production caller of `SubAgentOrchestrator`.
///
/// Prior to this commit the type existed in `cyberclaw-agent-runtime` with
/// 6+ unit tests but had **zero production references** — multi-agent
/// delegation was implemented at the type level but no production code
/// path could spawn a child agent.
///
/// `POST /api/v1/agents/delegate` constructs a per-request
/// `SubAgentOrchestrator` (it must be per-request because `caller_identity`
/// is request-scoped), spawns one child agent via `spawn_child`, then
/// drives it to completion via `run_child` and returns the output.
///
/// This is the minimum honest wiring: the type now has a production call
/// site that exercises both spawn and run. Full integration into the
/// agentic loop's tool-call path (so the LLM can autonomously delegate
/// via a `delegate_to_sub_agent` tool call) is grade B follow-up; the
/// constraint is that `IterationResult` would need a `Delegate` variant
/// and `LoopDelegate` would need a hook to route it through the
/// orchestrator. That refactor is out-of-scope for the orphan-audit
/// closure.
async fn delegate_subtask(
    State(state): State<Arc<AppState>>,
    axum::Extension(claims): axum::Extension<Claims>,
    Json(req): Json<DelegateSubtaskRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    use cyberclaw_agent_runtime::agentic_loop::IterationBudget;
    use cyberclaw_agent_runtime::sub_agent::{SpawnPolicy, SubAgentOrchestrator};
    use cyberclaw_core::identity::{ActorRef, ActorType};
    use cyberclaw_core::ids::ActorId;
    use std::time::Duration;

    info!(task = %req.task_description, "delegate_subtask start");

    let caller_actor = ActorRef {
        id: ActorId::from_string(claims.sub.as_str().to_string()).unwrap_or_else(|_| {
            ActorId::from_string("delegate-caller".to_string()).expect("fallback actor id")
        }),
        actor_type: ActorType::Human,
        tenant_id: claims.tenant.clone(),
        home_node_id: None,
        display_name: claims.sub.as_str().to_string(),
    };

    let gateway = crate::api::chat_handler::build_governing_gateway(&state);
    let mut orchestrator = SubAgentOrchestrator::new(
        SpawnPolicy::default(),
        state.llm_client.clone(),
        gateway,
        0,
        caller_actor,
    );

    let parent_id = match req.parent_agent_id.as_ref() {
        Some(s) => AgentId::from_string(s.clone()).unwrap_or_else(|_| AgentId::new()),
        None => AgentId::new(),
    };

    let budget = IterationBudget {
        max_iterations: req.max_iterations.unwrap_or(8),
        max_tokens: 32_000,
        timeout: Duration::from_secs(120),
    };

    let child_id = orchestrator
        .spawn_child(parent_id.clone(), req.task_description.clone(), &budget)
        .map_err(|e| ApiError::InvalidRequest(format!("spawn_child failed: {}", e)))?;

    match orchestrator.run_child(&child_id).await {
        Ok(output) => Ok(Json(serde_json::json!({
            "parent_agent_id": parent_id.as_str(),
            "child_agent_id": child_id.as_str(),
            "task_description": req.task_description,
            "output": output,
            "success": true,
        }))),
        Err(e) => {
            error!(error = %e, "run_child failed");
            Ok(Json(serde_json::json!({
                "parent_agent_id": parent_id.as_str(),
                "child_agent_id": child_id.as_str(),
                "task_description": req.task_description,
                "error": format!("{}", e),
                "success": false,
            })))
        }
    }
}

/// POST /api/v1/agents body — matches frontend NewAgentDialog shape.
#[derive(Debug, Deserialize)]
pub struct CreateAgentRequest {
    pub name: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub persona_file: Option<String>,
    #[serde(default)]
    pub workspace_mode: Option<String>,
    #[serde(default)]
    pub default_skills: Vec<String>,
    #[serde(default)]
    pub soul: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// Trust level: `"high" | "medium" | "low"`. Defaults to `"medium"`
    /// when omitted. Validated by `normalize_trust_level`.
    #[serde(default, alias = "trust_level")]
    pub trust: Option<String>,
}

/// PATCH /api/v1/agents/:id body — partial update.
#[derive(Debug, Deserialize)]
pub struct UpdateAgentRequest {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default, alias = "trust_level")]
    pub trust: Option<String>,
    /// `"active" | "idle" | "paused"`.
    #[serde(default)]
    pub status: Option<String>,
}

/// POST /api/v1/agents — scaffold a new AdminAgent entry.
///
/// Sprint 11 scope: writes to `admin_store.agents` so the admin SPA sees the
/// new agent immediately. Real AgentRuntime spawn / persona binding is
/// deferred to Sprint 12.
async fn create_agent(
    State(state): State<Arc<AppState>>,
    claims: Option<axum::extract::Extension<Claims>>,
    Json(req): Json<CreateAgentRequest>,
) -> Result<(axum::http::StatusCode, Json<crate::admin_store::AdminAgent>), ApiError> {
    let name = req.name.trim().to_string();
    if name.is_empty() {
        return Err(ApiError::InvalidRequest("name is required".into()));
    }
    let valid = name.len() >= 3
        && name.len() <= 32
        && name.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !valid {
        return Err(ApiError::InvalidRequest(
            "name must match ^[a-z][a-z0-9-]{2,31}$".into(),
        ));
    }

    let actor = claims
        .map(|c| c.0.sub.as_str().to_string())
        .unwrap_or_else(|| "system".to_string());

    let trust_level = match req.trust.as_deref() {
        Some(raw) => normalize_trust_level(raw)?,
        None => "medium".to_string(),
    };

    let agent = crate::admin_store::AdminAgent {
        agent_id: format!("ag_{}", name),
        name: name.clone(),
        description: req
            .description
            .or(req.soul.clone())
            .unwrap_or_else(|| format!("Agent {}", name)),
        trust_level,
        status: "idle".to_string(),
        tools: 0,
        skills: req.default_skills.len() as u32,
        executions_24h: 0,
    };

    {
        let mut agents = state.admin_store.agents.write().await;
        if agents.iter().any(|a| a.name == name) {
            return Err(ApiError::InvalidRequest(format!(
                "agent '{}' already exists",
                name
            )));
        }
        agents.insert(0, agent.clone());
    }
    state.admin_store.save_agents_to_disk().await;

    if let Some(sink) = state.audit.as_ref() {
        sink.record(AuditEntry::now(
            actor.clone(),
            AuditKind::Mutation,
            "agent.create".to_string(),
            Some(agent.agent_id.clone()),
            serde_json::json!({
                "name": agent.name,
                "persona_file": req.persona_file,
                "workspace_mode": req.workspace_mode,
                "default_skills": req.default_skills,
            }),
            AuditResult::Success,
        ))
        .await;
    }

    info!(name = %name, actor = %actor, "agent created via admin console");
    Ok((axum::http::StatusCode::CREATED, Json(agent)))
}

/// PATCH /api/v1/agents/:id — partial update of an admin-store agent row.
///
/// Accepts any subset of `description`, `trust` (alias `trust_level`),
/// `status`. Validates trust against `{high, medium, low}`; invalid trust
/// → 400. Unknown agent id → 404. Persists to `agents.json` on success.
async fn update_agent(
    State(state): State<Arc<AppState>>,
    axum::extract::Extension(claims): axum::extract::Extension<Claims>,
    Path(id): Path<String>,
    Json(req): Json<UpdateAgentRequest>,
) -> Result<Json<crate::admin_store::AdminAgent>, ApiError> {
    let new_trust = match req.trust.as_deref() {
        Some(raw) => Some(normalize_trust_level(raw)?),
        None => None,
    };

    let updated = {
        let mut agents = state.admin_store.agents.write().await;
        let entry = agents
            .iter_mut()
            .find(|a| a.agent_id == id || a.name == id)
            .ok_or_else(|| ApiError::NotFound(format!("agent '{}' not found", id)))?;
        if let Some(desc) = req.description.as_ref() {
            entry.description = desc.clone();
        }
        if let Some(trust) = new_trust {
            entry.trust_level = trust;
        }
        if let Some(status) = req.status.as_ref() {
            entry.status = status.clone();
        }
        entry.clone()
    };
    state.admin_store.save_agents_to_disk().await;

    if let Some(sink) = state.audit.as_ref() {
        sink.record(AuditEntry::now(
            claims.sub.as_str().to_string(),
            AuditKind::Mutation,
            "agent.update".to_string(),
            Some(updated.agent_id.clone()),
            serde_json::json!({
                "description": req.description,
                "trust": req.trust,
                "status": req.status,
            }),
            AuditResult::Success,
        ))
        .await;
    }

    info!(agent_id = %updated.agent_id, "agent updated via admin console");
    Ok(Json(updated))
}

/// DELETE /api/v1/agents/:id — hard-remove from admin_store and persist.
async fn delete_agent(
    State(state): State<Arc<AppState>>,
    axum::extract::Extension(claims): axum::extract::Extension<Claims>,
    Path(id): Path<String>,
) -> Result<axum::http::StatusCode, ApiError> {
    let removed = {
        let mut agents = state.admin_store.agents.write().await;
        let before = agents.len();
        agents.retain(|a| a.agent_id != id && a.name != id);
        before != agents.len()
    };
    if !removed {
        return Err(ApiError::NotFound(format!("agent '{}' not found", id)));
    }
    state.admin_store.save_agents_to_disk().await;

    if let Some(sink) = state.audit.as_ref() {
        sink.record(AuditEntry::now(
            claims.sub.as_str().to_string(),
            AuditKind::Mutation,
            "agent.delete".to_string(),
            Some(id.clone()),
            serde_json::json!({ "agent_id": id }),
            AuditResult::Success,
        ))
        .await;
    }

    info!(agent_id = %id, "agent deleted via admin console");
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// Query string for `GET /api/v1/agents/:id/digest`.
#[derive(Debug, Deserialize)]
pub struct DigestQuery {
    /// Lookback window in days. Default 30, capped at 365.
    pub days: Option<u32>,
}

/// GET /api/v1/agents/:id/digest?days=30 — return recent `DailyDigestEntry`s.
///
/// Sprint 9 L9: reads from `state.digest_repo` (file-backed placeholder).
/// Never returns a 500 for "no digests yet"; unknown agents return `[]`.
async fn list_agent_digests(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(query): Query<DigestQuery>,
) -> Result<impl IntoResponse, ApiError> {
    debug!(agent_id = %id, days = ?query.days, "listing agent digests");

    let agent_id = AgentId::from_string(id.clone())
        .map_err(|e| ApiError::InvalidRequest(format!("Invalid agent id: {}", e)))?;

    let days = query.days.unwrap_or(30).clamp(1, 365) as i64;
    let since = chrono::Utc::now() - chrono::Duration::days(days);

    let entries = state
        .digest_repo
        .list_for_agent(&agent_id, since)
        .await
        .map_err(|e| ApiError::InternalError(format!("digest lookup failed: {}", e)))?;

    info!(
        agent_id = %id,
        count = entries.len(),
        days,
        "returned agent digests"
    );
    Ok(Json(serde_json::json!({
        "agent_id": id,
        "days": days,
        "count": entries.len(),
        "entries": entries,
    })))
}

/// GET /api/v1/agents - 列出所有已注册的 agents
///
/// Sprint 8 Phase A: when the agent_runtime has zero registered configs
/// and the admin_store has seeded agents, return the seeded admin view
/// so the UI shows content. This keeps MOCK content out of the frontend
/// without blocking the real runtime path for production deploys.
async fn list_agents(State(state): State<Arc<AppState>>) -> Result<impl IntoResponse, ApiError> {
    debug!("Listing all registered agents");

    // 从 AgentRuntime 读取真实已注册 Agent 配置
    let agent_configs = state.agent_runtime.list_registered_configs().await;
    let statuses = state.agent_status_store.read().await;

    let mut agents: Vec<AgentInfo> = agent_configs
        .into_iter()
        .map(|cfg| {
            let id_str = cfg.agent_id.as_str().to_string();
            let status = statuses
                .get(&id_str)
                .cloned()
                .unwrap_or(AgentExecutionStatus::Idle);

            AgentInfo {
                id: id_str.clone(),
                name: cfg.name,
                description: cfg.description,
                status,
            }
        })
        .collect();

    // 补充只有状态但未注册配置的 Agent（例如历史执行残留）
    for (agent_id, status) in statuses.iter() {
        if !agents.iter().any(|agent| agent.id == *agent_id) {
            agents.push(AgentInfo {
                id: agent_id.clone(),
                name: agent_id.clone(),
                description: "Agent status tracked without runtime config".to_string(),
                status: status.clone(),
            });
        }
    }

    // Fall back to admin_store seed content when nothing real is registered.
    if agents.is_empty() {
        let seeded = state.admin_store.agents.read().await;
        let seeded_list: Vec<serde_json::Value> = seeded
            .iter()
            .map(|a| serde_json::to_value(a).unwrap())
            .collect();
        info!("Returned {} seeded admin agents", seeded_list.len());
        return Ok(Json(serde_json::Value::Array(seeded_list)));
    }

    let agents_json: Vec<serde_json::Value> = agents
        .into_iter()
        .map(|a| serde_json::to_value(a).unwrap())
        .collect();
    info!("Returned {} agents", agents_json.len());
    Ok(Json(serde_json::Value::Array(agents_json)))
}

/// POST /api/v1/agents/:name/invoke - 调用 agent
async fn invoke_agent(
    State(state): State<Arc<AppState>>,
    axum::extract::Extension(claims): axum::extract::Extension<Claims>,
    Path(name): Path<String>,
    Json(req): Json<InvokeAgentRequest>,
) -> Result<impl IntoResponse, ApiError> {
    info!("Invoking agent: name={}", name);

    // 更新状态为 Running
    {
        let mut statuses = state.agent_status_store.write().await;
        statuses.insert(name.clone(), AgentExecutionStatus::Running);
    }

    // 创建 AgentId
    let agent_id = AgentId::from_string(name.clone())
        .map_err(|e| ApiError::InvalidRequest(format!("Invalid agent name: {}", e)))?;

    // 若未注册则先注册配置，已注册则复用现有配置
    if state.agent_runtime.load_config(&agent_id).await.is_err() {
        let config = AgentConfig::new(agent_id.clone(), &name, "API-invoked agent");
        state
            .agent_runtime
            .register(config)
            .await
            .map_err(|e| ApiError::InternalError(format!("Failed to register agent: {}", e)))?;
    }

    // 创建执行请求
    let mut agent_request = AgentRequest::new(agent_id.clone(), &req.input);
    if let Some(context) = req.context.as_object() {
        for (key, value) in context {
            agent_request = agent_request.with_context(key.clone(), value.clone());
        }
    }

    // Sprint 8 Phase A — record an AdminExecution for the admin console.
    let exec_id = format!("ex_{}", uuid::Uuid::new_v4().simple());
    {
        let mut execs = state.admin_store.executions.write().await;
        execs.insert(
            0,
            AdminExecution {
                execution_id: exec_id.clone(),
                agent: name.clone(),
                capability: "agent.invoke".to_string(),
                status: "running".to_string(),
                risk_level: "medium".to_string(),
                started_at: chrono::Utc::now().to_rfc3339(),
                duration: "00:00".to_string(),
            },
        );
    }
    let _ = state.admin_event_bus.send(AdminEvent::ExecutionStarted {
        execution_id: exec_id.clone(),
        agent: name.clone(),
        capability: "agent.invoke".to_string(),
    });

    // Sprint 18 W3 — replace the stub MinimalAgentRuntime.execute (which
    // returned `agent X processed input: …` no matter what) with a real
    // LLM round-trip carrying the BuiltinToolRegistry tool palette. The
    // call now exposes the same agentic surface as POST /v1/chat/completions
    // — a single iteration here is enough to produce useful output;
    // multi-step orchestration still belongs in the streaming chat
    // endpoint.
    let result: cyberclaw_agent_runtime::error::AgentRuntimeResult<AgentResponse> = (async {
        let tools: Vec<ToolDefinition> = BuiltinToolRegistry::with_defaults()
            .get_facades(&ToolsetConfig::default_config())
            .iter()
            .map(|f: &CapabilityFacade| ToolDefinition {
                tool_type: "function".to_string(),
                function: FunctionDefinition {
                    name: f.name.clone(),
                    description: f.description.clone(),
                    parameters: f
                        .input_schema
                        .clone()
                        .unwrap_or_else(|| serde_json::json!({"type":"object","properties":{}})),
                },
                cache_control: None,
            })
            .collect();

        let model = std::env::var("LLM_DEFAULT_MODEL").unwrap_or_else(|_| "gpt-4".to_string());
        // 2026-05-17: agent test path was bypassing constitution — only had a 3-line
        // identity string with no IRON LAWs (governance / fabrication / interrogation
        // bias). Prefix the full constitution so every user-facing chat path is
        // governed uniformly; append agent identity AFTER so agent specificity is
        // preserved but cannot dilute the IRON LAWs.
        let core_prompt = cyberclaw_agent_runtime::constitution::cyberclaw_constitution_text(
            cyberclaw_agent_runtime::constitution::ConstitutionProfile::SkillFirst,
        );
        let system_prompt = format!(
            "{core_prompt}\n\n<AgentIdentity>\nYou are agent '{name}'. Use the available tools when they help complete the task. Respond concisely.\n</AgentIdentity>"
        );

        let llm_req = ChatRequest {
            model,
            messages: vec![Message::system(&system_prompt), Message::user(&req.input)],
            tools: if tools.is_empty() { None } else { Some(tools) },
            ..Default::default()
        };

        let response = state
            .llm_client
            .chat_completion(llm_req)
            .await
            .map_err(|e| anyhow::anyhow!("LLM call failed: {e}"))?;

        let choice = response
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("LLM returned no choices"))?;
        let msg = choice.message;
        let mut output = msg.content;
        if let Some(calls) = msg.tool_calls {
            if !calls.is_empty() {
                let names: Vec<&str> = calls.iter().map(|c| c.function.name.as_str()).collect();
                if !output.is_empty() {
                    output.push_str("\n\n");
                }
                output.push_str(&format!(
                    "[agent emitted {} tool_call(s): {}]",
                    calls.len(),
                    names.join(", ")
                ));
            }
        }

        let mut resp = AgentResponse::ok(agent_id.clone(), output);
        resp.metadata
            .insert("agent_name".to_string(), serde_json::json!(name));
        resp.metadata
            .insert("model".to_string(), serde_json::json!(response.model));
        if let Some(usage) = response.usage {
            resp.metadata
                .insert("usage".to_string(), serde_json::json!(usage));
        }
        Ok::<AgentResponse, anyhow::Error>(resp)
    })
    .await
    .map_err(|e: anyhow::Error| {
        error!("invoke_agent LLM execution failed: {}", e);
        cyberclaw_agent_runtime::error::AgentRuntimeError::Internal(format!(
            "LLM execution failed: {}",
            e
        ))
    });

    // Keep the AgentRequest construction so the legacy runtime path
    // (registration + bookkeeping) still runs as before.
    let _legacy_request = agent_request;

    // 更新状态
    let mut statuses = state.agent_status_store.write().await;
    let final_status = if result.is_ok() {
        AgentExecutionStatus::Completed
    } else {
        AgentExecutionStatus::Failed
    };
    statuses.insert(name.clone(), final_status);

    // Update the AdminExecution we just pushed.
    {
        let mut execs = state.admin_store.executions.write().await;
        if let Some(entry) = execs.iter_mut().find(|e| e.execution_id == exec_id) {
            entry.status = if result.is_ok() {
                "done".to_string()
            } else {
                "failed".to_string()
            };
        }
    }
    let _ = state.admin_event_bus.send(AdminEvent::ExecutionCompleted {
        execution_id: exec_id.clone(),
        status: if result.is_ok() {
            "done".into()
        } else {
            "failed".into()
        },
    });

    let audit_result = match &result {
        Ok(_) => AuditResult::Success,
        Err(e) => AuditResult::Failure {
            reason: e.to_string(),
        },
    };
    if let Some(sink) = state.audit.as_ref() {
        sink.record(AuditEntry::now(
            claims.sub.as_str().to_string(),
            AuditKind::Mutation,
            "agent.invoke".to_string(),
            Some(format!("agent:{}", name)),
            serde_json::json!({ "execution_id": exec_id }),
            audit_result,
        ))
        .await;
    }

    match result {
        Ok(response) => {
            info!("Agent execution succeeded: name={}", name);
            let api_response: InvokeAgentResponse = response.into();
            Ok(Json(api_response))
        }
        Err(e) => {
            error!("Agent execution failed: name={}, error={}", name, e);
            Err(ApiError::InternalError(format!(
                "Agent execution failed: {}",
                e
            )))
        }
    }
}

/// GET /api/v1/agents/:name/status - 查询 agent 状态
///
/// Returns 404 when the agent is not found in either the status store,
/// the agent runtime configs, or the admin_store — so callers can distinguish
/// "agent exists but is idle" from "agent does not exist".
async fn get_agent_status(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    debug!("Getting agent status: name={}", name);

    #[derive(Serialize)]
    struct AgentStatusResponse {
        agent_id: String,
        status: AgentExecutionStatus,
    }

    // If the agent has a recorded runtime status it definitely exists.
    let recorded = {
        let statuses = state.agent_status_store.read().await;
        statuses.get(&name).cloned()
    };
    if let Some(status) = recorded {
        return Ok(Json(AgentStatusResponse {
            agent_id: name,
            status,
        }));
    }

    // Check the runtime config registry.
    let agent_id_res = AgentId::from_string(name.clone());
    let in_runtime = if let Ok(ref agent_id) = agent_id_res {
        state.agent_runtime.load_config(agent_id).await.is_ok()
    } else {
        false
    };

    // Check the admin_store seed list.
    let in_admin_store = {
        let agents = state.admin_store.agents.read().await;
        agents.iter().any(|a| a.name == name || a.agent_id == name)
    };

    if !in_runtime && !in_admin_store {
        return Err(ApiError::NotFound(format!("agent '{}' not found", name)));
    }

    Ok(Json(AgentStatusResponse {
        agent_id: name,
        status: AgentExecutionStatus::Idle,
    }))
}

// ===========================================================================
// v1.1 — GET /api/v1/agents/:id/effective-prompt (operator transparency)
// ===========================================================================

#[derive(serde::Serialize)]
struct EffectivePromptResponse {
    /// The agent id this prompt applies to.
    agent_id: String,
    /// Profile used (SkillFirst vs Generic) — see ConstitutionProfile.
    profile: &'static str,
    /// Schema version of the constitution text.
    version: &'static str,
    /// Number of <Section> blocks emitted by the renderer.
    section_count: usize,
    /// The fully assembled system prompt that would be sent to the LLM for
    /// this agent. Includes the constitutional core + (when present)
    /// universal-resilience body.
    prompt: String,
    /// Length in chars — a quick sanity / token-budget indicator.
    char_length: usize,
}

/// `GET /api/v1/agents/:id/effective-prompt`
///
/// v1.1 operator-transparency endpoint. Returns the exact system prompt that
/// would be sent to the LLM if this agent were invoked via the rich
/// chat_handler path. Lets operators inspect the constitutional layer
/// without reading source, and diff prompt drift between deploys.
async fn get_effective_prompt(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<EffectivePromptResponse>, ApiError> {
    use cyberclaw_agent_runtime::constitution::{cyberclaw_constitution_text, ConstitutionProfile};

    // Build the prompt with the same rules as chat_handler.rs:default_system_prompt.
    let core = cyberclaw_constitution_text(ConstitutionProfile::SkillFirst);
    let resilience_body = {
        let hub = state.skill_hub.read().await;
        let path = hub
            .base_dir()
            .join("installed")
            .join("universal-resilience")
            .join("SKILL.md");
        std::fs::read_to_string(&path).ok()
    };
    let prompt = match resilience_body {
        Some(body) => format!(
            "{core}\n\n<default_skill name=\"universal-resilience\">\n{body}\n</default_skill>"
        ),
        None => core,
    };

    let section_count = prompt.matches("\n  <").count();

    Ok(Json(EffectivePromptResponse {
        agent_id: id,
        profile: "SkillFirst",
        version: "v1.0",
        section_count,
        char_length: prompt.chars().count(),
        prompt,
    }))
}
