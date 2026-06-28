//! Settings API — runtime config, environment, and about.
//!
//! Sprint 7. Backs the admin console's Settings tab. Three concerns:
//!
//! 1. **Config** (`/settings/config`) — raw TOML body from the server
//!    config file. Read-only GET (safe to expose to operators) plus a
//!    write PUT that rewrites the on-disk file.
//! 2. **Env** (`/settings/env`) — environment variables as `[{key, value,
//!    secret}]`. Sensitive keys (`*_KEY`, `*_TOKEN`, `*_SECRET`) are
//!    masked. `PUT` sets a variable for the running process (does NOT
//!    persist to shell / service manager).
//! 3. **About** (`/settings/about`) — version, build info, uptime.

use std::sync::Arc;

use axum::extract::Query;
use axum::{extract::State, routing::get, Json, Router};
use cyberclaw_governance::{
    DangerSeverity, DangerousCapabilityFilter, PermissionDecision, ToolPermissionMatcher,
};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::audit::{AuditEntry, AuditKind, AuditResult};
use crate::error::ApiError;
use crate::middleware::auth::{require_admin, Claims};
use crate::state::AppState;

/// Substring markers that classify an env var as secret.
const SECRET_MARKERS: &[&str] = &[
    "KEY",
    "TOKEN",
    "SECRET",
    "PASSWORD",
    "PASSPHRASE",
    "CREDENTIAL",
];
/// Masked replacement string for secret env values.
const MASKED: &str = "••••••••";

/// Query params for `GET /api/v1/settings/config`.
#[derive(Debug, Deserialize)]
pub struct GetConfigQuery {
    /// Pass `?reveal=true` (admin only) to skip secret redaction.
    #[serde(default)]
    pub reveal: bool,
}

/// `GET /api/v1/settings/config` response.
#[derive(Debug, Serialize)]
pub struct ConfigResponse {
    pub path: String,
    pub content: String,
    /// Whether secret values have been redacted in the returned content.
    pub redacted: bool,
}

/// `PUT /api/v1/settings/config` body.
#[derive(Debug, Deserialize)]
pub struct UpdateConfigRequest {
    pub content: String,
}

/// One row of `GET /api/v1/settings/env`.
#[derive(Debug, Serialize)]
pub struct EnvEntry {
    pub key: String,
    pub value: String,
    pub secret: bool,
}

/// `PUT /api/v1/settings/env` body.
#[derive(Debug, Deserialize)]
pub struct SetEnvRequest {
    pub key: String,
    pub value: String,
}

/// One row of the `dangerous` list in `GET /api/v1/settings/policies`.
#[derive(Debug, Serialize)]
pub struct DangerousPolicyRow {
    pub rule: String,
    pub pattern: String,
    pub action: String,
    pub risk: String,
    pub reason: String,
}

/// One row of the `tools` list in `GET /api/v1/settings/policies`.
#[derive(Debug, Serialize)]
pub struct ToolPolicyRow {
    pub tool: String,
    pub argument: Option<String>,
    pub action: String,
    pub reason: String,
}

/// `GET /api/v1/settings/policies` response.
#[derive(Debug, Serialize)]
pub struct PoliciesResponse {
    pub dangerous: Vec<DangerousPolicyRow>,
    pub tools: Vec<ToolPolicyRow>,
    pub tools_default: String,
}

/// `GET /api/v1/settings/about` response.
#[derive(Debug, Serialize)]
pub struct AboutResponse {
    pub version: String,
    pub commit: String,
    pub build_time: String,
    pub uptime_secs: u64,
    pub node_id: String,
}

/// Build the `/api/v1/settings/*` router.
pub fn create_settings_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/settings/config", get(get_config).put(put_config))
        .route("/api/v1/settings/env", get(get_env).put(put_env))
        .route("/api/v1/settings/about", get(get_about))
        .route("/api/v1/settings/policies", get(get_policies))
}

