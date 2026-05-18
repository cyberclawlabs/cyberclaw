//! Capabilities API - 能力查询接口

use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::info;

use cyberclaw_core::capability::CapabilityEffect;
use cyberclaw_core::ids::CapabilityId;

use crate::error::ApiError;
use crate::state::AppState;

/// 能力列表响应
#[derive(Debug, Serialize)]
pub struct CapabilityListResponse {
    pub capabilities: Vec<CapabilitySummary>,
    pub total: usize,
}

/// 能力摘要
#[derive(Debug, Serialize)]
pub struct CapabilitySummary {
    pub id: String,
    pub title: String,
    pub connector_id: String,
    pub risk_level: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// 能力发现响应（包含 Contract 元数据）
#[derive(Debug, Serialize)]
pub struct CapabilityDiscoveryResponse {
    pub capabilities: Vec<CapabilityDiscoveryItem>,
    pub total: usize,
}

/// 能力发现条目（含行为合约信息）
#[derive(Debug, Serialize)]
pub struct CapabilityDiscoveryItem {
    pub id: String,
    pub title: String,
    pub connector_id: String,
    pub risk_level: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 是否只读
    pub is_read_only: bool,
    /// 是否具有破坏性
    pub is_destructive: bool,
    /// 是否并发安全
    pub is_concurrency_safe: bool,
    /// 副作用类型
    pub effects: Vec<String>,
}

/// 创建 Capabilities API 路由
pub fn create_capabilities_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/capabilities", get(list_capabilities))
        .route("/api/v1/capabilities/discover", get(discover_capabilities))
        .route(
            "/api/v1/capabilities/discover_for_goal",
            post(discover_for_goal),
        )
        .route("/api/v1/capabilities/:id", get(get_capability))
        // F6 — McpToolBridge admin surface.
        .route("/api/v1/mcp/bridge_tool", post(mcp_bridge_tool))
}

/// GET /api/v1/capabilities - 列出所有能力
async fn list_capabilities(
    State(state): State<Arc<AppState>>,
) -> Result<Json<CapabilityListResponse>, ApiError> {
    info!("Listing capabilities");

    let capabilities = state.connector_registry.list_capabilities();

    let summaries: Vec<CapabilitySummary> = capabilities
        .iter()
        .filter_map(|(connector_id, capability_id)| {
            state
                .connector_registry
                .get_capability(capability_id)
                .map(|(_, contract)| CapabilitySummary {
                    id: capability_id.as_str().to_string(),
                    title: contract.title.clone(),
                    connector_id: connector_id.as_str().to_string(),
                    risk_level: format!("{:?}", contract.risk),
                    description: contract.description.clone(),
                })
        })
        .collect();

    Ok(Json(CapabilityListResponse {
        total: summaries.len(),
        capabilities: summaries,
    }))
}

/// GET /api/v1/capabilities/:id - 获取能力详情
async fn get_capability(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    info!("Getting capability: {}", id);

    let capability_id = CapabilityId::from_string(id)
        .map_err(|e| ApiError::InvalidInput(format!("Invalid capability ID: {}", e)))?;

    let (connector_id, contract) = state
        .connector_registry
        .get_capability(&capability_id)
        .ok_or_else(|| ApiError::NotFound(format!("Capability {} not found", capability_id)))?;

    Ok(Json(serde_json::json!({
        "id": capability_id.as_str(),
        "connector_id": connector_id.as_str(),
        "title": contract.title,
        "description": contract.description,
        "risk_level": format!("{:?}", contract.risk),
        "effects": contract.effects,
        "input_schema": contract.input_schema,
        "output_schema": contract.output_schema,
        "placement": contract.placement,
        "timeouts": contract.timeouts,
    })))
}

/// GET /api/v1/capabilities/discover - 发现能力及其 Contract 元数据
///
/// 返回所有已注册能力的详细信息，包括行为合约（is_read_only、is_destructive 等）。
///
/// Sprint 8 Phase A: when the `ConnectorRegistry` has zero registered
/// capabilities, returns a 12-entry sample grouping (grouped by
/// connector) sourced from [`crate::admin_store::sample_capabilities`]
/// so the admin SPA's "Capabilities" tab is populated out of the box.
/// Optional query for GET /api/v1/capabilities/discover.
#[derive(Debug, Deserialize)]
pub struct DiscoverQuery {
    /// Optional NL goal to filter / rank capabilities.
    /// Case-insensitive keyword match against capability id, title, and
    /// description. When empty, returns all capabilities (legacy behavior).
    pub goal: Option<String>,
}

