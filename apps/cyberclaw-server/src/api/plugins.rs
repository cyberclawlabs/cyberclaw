//! Platform Plugin manifest discovery.
//!
//! Scans `~/.cyberclaw/plugins/*/plugin.json` and returns a list of
//! [`PluginManifest`] records. Empty list when the directory is missing —
//! that is the normal state for a fresh install.
//!
//! # Routes
//!
//! | Route | Purpose |
//! |---|---|
//! | `GET /api/v1/plugins` | List installed platform-plugin manifests |

use std::sync::Arc;

use axum::{
    extract::State,
    routing::{get, post},
    Extension, Json, Router,
};
use serde::{Deserialize, Serialize};

use crate::audit::{AuditEntry, AuditKind, AuditResult};
use crate::error::ApiError;
use crate::middleware::auth::{require_admin, Claims};
use tracing::warn;

use crate::state::AppState;

/// Frontend-facing projection of a `plugin.json` manifest.
///
/// Mirrors the hermes `plugin.json` schema; all fields beyond `name` are
/// optional so manifests that omit them still parse cleanly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
    /// Human-readable display label (falls back to `name` when absent).
    pub label: Option<String>,
    pub description: Option<String>,
    /// Unicode icon or image path shown in the sidebar/table.
    pub icon: Option<String>,
    /// Route to mount inside the shell, e.g. `"/example"`.
    pub tab_path: Option<String>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// When `true` the plugin is not shown in the default listing.
    #[serde(default)]
    pub hidden: bool,
}

fn default_version() -> String {
    "0.0.0".to_string()
}

fn default_enabled() -> bool {
    true
}

/// `GET /api/v1/plugins` response envelope.
#[derive(Debug, Serialize)]
pub struct PluginsResponse {
    pub plugins: Vec<PluginManifest>,
    pub total: usize,
}

/// Build the `/api/v1/plugins` router.
pub fn create_plugins_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/plugins", get(list_plugins))
        .route("/api/v1/plugins/rescan", post(rescan_plugins))
        .route("/api/v1/plugins/:id/invoke", post(invoke_plugin))
}

/// `POST /api/v1/plugins/:id/invoke` — execute a registered platform plugin.
///
/// v0.3.0-plugins (sprint): exposes the existing PluginRegistry.execute()
/// path via HTTP. Plugins must:
///   1. Be compiled as a cdylib with the `Plugin` trait + plugin_constructor()
///      extern "C" function (see `cyberclaw-plugin-runtime::loader`)
///   2. Have a `plugin.json` manifest in `~/.cyberclaw/plugins/<name>/`
///   3. Be discovered + initialized at server startup
///
/// Returns:
///   200 + JSON output on successful execution
///   404 when plugin id is not found (most common state today — no
///       sample plugin .dylib has been compiled yet)
///   500 on plugin execution error
///
/// Audit: emits `plugin.invoke` (success) or `plugin.invoke:failed` on error.
async fn invoke_plugin(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(input): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims).await?;

    let actor = claims.sub.to_string();
    let registry = state.control_plane.plugin_registry();

    let result = registry.execute(&id, input.clone()).await;

    if let Some(sink) = state.audit.as_ref() {
        match &result {
            Ok(_) => {
                sink.record(AuditEntry::now(
                    actor.clone(),
                    AuditKind::Mutation,
                    "plugin.invoke".to_string(),
                    Some(format!("plugin:{id}")),
                    serde_json::json!({ "plugin_id": id, "input_keys":
                        input.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>())
                            .unwrap_or_default()
                    }),
                    AuditResult::Success,
                ))
                .await;
            }
            Err(e) => {
                sink.record(AuditEntry::now(
                    actor.clone(),
                    AuditKind::Mutation,
                    "plugin.invoke:failed".to_string(),
                    Some(format!("plugin:{id}")),
                    serde_json::json!({ "plugin_id": id, "error": e.to_string() }),
                    AuditResult::Failure {
                        reason: e.to_string(),
                    },
                ))
                .await;
            }
        }
    }

    match result {
        Ok(out) => Ok(Json(out)),
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("NotFound") || msg.contains("not found") {
                Err(ApiError::NotFound(format!(
                    "plugin '{id}' is not registered (no .dylib loaded — see /docs/plugins)"
                )))
            } else {
                Err(ApiError::InternalError(format!(
                    "plugin invoke failed: {msg}"
                )))
            }
        }
    }
}

