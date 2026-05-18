//! Tasks API
//!
//! 提供任务 CRUD 操作的 RESTful API

use axum::{
    extract::{Extension, Path, State},
    response::IntoResponse,
    routing::{delete, get, post},
    Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;
use tracing::{debug, error, info};

use cyberclaw_control_plane::execution_service::ExecutionService;
use cyberclaw_core::enums::Priority;
use cyberclaw_core::execution::ExecutionStatus;
use cyberclaw_core::identity::{ActorRef, ActorType};
use cyberclaw_core::ids::{ActorId, ExecutionId, TaskId};
use cyberclaw_core::task::{Task, TaskInput, TaskKind, TriggerRef};

use crate::admin_store::AdminTask;
use crate::api::admin::events::AdminEvent;
use crate::audit::{AuditEntry, AuditKind, AuditResult};
use crate::error::ApiError;
use crate::middleware::auth::Claims;
use crate::state::AppState;
use crate::types::{TaskResponse, TaskStatus, TaskStatusResponse, TaskWithStatus};

/// 创建任务请求
#[derive(Debug, Deserialize)]
pub struct CreateTaskRequest {
    pub title: String,
    /// Task body. Admin SPA NewTaskTemplate dialog sends `description`;
    /// keep `summary` as the canonical field with `description` as alias
    /// so the frontend works without re-mapping.
    #[serde(alias = "description", default)]
    pub summary: String,
    /// Task kind. Frontend NewTaskTemplate infers this from the selected
    /// template (Bug Fix / Feature / Code Review / Research) but does not
    /// send an explicit `kind` field — default to Generic.
    #[serde(default = "default_task_kind")]
    pub kind: TaskKind,
    #[serde(default = "default_priority")]
    pub priority: Priority,
    pub input: Option<serde_json::Value>,
    /// Accept either `labels` (canonical) or `tags` (frontend).
    #[serde(alias = "tags", default)]
    pub labels: Vec<String>,
    /// Frontend sends `assigned_agent` in the template dialog; keep for audit.
    #[serde(default)]
    pub assigned_agent: Option<String>,
}

fn default_task_kind() -> TaskKind {
    TaskKind::Custom("generic".to_string())
}

fn default_priority() -> Priority {
    Priority::Medium
}

fn map_execution_status(status: &ExecutionStatus) -> TaskStatus {
    match status {
        ExecutionStatus::Pending
        | ExecutionStatus::WaitingReview
        | ExecutionStatus::WaitingApproval => TaskStatus::Pending,
        ExecutionStatus::Running => TaskStatus::Running,
        ExecutionStatus::Completed => TaskStatus::Completed,
        ExecutionStatus::Failed | ExecutionStatus::TimedOut => TaskStatus::Failed,
        ExecutionStatus::Cancelled => TaskStatus::Cancelled,
    }
}

async fn refresh_task_from_execution(
    state: &Arc<AppState>,
    task_key: &str,
) -> Result<TaskWithStatus, ApiError> {
    let mut task = {
        let tasks = state.task_store.read().await;
        tasks
            .get(task_key)
            .cloned()
            .ok_or_else(|| ApiError::NotFound(format!("Task not found: {}", task_key)))?
    };

    let mut execution_status = None;

    if let Some(execution_id_str) = &task.execution_id {
        let execution_id = ExecutionId::from_string(execution_id_str.clone()).map_err(|e| {
            ApiError::InternalError(format!(
                "Invalid execution_id '{}' for task {}: {}",
                execution_id_str, task_key, e
            ))
        })?;

        execution_status = state
            .execution_service
            .get(&execution_id)
            .await
            .map_err(|e| ApiError::InternalError(format!("Failed to fetch execution: {}", e)))?
            .map(|execution| (execution.id, execution.status));
    }

    if execution_status.is_none() {
        let executions = state
            .execution_service
            .list_by_task_id(&task.task.id)
            .await
            .map_err(|e| {
                ApiError::InternalError(format!(
                    "Failed to list executions by task id {}: {}",
                    task.task.id, e
                ))
            })?;

        if let Some(execution) = executions.first() {
            execution_status = Some((execution.id.clone(), execution.status.clone()));
        }
    }

    if let Some((execution_id, status)) = execution_status {
        task.execution_id = Some(execution_id.as_str().to_string());
        task.status = map_execution_status(&status);
        if matches!(task.status, TaskStatus::Failed) && task.error.is_none() {
            task.error = Some(format!("Execution ended in {:?} state", status));
        }

        let mut tasks = state.task_store.write().await;
        tasks.insert(task_key.to_string(), task.clone());
    }

    Ok(task)
}

/// 创建 Tasks API 路由
pub fn create_tasks_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/tasks", post(create_task))
        .route("/api/v1/tasks", get(list_tasks))
        .route("/api/v1/tasks/:id", get(get_task))
        .route("/api/v1/tasks/:id/status", get(get_task_status))
        .route("/api/v1/tasks/:id", delete(cancel_task))
}