async fn discover_capabilities(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(query): axum::extract::Query<DiscoverQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    info!(
        "Discovering capabilities with contract metadata (goal={:?})",
        query.goal
    );

    let capabilities = state.connector_registry.list_capabilities();

    // Empty-registry fallback.
    if capabilities.is_empty() {
        let sample = crate::admin_store::sample_capabilities();
        let connector_count = state.connector_registry.list_connectors().len();
        let items: Vec<serde_json::Value> = sample
            .iter()
            .flat_map(|(connector_id, caps)| {
                let connector_id = connector_id.clone();
                caps.iter().map(move |c| {
                    let mut obj = c.clone();
                    if let Some(map) = obj.as_object_mut() {
                        map.insert(
                            "connector_id".to_string(),
                            serde_json::Value::String(connector_id.clone()),
                        );
                    }
                    obj
                })
            })
            .collect();
        let groups: Vec<serde_json::Value> = sample
            .iter()
            .map(|(connector_id, caps)| {
                serde_json::json!({
                    "connector_id": connector_id,
                    "name": connector_id,
                    "capabilities": caps,
                })
            })
            .collect();
        return Ok(Json(serde_json::json!({
            "total": items.len(),
            "connector_count": connector_count,
            "capabilities": items,
            "groups": groups,
        })));
    }

    let mut items: Vec<CapabilityDiscoveryItem> = capabilities
        .iter()
        .filter_map(|(connector_id, capability_id)| {
            state
                .connector_registry
                .get_capability(capability_id)
                .map(|(_, contract)| {
                    // 从 effects 推导行为合约属性
                    let has_write = contract
                        .effects
                        .iter()
                        .any(|e| matches!(e, CapabilityEffect::Write));
                    let has_execute = contract
                        .effects
                        .iter()
                        .any(|e| matches!(e, CapabilityEffect::Execute));
                    let is_read_only = contract
                        .effects
                        .iter()
                        .all(|e| matches!(e, CapabilityEffect::Read));

                    // 破坏性：有 Write 或 Execute 效果且风险级别为 High 或 Critical
                    let is_destructive = (has_write || has_execute)
                        && matches!(
                            contract.risk,
                            cyberclaw_core::capability::RiskLevel::High
                                | cyberclaw_core::capability::RiskLevel::Critical
                        );

                    // 并发安全：只读能力通常是并发安全的
                    let is_concurrency_safe = is_read_only;

                    let effects: Vec<String> = contract
                        .effects
                        .iter()
                        .map(|e| format!("{:?}", e))
                        .collect();

                    CapabilityDiscoveryItem {
                        id: capability_id.as_str().to_string(),
                        title: contract.title.clone(),
                        connector_id: connector_id.as_str().to_string(),
                        risk_level: format!("{:?}", contract.risk),
                        description: contract.description.clone(),
                        is_read_only,
                        is_destructive,
                        is_concurrency_safe,
                        effects,
                    }
                })
        })
        .collect();

    // Goal-aware ranking: when ?goal=X is supplied, filter items whose
    // id/title/description case-insensitively contain ANY whitespace-
    // separated term of the goal. Matched items are returned ordered by
    // term-hit count (more matches = higher rank). Empty goal returns
    // all (legacy behavior).
    if let Some(goal) = query
        .goal
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let terms: Vec<String> = goal
            .to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|s| s.len() >= 2)
            .map(str::to_string)
            .collect();
        if !terms.is_empty() {
            let scored: Vec<(usize, CapabilityDiscoveryItem)> = items
                .into_iter()
                .filter_map(|it| {
                    let haystack = format!(
                        "{} {} {}",
                        it.id.to_lowercase(),
                        it.title.to_lowercase(),
                        it.description.as_deref().unwrap_or("").to_lowercase()
                    );
                    let hits = terms
                        .iter()
                        .filter(|t| haystack.contains(t.as_str()))
                        .count();
                    if hits > 0 {
                        Some((hits, it))
                    } else {
                        None
                    }
                })
                .collect();
            let mut scored = scored;
            scored.sort_by_key(|x| std::cmp::Reverse(x.0)); // descending hit count
            items = scored.into_iter().map(|(_, it)| it).collect();
        }
    }

    Ok(Json(
        serde_json::to_value(CapabilityDiscoveryResponse {
            total: items.len(),
            capabilities: items,
        })
        .unwrap(),
    ))
}

// ---------------------------------------------------------------------------
// F3 — discover_for_goal: real CapabilityDiscovery wired to production
// ---------------------------------------------------------------------------

/// Request body for `POST /api/v1/capabilities/discover_for_goal`.
///
/// Matches `cyberclaw_control_plane::capability_discovery::DiscoveryQuery`
/// (snake_case JSON). `deliverable_kind` is required; `modalities` and
/// `search_terms` default to empty.
#[derive(Debug, Deserialize)]
pub struct DiscoverForGoalRequest {
    pub deliverable_kind: String,
    #[serde(default)]
    pub search_terms: Vec<String>,
    /// When `true`, also run async segments 4 (SkillHub remote) + 5
    /// (provider modality probe). Defaults to `false` for fast local-only
    /// discovery suitable for synchronous admin UI requests.
    #[serde(default)]
    pub include_remote: bool,
}

