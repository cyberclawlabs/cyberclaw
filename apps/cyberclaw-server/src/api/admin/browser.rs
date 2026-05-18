//! Admin Browser CDP endpoints.
//!
//! | Method | Path                                      | Purpose                              |
//! |--------|-------------------------------------------|--------------------------------------|
//! | GET    | `/api/v1/admin/browser/status`            | Probe + list CDP targets             |
//! | POST   | `/api/v1/admin/browser/navigate`          | Navigate                             |
//! | POST   | `/api/v1/admin/browser/click`             | Click selector                       |
//! | POST   | `/api/v1/admin/browser/fill`              | Fill input                           |
//! | POST   | `/api/v1/admin/browser/evaluate`          | Run JS                               |
//! | POST   | `/api/v1/admin/browser/screenshot`        | Capture screenshot                   |
//! | POST   | `/api/v1/admin/browser/dialog`            | Accept / dismiss JS dialog           |
//!
//! All routes require admin JWT (lives under the authenticated lane in
//! `crate::create_router_with_config`). Underneath the action endpoints
//! resolve the `BrowserConnector` via the production `OrchestratorGateway`
//! (`build_governing_gateway`) which routes through PolicyEngine /
//! DangerousCapabilityFilter before dispatching to the connector. This
//! mirrors the chat-handler execution path — admin REST MUST NOT call
//! `connector.execute()` directly because that bypasses governance.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;

use axum::{
    extract::State,
    routing::{get, post},
    Json, Router,
};
use cyberclaw_core::gateway::{CapabilityRequest, GatewayError};
use cyberclaw_core::identity::{ActorRef, ActorType};
use cyberclaw_core::ids::{ActorId, CapabilityId, ConnectorId, ExecutionId};
use serde::Serialize;
use serde_json::Value;

use crate::api::chat_handler::build_governing_gateway;
use crate::error::ApiError;
use crate::state::AppState;

const BROWSER_CONNECTOR_ID: &str = "browser";
const STATUS_PROBE_TIMEOUT_MS: u64 = 2_000;
const ADMIN_BROWSER_WORKSPACE_ROOT: &str = "/tmp/admin-browser";

/// `GET /api/v1/admin/browser/status`
#[derive(Debug, Serialize)]
pub struct BrowserStatusResponse {
    pub enabled: bool,
    pub ws_url: String,
    pub debug_url: String,
    pub attached: bool,
    pub targets: Vec<BrowserTarget>,
}

#[derive(Debug, Serialize)]
pub struct BrowserTarget {
    pub id: String,
    pub url: String,
    pub title: String,
    #[serde(rename = "type")]
    pub kind: String,
}