/// Return the default config path. Picks up
/// `CYBERCLAW_CONFIG_PATH` when set; otherwise falls back to the
/// `config.toml` next to the server binary / manifest.
fn config_path() -> std::path::PathBuf {
    if let Ok(path) = std::env::var("CYBERCLAW_CONFIG_PATH") {
        return std::path::PathBuf::from(path);
    }
    // Dev convenience: config.toml next to the crate manifest. Gated to debug
    // builds so the shipped release binary never embeds the build machine's
    // absolute manifest path (env!("CARGO_MANIFEST_DIR") resolves at compile
    // time; --remap-path-prefix does not touch it). Release falls back to a
    // CWD-relative path.
    #[cfg(debug_assertions)]
    {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config.toml")
    }
    #[cfg(not(debug_assertions))]
    {
        std::path::PathBuf::from("config.toml")
    }
}

/// `GET /api/v1/settings/config` — return the server's TOML config body.
///
/// Requires admin role. Secret values (keys containing `key`, `secret`,
/// `token`, `password`, or `pepper`) are redacted by default.
/// Pass `?reveal=true` to receive the raw content (admin-only override).
async fn get_config(
    State(state): State<Arc<AppState>>,
    claims: Option<axum::extract::Extension<Claims>>,
    Query(query): Query<GetConfigQuery>,
) -> Result<Json<ConfigResponse>, ApiError> {
    // L7 — admin gate: config file must not be readable by viewers.
    if let Some(axum::extract::Extension(ref c)) = claims {
        if let Err(err) = require_admin(c).await {
            audit_role_denied(
                state.as_ref(),
                c.sub.as_str(),
                "settings.config.get:role_denied",
            )
            .await;
            return Err(err);
        }
    } else {
        // No JWT at all — treat as unauthorized.
        return Err(ApiError::Unauthorized(
            "authentication required for config".to_string(),
        ));
    }

    let path = config_path();
    match std::fs::read_to_string(&path) {
        Ok(raw) => {
            let (content, redacted) = if query.reveal {
                (raw, false)
            } else {
                (redact_secrets_in_toml(&raw), true)
            };
            Ok(Json(ConfigResponse {
                path: path.display().to_string(),
                content,
                redacted,
            }))
        }
        Err(err) => {
            warn!(path = %path.display(), %err, "config read failed");
            Err(ApiError::NotFound(format!(
                "config file not readable at {}",
                path.display()
            )))
        }
    }
}