/// POST /api/v1/capabilities/discover_for_goal
///
/// F3 — first production caller of
/// [`cyberclaw_control_plane::capability_discovery::CapabilityDiscovery`].
/// Prior to this commit the type existed in control-plane with 12+ unit
/// tests but had **zero production references**: the existing GET
/// `/api/v1/capabilities/discover` walked `connector_registry` directly.
///
/// Returns a [`DiscoveryResult`]-shaped JSON: `native` (connector caps),
/// `installed_skills` (skill IDs from the SkillHub index), `cmd_runtime`
/// (binaries on PATH), and the `*_pending` flags advertising which async
/// segments did not run.
async fn discover_for_goal(
    State(state): State<Arc<AppState>>,
    Json(req): Json<DiscoverForGoalRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    use cyberclaw_control_plane::capability_discovery::DiscoveryQuery;

    info!(
        deliverable_kind = %req.deliverable_kind,
        terms = ?req.search_terms,
        include_remote = req.include_remote,
        "CapabilityDiscovery: discover_for_goal"
    );

    let query = DiscoveryQuery::new(req.deliverable_kind.clone())
        .with_search_terms(req.search_terms.clone());

    let local = state.capability_discovery.discover_local(&query);

    if !req.include_remote {
        return Ok(Json(serde_json::json!({
            "deliverable_kind": req.deliverable_kind,
            "native": local
                .native
                .iter()
                .map(|(c, k)| {
                    serde_json::json!({
                        "connector_id": c.as_str(),
                        "capability_id": k.as_str(),
                    })
                })
                .collect::<Vec<_>>(),
            "installed_skills": local
                .installed_skills
                .iter()
                .map(|s| s.as_str().to_string())
                .collect::<Vec<_>>(),
            "cmd_runtime": local.cmd_runtime,
            "skill_hub_pending": local.skill_hub_pending,
            "provider_modalities_pending": local.provider_modalities_pending,
            "request_pending": local.request_pending,
        })));
    }

    let remote = state.capability_discovery.discover_remote(&query).await;
    Ok(Json(serde_json::json!({
        "deliverable_kind": req.deliverable_kind,
        "native": local
            .native
            .iter()
            .map(|(c, k)| {
                serde_json::json!({
                    "connector_id": c.as_str(),
                    "capability_id": k.as_str(),
                })
            })
            .collect::<Vec<_>>(),
        "installed_skills": local
            .installed_skills
            .iter()
            .map(|s| s.as_str().to_string())
            .collect::<Vec<_>>(),
        "cmd_runtime": local.cmd_runtime,
        "skill_hub_hits": remote
            .skill_hub_hits
            .iter()
            .map(|h| {
                serde_json::json!({
                    "name": h.name,
                    "version": h.version,
                    "description": h.description,
                    "source": h.source,
                    "install_required": h.install_required,
                })
            })
            .collect::<Vec<_>>(),
        "provider_modalities": remote
            .provider_modalities
            .iter()
            .map(|m| {
                serde_json::json!({
                    "provider": m.provider,
                    "api": m.api,
                })
            })
            .collect::<Vec<_>>(),
        "request_id": remote.request_id,
    })))
}

// ---------------------------------------------------------------------------
// F6 — McpToolBridge: bridge an MCP server tool to a BridgedTool projection
// ---------------------------------------------------------------------------

/// Request body for `POST /api/v1/mcp/bridge_tool`.
#[derive(Debug, Deserialize)]
pub struct McpBridgeToolRequest {
    pub server_name: String,
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// POST /api/v1/mcp/bridge_tool
///
/// F6 — first production caller of
/// [`cyberclaw_connectors::mcp::tool_bridge::McpToolBridge`].
/// Prior to this commit the type existed in cyberclaw-connectors with 18+
/// unit tests but had **zero production references** — admins had no way
/// to convert an MCP server's tool descriptor into a CyberClaw
/// `BridgedTool` (namespaced + risk-classified) without writing custom
/// code.
///
/// Returns a JSON projection of the resulting `BridgedTool`. The risk
/// level is auto-classified from the tool name (CRITICAL_PATTERNS /
/// HIGH_PATTERNS / MEDIUM_PATTERNS / LOW_PATTERNS in
/// `tool_bridge.rs`). The capability_id format is
/// `connector:mcp:{server}:{tool}` which participates in the same
/// governance dangerous-pattern allowlist as first-party connector caps.
async fn mcp_bridge_tool(
    State(state): State<Arc<AppState>>,
    Json(req): Json<McpBridgeToolRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    use cyberclaw_connectors::mcp::McpTool;

    info!(
        server = %req.server_name,
        tool = %req.name,
        "McpToolBridge: bridging tool"
    );

    let mcp_tool = McpTool {
        name: req.name.clone(),
        description: req.description.clone(),
        input_schema: req.input_schema.clone(),
    };
    match state
        .mcp_tool_bridge
        .bridge_tool(&mcp_tool, &req.server_name)
    {
        Ok(bridged) => Ok(Json(serde_json::json!({
            "name": bridged.name,
            "description": bridged.description,
            "input_schema": bridged.input_schema,
            "server_name": bridged.server_name,
            "original_name": bridged.original_name,
            "connector_id": bridged.connector_id,
            "capability_id": bridged.capability_id,
            "risk_level": format!("{:?}", bridged.risk_level),
        }))),
        Err(e) => Err(ApiError::InvalidRequest(format!(
            "MCP tool bridge failed: {}",
            e
        ))),
    }
}
