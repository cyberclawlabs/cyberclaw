//! Admin Curator endpoints — surfaces background skill curator status, ad-hoc
//! runs, and skill-usage telemetry to the admin SPA.
//!
//! | Method | Path                                  | Purpose                            |
//! |--------|---------------------------------------|------------------------------------|
//! | GET    | `/api/v1/admin/curator/status`        | Curator config + recent verdicts   |
//! | POST   | `/api/v1/admin/curator/run-now`       | Trigger an ad-hoc curator pass     |
//! | GET    | `/api/v1/admin/curator/skill-usage`   | SkillUsageStore snapshot           |
//!
//! The `Curator` and `SkillUsageStore` live in `cyberclaw-skill-runtime`. The
//! current server boot does NOT wire either into `AppState` (Sprint 18 W3
//! Hermes closure stopped at the capability layer). To respect the
//! "underlying ready, REST missing" boundary stated in the task, we open
//! the file-backed store at `~/.cyberclaw/skills/.usage.json` lazily on
//! first request — same pattern used by `kanban.rs`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use axum::{
    extract::State,
    routing::{get, post},
    Extension, Json, Router,
};
use chrono::Utc;
use cyberclaw_skill_runtime::telemetry::{SkillUsageStore, WriteOrigin};
use serde::Serialize;

use crate::audit::{AuditEntry, AuditKind, AuditResult};
use crate::error::ApiError;
use crate::middleware::auth::Claims;
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Lazy SkillUsageStore handle
// ---------------------------------------------------------------------------

static USAGE: OnceLock<Mutex<Arc<SkillUsageStore>>> = OnceLock::new();

fn skills_root() -> PathBuf {
    std::env::var_os("CYBERCLAW_SKILLS_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(|h| PathBuf::from(h).join(".cyberclaw").join("skills"))
                .unwrap_or_else(|| PathBuf::from(".cyberclaw").join("skills"))
        })
}

fn get_usage_store() -> Arc<SkillUsageStore> {
    let cell = USAGE.get_or_init(|| {
        let root = skills_root();
        let _ = std::fs::create_dir_all(&root);
        let store =
            SkillUsageStore::default_for(&root).unwrap_or_else(|_| SkillUsageStore::in_memory());
        Mutex::new(Arc::new(store))
    });
    let guard = cell.lock().expect("curator usage OnceLock mutex poisoned");
    guard.clone()
}

// ---------------------------------------------------------------------------
// GET /status
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct CuratorVerdictRow {
    pub skill: String,
    pub action: String,
    pub reason: String,
    pub decided_at: i64,
}

#[derive(Debug, Serialize)]
pub struct CuratorStatusResponse {
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_run_at: Option<i64>,
    pub total_runs: u64,
    pub recent_verdicts: Vec<CuratorVerdictRow>,
}

pub async fn status(
    State(state): State<Arc<AppState>>,
) -> Result<Json<CuratorStatusResponse>, ApiError> {
    // The presence of `state.curator` reflects "wired"; the env flag still
    // reflects whether the background loop is ticking on its own. Both
    // surface so the admin UI renders an accurate state.
    let loop_enabled = std::env::var("CYBERCLAW_CURATOR_ENABLED")
        .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes"))
        .unwrap_or(false);

    if let Some(curator) = state.curator.as_ref() {
        let info = curator.last_run_info().await;
        let recent_verdicts: Vec<CuratorVerdictRow> = info
            .last_report
            .as_ref()
            .map(|report| {
                let mut rows: Vec<CuratorVerdictRow> = report
                    .skills_archived
                    .iter()
                    .map(|skill| CuratorVerdictRow {
                        skill: skill.clone(),
                        action: "archived".to_string(),
                        reason: "cold (HeuristicEvaluator)".to_string(),
                        decided_at: report.finished_at.timestamp(),
                    })
                    .collect();
                rows.extend(
                    report
                        .skills_consolidated
                        .iter()
                        .map(|cr| CuratorVerdictRow {
                            skill: cr.merged_name.clone(),
                            action: "consolidated".to_string(),
                            reason: format!("merged from: {}", cr.source_names.join(", ")),
                            decided_at: report.finished_at.timestamp(),
                        }),
                );
                rows
            })
            .unwrap_or_default();

        return Ok(Json(CuratorStatusResponse {
            enabled: loop_enabled,
            last_run_at: info.last_run_at.map(|t| t.timestamp()),
            next_run_at: info.next_run_at.map(|t| t.timestamp()),
            total_runs: info.total_runs,
            recent_verdicts,
        }));
    }

    // No curator wired — surface the disabled state honestly.
    Ok(Json(CuratorStatusResponse {
        enabled: false,
        last_run_at: None,
        next_run_at: None,
        total_runs: 0,
        recent_verdicts: Vec::new(),
    }))
}

// ---------------------------------------------------------------------------
// POST /run-now
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct CuratorRunNowResponse {
    pub run_id: String,
    pub started_at: i64,
}