/// Redact secret values in a TOML document.
///
/// Scans line by line for assignments of the form `key = "value"` where the
/// key name contains a secret marker (case-insensitive). Matching lines have
/// their quoted string value replaced with `"***"`. Lines that do not match
/// the pattern are passed through unchanged, preserving comments, structure,
/// and non-sensitive values.
pub fn redact_secrets_in_toml(raw: &str) -> String {
    /// TOML key markers that indicate a secret value.
    const TOML_SECRET_MARKERS: &[&str] = &[
        "key",
        "secret",
        "token",
        "password",
        "pepper",
        "passphrase",
        "credential",
    ];

    raw.lines()
        .map(|line| {
            // Match `identifier = "..."` or `identifier = '...'`
            // We look for the first `=` to split key from value.
            if let Some(eq_pos) = line.find('=') {
                let key_part = line[..eq_pos].trim().to_lowercase();
                let is_secret = TOML_SECRET_MARKERS
                    .iter()
                    .any(|marker| key_part.contains(marker));
                if is_secret {
                    // Replace the quoted string value portion with "***".
                    // This handles `key = "value"`, `key = 'value'`, and
                    // `key = "value" # comment` (comment is dropped to keep
                    // the line simple — acceptable for an admin-only view).
                    let value_part = line[eq_pos + 1..].trim();
                    if (value_part.starts_with('"') && value_part.contains('"'))
                        || (value_part.starts_with('\'') && value_part.contains('\''))
                    {
                        let key_str = line[..eq_pos].to_string();
                        return format!("{}= \"***\"", key_str);
                    }
                }
            }
            line.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `PUT /api/v1/settings/config` — rewrite the config body.
///
/// Validates the new body parses as TOML before writing so a broken
/// config cannot be persisted through the admin UI.
async fn put_config(
    State(state): State<Arc<AppState>>,
    claims: Option<axum::extract::Extension<Claims>>,
    Json(req): Json<UpdateConfigRequest>,
) -> Result<Json<ConfigResponse>, ApiError> {
    // L7 — server-side role enforcement. When JWT is present, require admin.
    if let Some(axum::extract::Extension(ref c)) = claims {
        if let Err(err) = require_admin(c).await {
            audit_role_denied(
                state.as_ref(),
                c.sub.as_str(),
                "settings.config.put:role_denied",
            )
            .await;
            return Err(err);
        }
    }

    let actor = claims
        .map(|c| c.0.sub.as_str().to_string())
        .unwrap_or_else(|| "system".to_string());

    // Parse-validate the TOML before writing so we don't persist garbage.
    if let Err(err) = toml::from_str::<toml::Value>(&req.content) {
        if let Some(sink) = state.audit.as_ref() {
            sink.record(AuditEntry::now(
                actor,
                AuditKind::Config,
                "settings.config.put".to_string(),
                None,
                serde_json::json!({ "bytes": req.content.len() }),
                AuditResult::Failure {
                    reason: format!("invalid toml: {err}"),
                },
            ))
            .await;
        }
        return Err(ApiError::InvalidRequest(format!(
            "config is not valid TOML: {err}"
        )));
    }

    let path = config_path();
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ApiError::InternalError(format!("failed to create parent: {e}")))?;
        }
    }

    std::fs::write(&path, &req.content)
        .map_err(|e| ApiError::InternalError(format!("failed to write config: {e}")))?;

    info!(path = %path.display(), bytes = req.content.len(), "config persisted");
    if let Some(sink) = state.audit.as_ref() {
        sink.record(AuditEntry::now(
            actor,
            AuditKind::Config,
            "settings.config.put".to_string(),
            Some(format!("config:{}", path.display())),
            serde_json::json!({ "bytes": req.content.len() }),
            AuditResult::Success,
        ))
        .await;
    }
    Ok(Json(ConfigResponse {
        path: path.display().to_string(),
        content: req.content,
        redacted: false,
    }))
}

/// Classify an env var key as secret.
pub fn is_secret_key(key: &str) -> bool {
    let upper = key.to_uppercase();
    SECRET_MARKERS.iter().any(|marker| upper.contains(marker))
}

/// Mask a value if the key is marked secret; pass-through otherwise.
pub fn mask_value(key: &str, value: &str) -> (String, bool) {
    if is_secret_key(key) {
        (MASKED.to_string(), true)
    } else {
        (value.to_string(), false)
    }
}

/// `GET /api/v1/settings/env` — list env vars with sensitive values masked.
async fn get_env() -> Json<Vec<EnvEntry>> {
    let mut entries: Vec<EnvEntry> = std::env::vars()
        .map(|(key, value)| {
            let (masked, secret) = mask_value(&key, &value);
            EnvEntry {
                key,
                value: masked,
                secret,
            }
        })
        .collect();
    entries.sort_by(|a, b| a.key.cmp(&b.key));
    Json(entries)
}

