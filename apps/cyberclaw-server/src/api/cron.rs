//! Cron job CRUD API.
//!
//! # Routes
//!
//! | Method | Path | Description |
//! |--------|------|-------------|
//! | `GET`    | `/api/v1/cron`          | List all jobs |
//! | `POST`   | `/api/v1/cron`          | Create a new job |
//! | `PATCH`  | `/api/v1/cron/:id`      | Update fields (enable/disable/rename/…) |
//! | `DELETE` | `/api/v1/cron/:id`      | Remove a job |
//! | `POST`   | `/api/v1/cron/:id/run`  | Fire immediately |

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, patch, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use crate::audit::{AuditEntry, AuditKind, AuditResult};
use crate::cron::{build_new_job, fire_job, normalize_schedule, CronJob};
use crate::error::ApiError;
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct CronListResponse {
    jobs: Vec<CronJob>,
    total: usize,
}

// ---------------------------------------------------------------------------
// Request bodies
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CreateCronJobRequest {
    pub name: String,
    pub schedule: String,
    #[serde(default = "default_action_type")]
    pub action_type: String,
    #[serde(default)]
    pub action_payload: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_action_type() -> String {
    "echo".to_string()
}

fn default_enabled() -> bool {
    true
}

#[derive(Debug, Deserialize)]
pub struct UpdateCronJobRequest {
    pub name: Option<String>,
    pub schedule: Option<String>,
    pub action_type: Option<String>,
    pub action_payload: Option<String>,
    pub enabled: Option<bool>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /api/v1/cron` — list all scheduled jobs.
async fn list_jobs(State(state): State<Arc<AppState>>) -> Json<CronListResponse> {
    let jobs = state.cron_store.list();
    let total = jobs.len();
    Json(CronListResponse { jobs, total })
}

/// `POST /api/v1/cron` — create a new job.
///
/// Validates the cron expression and persists the job. Returns `400` if the
/// expression is invalid.
async fn create_job(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateCronJobRequest>,
) -> Result<(StatusCode, Json<CronJob>), ApiError> {
    let job = build_new_job(
        body.name,
        body.schedule,
        body.action_type,
        body.action_payload,
        body.enabled,
    )
    .map_err(ApiError::InvalidInput)?;

    let job = state.cron_store.add(job);
    Ok((StatusCode::CREATED, Json(job)))
}

/// `PATCH /api/v1/cron/:id` — update fields of an existing job.
///
/// All fields are optional. If `schedule` is updated, `next_run_at` is
/// recomputed. 5-field expressions are auto-normalised to 6 fields.
/// Returns `404` if the id is unknown.
async fn update_job(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateCronJobRequest>,
) -> Result<Json<CronJob>, ApiError> {
    // Normalise and validate new schedule before applying, if provided.
    let normalised_schedule = if let Some(ref sched) = body.schedule {
        let normalised = normalize_schedule(sched);
        normalised.parse::<cron::Schedule>().map_err(|e| {
            ApiError::InvalidInput(format!("invalid cron expression '{normalised}': {e}"))
        })?;
        Some(normalised)
    } else {
        None
    };

    let job = state
        .cron_store
        .update(&id, |j| {
            if let Some(ref name) = body.name {
                j.name = name.clone();
            }
            if let Some(ref sched) = normalised_schedule {
                // Recompute next_run_at when schedule changes.
                j.schedule = sched.clone();
                j.next_run_at = crate::cron::CronJob::compute_next_run(sched, chrono::Utc::now());
            }
            if let Some(ref at) = body.action_type {
                j.action_type = at.clone();
            }
            if let Some(ref ap) = body.action_payload {
                j.action_payload = ap.clone();
            }
            if let Some(en) = body.enabled {
                j.enabled = en;
            }
        })
        .ok_or_else(|| ApiError::NotFound(format!("cron job '{id}' not found")))?;

    if let Some(sink) = state.audit.as_ref() {
        sink.record(AuditEntry::now(
            "system",
            AuditKind::Mutation,
            "cron.update",
            Some(format!("cron:{id}")),
            serde_json::json!({ "job_id": id, "job_name": job.name }),
            AuditResult::Success,
        ))
        .await;
    }

    Ok(Json(job))
}

/// `DELETE /api/v1/cron/:id` — remove a job.
async fn delete_job(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    if state.cron_store.remove(&id) {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound(format!("cron job '{id}' not found")))
    }
}

/// `POST /api/v1/cron/:id/run` — fire a job immediately, ignoring the schedule.
async fn run_job_now(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    claims: Option<axum::Extension<crate::middleware::auth::Claims>>,
) -> Result<Json<CronJob>, ApiError> {
    let job = state
        .cron_store
        .get(&id)
        .ok_or_else(|| ApiError::NotFound(format!("cron job '{id}' not found")))?;

    fire_job(&state.cron_store, &job, chrono::Utc::now());

    // Return the updated job.
    let updated = state
        .cron_store
        .get(&id)
        .ok_or_else(|| ApiError::InternalError("job disappeared after fire".to_string()))?;

    if let Some(sink) = state.audit.as_ref() {
        let actor = claims
            .as_ref()
            .map(|c| c.sub.to_string())
            .unwrap_or_else(|| "system".to_string());
        sink.record(AuditEntry::now(
            actor,
            AuditKind::Mutation,
            "cron.run_now".to_string(),
            Some(format!("cron:{id}")),
            serde_json::json!({ "job_id": id, "name": updated.name }),
            AuditResult::Success,
        ))
        .await;
    }

    Ok(Json(updated))
}

// ---------------------------------------------------------------------------
// Router factory
// ---------------------------------------------------------------------------

pub fn create_cron_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/cron", get(list_jobs).post(create_job))
        .route("/api/v1/cron/:id", patch(update_job).delete(delete_job))
        .route("/api/v1/cron/:id/run", post(run_job_now))
}