/// `GET /api/v1/plugins` — scan `~/.cyberclaw/plugins/*/plugin.json`.
///
/// Returns 200 with an empty list when the plugins directory does not exist.
/// Individual manifests that fail to parse are skipped with a warning.
async fn list_plugins(_state: State<Arc<AppState>>) -> Json<PluginsResponse> {
    let plugins = discover_plugins();
    let total = plugins.len();
    Json(PluginsResponse { plugins, total })
}

/// `POST /api/v1/plugins/rescan` — explicit hot-reload trigger. Admin only.
///
/// Re-scans `~/.cyberclaw/plugins/*/plugin.json` (the same path `GET /plugins`
/// uses on every request — there is no in-memory cache today). Emits an
/// audit `plugin.rescan` event so operators have a trail of who reloaded
/// when. Returns the fresh list.
///
/// Semantically equivalent to `GET /plugins` (since the list is filesystem-
/// backed without cache), but the explicit POST + audit event differentiates
/// "I was idly viewing the list" from "I want a fresh read after dropping
/// a new plugin.json into the directory".
async fn rescan_plugins(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<PluginsResponse>, ApiError> {
    require_admin(&claims).await?;

    let plugins = discover_plugins();
    let total = plugins.len();

    if let Some(audit) = state.audit.as_ref() {
        audit
            .record(AuditEntry::now(
                claims.sub.to_string(),
                AuditKind::Mutation,
                "plugin.rescan".to_string(),
                None,
                serde_json::json!({ "discovered": total }),
                AuditResult::Success,
            ))
            .await;
    }

    Ok(Json(PluginsResponse { plugins, total }))
}

/// Walk `~/.cyberclaw/plugins/` and parse every `plugin.json` found.
fn discover_plugins() -> Vec<PluginManifest> {
    let plugins_dir = match std::env::var_os("HOME") {
        Some(h) => std::path::PathBuf::from(h)
            .join(".cyberclaw")
            .join("plugins"),
        None => return vec![],
    };

    let read_dir = match std::fs::read_dir(&plugins_dir) {
        Ok(rd) => rd,
        Err(_) => return vec![],
    };

    let mut manifests = Vec::new();
    for entry in read_dir.flatten() {
        let manifest_path = entry.path().join("plugin.json");
        match std::fs::read_to_string(&manifest_path) {
            Ok(raw) => match serde_json::from_str::<PluginManifest>(&raw) {
                Ok(m) => manifests.push(m),
                Err(e) => warn!(
                    path = %manifest_path.display(),
                    error = %e,
                    "plugin.json parse error — skipping"
                ),
            },
            Err(_) => {
                // Not every subdirectory has a plugin.json; skip silently.
            }
        }
    }

    // Stable order: sort by name so the list is deterministic across runs.
    manifests.sort_by(|a, b| a.name.cmp(&b.name));
    manifests
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_manifest_defaults() {
        let raw = r#"{"name":"example-plugin"}"#;
        let m: PluginManifest = serde_json::from_str(raw).unwrap();
        assert_eq!(m.name, "example-plugin");
        assert_eq!(m.version, "0.0.0");
        assert!(m.enabled);
        assert!(!m.hidden);
        assert!(m.label.is_none());
    }

    #[test]
    fn plugin_manifest_full() {
        let raw = r#"{
            "name": "my-plugin",
            "version": "1.2.3",
            "label": "My Plugin",
            "description": "Does something",
            "icon": "🧩",
            "tab_path": "/my-plugin",
            "enabled": false,
            "hidden": true
        }"#;
        let m: PluginManifest = serde_json::from_str(raw).unwrap();
        assert_eq!(m.version, "1.2.3");
        assert_eq!(m.label.as_deref(), Some("My Plugin"));
        assert!(!m.enabled);
        assert!(m.hidden);
    }

    #[tokio::test]
    async fn list_plugins_router_returns_200() {
        use crate::api::test_helpers::build_test_state;
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt;

        let state = build_test_state();
        let app = create_plugins_router().with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/plugins")
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
        assert!(json["plugins"].is_array());
        assert!(json["total"].is_number());
    }
}
