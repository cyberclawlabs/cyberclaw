//! Admin MCP server hot-reload endpoints (BT-37 from Hermes benchmark).
//!
//! | Method | Path                                  | Purpose                                        |
//! |--------|---------------------------------------|------------------------------------------------|
//! | GET    | `/api/v1/admin/mcp/servers`           | List currently registered MCP connectors       |
//! | POST   | `/api/v1/admin/mcp/servers`           | Register a new MCP server (HTTP transport)     |
//! | DELETE | `/api/v1/admin/mcp/servers/:name`     | Unregister an MCP server                       |
//!
//! All routes require admin JWT. Underneath, register/unregister call into
//! `ConnectorRegistry::{register, unregister}`. The CLI surface lives at
//! `cyberclaw mcp {list,register,unregister}`.

use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete as delete_method, get, post},
    Json, Router,
};
use cyberclaw_connectors::mcp::{McpConnector, McpServerConfig, TransportConfig};
use cyberclaw_connectors::types::Connector;
use cyberclaw_core::ids::ConnectorId;
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct McpServerRow {
    pub name: String,
    pub connector_id: String,
    pub url: String,
    pub capabilities: u32,
}

#[derive(Debug, Deserialize)]
pub struct RegisterMcpRequest {
    pub name: String,
    /// MCP transport config (matches `cyberclaw_connectors::mcp::TransportConfig`
    /// JSON shape; the CLI sends `{ "Http": { "url": "...", "headers": {} } }`).
    pub transport: TransportConfig,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_timeout() -> u64 {
    30
}

#[derive(Debug, Serialize)]
pub struct RegisterMcpResponse {
    pub success: bool,
    pub connector_id: String,
    pub capabilities: u32,
}

/// `GET /api/v1/admin/mcp/servers` — list `mcp-*` connectors.
pub async fn list_servers(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<McpServerRow>>, ApiError> {
    let mut rows = Vec::new();
    for id in state.connector_registry.list_connectors() {
        let id_str = id.as_str();
        if !id_str.starts_with("mcp-") {
            continue;
        }
        let entry = match state.connector_registry.get_entry(&id) {
            Some(e) => e,
            None => continue,
        };
        // The connector id has shape "mcp-<name>"; strip the prefix.
        let name = id_str.strip_prefix("mcp-").unwrap_or(id_str).to_string();
        rows.push(McpServerRow {
            name,
            connector_id: id_str.to_string(),
            url: String::new(), // Transport config is private; expose only in registration response.
            capabilities: entry.capabilities.len() as u32,
        });
    }
    Ok(Json(rows))
}

/// `POST /api/v1/admin/mcp/servers` — register a new MCP server at runtime.
pub async fn register_server(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RegisterMcpRequest>,
) -> Result<Json<RegisterMcpResponse>, ApiError> {
    if req.name.is_empty() {
        return Err(ApiError::InvalidInput("name must not be empty".to_string()));
    }
    let connector_id_str = format!("mcp-{}", req.name);
    let connector_id = ConnectorId::from_string(connector_id_str.clone())
        .map_err(|e| ApiError::InvalidInput(format!("invalid connector id: {e}")))?;
    if state.connector_registry.get(&connector_id).is_some() {
        return Err(ApiError::InvalidInput(format!(
            "MCP server '{}' is already registered",
            req.name
        )));
    }

    let config = McpServerConfig {
        name: req.name.clone(),
        transport: req.transport,
        timeout: Duration::from_secs(req.timeout_secs),
        enable_cache: true,
    };

    // Construct the connector — this calls `discover_capabilities` which
    // hits the MCP server over the transport. If the server is unreachable
    // we surface that as a 4xx so the operator sees the error.
    let connector = McpConnector::new(config).await.map_err(|e| {
        ApiError::InvalidInput(format!(
            "failed to construct MCP connector for '{}': {e}",
            req.name
        ))
    })?;
    let cap_count = connector.capabilities().len() as u32;

    state
        .connector_registry
        .register(Arc::new(connector))
        .map_err(|e| ApiError::InternalError(format!("register: {e}")))?;

    Ok(Json(RegisterMcpResponse {
        success: true,
        connector_id: connector_id_str,
        capabilities: cap_count,
    }))
}

/// `DELETE /api/v1/admin/mcp/servers/:name` — unregister.
pub async fn unregister_server(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let connector_id_str = format!("mcp-{}", name);
    let connector_id = ConnectorId::from_string(connector_id_str)
        .map_err(|e| ApiError::InvalidInput(format!("invalid connector id: {e}")))?;
    state
        .connector_registry
        .unregister(&connector_id)
        .map_err(|e| ApiError::InvalidInput(format!("unregister failed: {e}")))?;
    Ok(StatusCode::NO_CONTENT)
}

/// Mount the BT-37 admin MCP routes onto a fresh `Router`.
pub fn create_admin_mcp_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/admin/mcp/servers", get(list_servers))
        .route("/api/v1/admin/mcp/servers", post(register_server))
        .route(
            "/api/v1/admin/mcp/servers/:name",
            delete_method(unregister_server),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    // Smoke test that proves the request structs deserialize from the
    // shape the CLI sends.
    #[test]
    fn register_request_deserializes_from_cli_shape() {
        // TransportConfig uses #[serde(tag = "type", rename_all = "lowercase")]
        // so the wire format is {"type":"http","url":"...","headers":{}}.
        let json = r#"{
            "name": "echo",
            "transport": { "type": "http", "url": "http://localhost:9001", "headers": {} },
            "timeout_secs": 30
        }"#;
        let req: RegisterMcpRequest = serde_json::from_str(json).expect("deserializes");
        assert_eq!(req.name, "echo");
        assert_eq!(req.timeout_secs, 30);
        match req.transport {
            TransportConfig::Http { url, .. } => assert_eq!(url, "http://localhost:9001"),
            _ => panic!("expected Http transport"),
        }
    }

    #[test]
    fn register_request_default_timeout_is_30() {
        let json = r#"{
            "name": "x",
            "transport": { "type": "http", "url": "http://x", "headers": {} }
        }"#;
        let req: RegisterMcpRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.timeout_secs, 30);
    }
}