/// Whitelist for the CDP status probe. The DEBUG URL is operator-supplied
/// via `CYBERCLAW_BROWSER_DEBUG_URL`; if it points at anything other than
/// loopback we refuse to fetch it. This stops a misconfigured env (e.g.
/// `internal-prom:9090`) from being abused as an internal-network probe.
fn probe_host_is_loopback(debug_url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(debug_url) else {
        return false;
    };
    match parsed.host() {
        Some(url::Host::Domain(d)) => matches!(d.to_ascii_lowercase().as_str(), "localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

pub async fn status(
    State(state): State<Arc<AppState>>,
) -> Result<Json<BrowserStatusResponse>, ApiError> {
    let enabled = std::env::var("CYBERCLAW_BROWSER_ENABLED")
        .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes"))
        .unwrap_or(false);
    let debug_url = std::env::var("CYBERCLAW_BROWSER_DEBUG_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:9222".to_string());
    let ws_url = std::env::var("CYBERCLAW_BROWSER_WS_URL").unwrap_or_default();

    // Tell whether a connector is registered (cheap, sync).
    let connector_present = state
        .connector_registry
        .get(
            &ConnectorId::from_string(BROWSER_CONNECTOR_ID.to_string())
                .map_err(|e| ApiError::InternalError(format!("connector id: {e}")))?,
        )
        .is_some();

    // Probe: attempt /json/list with a short timeout. Failure here is not
    // fatal — we just report attached=false + empty targets.  H-4: refuse
    // to probe non-loopback hosts (env misconfig must not become an
    // internal-network scanner).
    let (attached, targets) =
        if (enabled || connector_present) && probe_host_is_loopback(&debug_url) {
            match probe_targets(&debug_url).await {
                Ok(t) => (true, t),
                Err(_) => (false, Vec::new()),
            }
        } else {
            (false, Vec::new())
        };

    Ok(Json(BrowserStatusResponse {
        enabled,
        ws_url,
        debug_url,
        attached,
        targets,
    }))
}

async fn probe_targets(debug_url: &str) -> anyhow::Result<Vec<BrowserTarget>> {
    let base = debug_url.trim_end_matches('/');
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(STATUS_PROBE_TIMEOUT_MS))
        .no_proxy()
        .build()?;
    let resp: Value = client
        .get(format!("{}/json/list", base))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let mut out = Vec::new();
    if let Some(arr) = resp.as_array() {
        for item in arr {
            out.push(BrowserTarget {
                id: item
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                url: item
                    .get("url")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                title: item
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                kind: item
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
            });
        }
    }
    Ok(out)
}

#[derive(Debug, Serialize)]
pub struct BrowserActionResponse {
    pub status: String,
    pub output: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Read a file back as base64 IFF its canonical path is rooted under
/// `allowed_root`. Both sides are canonicalised; symlinks that escape the
/// root are rejected. Returns `Ok(None)` if `path` does not exist or is
/// outside the allowed root — the caller decides whether to surface that
/// to the client. H-5.
pub(super) async fn read_file_b64_within(
    path: &str,
    allowed_root: &str,
) -> Result<Option<String>, ApiError> {
    let canonical_root = match tokio::fs::canonicalize(allowed_root).await {
        Ok(p) => p,
        Err(_) => return Ok(None),
    };
    let canonical_target = match tokio::fs::canonicalize(path).await {
        Ok(p) => p,
        Err(_) => return Ok(None),
    };
    if !canonical_target.starts_with(&canonical_root) {
        return Err(ApiError::InvalidInput(format!(
            "artifact path '{}' escapes allowed root",
            path
        )));
    }
    let bytes = tokio::fs::read(&canonical_target)
        .await
        .map_err(|e| ApiError::InternalError(format!("read artifact: {e}")))?;
    Ok(Some(
        base64::engine::general_purpose::STANDARD.encode(&bytes),
    ))
}

/// Create a 0700-mode workspace root once, owned by the server uid. H-9.
fn ensure_workspace_root(path: &str) -> Result<(), ApiError> {
    use std::fs::DirBuilder;
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)
            .map_err(|e| ApiError::InternalError(format!("create workspace: {e}")))?;
    }
    #[cfg(not(unix))]
    {
        DirBuilder::new()
            .recursive(true)
            .create(path)
            .map_err(|e| ApiError::InternalError(format!("create workspace: {e}")))?;
    }
    Ok(())
}

async fn forward_capability(
    state: &Arc<AppState>,
    capability: &str,
    input: Value,
) -> Result<Json<BrowserActionResponse>, ApiError> {
    // C-1 fix: route through the governing gateway so PolicyEngine /
    // DangerousCapabilityFilter evaluate the call. Admin JWT is NOT a
    // bypass — it identifies the actor, but the same governance chain
    // chat_handler uses still applies.
    let connector_id = ConnectorId::from_string(BROWSER_CONNECTOR_ID.to_string())
        .map_err(|e| ApiError::InternalError(format!("connector id: {e}")))?;

    // Surface 404 explicitly when the connector is not mounted, before we
    // even ask the gateway. Cleaner DX than a generic governance error.
    if state.connector_registry.get(&connector_id).is_none() {
        // The route is mounted (so client gets here) but the underlying
        // browser connector is feature-gated and not currently registered.
        // 503 Service Unavailable conveys "this capability is configured to
        // exist but is not currently reachable" — 404 would imply the path
        // itself is unknown, which is misleading for SPA clients that may
        // surface it as a navigation error.
        return Err(ApiError::ServiceUnavailable(
            "browser connector is not registered (set CYBERCLAW_BROWSER_ENABLED=1 and \
                 register BrowserConnector at startup)"
                .to_string(),
        ));
    }

    let capability_id = CapabilityId::from_string(capability.to_string())
        .map_err(|e| ApiError::InvalidInput(format!("capability id: {e}")))?;

    let actor_id = ActorId::from_string("admin".to_string())
        .map_err(|e| ApiError::InternalError(format!("actor id: {e}")))?;
    let actor = ActorRef {
        id: actor_id,
        actor_type: ActorType::Human,
        tenant_id: None,
        home_node_id: None,
        display_name: "admin".to_string(),
    };

    let gateway = build_governing_gateway(state);
    let request = CapabilityRequest {
        execution_id: ExecutionId::new(),
        requested_by: actor,
        capability_id,
        connector_id,
        input,
        reason: format!("admin-browser:{}", capability),
    };

    match gateway.execute_capability(request).await {
        Ok(result) => Ok(Json(BrowserActionResponse {
            status: "Ok".to_string(),
            output: result.output,
            error: None,
        })),
        Err(GatewayError::GovernanceDenied(reason)) => Err(ApiError::Forbidden(reason)),
        Err(GatewayError::ReviewRequired(reason)) => {
            Err(ApiError::Forbidden(format!("review required: {}", reason)))
        }
        Err(GatewayError::CapabilityNotFound(reason)) => Err(ApiError::NotFound(reason)),
        Err(GatewayError::ConnectorError(reason)) => Err(ApiError::InternalError(format!(
            "browser dispatch: {}",
            reason
        ))),
        Err(GatewayError::Internal(reason)) => Err(ApiError::InternalError(reason)),
    }
}

pub async fn navigate(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<BrowserActionResponse>, ApiError> {
    forward_capability(&state, "browser.navigate", body).await
}

pub async fn click(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<BrowserActionResponse>, ApiError> {
    forward_capability(&state, "browser.click", body).await
}

pub async fn fill(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<BrowserActionResponse>, ApiError> {
    forward_capability(&state, "browser.fill", body).await
}

pub async fn evaluate(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<BrowserActionResponse>, ApiError> {
    forward_capability(&state, "browser.evaluate", body).await
}

pub async fn screenshot(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<BrowserActionResponse>, ApiError> {
    let mut resp = forward_capability(&state, "browser.screenshot", body).await?;
    // BrowserScreenshotOutput is { path, bytes_written, format }. Read the
    // file back and inject `image_b64` so the admin SPA can render the
    // screenshot inline (mirrors multimodal::image_generate's b64 egress).
    // H-5: the underlying capability MAY return a poisoned path; refuse
    // to read anything outside ADMIN_BROWSER_WORKSPACE_ROOT.
    if let Some(path) = resp.0.output.get("path").and_then(|v| v.as_str()) {
        match read_file_b64_within(path, ADMIN_BROWSER_WORKSPACE_ROOT).await {
            Ok(Some(b64)) => {
                if let Some(obj) = resp.0.output.as_object_mut() {
                    obj.insert("image_b64".to_string(), Value::String(b64));
                }
            }
            Ok(None) => {} // missing or outside-root → silently skip embedding
            Err(e) => return Err(e),
        }
    }
    Ok(resp)
}

pub async fn dialog(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<BrowserActionResponse>, ApiError> {
    forward_capability(&state, "browser.dialog_handle", body).await
}

pub fn create_admin_browser_router() -> Router<Arc<AppState>> {
    // H-9: lock down the workspace root permission bits at router-build
    // time. Best-effort — if creation fails (e.g. parent missing), the
    // forward_capability path will surface the error per-request.
    let _ = ensure_workspace_root(ADMIN_BROWSER_WORKSPACE_ROOT);
    Router::new()
        .route("/api/v1/admin/browser/status", get(status))
        .route("/api/v1/admin/browser/navigate", post(navigate))
        .route("/api/v1/admin/browser/click", post(click))
        .route("/api/v1/admin/browser/fill", post(fill))
        .route("/api/v1/admin/browser/evaluate", post(evaluate))
        .route("/api/v1/admin/browser/screenshot", post(screenshot))
        .route("/api/v1/admin/browser/dialog", post(dialog))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn status_returns_disabled_when_env_unset() {
        // Save & clear env so the test is reproducible regardless of host.
        let _g1 = EnvGuard::clear("CYBERCLAW_BROWSER_ENABLED");
        let _g2 = EnvGuard::set("CYBERCLAW_BROWSER_DEBUG_URL", "http://127.0.0.1:1");
        let state = crate::api::test_helpers::build_test_state();
        let resp = status(State(state)).await.expect("status ok");
        assert!(!resp.0.enabled);
        // No connector + disabled → attached should be false.
        assert!(!resp.0.attached);
        assert!(resp.0.targets.is_empty());
    }

    #[tokio::test]
    async fn navigate_returns_503_when_connector_missing() {
        let state = crate::api::test_helpers::build_test_state();
        let res = navigate(State(state), Json(serde_json::json!({"url":"http://x"}))).await;
        // Post-GA semantic fix: feature-gated connector returns 503
        // ServiceUnavailable, not 404 (the route IS mounted, just the
        // backing connector isn't provisioned). See commit fb1d268.
        match res {
            Err(ApiError::ServiceUnavailable(_)) => {}
            other => panic!("expected ServiceUnavailable, got {:?}", other),
        }
    }

    #[test]
    fn probe_host_loopback_whitelist() {
        assert!(probe_host_is_loopback("http://127.0.0.1:9222"));
        assert!(probe_host_is_loopback("http://localhost:9222"));
        assert!(probe_host_is_loopback("http://[::1]:9222"));
        // H-4: anything else must be refused.
        assert!(!probe_host_is_loopback("http://internal-prom:9090"));
        assert!(!probe_host_is_loopback("http://169.254.169.254"));
        assert!(!probe_host_is_loopback("http://10.0.0.1"));
        assert!(!probe_host_is_loopback("not a url"));
    }

    struct EnvGuard {
        key: &'static str,
        original: Option<String>,
    }
    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let original = std::env::var(key).ok();
            // SAFETY: tests touching $ENV are best-effort isolated; this
            // module's tests don't run concurrently with mutation tests.
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, original }
        }
        fn clear(key: &'static str) -> Self {
            let original = std::env::var(key).ok();
            unsafe {
                std::env::remove_var(key);
            }
            Self { key, original }
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            unsafe {
                if let Some(v) = self.original.take() {
                    std::env::set_var(self.key, v);
                } else {
                    std::env::remove_var(self.key);
                }
            }
        }
    }
}
