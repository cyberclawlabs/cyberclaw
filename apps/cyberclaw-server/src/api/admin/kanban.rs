//! Admin Kanban endpoints — thin wrapper over `cyberclaw_control_plane::kanban`.
//!
//! | Method | Path                                  | Purpose                       |
//! |--------|---------------------------------------|-------------------------------|
//! | GET    | `/api/v1/admin/kanban/tasks`          | List all tasks                |
//! | POST   | `/api/v1/admin/kanban/tasks`          | Create a new task             |
//! | PUT    | `/api/v1/admin/kanban/tasks/:id`      | Patch task fields             |
//! | DELETE | `/api/v1/admin/kanban/tasks/:id`      | Cancel a task (soft delete)   |
//!
//! The `KanbanBoard` is a SQLite store at `~/.cyberclaw/kanban.db`. We open
//! it lazily on first request and cache the handle in a process-wide
//! `OnceLock`. Mapping notes:
//!
//! - The contract surfaces `priority` + `due_at` + `claimed_by` fields. The
//!   underlying schema does NOT have dedicated columns for `priority` /
//!   `due_at` — those are stored under the freeform `metadata` JSON column.
//! - `claimed_by` corresponds to `KanbanTask.assigned_to`.
//! - Body content (`body`) is stored in the existing `description` column.
//! - F-4 (commit aabcf34 H-8 follow-up): partial updates now flow through
//!   `KanbanBoard::update_task` (COALESCE semantics), replacing the
//!   previous "fields not supported" InvalidInput stub.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use axum::{
    extract::{Path, State},
    routing::{delete as delete_method, get, post, put},
    Json, Router,
};
use cyberclaw_control_plane::kanban::{KanbanBoard, KanbanStatus, KanbanTask, KanbanTaskUpdate};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Lazy board handle
// ---------------------------------------------------------------------------

static BOARD: OnceLock<Mutex<Arc<KanbanBoard>>> = OnceLock::new();

/// Default location: `~/.cyberclaw/kanban.db`. H-6: when `HOME` is
/// unset, fail-loud instead of writing to a CWD-relative path. The
/// server CWD is not under operator control (could be `/`, could be
/// `/var/run`) — silently rooting an SQLite DB there is a footgun.
fn default_kanban_db_path() -> Result<PathBuf, ApiError> {
    if let Some(explicit) = std::env::var_os("CYBERCLAW_KANBAN_DB") {
        return Ok(PathBuf::from(explicit));
    }
    match std::env::var_os("HOME") {
        Some(h) => Ok(PathBuf::from(h).join(".cyberclaw").join("kanban.db")),
        None => Err(ApiError::InternalError(
            "kanban: HOME env var is unset and CYBERCLAW_KANBAN_DB is not configured".to_string(),
        )),
    }
}

fn get_board() -> Result<Arc<KanbanBoard>, ApiError> {
    if let Some(cell) = BOARD.get() {
        let guard = cell.lock().expect("kanban OnceLock mutex poisoned");
        return Ok(guard.clone());
    }
    // H-6: resolve the path BEFORE the OnceLock initializer runs so we can
    // surface an InternalError on missing HOME instead of swallowing it.
    let path = default_kanban_db_path()?;
    let cell = BOARD.get_or_init(|| {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let board = KanbanBoard::open(&path).unwrap_or_else(|e| {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "kanban: failed to open file-backed board, falling back to in-memory"
            );
            KanbanBoard::in_memory().expect("in-memory kanban must construct")
        });
        Mutex::new(Arc::new(board))
    });
    let guard = cell.lock().expect("kanban OnceLock mutex poisoned");
    Ok(guard.clone())
}

// ---------------------------------------------------------------------------
// Public DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct KanbanTaskRow {
    pub id: String,
    pub title: String,
    pub body: String,
    pub status: String,
    pub priority: u8,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claimed_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due_at: Option<i64>,
}

fn status_str(s: KanbanStatus) -> &'static str {
    match s {
        KanbanStatus::Todo => "todo",
        KanbanStatus::InProgress => "in_progress",
        KanbanStatus::Blocked => "blocked",
        KanbanStatus::Done => "done",
        // H-7: Cancelled MUST surface as "cancelled" so an audit reader can
        // tell completed work apart from rolled-back work. parse_status
        // already round-trips this string back to KanbanStatus::Cancelled.
        KanbanStatus::Cancelled => "cancelled",
    }
}