/// POST /api/v1/tasks - 创建任务
async fn create_task(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<CreateTaskRequest>,
) -> Result<impl IntoResponse, ApiError> {
    info!(
        "Creating new task: title={}, caller={}",
        req.title, claims.sub
    );

    // 验证输入
    if req.title.is_empty() {
        return Err(ApiError::InvalidRequest(
            "title cannot be empty".to_string(),
        ));
    }

    // 创建任务 ID
    let new_task_id = TaskId::from_string(format!("task-{}", uuid::Uuid::new_v4()))
        .map_err(|e| ApiError::InternalError(format!("Failed to create task ID: {}", e)))?;

    // 从 JWT Claims 构建 ActorRef（使用真实身份）
    let actor_id = ActorId::from_string(claims.sub.as_str().to_string()).unwrap_or_else(|_| {
        ActorId::from_string("api-user".to_string()).expect("fallback actor id must be valid")
    });

    // 构造 ActorRef
    let actor = ActorRef {
        id: actor_id,
        actor_type: ActorType::Human,
        tenant_id: claims.tenant.clone(),
        home_node_id: None,
        display_name: claims.sub.as_str().to_string(),
    };

    let task = Task {
        id: new_task_id.clone(),
        case_id: None,
        title: req.title,
        summary: req.summary,
        kind: req.kind,
        priority: req.priority,
        requested_by: actor.clone(),
        requested_at: chrono::Utc::now(),
        trigger: TriggerRef {
            kind: "api".to_string(),
            source: "rest-api".to_string(),
        },
        input: TaskInput {
            payload: req.input.unwrap_or(serde_json::Value::Null),
        },
        desired_outputs: vec![],
        labels: req.labels,
        preferred_agent_id: None,
    };

    // 验证任务
    task.validate()
        .map_err(|e| ApiError::InvalidRequest(format!("Task validation failed: {}", e)))?;

    // P0-2 Phase: 使用 process_ingress 主链（修复架构旁路）
    use cyberclaw_control_plane::gateway_router::IngressRequest;

    let ingress_request = IngressRequest {
        actor,
        session: None,
        workspace: None,
        task: task.clone(),
    };

    // 通过 process_ingress 主链提交（完整治理）
    let submit_result = state
        .control_plane
        .process_ingress(ingress_request)
        .await
        .map_err(|e| {
            error!("Task ingress failed for caller={}: {:?}", claims.sub, e);
            ApiError::InternalError(format!("Task ingress failed: {:?}", e))
        })?;

    let dispatched_task_id = new_task_id; // 使用原 task_id

    debug!(
        "Task submitted via process_ingress: id={}, execution_id={:?}, caller={}",
        dispatched_task_id.as_str(),
        submit_result.execution_id,
        claims.sub
    );

    // 存储任务状态到本地 task_store（供查询使用）
    let task_with_status = TaskWithStatus {
        task: task.clone(),
        execution_id: Some(submit_result.execution_id.as_str().to_string()),
        status: TaskStatus::Pending,
        result: None,
        error: None,
    };

    let task_key = dispatched_task_id.as_str().to_string();
    {
        let mut tasks = state.task_store.write().await;
        tasks.insert(task_key.clone(), task_with_status.clone());
    }

    info!(
        "Task created and dispatched: id={}, caller={}",
        task_key, claims.sub
    );

    // Sprint 8 Phase A — reflect in admin_store + broadcast event.
    {
        let mut seeded = state.admin_store.tasks.write().await;
        seeded.insert(
            0,
            AdminTask {
                task_id: task_key.clone(),
                description: task_with_status.task.summary.clone(),
                status: "pending".to_string(),
                created_at: task_with_status.task.requested_at.to_rfc3339(),
                elapsed: "—".to_string(),
                task_type: format!("{:?}", task_with_status.task.kind).to_lowercase(),
                error: None,
            },
        );
    }
    let _ = state.admin_event_bus.send(AdminEvent::TaskStatusChanged {
        task_id: task_key.clone(),
        status: "pending".to_string(),
    });

    if let Some(sink) = state.audit.as_ref() {
        sink.record(AuditEntry::now(
            claims.sub.as_str().to_string(),
            AuditKind::Mutation,
            "task.create".to_string(),
            Some(format!("task:{}", task_key)),
            serde_json::json!({ "title": task_with_status.task.title }),
            AuditResult::Success,
        ))
        .await;
    }

    let response: TaskResponse = task_with_status.into();
    Ok((axum::http::StatusCode::CREATED, Json(response)))
}