/// `PUT /api/v1/settings/env` — set an env var for the running process.
async fn put_env(
    State(state): State<Arc<AppState>>,
    claims: Option<axum::extract::Extension<Claims>>,
    Json(req): Json<SetEnvRequest>,
) -> Result<Json<EnvEntry>, ApiError> {
    // L7 — server-side role enforcement. When JWT is present, require admin.
    if let Some(axum::extract::Extension(ref c)) = claims {
        if let Err(err) = require_admin(c).await {
            audit_role_denied(
                state.as_ref(),
                c.sub.as_str(),
                "settings.env.put:role_denied",
            )
            .await;
            return Err(err);
        }
    }

    let actor = claims
        .map(|c| c.0.sub.as_str().to_string())
        .unwrap_or_else(|| "system".to_string());
    let key = req.key.trim();
    if key.is_empty() {
        if let Some(sink) = state.audit.as_ref() {
            sink.record(AuditEntry::now(
                actor,
                AuditKind::Config,
                "settings.env.put".to_string(),
                None,
                serde_json::json!({}),
                AuditResult::Failure {
                    reason: "key is required".to_string(),
                },
            ))
            .await;
        }
        return Err(ApiError::InvalidRequest("key is required".to_string()));
    }

    // SAFETY: set_var is marked unsafe as of recent std. We accept this
    // for admin-driven runtime configuration; callers are authenticated.
    unsafe {
        std::env::set_var(key, &req.value);
    }

    let (masked, secret) = mask_value(key, &req.value);
    info!(%key, is_secret = secret, "env var set via admin API");
    if let Some(sink) = state.audit.as_ref() {
        sink.record(AuditEntry::now(
            actor,
            AuditKind::Config,
            "settings.env.put".to_string(),
            Some(format!("env:{}", key)),
            serde_json::json!({ "is_secret": secret }),
            AuditResult::Success,
        ))
        .await;
    }
    Ok(Json(EnvEntry {
        key: key.to_string(),
        value: masked,
        secret,
    }))
}

/// `GET /api/v1/settings/about` — version / build info / uptime.
async fn get_about(State(state): State<Arc<AppState>>) -> Json<AboutResponse> {
    let uptime = state.started_at.elapsed().as_secs();
    Json(AboutResponse {
        version: env!("CARGO_PKG_VERSION").to_string(),
        commit: option_env!("CYBERCLAW_COMMIT")
            .unwrap_or("unknown")
            .to_string(),
        build_time: option_env!("CYBERCLAW_BUILD_TIME")
            .unwrap_or("unknown")
            .to_string(),
        uptime_secs: uptime,
        node_id: state.node_id.clone(),
    })
}

/// `GET /api/v1/settings/policies` — surface the in-process governance rule
/// sets (DangerousCapabilityFilter + ToolPermissionMatcher) so the admin
/// console can render them without bundling a MOCK copy in the frontend.
///
/// Rules are drawn from the defaults baked into `cyberclaw-governance`. A
/// future iteration will source from the runtime engine once the admin
/// console supports editing.
async fn get_policies() -> Json<PoliciesResponse> {
    let filter = DangerousCapabilityFilter::with_defaults();
    let dangerous = filter
        .rules()
        .iter()
        .map(|r| {
            let (action, risk) = match r.severity {
                DangerSeverity::Critical => ("deny", "critical"),
                DangerSeverity::High => ("require_review", "high"),
                DangerSeverity::Medium => ("warn", "medium"),
            };
            DangerousPolicyRow {
                rule: r.id.clone(),
                pattern: r.capability_pattern.clone(),
                action: action.to_string(),
                risk: risk.to_string(),
                reason: r.reason.clone(),
            }
        })
        .collect();

    let matcher = ToolPermissionMatcher::default();
    let tools = matcher
        .rules()
        .iter()
        .map(|r| ToolPolicyRow {
            tool: r.tool_pattern.clone(),
            argument: r.argument_pattern.clone(),
            action: permission_decision_label(r.decision).to_string(),
            reason: r.reason.clone(),
        })
        .collect();

    Json(PoliciesResponse {
        dangerous,
        tools,
        tools_default: permission_decision_label(matcher.default_decision()).to_string(),
    })
}

/// Lower-case label for a [`PermissionDecision`] suitable for JSON output.
fn permission_decision_label(d: PermissionDecision) -> &'static str {
    match d {
        PermissionDecision::Allow => "allow",
        PermissionDecision::Ask => "ask",
        PermissionDecision::Deny => "deny",
    }
}

