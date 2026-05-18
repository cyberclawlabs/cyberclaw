//! Profile CRUD API — user-level personality / SOUL preference layer.
//!
//! # Routes
//!
//! | Method   | Path                    | Description                      |
//! |----------|-------------------------|----------------------------------|
//! | `GET`    | `/api/v1/profiles`      | List profiles (tenant-scoped)    |
//! | `POST`   | `/api/v1/profiles`      | Create a new profile             |
//! | `PATCH`  | `/api/v1/profiles/:id`  | Update partial fields            |
//! | `DELETE` | `/api/v1/profiles/:id`  | Remove a profile                 |

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, patch},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use crate::audit::{AuditEntry, AuditKind, AuditResult};
use crate::error::ApiError;
use crate::middleware::auth::Claims;
use crate::profiles_store::{build_new_profile, Profile};
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ProfileListResponse {
    profiles: Vec<Profile>,
    total: usize,
}

// ---------------------------------------------------------------------------
// Request bodies
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CreateProfileRequest {
    pub name: String,
    #[serde(default)]
    pub system_prompt: String,
    pub default_agent_id: Option<String>,
    pub default_model: Option<String>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateProfileRequest {
    pub name: Option<String>,
    pub system_prompt: Option<String>,
    pub default_agent_id: Option<String>,
    pub default_model: Option<String>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /api/v1/profiles` — list profiles.
///
/// When the caller has a tenant scope (`claims.tenant.is_some()`), only
/// profiles owned by `claims.sub` are returned. System actors (no tenant)
/// see all profiles.
async fn list_profiles(
    State(state): State<Arc<AppState>>,
    axum::Extension(claims): axum::Extension<Claims>,
) -> Json<ProfileListResponse> {
    // Tenant-scoped callers see only their own profiles.
    let owner_id = claims.sub.to_string();
    let owner_filter = if claims.tenant().is_some() {
        Some(owner_id.as_str())
    } else {
        None
    };
    let profiles = state.profile_store.list(owner_filter);
    let total = profiles.len();
    Json(ProfileListResponse { profiles, total })
}

/// `POST /api/v1/profiles` — create a new profile.
async fn create_profile(
    State(state): State<Arc<AppState>>,
    axum::Extension(claims): axum::Extension<Claims>,
    Json(body): Json<CreateProfileRequest>,
) -> Result<(StatusCode, Json<Profile>), ApiError> {
    let name = body.name.trim().to_string();
    if name.is_empty() {
        return Err(ApiError::InvalidInput("name is required".into()));
    }

    let profile = build_new_profile(
        name,
        body.system_prompt,
        body.default_agent_id,
        body.default_model,
        body.temperature,
        body.max_tokens,
        claims.sub.to_string(),
    );

    let profile = state.profile_store.create(profile);

    if let Some(sink) = state.audit.as_ref() {
        sink.record(AuditEntry::now(
            claims.sub.to_string(),
            AuditKind::Mutation,
            "profile.create".to_string(),
            Some(profile.id.clone()),
            serde_json::json!({ "name": profile.name }),
            AuditResult::Success,
        ))
        .await;
    }

    Ok((StatusCode::CREATED, Json(profile)))
}

/// `PATCH /api/v1/profiles/:id` — update partial fields.
async fn update_profile(
    State(state): State<Arc<AppState>>,
    axum::Extension(claims): axum::Extension<Claims>,
    Path(id): Path<String>,
    Json(body): Json<UpdateProfileRequest>,
) -> Result<Json<Profile>, ApiError> {
    // Ownership check: non-admin can only update their own profiles.
    let existing = state
        .profile_store
        .get(&id)
        .ok_or_else(|| ApiError::NotFound(format!("profile '{id}' not found")))?;

    // Tenant-scoped callers can only update their own profiles.
    if claims.tenant().is_some() && existing.owner_user_id != claims.sub.as_str() {
        return Err(ApiError::Forbidden("not your profile".into()));
    }

    let profile = state
        .profile_store
        .update(&id, |p| {
            if let Some(name) = body.name {
                let trimmed = name.trim().to_string();
                if !trimmed.is_empty() {
                    p.name = trimmed;
                }
            }
            if let Some(sp) = body.system_prompt {
                p.system_prompt = sp;
            }
            if body.default_agent_id.is_some() {
                p.default_agent_id = body.default_agent_id;
            }
            if body.default_model.is_some() {
                p.default_model = body.default_model;
            }
            if body.temperature.is_some() {
                p.temperature = body.temperature;
            }
            if body.max_tokens.is_some() {
                p.max_tokens = body.max_tokens;
            }
        })
        .ok_or_else(|| ApiError::NotFound(format!("profile '{id}' not found")))?;

    if let Some(sink) = state.audit.as_ref() {
        sink.record(AuditEntry::now(
            claims.sub.to_string(),
            AuditKind::Mutation,
            "profile.update".to_string(),
            Some(id.clone()),
            serde_json::json!({ "name": profile.name }),
            AuditResult::Success,
        ))
        .await;
    }

    Ok(Json(profile))
}

/// `DELETE /api/v1/profiles/:id` — remove a profile.
async fn delete_profile(
    State(state): State<Arc<AppState>>,
    axum::Extension(claims): axum::Extension<Claims>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    // Ownership check: non-admin can only delete their own profiles.
    let existing = state
        .profile_store
        .get(&id)
        .ok_or_else(|| ApiError::NotFound(format!("profile '{id}' not found")))?;

    // Tenant-scoped callers can only delete their own profiles.
    if claims.tenant().is_some() && existing.owner_user_id != claims.sub.as_str() {
        return Err(ApiError::Forbidden("not your profile".into()));
    }

    state.profile_store.delete(&id);

    if let Some(sink) = state.audit.as_ref() {
        sink.record(AuditEntry::now(
            claims.sub.to_string(),
            AuditKind::Mutation,
            "profile.delete".to_string(),
            Some(id.clone()),
            serde_json::json!({ "profile_id": id }),
            AuditResult::Success,
        ))
        .await;
    }

    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Router factory
// ---------------------------------------------------------------------------

pub fn create_profiles_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/profiles", get(list_profiles).post(create_profile))
        .route(
            "/api/v1/profiles/:id",
            patch(update_profile).delete(delete_profile),
        )
}