/// GET /api/v1/tasks - 列出所有任务
///
/// Sprint 8 Phase A: supports optional `?status=` filter and falls back
/// to the admin_store seed list when the real task_store is empty.
#[derive(Debug, Deserialize)]
pub struct ListTasksQuery {
    pub status: Option<String>,
}

async fn list_tasks(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(query): axum::extract::Query<ListTasksQuery>,
) -> Result<impl IntoResponse, ApiError> {
    debug!("Listing all tasks (filter={:?})", query.status);

    let task_keys: Vec<String> = {
        let tasks = state.task_store.read().await;
        tasks.keys().cloned().collect()
    };
    for task_key in task_keys {
        let _ = refresh_task_from_execution(&state, &task_key).await?;
    }

    let tasks = state.task_store.read().await;
    let real_list: Vec<TaskResponse> = tasks.values().cloned().map(TaskResponse::from).collect();

    // Fall back to admin_store seed list if there's nothing real.
    if real_list.is_empty() {
        let seeded = state.admin_store.tasks.read().await;
        let filtered: Vec<serde_json::Value> = seeded
            .iter()
            .filter(|t| match &query.status {
                Some(s) => !s.is_empty() && &t.status == s,
                None => true,
            })
            .map(|t| serde_json::to_value(t).unwrap())
            .collect();
        info!("Returned {} seeded admin tasks", filtered.len());
        return Ok(Json(serde_json::Value::Array(filtered)));
    }

    info!("Returned {} tasks", real_list.len());
    Ok(Json(
        serde_json::to_value(real_list).unwrap_or(serde_json::Value::Null),
    ))
}

/// GET /api/v1/tasks/:id - 获取任务详情
async fn get_task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    debug!("Getting task: id={}", id);

    // Try the real task_store first, fall back to admin_store.
    match refresh_task_from_execution(&state, &id).await {
        Ok(task) => {
            let response: TaskResponse = task.into();
            Ok(Json(serde_json::to_value(response).unwrap()))
        }
        Err(_) => {
            let seeded = state.admin_store.tasks.read().await;
            let found = seeded.iter().find(|t| t.task_id == id);
            match found {
                Some(t) => Ok(Json(serde_json::to_value(t).unwrap())),
                None => Err(ApiError::NotFound(format!("Task not found: {}", id))),
            }
        }
    }
}

