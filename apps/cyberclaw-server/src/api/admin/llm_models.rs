//! Admin LLM models management — read/delete/set-default for the model
//! catalog the WebUI ModelsPage and chat composer use.
//!
//! Storage: `~/.cyberclaw/models.json`. If absent, seeded from a built-in
//! default set on first GET. Provider-agnostic flat list.
//!
//! Endpoints:
//! - GET    /api/v1/admin/llm/models           — list + current_default
//! - DELETE /api/v1/admin/llm/models/:id       — remove from list
//! - PUT    /api/v1/admin/llm/models/default   — body `{ "id": "..." }`
//! - POST   /api/v1/admin/llm/models           — add new model

use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    extract::{Path, State},
    routing::{delete as delete_method, get, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tokio::fs;

use crate::error::ApiError;
use crate::state::AppState;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelEntry {
    pub id: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsCatalog {
    pub current_default: String,
    pub models: Vec<ModelEntry>,
}

impl ModelsCatalog {
    /// 种子：只包含 server 当前配置的 default model（从 LLM_DEFAULT_MODEL env）。
    /// 不预填其他 provider 的模型，避免 dropdown 显示 server 实际无法调用的条目。
    /// 用户可在 Models 页手动 add 其他模型。
    fn seed() -> Self {
        let default_model =
            std::env::var("LLM_DEFAULT_MODEL").unwrap_or_else(|_| "deepseek-chat".to_string());
        let provider = std::env::var("LLM_PROVIDER").ok();
        Self {
            current_default: default_model.clone(),
            models: vec![ModelEntry {
                id: default_model.clone(),
                label: Some(default_model),
                provider,
                role: Some("default".to_string()),
            }],
        }
    }
}

fn models_file_path() -> Result<PathBuf, ApiError> {
    let home = std::env::var("HOME")
        .map_err(|_| ApiError::InternalError("HOME env var not set".to_string()))?;
    Ok(PathBuf::from(home).join(".cyberclaw").join("models.json"))
}

async fn load_or_seed() -> Result<ModelsCatalog, ApiError> {
    let path = models_file_path()?;
    if let Ok(body) = fs::read_to_string(&path).await {
        let cat: ModelsCatalog = serde_json::from_str(&body)
            .map_err(|e| ApiError::InternalError(format!("parse models.json: {e}")))?;
        return Ok(cat);
    }
    // seed + persist
    let seeded = ModelsCatalog::seed();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent).await;
    }
    let body = serde_json::to_string_pretty(&seeded)
        .map_err(|e| ApiError::InternalError(format!("serialize seed: {e}")))?;
    let _ = fs::write(&path, body).await;
    Ok(seeded)
}

async fn write_catalog(cat: &ModelsCatalog) -> Result<(), ApiError> {
    let path = models_file_path()?;
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent).await;
    }
    let body = serde_json::to_string_pretty(cat)
        .map_err(|e| ApiError::InternalError(format!("serialize models.json: {e}")))?;
    fs::write(&path, body)
        .await
        .map_err(|e| ApiError::InternalError(format!("write models.json: {e}")))?;
    Ok(())
}

// GET /api/v1/admin/llm/models
pub async fn list_models(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<ModelsCatalog>, ApiError> {
    Ok(Json(load_or_seed().await?))
}

// DELETE /api/v1/admin/llm/models/:id
pub async fn delete_model(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ModelsCatalog>, ApiError> {
    let mut cat = load_or_seed().await?;
    let before = cat.models.len();
    cat.models.retain(|m| m.id != id);
    if cat.models.len() == before {
        return Err(ApiError::InvalidInput(format!("model '{}' not found", id)));
    }
    if cat.models.is_empty() {
        return Err(ApiError::InvalidInput(
            "cannot delete last model; at least one must remain".to_string(),
        ));
    }
    // 删除的是当前默认 → 选第一条作为新默认
    if cat.current_default == id {
        cat.current_default = cat.models[0].id.clone();
    }
    write_catalog(&cat).await?;
    Ok(Json(cat))
}

#[derive(Debug, Deserialize)]
pub struct SetDefaultRequest {
    pub id: String,
}

// PUT /api/v1/admin/llm/models/default
pub async fn set_default(
    State(_state): State<Arc<AppState>>,
    Json(req): Json<SetDefaultRequest>,
) -> Result<Json<ModelsCatalog>, ApiError> {
    let mut cat = load_or_seed().await?;
    if !cat.models.iter().any(|m| m.id == req.id) {
        return Err(ApiError::InvalidInput(format!(
            "model '{}' is not in the catalog — add it first",
            req.id
        )));
    }
    cat.current_default = req.id;
    write_catalog(&cat).await?;
    Ok(Json(cat))
}

// POST /api/v1/admin/llm/models
pub async fn add_model(
    State(_state): State<Arc<AppState>>,
    Json(req): Json<ModelEntry>,
) -> Result<Json<ModelsCatalog>, ApiError> {
    if req.id.trim().is_empty() {
        return Err(ApiError::InvalidInput("id must not be empty".to_string()));
    }
    let mut cat = load_or_seed().await?;
    if cat.models.iter().any(|m| m.id == req.id) {
        return Err(ApiError::InvalidInput(format!(
            "model '{}' already in catalog",
            req.id
        )));
    }
    cat.models.push(req);
    write_catalog(&cat).await?;
    Ok(Json(cat))
}

pub fn create_admin_llm_models_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/admin/llm/models", get(list_models).post(add_model))
        .route("/api/v1/admin/llm/models/default", put(set_default))
        .route(
            "/api/v1/admin/llm/models/:id",
            delete_method(delete_model),
        )
}