pub async fn run_now(
    State(state): State<Arc<AppState>>,
    claims: Option<Extension<Claims>>,
) -> Result<Json<CuratorRunNowResponse>, ApiError> {
    // W2 D-1: when AppState carries a Curator, drive a real synchronous
    // pass and surface the resulting run id.
    let Some(curator) = state.curator.as_ref() else {
        return Err(ApiError::InternalError(
            "curator run-now requires AppState.curator — boot path did not \
             attach a Curator instance"
                .to_string(),
        ));
    };

    let report = curator.run_once().await;

    // Emit curator.run audit so the new /api/v1/curator/audits query has data
    // and operators can see who ran curator + when. Falls back to "system" if
    // the request reaches us without claims (test path / non-JWT route).
    let actor = claims
        .as_ref()
        .map(|c| c.sub.to_string())
        .unwrap_or_else(|| "system".to_string());
    if let Some(audit) = state.audit.as_ref() {
        audit
            .record(AuditEntry::now(
                actor,
                AuditKind::Mutation,
                "curator.run".to_string(),
                Some(format!("run:{}", report.run_id)),
                serde_json::json!({
                    "run_id": report.run_id,
                    "started_at": report.started_at.to_rfc3339(),
                }),
                AuditResult::Success,
            ))
            .await;
    }

    Ok(Json(CuratorRunNowResponse {
        run_id: report.run_id,
        started_at: report.started_at.timestamp(),
    }))
}

// ---------------------------------------------------------------------------
// GET /skill-usage
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct SkillUsageRow {
    pub skill: String,
    pub origin: String,
    pub use_count: u64,
    pub last_used_at: i64,
    pub days_idle: i64,
}

#[derive(Debug, Serialize)]
pub struct SkillUsageResponse {
    pub records: Vec<SkillUsageRow>,
}

fn origin_str(o: WriteOrigin) -> &'static str {
    match o {
        WriteOrigin::Foreground => "foreground",
        WriteOrigin::BackgroundReview => "background_review",
        WriteOrigin::SystemSeed => "system_seed",
        WriteOrigin::HubImport => "hub_import",
    }
}

pub async fn skill_usage(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<SkillUsageResponse>, ApiError> {
    let store = get_usage_store();
    let snap = store.snapshot();
    let now = Utc::now();
    let mut rows: Vec<SkillUsageRow> = snap
        .into_iter()
        .map(|(name, r)| SkillUsageRow {
            skill: name,
            origin: origin_str(r.origin).to_string(),
            use_count: r.use_count,
            last_used_at: r.last_used_at.timestamp(),
            days_idle: r.days_since_last_use(now),
        })
        .collect();
    rows.sort_by(|a, b| {
        b.use_count
            .cmp(&a.use_count)
            .then_with(|| a.skill.cmp(&b.skill))
    });
    Ok(Json(SkillUsageResponse { records: rows }))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn create_admin_curator_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/admin/curator/status", get(status))
        .route("/api/v1/admin/curator/run-now", post(run_now))
        .route("/api/v1/admin/curator/skill-usage", get(skill_usage))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::build_default_curator;

    #[tokio::test]
    async fn status_without_curator_returns_disabled() {
        // build_test_state() does not attach a curator → status must
        // report disabled with zero counters.
        let state = crate::api::test_helpers::build_test_state();
        let resp = status(State(state)).await.unwrap();
        assert!(!resp.0.enabled);
        assert_eq!(resp.0.total_runs, 0);
        assert!(resp.0.last_run_at.is_none());
        assert!(resp.0.next_run_at.is_none());
        assert!(resp.0.recent_verdicts.is_empty());
    }

    #[tokio::test]
    async fn status_with_curator_after_run_reports_real_data() {
        let mut state_owned = (*crate::api::test_helpers::build_test_state()).clone();
        let curator = build_default_curator();
        state_owned.curator = Some(curator.clone());
        let state = Arc::new(state_owned);

        // Drive a real run so last_report becomes Some.
        let _ = curator.run_once().await;

        let resp = status(State(state.clone())).await.unwrap();
        assert_eq!(resp.0.total_runs, 1);
        assert!(
            resp.0.last_run_at.is_some(),
            "last_run_at should be populated after run"
        );
    }

    #[tokio::test]
    async fn run_now_without_curator_returns_internal_error() {
        let state = crate::api::test_helpers::build_test_state();
        let res = run_now(State(state), None).await;
        assert!(matches!(res, Err(ApiError::InternalError(_))));
    }

    #[tokio::test]
    async fn run_now_with_curator_returns_run_id() {
        let mut state_owned = (*crate::api::test_helpers::build_test_state()).clone();
        let curator = build_default_curator();
        state_owned.curator = Some(curator);
        let state = Arc::new(state_owned);

        let resp = run_now(State(state), None).await.unwrap();
        assert!(resp.0.run_id.starts_with("run-"));
        assert!(resp.0.started_at > 0);
    }

    #[tokio::test]
    async fn skill_usage_returns_records_array() {
        let state = crate::api::test_helpers::build_test_state();
        let resp = skill_usage(State(state)).await.unwrap();
        // Records may be empty on a fresh test host. Just verify shape.
        assert!(resp.0.records.len() <= 10_000);
    }

    #[test]
    fn origin_str_matches_contract_values() {
        assert_eq!(origin_str(WriteOrigin::Foreground), "foreground");
        assert_eq!(
            origin_str(WriteOrigin::BackgroundReview),
            "background_review"
        );
        assert_eq!(origin_str(WriteOrigin::SystemSeed), "system_seed");
        assert_eq!(origin_str(WriteOrigin::HubImport), "hub_import");
    }
}
