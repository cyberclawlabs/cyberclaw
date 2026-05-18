//! GET /api/v1/usage — in-memory usage statistics snapshot.

use axum::{extract::State, routing::get, Json, Router};
use std::sync::Arc;

use crate::state::AppState;
use crate::usage::UsageResponse;

/// Handler: return a snapshot of current usage counters.
async fn get_usage(State(state): State<Arc<AppState>>) -> Json<UsageResponse> {
    Json(state.usage.snapshot())
}

pub fn create_usage_router() -> Router<Arc<AppState>> {
    Router::new().route("/api/v1/usage", get(get_usage))
}
