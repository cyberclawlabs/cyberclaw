//! Admin login / logout / me endpoints.
//!
//! Sprint 6 MVP: password validation is intentionally a stub. The login
//! handler only checks that the supplied `user_id` exists in
//! `~/.cyberclaw/users.toml` (which the Sprint 5 onboarding wizard wrote).
//! Any password is accepted. This matches the "admin backend with login,
//! not full auth" scope agreed with the user — proper password hashing,
//! OAuth, and SSO are out of scope.
//!
//! The server-side session is stateless: the JWT IS the session. `/logout`
//! therefore has no server state to clear and simply returns `{ok:true}`;
//! the client drops the token from `sessionStorage`.

use std::sync::Arc;

use axum::{
    extract::{Extension, State},
    Json,
};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use cyberclaw_core::ids::UserId;
use cyberclaw_core::users::{OperatorRecord, UsersConfig};

use crate::audit::{AuditEntry, AuditKind, AuditResult};
use crate::error::ApiError;
use crate::middleware::auth::{generate_jwt, Claims};
use crate::state::AppState;

/// Admin login lifetime default (24h). Kept short enough that a stolen token
/// is not forever; long enough to avoid friction during a management session.
///
/// 2026-05-18 (v1.2.10): operator-overridable via `CYBERCLAW_JWT_TTL_SECS` env
/// for ops who want longer sessions (e.g. CI bots) or shorter (high-security
/// deployments). Use `admin_jwt_ttl_secs()` instead of this const directly.
pub const ADMIN_JWT_TTL_SECS_DEFAULT: i64 = 60 * 60 * 24;

/// Compute the effective JWT TTL — env override or default. Sanitizes against
/// 0/negative values (logs warn + falls back to default). Cap at 90 days
/// (`60 * 60 * 24 * 90`) to prevent accidentally-forever tokens; deployments
/// that legitimately want longer should rotate `JWT_SECRET` instead.
pub fn admin_jwt_ttl_secs() -> i64 {
    const MAX_TTL: i64 = 60 * 60 * 24 * 90; // 90 days
    match std::env::var("CYBERCLAW_JWT_TTL_SECS")
        .ok()
        .and_then(|s| s.parse::<i64>().ok())
    {
        Some(v) if v <= 0 => {
            tracing::warn!(
                requested = v,
                default = ADMIN_JWT_TTL_SECS_DEFAULT,
                "CYBERCLAW_JWT_TTL_SECS is non-positive; falling back to default"
            );
            ADMIN_JWT_TTL_SECS_DEFAULT
        }
        Some(v) if v > MAX_TTL => {
            tracing::warn!(
                requested = v,
                max = MAX_TTL,
                "CYBERCLAW_JWT_TTL_SECS exceeds 90-day cap; clamping"
            );
            MAX_TTL
        }
        Some(v) => v,
        None => ADMIN_JWT_TTL_SECS_DEFAULT,
    }
}

/// Backwards-compatible alias — DO NOT use in new code; prefer
/// [`admin_jwt_ttl_secs()`] which honors `CYBERCLAW_JWT_TTL_SECS`.
#[deprecated(since = "1.2.10", note = "use admin_jwt_ttl_secs() — env-aware")]
#[allow(dead_code)]
pub const ADMIN_JWT_TTL_SECS: i64 = ADMIN_JWT_TTL_SECS_DEFAULT;

/// `POST /admin/login` request body.
#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub user_id: String,
    /// Accepted but ignored in the MVP — see module docs.
    #[allow(dead_code)]
    #[serde(default)]
    pub password: Option<String>,
}

/// `POST /admin/login` response body.
#[derive(Debug, Serialize)]
pub struct LoginResponse {
    pub jwt: String,
    pub user: OperatorRecord,
}

/// `POST /admin/logout` response body.
#[derive(Debug, Serialize)]
pub struct LogoutResponse {
    pub ok: bool,
}