/// GET /api/v1/tasks/:id/status - 查询任务执行状态
async fn get_task_status(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    debug!("Getting task status: id={}", id);

    let task = refresh_task_from_execution(&state, &id).await?;

    let response = TaskStatusResponse {
        id: id.clone(),
        execution_id: task.execution_id.clone(),
        status: task.status.clone(),
        result: task.result.clone(),
        error: task.error.clone(),
    };

    Ok(Json(response))
}

/// DELETE /api/v1/tasks/:id - 取消任务
async fn cancel_task(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    info!("Cancelling task: id={}", id);

    let audit_target = Some(format!("task:{}", id));
    let record_audit = |state: &Arc<AppState>, result: AuditResult| {
        let audit = state.audit.clone();
        let actor = claims.sub.as_str().to_string();
        let target = audit_target.clone();
        async move {
            if let Some(sink) = audit {
                sink.record(AuditEntry::now(
                    actor,
                    AuditKind::Mutation,
                    "task.cancel".to_string(),
                    target,
                    serde_json::json!({}),
                    result,
                ))
                .await;
            }
        }
    };

    // If this is an admin_store-only task (not in the real task_store),
    // mark it cancelled and return immediately. The production task
    // path below still runs when the id exists in task_store.
    {
        let real = state.task_store.read().await;
        if !real.contains_key(&id) {
            let mut seeded = state.admin_store.tasks.write().await;
            if let Some(t) = seeded.iter_mut().find(|t| t.task_id == id) {
                t.status = "cancelled".to_string();
                let _ = state.admin_event_bus.send(AdminEvent::TaskStatusChanged {
                    task_id: id.clone(),
                    status: "cancelled".to_string(),
                });
                record_audit(&state, AuditResult::Success).await;
                return Ok(axum::http::StatusCode::NO_CONTENT);
            }
            record_audit(
                &state,
                AuditResult::Failure {
                    reason: "not_found".to_string(),
                },
            )
            .await;
            return Err(ApiError::NotFound(format!("Task not found: {}", id)));
        }
    }

    let latest_task = refresh_task_from_execution(&state, &id).await?;

    // 只能取消 Pending 或 Running 状态的任务
    match latest_task.status {
        TaskStatus::Pending | TaskStatus::Running => {
            if let Some(execution_id_str) = &latest_task.execution_id {
                let execution_id =
                    ExecutionId::from_string(execution_id_str.clone()).map_err(|e| {
                        ApiError::InternalError(format!(
                            "Invalid execution_id '{}' for task {}: {}",
                            execution_id_str, id, e
                        ))
                    })?;

                state
                    .execution_service
                    .cancel(&execution_id)
                    .await
                    .map_err(|e| {
                        ApiError::InternalError(format!("Failed to cancel execution: {}", e))
                    })?;
            }

            let mut tasks = state.task_store.write().await;
            let task = tasks
                .get_mut(&id)
                .ok_or_else(|| ApiError::NotFound(format!("Task not found: {}", id)))?;
            task.status = TaskStatus::Cancelled;
            debug!("Task cancelled: id={}", id);

            // Mirror into admin_store if this id happens to exist there too.
            {
                let mut seeded = state.admin_store.tasks.write().await;
                if let Some(t) = seeded.iter_mut().find(|t| t.task_id == id) {
                    t.status = "cancelled".to_string();
                }
            }
            let _ = state.admin_event_bus.send(AdminEvent::TaskStatusChanged {
                task_id: id.clone(),
                status: "cancelled".to_string(),
            });

            record_audit(&state, AuditResult::Success).await;
            Ok(axum::http::StatusCode::NO_CONTENT)
        }
        _ => {
            record_audit(
                &state,
                AuditResult::Failure {
                    reason: format!("invalid_state:{:?}", latest_task.status),
                },
            )
            .await;
            Err(ApiError::InvalidRequest(format!(
                "Cannot cancel task in {:?} state",
                latest_task.status
            )))
        }
    }
}
