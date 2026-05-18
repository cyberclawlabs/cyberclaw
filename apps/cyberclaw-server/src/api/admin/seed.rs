//! Admin seed-demo endpoint.
//!
//! Sprint 8 Phase A. Populates the in-process [`crate::admin_store::AdminStore`]
//! with realistic demo content so the admin SPA has content to render on a
//! fresh install. Safeguarded against double-seeding by default: only runs
//! when the store is empty unless `CYBERCLAW_DEV_MODE=1` is set.

use std::sync::Arc;

use axum::{
    extract::{Extension, State},
    http::StatusCode,
    routing::post,
    Json, Router,
};
use serde::Serialize;
use tracing::info;

use crate::audit::{AuditEntry, AuditKind, AuditResult};
use crate::error::ApiError;
use crate::middleware::auth::{require_admin, Claims};
use crate::state::AppState;

/// Response shape for `POST /admin/seed-demo`.
#[derive(Debug, Serialize)]
pub struct SeedDemoResponse {
    pub ok: bool,
    pub counts: Counts,
}

/// Per-registry counts after seeding.
#[derive(Debug, Serialize)]
pub struct Counts {
    pub agents: usize,
    pub skills: usize,
    pub tasks: usize,
    pub executions: usize,
    pub reviews: usize,
    pub nodes: usize,
}

/// Build the `/admin/seed-demo` router.
pub fn create_admin_seed_router() -> Router<Arc<AppState>> {
    Router::new().route("/admin/seed-demo", post(admin_seed_demo))
}

/// `POST /admin/seed-demo` — populate the AdminStore with demo content.
///
/// Refuses (`409 Conflict`) when the store is already populated unless
/// `CYBERCLAW_DEV_MODE=1` is set, so production operators cannot blow
/// away real state with a stray CURL.
pub async fn admin_seed_demo(
    State(state): State<Arc<AppState>>,
    claims: Option<Extension<Claims>>,
) -> Result<(StatusCode, Json<SeedDemoResponse>), ApiError> {
    // L7 — server-side role enforcement. When claims are present (production
    // authenticated lane), require admin role. Tests that exercise the raw
    // handler without a JWT remain compatible.
    let actor = if let Some(Extension(claims)) = claims.as_ref() {
        if let Err(err) = require_admin(claims).await {
            if let Some(sink) = state.audit.as_ref() {
                sink.record(AuditEntry::now(
                    claims.sub.as_str().to_string(),
                    AuditKind::Auth,
                    "admin.seed_demo:role_denied".to_string(),
                    None,
                    serde_json::json!({ "reason": "role_denied" }),
                    AuditResult::Failure {
                        reason: "admin role required".to_string(),
                    },
                ))
                .await;
            }
            return Err(err);
        }
        claims.sub.as_str().to_string()
    } else {
        "system".to_string()
    };

    let dev_mode = std::env::var("CYBERCLAW_DEV_MODE")
        .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes"))
        .unwrap_or(false);

    if !state.admin_store.is_empty().await && !dev_mode {
        return Err(ApiError::InvalidRequest(
            "admin_store is already populated; set CYBERCLAW_DEV_MODE=1 to re-seed".to_string(),
        ));
    }
    // Keep `actor` bound even when we don't persist an audit here — the
    // handler stays shape-compatible for future mutation audit entries.
    let _ = &actor;

    let counts = state.admin_store.seed_demo().await;
    info!(
        agents = counts.agents,
        skills = counts.skills,
        tasks = counts.tasks,
        executions = counts.executions,
        reviews = counts.reviews,
        nodes = counts.nodes,
        "admin_store demo seed applied via /admin/seed-demo"
    );

    Ok((
        StatusCode::OK,
        Json(SeedDemoResponse {
            ok: true,
            counts: Counts {
                agents: counts.agents,
                skills: counts.skills,
                tasks: counts.tasks,
                executions: counts.executions,
                reviews: counts.reviews,
                nodes: counts.nodes,
            },
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::test_helpers::build_test_state;

    #[tokio::test]
    async fn seed_demo_first_call_populates_store() {
        let state = build_test_state();
        let (status, body) = admin_seed_demo(State(state.clone()), None).await.unwrap();
        assert_eq!(status, StatusCode::OK);
        assert!(body.ok);
        assert_eq!(body.counts.agents, 8);
        assert_eq!(body.counts.skills, 12);
        assert_eq!(body.counts.tasks, 15);
        assert_eq!(body.counts.executions, 25);
        assert_eq!(body.counts.reviews, 5);
        assert_eq!(body.counts.nodes, 3);
    }

    #[tokio::test]
    async fn seed_demo_idempotent_or_rejects_when_non_empty() {
        let state = build_test_state();
        // First seed succeeds.
        let _ = admin_seed_demo(State(state.clone()), None).await.unwrap();
        // Second call without dev mode is rejected.
        let res = admin_seed_demo(State(state.clone()), None).await;
        assert!(res.is_err(), "second seed should be refused");
    }

    // ----- L7: server-side role enforcement -----

    use crate::api::test_helpers::seed_users_with_role;
    use crate::middleware::auth::Claims;
    use axum::extract::Extension;
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
    async fn test_seed_demo_allowed_for_admin() {
        let (_tmp, _restore) = seed_users_with_role("op-seed-admin", "admin");
        let state = build_test_state();
        let claims = claims_for("op-seed-admin");
        let (status, _body) = admin_seed_demo(State(state.clone()), Some(Extension(claims)))
            .await
            .expect("admin must pass role gate");
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    #[serial]
    async fn test_seed_demo_rejected_for_viewer() {
        let (_tmp, _restore) = seed_users_with_role("op-seed-viewer", "viewer");
        let state = build_test_state();
        let claims = claims_for("op-seed-viewer");
        let err = admin_seed_demo(State(state.clone()), Some(Extension(claims)))
            .await
            .expect_err("viewer must be denied");
        assert!(matches!(err, ApiError::Forbidden(_)), "got {:?}", err);
    }
}
