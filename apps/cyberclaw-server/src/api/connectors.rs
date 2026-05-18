//! Connectors API — surface the live [`ConnectorRegistry`] to the admin UI.
//!
//! Sprint 8 Lane 1. Replaces the admin SPA's hard-coded MOCK list with
//! the connectors actually registered in `cyberclaw-connectors`.
//!
//! # Fallback behaviour
//!
//! When the registry is empty (pre-seed / bare dev boot), this route
//! returns the ship list — the connector ids that actually exist in
//! `cyberclaw-connectors` and ship with the server binary. This keeps
//! the SPA populated without creating MOCK drift.

use std::sync::Arc;

use axum::{extract::State, routing::get, Json, Router};
use serde::Serialize;
use tracing::info;

use cyberclaw_core::capability::RiskLevel;
use cyberclaw_core::prelude::ConnectorRuntime;

use crate::state::AppState;

/// Admin-visible projection of a registered connector.
#[derive(Debug, Clone, Serialize)]
pub struct ConnectorInfo {
    pub connector_id: String,
    pub name: String,
    /// `"Native" | "Remote" | "Process" | "Container"`
    pub runtime: String,
    pub description: String,
    pub capability_count: u32,
    /// Worst risk across this connector's capabilities (`low|medium|high|critical`).
    pub risk_level: String,
    /// `"registered" | "enabled" | "degraded"` — registry sources always
    /// report `"registered"`; the fallback ship list reports `"enabled"`.
    pub status: String,
}

/// Response wrapper for the list endpoint.
#[derive(Debug, Serialize)]
pub struct ConnectorListResponse {
    pub connectors: Vec<ConnectorInfo>,
    pub total: usize,
}

/// Build the `/api/v1/connectors` router.
pub fn create_connectors_router() -> Router<Arc<AppState>> {
    Router::new().route("/api/v1/connectors", get(list_connectors))
}

/// `GET /api/v1/connectors` — list all known connectors.
///
/// Prefers the live [`ConnectorRegistry`]; falls back to the ship list
/// when the registry is empty.
async fn list_connectors(State(state): State<Arc<AppState>>) -> Json<ConnectorListResponse> {
    let ids = state.connector_registry.list_connectors();

    if ids.is_empty() {
        let fallback = ship_list();
        let total = fallback.len();
        info!(
            "connectors: registry empty, returning {} ship list entries",
            total
        );
        return Json(ConnectorListResponse {
            connectors: fallback,
            total,
        });
    }

    let mut out: Vec<ConnectorInfo> = Vec::with_capacity(ids.len());
    for id in ids {
        let Some(entry) = state.connector_registry.get_entry(&id) else {
            continue;
        };
        let worst = entry
            .capabilities
            .iter()
            .map(|c| c.risk)
            .max()
            .unwrap_or(RiskLevel::Low);
        let description = first_description(&entry.capabilities)
            .unwrap_or_else(|| format!("{} connector", id.as_str()));
        out.push(ConnectorInfo {
            connector_id: id.as_str().to_string(),
            name: id.as_str().to_string(),
            runtime: runtime_label(&entry.runtime).to_string(),
            description,
            capability_count: entry.capabilities.len() as u32,
            risk_level: risk_label(&worst).to_string(),
            status: "registered".to_string(),
        });
    }
    out.sort_by(|a, b| a.connector_id.cmp(&b.connector_id));
    let total = out.len();
    info!("connectors: registry returned {} entries", total);
    Json(ConnectorListResponse {
        connectors: out,
        total,
    })
}

fn first_description(caps: &[cyberclaw_core::prelude::CapabilityContract]) -> Option<String> {
    caps.iter()
        .filter_map(|c| c.description.clone())
        .find(|s| !s.is_empty())
}

fn runtime_label(r: &ConnectorRuntime) -> &'static str {
    match r {
        ConnectorRuntime::Native => "Native",
        ConnectorRuntime::Remote => "Remote",
        ConnectorRuntime::Process => "Process",
        ConnectorRuntime::Container => "Container",
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

/// The ship list — connector ids that ACTUALLY ship in
/// `cyberclaw-connectors` and are registered at server startup.
///
/// Used when the registry is empty (bare boot, tests). Each entry
/// describes one of the local-workspace connectors plus the sandbox.
fn ship_list() -> Vec<ConnectorInfo> {
    let seed: &[(&str, &str, RiskLevel)] = &[
        (
            "sandbox",
            "Isolated sandbox connector for dispatching mutation plans",
            RiskLevel::Medium,
        ),
        (
            "local.fs",
            "Local filesystem: read / write / edit / list / search files",
            RiskLevel::Medium,
        ),
        (
            "local.cmd",
            "Local shell: execute bash / sh commands in the workspace",
            RiskLevel::High,
        ),
        (
            "local.search",
            "Local content search over the workspace",
            RiskLevel::Low,
        ),
        (
            "local.lsp",
            "Local LSP-backed code intelligence (symbols, diagnostics)",
            RiskLevel::Low,
        ),
        (
            "local.task",
            "Local task primitives (spawn, status, cancel)",
            RiskLevel::Medium,
        ),
        (
            "local.memory",
            "Local agent memory (session / agent / global scopes)",
            RiskLevel::Low,
        ),
        (
            "local.workdir_checkpoint",
            "Local workdir checkpoint / restore",
            RiskLevel::Medium,
        ),
        (
            "local.host",
            "Local host introspection (env, uname, timezone)",
            RiskLevel::Low,
        ),
    ];

    seed.iter()
        .map(|(id, desc, risk)| ConnectorInfo {
            connector_id: (*id).to_string(),
            name: (*id).to_string(),
            runtime: "Native".to_string(),
            description: (*desc).to_string(),
            capability_count: 0,
            risk_level: risk_label(risk).to_string(),
            status: "enabled".to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ship_list_contains_expected_ids() {
        let list = ship_list();
        let ids: Vec<&str> = list.iter().map(|c| c.connector_id.as_str()).collect();
        assert!(ids.contains(&"sandbox"));
        assert!(ids.contains(&"local.fs"));
        assert!(ids.contains(&"local.cmd"));
        assert!(ids.contains(&"local.host"));
        assert_eq!(list.len(), 9);
    }

    #[test]
    fn runtime_label_roundtrip() {
        assert_eq!(runtime_label(&ConnectorRuntime::Native), "Native");
        assert_eq!(runtime_label(&ConnectorRuntime::Remote), "Remote");
        assert_eq!(runtime_label(&ConnectorRuntime::Process), "Process");
        assert_eq!(runtime_label(&ConnectorRuntime::Container), "Container");
    }

    #[tokio::test]
    async fn connectors_list_returns_known_set() {
        use crate::api::test_helpers::build_test_state;
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt;

        let state = build_test_state();
        let app = create_connectors_router().with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/connectors")
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
        assert!(
            total >= 9,
            "empty-registry fallback must expose the ship list"
        );
        let arr = json["connectors"].as_array().unwrap();
        assert!(arr
            .iter()
            .any(|c| c["connector_id"].as_str() == Some("sandbox")));
    }
}