/// `GET /admin/me` response body.
#[derive(Debug, Serialize)]
pub struct MeResponse {
    pub user: OperatorRecord,
}

/// `POST /admin/login` — public. Stub password check against `UsersConfig`.
pub async fn admin_login(
    State(state): State<Arc<AppState>>,
    Json(req): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, ApiError> {
    let user_id_str = req.user_id.trim();
    if user_id_str.is_empty() {
        return Err(ApiError::InvalidRequest("user_id is required".to_string()));
    }

    // Parse the caller-supplied id into the typed shape; mis-formatted ids
    // are outright rejected.
    let user_id = UserId::from_string(user_id_str.to_string())
        .map_err(|e| ApiError::InvalidRequest(format!("invalid user_id: {}", e)))?;

    // Load the operator registry from disk. A missing file is equivalent
    // to "no operators yet" — login is rejected.
    let path = UsersConfig::default_path();
    let cfg = UsersConfig::load_from_path(&path).map_err(|e| {
        warn!(path = %path.display(), error = %e, "admin login: failed to load users.toml");
        ApiError::InternalError(format!("failed to load users config: {}", e))
    })?;

    let record = match cfg.find(&user_id).cloned() {
        Some(r) => r,
        None => {
            info!(user_id = %user_id_str, "admin login rejected: unknown operator");
            if let Some(sink) = state.audit.as_ref() {
                sink.record(AuditEntry::now(
                    user_id_str.to_string(),
                    AuditKind::Auth,
                    "login.failed".to_string(),
                    None,
                    serde_json::json!({ "reason": "unknown_operator" }),
                    AuditResult::Failure {
                        reason: "unknown operator".to_string(),
                    },
                ))
                .await;
            }
            return Err(ApiError::Unauthorized("unknown operator".to_string()));
        }
    };

    // Issue a fresh JWT. We re-use the server's existing JWT secret so the
    // token is valid against the same `jwt_auth` middleware every other
    // protected route uses.
    let jwt = generate_jwt(&user_id, state.jwt_secret.as_bytes(), admin_jwt_ttl_secs())
        .map_err(|e| ApiError::InternalError(format!("failed to issue jwt: {}", e)))?;

    info!(user_id = %user_id_str, "admin login success");
    if let Some(sink) = state.audit.as_ref() {
        sink.record(AuditEntry::now(
            user_id_str.to_string(),
            AuditKind::Auth,
            "login.success".to_string(),
            None,
            serde_json::json!({}),
            AuditResult::Success,
        ))
        .await;
    }
    Ok(Json(LoginResponse { jwt, user: record }))
}

/// `POST /admin/logout` — authenticated. Stateless; just acknowledges.
pub async fn admin_logout(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<LogoutResponse>, ApiError> {
    info!(user_id = %claims.sub, "admin logout");
    if let Some(sink) = state.audit.as_ref() {
        sink.record(AuditEntry::now(
            claims.sub.as_str().to_string(),
            AuditKind::Auth,
            "logout".to_string(),
            None,
            serde_json::json!({}),
            AuditResult::Success,
        ))
        .await;
    }
    Ok(Json(LogoutResponse { ok: true }))
}

/// `GET /admin/me` — authenticated. Returns the caller's `OperatorRecord`.
pub async fn admin_me(Extension(claims): Extension<Claims>) -> Result<Json<MeResponse>, ApiError> {
    let path = UsersConfig::default_path();
    let cfg = UsersConfig::load_from_path(&path).map_err(|e| {
        warn!(path = %path.display(), error = %e, "admin me: failed to load users.toml");
        ApiError::InternalError(format!("failed to load users config: {}", e))
    })?;

    let record = cfg.find(&claims.sub).cloned().ok_or_else(|| {
        warn!(user_id = %claims.sub, "admin me: jwt references unknown operator");
        ApiError::Unauthorized("operator no longer exists".to_string())
    })?;

    Ok(Json(MeResponse { user: record }))
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::state::{AppState, ControlPlaneComponents};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::middleware as axum_middleware;
    use axum::routing::{get, post};
    use axum::Router;
    use cyberclaw_connectors::registry::ConnectorRegistry;
    use cyberclaw_control_plane::{
        event_bus::InMemoryEventBus, execution_service::InMemoryExecutionService,
        gateway_router::InMemoryGatewayRouter, lease_manager::InMemoryLeaseManager,
        membership_service::InMemoryMembershipService, orchestrator::ControlPlaneOrchestrator,
        placement_engine::InMemoryPlacementEngine, registry::InMemoryRegistry,
        resolver::InMemoryResolver, review_queue::InMemoryReviewQueue,
        task_manager::InMemoryTaskManager,
    };
    use cyberclaw_governance::engine::DefaultPolicyEngine;
    use cyberclaw_llm::prelude::GenericOpenAiClient;
    use cyberclaw_llm::LlmClient;
    use cyberclaw_observability::events::InMemoryEventRecorder;
    use cyberclaw_observability::security_event_store::InMemorySecurityEventStore;
    use serial_test::serial;
    use tower::ServiceExt;

    pub fn build_test_state() -> Arc<AppState> {
        let llm_client: Arc<dyn LlmClient> =
            Arc::new(GenericOpenAiClient::new(None, "http://localhost:8080").unwrap());
        let connector_registry = Arc::new(ConnectorRegistry::new());
        let policy_engine = Arc::new(DefaultPolicyEngine::default());
        let event_recorder = Arc::new(InMemoryEventRecorder::new());
        let task_manager = Arc::new(InMemoryTaskManager::new());
        let execution_service = Arc::new(InMemoryExecutionService::with_event_recorder(
            event_recorder.clone(),
        ));
        let registry = Arc::new(InMemoryRegistry::new());
        let resolver = Arc::new(InMemoryResolver::new(registry.clone()));
        let review_queue = Arc::new(InMemoryReviewQueue::new(None));
        let gateway = Arc::new(InMemoryGatewayRouter::new());
        let placement_engine = Arc::new(InMemoryPlacementEngine);
        let lease_manager = Arc::new(InMemoryLeaseManager::default());
        let membership_service = Arc::new(InMemoryMembershipService::default());
        let event_bus = Arc::new(InMemoryEventBus::default());
        let security_event_store = Arc::new(InMemorySecurityEventStore::new());
        let plugin_registry = Arc::new(cyberclaw_plugin_runtime::PluginRegistry::new(
            std::path::PathBuf::from("/tmp/test_plugins_admin"),
        ));
        let control_plane = Arc::new(
            ControlPlaneOrchestrator::new(
                gateway,
                resolver,
                registry,
                review_queue.clone(),
                task_manager.clone(),
                execution_service.clone(),
                placement_engine,
                lease_manager,
                membership_service,
                event_bus,
                policy_engine.clone(),
                plugin_registry,
                security_event_store,
            )
            .with_event_recorder(event_recorder.clone()),
        );
        let cp_components = ControlPlaneComponents {
            control_plane,
            task_manager,
            execution_service,
            review_queue,
        };
        Arc::new(AppState::new(
            llm_client,
            connector_registry,
            policy_engine,
            event_recorder,
            "test-admin-jwt-secret".to_string(),
            cp_components,
        ))
    }

    pub fn jwt_for(state: &Arc<AppState>, user: &str) -> String {
        let uid = UserId::from_string(user.to_string()).expect("valid user id");
        generate_jwt(&uid, state.jwt_secret.as_bytes(), 3600).expect("jwt")
    }

    /// Set up a temporary `$HOME` with a `users.toml` containing one
    /// operator record for `user_id`. Returns `(tempdir, user_id,
    /// restore-guard)`. The caller MUST keep the tempdir alive until the
    /// test finishes.
    fn seeded_users_toml(user_id: &str) -> (tempfile::TempDir, UserId, RestoreHome) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let restore = RestoreHome::set(tmp.path());

        let uid = UserId::from_string(user_id.to_string()).expect("valid uid");
        let record = OperatorRecord {
            user_id: uid.clone(),
            display_name: format!("Operator {}", user_id),
            created_at: "2026-04-18T12:00:00Z".to_string(),
            last_login: None,
            role: "admin".to_string(),
            onboarded_at: None,
            intent_auto_route: false,
        };
        let mut cfg = UsersConfig::default();
        cfg.upsert(record);
        let path = UsersConfig::default_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        cfg.save_to_path(&path).expect("save users.toml");
        (tmp, uid, restore)
    }

    /// RAII guard that restores `$HOME` on drop.
    pub(crate) struct RestoreHome {
        original: Option<std::ffi::OsString>,
    }

    impl RestoreHome {
        pub(crate) fn set(new_home: &std::path::Path) -> Self {
            let original = std::env::var_os("HOME");
            unsafe {
                std::env::set_var("HOME", new_home);
            }
            Self { original }
        }
    }

    impl Drop for RestoreHome {
        fn drop(&mut self) {
            unsafe {
                match self.original.take() {
                    Some(v) => std::env::set_var("HOME", v),
                    None => std::env::remove_var("HOME"),
                }
            }
        }
    }

    fn public_app(state: Arc<AppState>) -> Router {
        super::super::create_admin_public_router().with_state(state)
    }

    fn auth_app(state: Arc<AppState>) -> Router {
        Router::new()
            .route("/admin/me", get(admin_me))
            .route("/admin/logout", post(admin_logout))
            .layer(axum_middleware::from_fn_with_state(
                state.clone(),
                crate::middleware::jwt_auth,
            ))
            .with_state(state.clone())
    }

    async fn post_json(
        app: &Router,
        path: &str,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "decode {} body failed: {} — raw: {}",
                path,
                e,
                String::from_utf8_lossy(&bytes)
            )
        });
        (status, json)
    }

    /// 2026-05-18 (v1.2.10): JWT TTL is now env-overridable via
    /// CYBERCLAW_JWT_TTL_SECS. Verifies the parser handles: positive values,
    /// non-positive fallback to default, > 90-day cap clamping, absent env.
    #[test]
    #[serial]
    fn jwt_ttl_env_override() {
        // Absent env → default 24h
        std::env::remove_var("CYBERCLAW_JWT_TTL_SECS");
        assert_eq!(admin_jwt_ttl_secs(), ADMIN_JWT_TTL_SECS_DEFAULT);

        // Positive override → used as-is
        std::env::set_var("CYBERCLAW_JWT_TTL_SECS", "3600");
        assert_eq!(admin_jwt_ttl_secs(), 3600);

        // Negative → falls back to default + warn
        std::env::set_var("CYBERCLAW_JWT_TTL_SECS", "-1");
        assert_eq!(admin_jwt_ttl_secs(), ADMIN_JWT_TTL_SECS_DEFAULT);

        // Zero → falls back to default + warn (same as negative)
        std::env::set_var("CYBERCLAW_JWT_TTL_SECS", "0");
        assert_eq!(admin_jwt_ttl_secs(), ADMIN_JWT_TTL_SECS_DEFAULT);

        // Over 90-day cap → clamped to MAX_TTL (60*60*24*90)
        std::env::set_var("CYBERCLAW_JWT_TTL_SECS", "99999999");
        assert_eq!(admin_jwt_ttl_secs(), 60 * 60 * 24 * 90);

        // Unparseable → falls back to default (parse fails → None branch)
        std::env::set_var("CYBERCLAW_JWT_TTL_SECS", "not-a-number");
        assert_eq!(admin_jwt_ttl_secs(), ADMIN_JWT_TTL_SECS_DEFAULT);

        // Cleanup so other serial tests don't see leftover
        std::env::remove_var("CYBERCLAW_JWT_TTL_SECS");
    }

    #[tokio::test]
    #[serial]
    async fn login_rejects_unknown_user_id() {
        let (_tmp, _uid, _restore) = seeded_users_toml("op-existing");
        let state = build_test_state();
        let app = public_app(state);
        let (status, body) = post_json(
            &app,
            "/admin/login",
            serde_json::json!({ "user_id": "op-not-here", "password": "whatever" }),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"]["type"], "unauthorized");
    }

    #[tokio::test]
    #[serial]
    async fn login_accepts_existing_user_id() {
        let (_tmp, _uid, _restore) = seeded_users_toml("op-alpha");
        let state = build_test_state();
        let app = public_app(state);
        let (status, body) = post_json(
            &app,
            "/admin/login",
            serde_json::json!({ "user_id": "op-alpha", "password": "stub" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.get("jwt").and_then(|v| v.as_str()).is_some());
        assert_eq!(body["user"]["user_id"], "op-alpha");
    }

    #[tokio::test]
    #[serial]
    async fn login_returns_valid_jwt() {
        let (_tmp, _uid, _restore) = seeded_users_toml("op-beta");
        let state = build_test_state();
        let app = public_app(state.clone());
        let (_, body) = post_json(
            &app,
            "/admin/login",
            serde_json::json!({ "user_id": "op-beta" }),
        )
        .await;
        let jwt = body["jwt"].as_str().expect("jwt present");
        // Must verify under the server's secret and resolve to the caller.
        let claims =
            crate::middleware::verify_jwt(jwt, state.jwt_secret.as_bytes()).expect("jwt verifies");
        assert_eq!(claims.sub.as_str(), "op-beta");
    }

    #[tokio::test]
    #[serial]
    async fn login_rejects_empty_user_id() {
        let (_tmp, _uid, _restore) = seeded_users_toml("op-gamma");
        let state = build_test_state();
        let app = public_app(state);
        let (status, _) =
            post_json(&app, "/admin/login", serde_json::json!({ "user_id": "" })).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn me_requires_jwt() {
        let state = build_test_state();
        let app = auth_app(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/admin/me")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    #[serial]
    async fn me_returns_operator_record() {
        let (_tmp, _uid, _restore) = seeded_users_toml("op-delta");
        let state = build_test_state();
        let token = jwt_for(&state, "op-delta");
        let app = auth_app(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/admin/me")
                    .header("authorization", format!("Bearer {}", token))
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
        assert_eq!(json["user"]["user_id"], "op-delta");
        assert_eq!(json["user"]["display_name"], "Operator op-delta");
    }

    #[tokio::test]
    async fn logout_succeeds_with_valid_jwt() {
        let state = build_test_state();
        let token = jwt_for(&state, "op-epsilon");
        let app = auth_app(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/logout")
                    .header("authorization", format!("Bearer {}", token))
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
        assert_eq!(json["ok"], true);
    }

    #[tokio::test]
    async fn logout_requires_jwt() {
        let state = build_test_state();
        let app = auth_app(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/logout")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    #[serial]
    async fn me_rejects_unknown_operator_even_with_valid_jwt() {
        // JWT is signed correctly but references an operator not in users.toml.
        let (_tmp, _uid, _restore) = seeded_users_toml("op-known");
        let state = build_test_state();
        let token = jwt_for(&state, "op-unknown");
        let app = auth_app(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/admin/me")
                    .header("authorization", format!("Bearer {}", token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn login_missing_user_id_field_returns_422_or_400() {
        // Serde should reject the body outright when `user_id` is missing.
        let state = build_test_state();
        let app = public_app(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/login")
                    .header("content-type", "application/json")
                    .body(Body::from("{}".as_bytes().to_vec()))
                    .unwrap(),
            )
            .await
            .unwrap();
        // Axum returns 422 for JSON deserialization errors in tuple extractors.
        assert!(
            resp.status() == StatusCode::UNPROCESSABLE_ENTITY
                || resp.status() == StatusCode::BAD_REQUEST,
            "got unexpected status: {}",
            resp.status()
        );
    }
}