fn task_to_row(t: &KanbanTask) -> KanbanTaskRow {
    let priority = t
        .metadata
        .get("priority")
        .and_then(|v| v.as_u64())
        .map(|v| v.min(255) as u8)
        .unwrap_or(0);
    let due_at = t.metadata.get("due_at").and_then(|v| v.as_i64());
    KanbanTaskRow {
        id: t.id.clone(),
        title: t.title.clone(),
        body: t.description.clone(),
        status: status_str(t.status).to_string(),
        priority,
        created_at: t.created_at.timestamp(),
        updated_at: t.updated_at.timestamp(),
        claimed_by: t.assigned_to.clone(),
        due_at,
    }
}

// ---------------------------------------------------------------------------
// GET
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct ListTasksResponse {
    pub tasks: Vec<KanbanTaskRow>,
}

pub async fn list_tasks(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<ListTasksResponse>, ApiError> {
    let board = get_board()?;
    let tasks = board
        .list(None)
        .map_err(|e| ApiError::InternalError(format!("kanban list: {e}")))?;
    Ok(Json(ListTasksResponse {
        tasks: tasks.iter().map(task_to_row).collect(),
    }))
}

// ---------------------------------------------------------------------------
// POST
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CreateTaskRequest {
    pub title: String,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub priority: Option<u8>,
    #[serde(default)]
    pub due_at: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct CreateTaskResponse {
    pub id: String,
}

pub async fn create_task(
    State(_state): State<Arc<AppState>>,
    Json(req): Json<CreateTaskRequest>,
) -> Result<Json<CreateTaskResponse>, ApiError> {
    if req.title.trim().is_empty() {
        return Err(ApiError::InvalidInput(
            "title must not be empty".to_string(),
        ));
    }
    let board = get_board()?;
    let mut metadata = serde_json::Map::new();
    if let Some(p) = req.priority {
        metadata.insert("priority".to_string(), serde_json::json!(p));
    }
    if let Some(d) = req.due_at {
        metadata.insert("due_at".to_string(), serde_json::json!(d));
    }
    let task = board
        .create_task(
            req.title,
            req.body.unwrap_or_default(),
            Some(serde_json::Value::Object(metadata)),
        )
        .map_err(|e| ApiError::InternalError(format!("kanban create: {e}")))?;
    Ok(Json(CreateTaskResponse { id: task.id }))
}

// ---------------------------------------------------------------------------
// PUT
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct UpdateTaskRequest {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub priority: Option<u8>,
    #[serde(default)]
    pub due_at: Option<i64>,
    #[serde(default)]
    pub claimed_by: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct OkResponse {
    pub ok: bool,
}

fn parse_status(s: &str) -> Option<KanbanStatus> {
    match s {
        "todo" => Some(KanbanStatus::Todo),
        "in_progress" => Some(KanbanStatus::InProgress),
        "blocked" => Some(KanbanStatus::Blocked),
        "done" => Some(KanbanStatus::Done),
        "cancelled" => Some(KanbanStatus::Cancelled),
        _ => None,
    }
}

pub async fn update_task(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<UpdateTaskRequest>,
) -> Result<Json<OkResponse>, ApiError> {
    let board = get_board()?;

    // Translate the wire status (string) into KanbanStatus, fail-loud on
    // unknown values BEFORE we touch the DB.
    let status = match req.status.as_deref() {
        Some(s) => Some(
            parse_status(s)
                .ok_or_else(|| ApiError::InvalidInput(format!("unknown status: {s}")))?,
        ),
        None => None,
    };

    // F-4 (closes commit aabcf34 H-8 stub): real partial update via
    // KanbanBoard::update_task — title/body/priority/due_at/claimed_by are
    // now first-class. Empty Some-string for claimed_by is treated as
    // "clear" so admins can un-assign a worker.
    let updated = board
        .update_task(
            &id,
            KanbanTaskUpdate {
                title: req.title,
                body: req.body,
                status,
                priority: req.priority,
                due_at: req.due_at,
                claimed_by: req
                    .claimed_by
                    .map(|s| if s.is_empty() { None } else { Some(s) }),
            },
        )
        .map_err(|e| ApiError::InternalError(format!("kanban update: {e}")))?;

    if updated.is_none() {
        return Err(ApiError::NotFound(format!("task {} not found", id)));
    }

    Ok(Json(OkResponse { ok: true }))
}

// ---------------------------------------------------------------------------
// DELETE
// ---------------------------------------------------------------------------

pub async fn delete_task(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<OkResponse>, ApiError> {
    let board = get_board()?;
    let existing = board
        .get(&id)
        .map_err(|e| ApiError::InternalError(format!("kanban get: {e}")))?;
    if existing.is_none() {
        return Err(ApiError::NotFound(format!("task {} not found", id)));
    }
    // Soft delete: KanbanBoard has no row delete; we cancel.
    board
        .cancel(&id)
        .map_err(|e| ApiError::InternalError(format!("kanban cancel: {e}")))?;
    Ok(Json(OkResponse { ok: true }))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn create_admin_kanban_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/admin/kanban/tasks", get(list_tasks))
        .route("/api/v1/admin/kanban/tasks", post(create_task))
        .route("/api/v1/admin/kanban/tasks/:id", put(update_task))
        .route("/api/v1/admin/kanban/tasks/:id", delete_method(delete_task))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point_kanban_to_temp() -> tempfile::TempDir {
        // Each test wants an isolated DB. The OnceLock makes that hard, so
        // the tests below intentionally share state and just verify create
        // → list → update → cancel transitions exist.
        tempfile::TempDir::new().unwrap()
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn create_then_list_round_trip() {
        let _tmp = point_kanban_to_temp();
        let state = crate::api::test_helpers::build_test_state();
        let created = create_task(
            State(state.clone()),
            Json(CreateTaskRequest {
                title: "review PR".to_string(),
                body: Some("PR #42".to_string()),
                priority: Some(5),
                due_at: None,
            }),
        )
        .await
        .unwrap();
        assert!(created.0.id.starts_with("kt-"));

        let listed = list_tasks(State(state)).await.unwrap();
        assert!(
            listed.0.tasks.iter().any(|t| t.id == created.0.id),
            "newly created task must show up in list"
        );
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn update_unknown_id_returns_404() {
        let state = crate::api::test_helpers::build_test_state();
        let res = update_task(
            State(state),
            Path("kt-does-not-exist".to_string()),
            Json(UpdateTaskRequest {
                title: None,
                body: None,
                status: Some("done".to_string()),
                priority: None,
                due_at: None,
                claimed_by: None,
            }),
        )
        .await;
        assert!(matches!(res, Err(ApiError::NotFound(_))));
    }

    /// H-7 regression: Cancelled MUST NOT collapse to "done".
    #[test]
    fn h7_cancelled_serialises_distinctly() {
        assert_eq!(status_str(KanbanStatus::Cancelled), "cancelled");
        assert_eq!(status_str(KanbanStatus::Done), "done");
    }

    /// F-4 (replaces H-8 InvalidInput stub): priority is now persisted.
    /// The PUT call must succeed and the updated value must round-trip
    /// through the LIST endpoint.
    #[tokio::test]
    #[serial_test::serial]
    async fn f4_priority_change_persists_via_real_update() {
        let state = crate::api::test_helpers::build_test_state();
        let created = create_task(
            State(state.clone()),
            Json(CreateTaskRequest {
                title: "f4 fixture".to_string(),
                body: None,
                priority: Some(1),
                due_at: None,
            }),
        )
        .await
        .unwrap();
        let res = update_task(
            State(state.clone()),
            Path(created.0.id.clone()),
            Json(UpdateTaskRequest {
                title: None,
                body: None,
                status: None,
                priority: Some(9),
                due_at: None,
                claimed_by: None,
            }),
        )
        .await
        .expect("real update must succeed");
        assert!(res.0.ok);
        // Round-trip via LIST so we exercise the full read path.
        let listed = list_tasks(State(state)).await.unwrap();
        let row = listed
            .0
            .tasks
            .iter()
            .find(|t| t.id == created.0.id)
            .expect("created task must show up in list");
        assert_eq!(row.priority, 9, "priority must round-trip through update");
    }

    /// H-6 regression: missing HOME (and no explicit override) must surface
    /// an InternalError, not silently fall back to a CWD path.
    #[test]
    #[serial_test::serial]
    fn h6_missing_home_fails_loud() {
        let original_home = std::env::var_os("HOME");
        let original_db = std::env::var_os("CYBERCLAW_KANBAN_DB");
        unsafe {
            std::env::remove_var("HOME");
            std::env::remove_var("CYBERCLAW_KANBAN_DB");
        }
        let res = default_kanban_db_path();
        // Restore env BEFORE asserting so failure does not leak state.
        unsafe {
            match original_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            match original_db {
                Some(v) => std::env::set_var("CYBERCLAW_KANBAN_DB", v),
                None => std::env::remove_var("CYBERCLAW_KANBAN_DB"),
            }
        }
        assert!(matches!(res, Err(ApiError::InternalError(_))));
    }

    #[test]
    fn parse_status_known_values() {
        assert_eq!(parse_status("todo"), Some(KanbanStatus::Todo));
        assert_eq!(parse_status("in_progress"), Some(KanbanStatus::InProgress));
        assert_eq!(parse_status("done"), Some(KanbanStatus::Done));
        assert_eq!(parse_status("nope"), None);
    }
}
