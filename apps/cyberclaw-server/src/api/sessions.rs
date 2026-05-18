//! Sessions aggregate view — GET /api/v1/sessions
//!
//! Each "session" maps to a `Conversation` from the conversation store, enriched
//! with computed aggregate fields (message_count, estimated_tokens, model, etc.)
//! that the admin UI Sessions tab displays.

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    routing::get,
    Extension, Json, Router,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::api::chat_conversations::store as conv_store;
use crate::error::ApiError;
use crate::middleware::auth::Claims;
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct SessionSummary {
    pub id: String,
    pub title: String,
    pub owner_user_id: String,
    pub message_count: usize,
    pub model: Option<String>,
    pub estimated_tokens: u64,
    pub created_at: String,
    pub updated_at: String,
    pub last_activity: String,
    pub active_agent_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SessionsResponse {
    pub sessions: Vec<SessionSummary>,
    pub total: usize,
}

// ---------------------------------------------------------------------------
// Query params
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct SessionsQuery {
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub offset: usize,
}

fn default_limit() -> usize {
    20
}

// ---------------------------------------------------------------------------
// Relative time helper
// ---------------------------------------------------------------------------

fn relative_time(updated_at: &chrono::DateTime<Utc>) -> String {
    let secs = (Utc::now() - *updated_at).num_seconds();
    if secs < 0 {
        return "just now".to_string();
    }
    match secs {
        0..=59 => format!("{}s ago", secs),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86399 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86400),
    }
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

async fn list_sessions(
    State(_state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Query(q): Query<SessionsQuery>,
) -> Result<Json<SessionsResponse>, ApiError> {
    let limit = q.limit.clamp(1, 100);
    let offset = q.offset;

    let store = conv_store();
    let all = store.list_all().await;

    // Filter by owner unless the token has no tenant scope (system/admin context).
    // System tokens (tenant == None) see all sessions.
    let caller = claims.sub.to_string();
    let filtered: Vec<_> = if claims.tenant().is_some() {
        all.into_iter()
            .filter(|c| c.owner_user_id == caller)
            .collect()
    } else {
        all
    };

    let total = filtered.len();

    let sessions: Vec<SessionSummary> = filtered
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|conv| {
            let message_count = conv.messages.len();

            // Extract model from last assistant message metadata if available.
            let model = conv
                .messages
                .iter()
                .rev()
                .find(|m| m.role == "assistant")
                .and_then(|m| m.metadata.as_ref())
                .and_then(|meta| meta.get("model"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            // Rough token estimate: total chars / 4.
            let estimated_tokens: u64 = conv
                .messages
                .iter()
                .map(|m| (m.content.len() as u64) / 4)
                .sum();

            let last_activity = relative_time(&conv.updated_at);

            SessionSummary {
                id: conv.id,
                title: conv.title,
                owner_user_id: conv.owner_user_id,
                message_count,
                model,
                estimated_tokens,
                created_at: conv.created_at.to_rfc3339(),
                updated_at: conv.updated_at.to_rfc3339(),
                last_activity,
                active_agent_id: conv.active_agent_id.map(|id| id.to_string()),
            }
        })
        .collect();

    Ok(Json(SessionsResponse { sessions, total }))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn create_sessions_router() -> Router<Arc<AppState>> {
    Router::new().route("/api/v1/sessions", get(list_sessions))
}