/// Record a role-denied attempt as an `Auth` failure entry. Mirrors
/// `audit_role_denied` in `api::reviews`; kept local so settings-specific
/// actions show up under the Config AuditKind-adjacent namespace.
async fn audit_role_denied(state: &AppState, actor: &str, action: &str) {
    if let Some(sink) = state.audit.as_ref() {
        sink.record(AuditEntry::now(
            actor.to_string(),
            AuditKind::Auth,
            action.to_string(),
            None,
            serde_json::json!({ "reason": "role_denied" }),
            AuditResult::Failure {
                reason: "admin role required".to_string(),
            },
        ))
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_secret_key_flags_sensitive() {
        assert!(is_secret_key("ANTHROPIC_API_KEY"));
        assert!(is_secret_key("CYBERCLAW_JWT_SECRET"));
        assert!(is_secret_key("DATABASE_PASSWORD"));
        assert!(is_secret_key("api_token"));
        assert!(is_secret_key("USER_CREDENTIAL"));
    }

    #[test]
    fn is_secret_key_passes_non_sensitive() {
        assert!(!is_secret_key("LOG_LEVEL"));
        assert!(!is_secret_key("METRICS_PORT"));
        assert!(!is_secret_key("REGION"));
        assert!(!is_secret_key("HOME"));
    }

    #[test]
    fn mask_value_redacts_secrets() {
        let (v, is_s) = mask_value("ANTHROPIC_API_KEY", "sk-ant-real-secret");
        assert_eq!(v, MASKED);
        assert!(is_s);

        let (v, is_s) = mask_value("LOG_LEVEL", "info");
        assert_eq!(v, "info");
        assert!(!is_s);
    }

    #[tokio::test]
    async fn settings_env_router_masks_sensitive_keys() {
        use crate::api::test_helpers::build_test_state;
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt;

        // Set a synthetic secret env var for the scope of this test.
        let key = "CYBERCLAW_TEST_FAKE_SECRET_KEY";
        // SAFETY: test-only, thread-local semantics acceptable here.
        unsafe {
            std::env::set_var(key, "super-secret-value");
        }

        let state = build_test_state();
        let app = create_settings_router().with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/settings/env")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let entries = json.as_array().expect("array");
        let found = entries
            .iter()
            .find(|e| e.get("key").and_then(|k| k.as_str()) == Some(key))
            .expect("our fake secret should be surfaced");
        assert_eq!(found["value"], MASKED);
        assert_eq!(found["secret"], true);

        unsafe {
            std::env::remove_var(key);
        }
    }

    #[tokio::test]
    async fn settings_about_router_returns_known_fields() {
        use crate::api::test_helpers::build_test_state;
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt;

        let state = build_test_state();
        let app = create_settings_router().with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/settings/about")
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
        assert!(!json["version"].as_str().unwrap().is_empty());
        assert!(!json["node_id"].as_str().unwrap().is_empty());
        // uptime_secs is a number (may be 0 on a very fast test).
        assert!(json["uptime_secs"].is_u64());
    }

    // ----- L7: server-side role enforcement -----

    use crate::api::test_helpers::{build_test_state as btst, seed_users_with_role};
    use axum::extract::{Extension, State};
    use cyberclaw_core::ids::UserId;
    use serial_test::serial;

    fn claims_for(user_id: &str) -> Claims {
        let uid = UserId::from_string(user_id.to_string()).expect("valid user id");
        let now = chrono::Utc::now().timestamp();
        Claims {
            sub: uid,
            tenant: None,
            iat: now,
            exp: now + 3600,
        }
    }

    #[tokio::test]
    #[serial]
    async fn test_settings_env_put_allowed_for_admin() {
        let (_tmp, _restore) = seed_users_with_role("op-env-admin", "admin");
        let state = btst();
        let claims = claims_for("op-env-admin");
        let res = put_env(
            State(state.clone()),
            Some(Extension(claims)),
            axum::Json(SetEnvRequest {
                key: "CYBERCLAW_L7_ADMIN_ENV_TEST".to_string(),
                value: "ok".to_string(),
            }),
        )
        .await
        .expect("admin must be allowed to set env");
        assert_eq!(res.0.key, "CYBERCLAW_L7_ADMIN_ENV_TEST");
        // Clean up.
        unsafe {
            std::env::remove_var("CYBERCLAW_L7_ADMIN_ENV_TEST");
        }
    }

    #[tokio::test]
    #[serial]
    async fn test_settings_env_put_rejected_for_viewer() {
        let (_tmp, _restore) = seed_users_with_role("op-env-viewer", "viewer");
        let state = btst();
        let claims = claims_for("op-env-viewer");
        let err = put_env(
            State(state.clone()),
            Some(Extension(claims)),
            axum::Json(SetEnvRequest {
                key: "CYBERCLAW_L7_VIEWER_ENV_TEST".to_string(),
                value: "nope".to_string(),
            }),
        )
        .await
        .expect_err("viewer must be forbidden");
        assert!(matches!(err, ApiError::Forbidden(_)), "got {:?}", err);
        // Must not have leaked into env on reject.
        assert!(std::env::var("CYBERCLAW_L7_VIEWER_ENV_TEST").is_err());
    }

    // ----- get_config: redact_secrets_in_toml unit tests -----

    #[test]
    fn redact_secrets_in_toml_masks_secret_keys() {
        let input = r#"
[server]
host = "localhost"
port = 8080
api_key = "sk-real-secret"
jwt_secret = "super-secret-pepper"
database_password = "hunter2"
log_level = "info"
token = "bearer-token-value"
"#;
        let output = redact_secrets_in_toml(input);
        assert!(
            output.contains("host = \"localhost\""),
            "non-secret preserved"
        );
        assert!(
            output.contains("port = 8080"),
            "non-secret integer preserved"
        );
        assert!(
            output.contains("log_level = \"info\""),
            "non-secret preserved"
        );
        assert!(
            !output.contains("sk-real-secret"),
            "api_key value must be redacted"
        );
        assert!(
            !output.contains("super-secret-pepper"),
            "jwt_secret value must be redacted"
        );
        assert!(
            !output.contains("hunter2"),
            "password value must be redacted"
        );
        assert!(
            !output.contains("bearer-token-value"),
            "token value must be redacted"
        );
        assert!(output.contains("\"***\""), "redacted marker present");
    }

    #[test]
    fn redact_secrets_in_toml_leaves_non_secret_unchanged() {
        let input = "region = \"us-east-1\"\nmetrics_port = 9090\n";
        let output = redact_secrets_in_toml(input);
        assert!(output.contains("us-east-1"));
        assert!(output.contains("9090"));
        assert!(!output.contains("***"));
    }

    // ----- get_config: admin gate tests -----

    #[tokio::test]
    #[serial]
    async fn test_settings_config_get_rejected_without_claims() {
        let state = btst();
        // No claims at all → Unauthorized
        let err = get_config(
            State(state.clone()),
            None,
            Query(GetConfigQuery { reveal: false }),
        )
        .await
        .expect_err("unauthenticated request must fail");
        assert!(matches!(err, ApiError::Unauthorized(_)), "got {:?}", err);
    }

    #[tokio::test]
    #[serial]
    async fn test_settings_config_get_rejected_for_viewer() {
        let (_tmp, _restore) = seed_users_with_role("op-cfg-get-viewer", "viewer");
        let state = btst();
        let claims = claims_for("op-cfg-get-viewer");
        let err = get_config(
            State(state.clone()),
            Some(Extension(claims)),
            Query(GetConfigQuery { reveal: false }),
        )
        .await
        .expect_err("viewer must be forbidden");
        assert!(matches!(err, ApiError::Forbidden(_)), "got {:?}", err);
    }

    #[tokio::test]
    #[serial]
    async fn test_settings_config_get_redacts_secrets_for_admin() {
        let (_tmp, _restore) = seed_users_with_role("op-cfg-get-admin", "admin");
        // Point config at a temp file with a known secret value.
        let cfg_tmp = tempfile::tempdir().expect("cfg tempdir");
        let cfg_path = cfg_tmp.path().join("cyberclaw-test.toml");
        std::fs::write(
            &cfg_path,
            "api_key = \"real-secret-value\"\nhost = \"localhost\"\n",
        )
        .expect("write test config");
        let original = std::env::var_os("CYBERCLAW_CONFIG_PATH");
        unsafe {
            std::env::set_var("CYBERCLAW_CONFIG_PATH", &cfg_path);
        }
        let state = btst();
        let claims = claims_for("op-cfg-get-admin");
        let res = get_config(
            State(state.clone()),
            Some(Extension(claims)),
            Query(GetConfigQuery { reveal: false }),
        )
        .await;
        unsafe {
            match original {
                Some(v) => std::env::set_var("CYBERCLAW_CONFIG_PATH", v),
                None => std::env::remove_var("CYBERCLAW_CONFIG_PATH"),
            }
        }
        let resp = res.expect("admin must succeed");
        assert!(resp.0.redacted, "redacted flag must be true");
        assert!(
            !resp.0.content.contains("real-secret-value"),
            "secret must not appear"
        );
        assert!(resp.0.content.contains("localhost"), "non-secret preserved");
    }

    #[tokio::test]
    #[serial]
    async fn test_settings_config_get_reveal_shows_raw_for_admin() {
        let (_tmp, _restore) = seed_users_with_role("op-cfg-reveal-admin", "admin");
        let cfg_tmp = tempfile::tempdir().expect("cfg tempdir");
        let cfg_path = cfg_tmp.path().join("cyberclaw-reveal.toml");
        std::fs::write(&cfg_path, "api_key = \"raw-secret\"\n").expect("write test config");
        let original = std::env::var_os("CYBERCLAW_CONFIG_PATH");
        unsafe {
            std::env::set_var("CYBERCLAW_CONFIG_PATH", &cfg_path);
        }
        let state = btst();
        let claims = claims_for("op-cfg-reveal-admin");
        let res = get_config(
            State(state.clone()),
            Some(Extension(claims)),
            Query(GetConfigQuery { reveal: true }),
        )
        .await;
        unsafe {
            match original {
                Some(v) => std::env::set_var("CYBERCLAW_CONFIG_PATH", v),
                None => std::env::remove_var("CYBERCLAW_CONFIG_PATH"),
            }
        }
        let resp = res.expect("admin with reveal must succeed");
        assert!(
            !resp.0.redacted,
            "redacted flag must be false when reveal=true"
        );
        assert!(
            resp.0.content.contains("raw-secret"),
            "raw value exposed on ?reveal=true"
        );
    }

    #[tokio::test]
    #[serial]
    async fn test_settings_config_put_allowed_for_admin() {
        let (_tmp_home, _restore) = seed_users_with_role("op-cfg-admin", "admin");
        // Redirect CYBERCLAW_CONFIG_PATH to a tempdir so we don't clobber a real file.
        let cfg_tmp = tempfile::tempdir().expect("cfg tempdir");
        let cfg_path = cfg_tmp.path().join("cyberclaw-l7.toml");
        let original = std::env::var_os("CYBERCLAW_CONFIG_PATH");
        unsafe {
            std::env::set_var("CYBERCLAW_CONFIG_PATH", &cfg_path);
        }
        let state = btst();
        let claims = claims_for("op-cfg-admin");
        let res = put_config(
            State(state.clone()),
            Some(Extension(claims)),
            axum::Json(UpdateConfigRequest {
                content: "key = \"value\"\n".to_string(),
            }),
        )
        .await;
        // Restore env before asserting so a failure doesn't leak.
        unsafe {
            match original {
                Some(v) => std::env::set_var("CYBERCLAW_CONFIG_PATH", v),
                None => std::env::remove_var("CYBERCLAW_CONFIG_PATH"),
            }
        }
        let _ = res.expect("admin must be allowed to put config");
    }

    #[tokio::test]
    #[serial]
    async fn test_settings_config_put_rejected_for_viewer() {
        let (_tmp_home, _restore) = seed_users_with_role("op-cfg-viewer", "viewer");
        let state = btst();
        let claims = claims_for("op-cfg-viewer");
        let err = put_config(
            State(state.clone()),
            Some(Extension(claims)),
            axum::Json(UpdateConfigRequest {
                content: "key = \"value\"\n".to_string(),
            }),
        )
        .await
        .expect_err("viewer must be forbidden");
        assert!(matches!(err, ApiError::Forbidden(_)), "got {:?}", err);
    }
}
