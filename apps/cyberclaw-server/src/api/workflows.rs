//! Workflows API - 工作流管理接口
//!
//! 提供工作流的创建、查询、暂停、恢复、取消和触发器管理。

use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::info;

use crate::audit::{AuditEntry, AuditKind, AuditResult};
use crate::error::ApiError;
use crate::middleware::auth::Claims;
use crate::state::AppState;

/// Emit an audit row for a workflow lifecycle event. Best-effort: when the
/// audit sink isn't wired (test fixtures) we silently skip. Actor falls back
/// to "system" if the route ever reaches us without claims.
async fn emit_workflow_audit(
    state: &Arc<AppState>,
    claims: Option<&axum::Extension<Claims>>,
    action: &str,
    target: String,
    detail: serde_json::Value,
) {
    if let Some(sink) = state.audit.as_ref() {
        let actor = claims
            .map(|c| c.sub.to_string())
            .unwrap_or_else(|| "system".to_string());
        sink.record(AuditEntry::now(
            actor,
            AuditKind::Mutation,
            action.to_string(),
            Some(target),
            detail,
            AuditResult::Success,
        ))
        .await;
    }
}

/// 创建工作流请求
#[derive(Debug, Deserialize)]
pub struct CreateWorkflowRequest {
    /// 工作流名称
    pub name: String,
    /// 工作流版本
    #[serde(default = "default_version")]
    pub version: String,
    /// 工作流描述
    #[serde(default)]
    pub description: Option<String>,
    /// 工作流步骤
    #[serde(default)]
    pub steps: Vec<WorkflowStepInput>,
    /// 附加元数据
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
    /// 初始上下文变量
    #[serde(default)]
    pub variables: HashMap<String, serde_json::Value>,
}

fn default_version() -> String {
    "1.0.0".to_string()
}

/// 工作流步骤输入
#[derive(Debug, Clone, Deserialize)]
pub struct WorkflowStepInput {
    pub id: String,
    pub name: String,
    #[serde(default = "default_step_type")]
    pub step_type: String,
    #[serde(default)]
    pub config: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub dependencies: Vec<String>,
}

fn default_step_type() -> String {
    "task".to_string()
}

/// 创建工作流响应
#[derive(Debug, Serialize)]
pub struct CreateWorkflowResponse {
    pub success: bool,
    pub workflow_id: String,
    pub instance_id: String,
}

/// 工作流状态响应
#[derive(Debug, Serialize)]
pub struct WorkflowStatusResponse {
    pub instance_id: String,
    pub workflow_id: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_step: Option<String>,
    pub started_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
}

/// 工作流操作响应
#[derive(Debug, Serialize)]
pub struct WorkflowActionResponse {
    pub success: bool,
    pub message: String,
}

/// 触发器列表响应
#[derive(Debug, Serialize)]
pub struct TriggerListResponse {
    pub workflow_id: String,
    pub triggers: Vec<TriggerInfo>,
    pub total: usize,
}

/// 触发器信息
#[derive(Debug, Serialize)]
pub struct TriggerInfo {
    pub trigger_id: String,
    pub trigger_type: String,
    pub enabled: bool,
    pub created_at: String,
}

/// BT-40 chain config — operator-facing convenience for "task A → B → C".
/// Server expands this into a full WorkflowDefinition with dependencies wired
/// (each task depends on the previous one's completion).
#[derive(Debug, Deserialize)]
pub struct CreateChainRequest {
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
    /// Ordered list of tasks. The Nth task will be configured to start
    /// only after task N-1 reaches the "completed" state.
    pub tasks: Vec<ChainTaskInput>,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct ChainTaskInput {
    /// Stable identifier for this task in the chain (used in dependencies).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Free-form task config: prompts, agent IDs, capability args, etc.
    #[serde(default)]
    pub config: HashMap<String, serde_json::Value>,
}

/// 创建 Workflows API 路由
pub fn create_workflows_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/workflows", post(create_workflow))
        .route("/api/v1/admin/workflows/chain", post(create_chain))
        .route("/api/v1/workflows/:id", get(get_workflow_status))
        .route("/api/v1/workflows/:id/pause", post(pause_workflow))
        .route("/api/v1/workflows/:id/resume", post(resume_workflow))
        .route("/api/v1/workflows/:id/cancel", post(cancel_workflow))
        .route(
            "/api/v1/workflows/:id/triggers",
            get(list_workflow_triggers),
        )
}

/// POST /api/v1/workflows - 创建并提交工作流
async fn create_workflow(
    State(state): State<Arc<AppState>>,
    claims: Option<axum::Extension<Claims>>,
    Json(req): Json<CreateWorkflowRequest>,
) -> Result<Json<CreateWorkflowResponse>, ApiError> {
    info!(name = %req.name, version = %req.version, "Creating workflow");

    let workflow_id =
        cyberclaw_core::ids::WorkflowId::from_string(format!("wf-{}", uuid::Uuid::new_v4()))
            .map_err(|e| ApiError::InternalError(format!("Failed to create workflow ID: {}", e)))?;

    // 将输入步骤转换为 WorkflowStep
    let steps: Vec<cyberclaw_workflow::WorkflowStep> = req
        .steps
        .into_iter()
        .map(|s| {
            let step_type = match s.step_type.as_str() {
                "parallel" => cyberclaw_workflow::StepType::Parallel,
                "condition" => cyberclaw_workflow::StepType::Condition,
                "loop" => cyberclaw_workflow::StepType::Loop,
                "sub_workflow" => cyberclaw_workflow::StepType::SubWorkflow,
                _ => cyberclaw_workflow::StepType::Task,
            };
            cyberclaw_workflow::WorkflowStep {
                id: s.id,
                name: s.name,
                step_type,
                config: s.config,
                dependencies: s.dependencies,
                retry_policy: None,
            }
        })
        .collect();

    let definition = cyberclaw_workflow::WorkflowDefinition {
        id: workflow_id.clone(),
        name: req.name,
        version: req.version,
        description: req.description,
        steps,
        metadata: req.metadata,
    };

    // 注册工作流定义
    state
        .workflow_engine
        .register_workflow(definition)
        .await
        .map_err(|e| ApiError::InternalError(format!("Failed to register workflow: {}", e)))?;

    // 创建工作流实例
    let context = cyberclaw_workflow::WorkflowContext {
        variables: req.variables,
        step_results: HashMap::new(),
        error_messages: Vec::new(),
        step_retry_counts: HashMap::new(),
    };

    let instance_id = state
        .workflow_engine
        .create_instance(&workflow_id, context)
        .await
        .map_err(|e| {
            ApiError::InternalError(format!("Failed to create workflow instance: {}", e))
        })?;

    info!(
        workflow_id = %workflow_id,
        instance_id = %instance_id,
        "Workflow created successfully"
    );

    emit_workflow_audit(
        &state,
        claims.as_ref(),
        "workflow.create",
        format!("workflow:{instance_id}"),
        serde_json::json!({
            "workflow_id": workflow_id.as_str(),
            "instance_id": instance_id,
        }),
    )
    .await;

    Ok(Json(CreateWorkflowResponse {
        success: true,
        workflow_id: workflow_id.as_str().to_string(),
        instance_id,
    }))
}

/// Build a sequence of workflow steps where step N depends on step N-1.
/// Pure function so unit tests can verify the dependency wiring without
/// touching the workflow engine.
fn wire_chain_dependencies(tasks: &[ChainTaskInput]) -> Vec<cyberclaw_workflow::WorkflowStep> {
    let mut steps: Vec<cyberclaw_workflow::WorkflowStep> = Vec::with_capacity(tasks.len());
    let mut prev_id: Option<String> = None;
    for task in tasks {
        let dependencies = match prev_id.as_ref() {
            Some(p) => vec![p.clone()],
            None => vec![],
        };
        steps.push(cyberclaw_workflow::WorkflowStep {
            id: task.id.clone(),
            name: task.name.clone(),
            step_type: cyberclaw_workflow::StepType::Task,
            config: task.config.clone(),
            dependencies,
            retry_policy: None,
        });
        prev_id = Some(task.id.clone());
    }
    steps
}

/// POST /api/v1/admin/workflows/chain — BT-40 convenience endpoint.
///
/// Takes an ordered list of tasks and registers a workflow where task[N]
/// depends on task[N-1]. Operators no longer need to hand-craft the full
/// WorkflowDefinition shape with dependency arrays — they just say
/// "run A then B then C".
async fn create_chain(
    State(state): State<Arc<AppState>>,
    claims: Option<axum::Extension<Claims>>,
    Json(req): Json<CreateChainRequest>,
) -> Result<Json<CreateWorkflowResponse>, ApiError> {
    if req.tasks.is_empty() {
        return Err(ApiError::InvalidInput(
            "chain must contain at least one task".to_string(),
        ));
    }
    info!(name = %req.name, task_count = req.tasks.len(), "Creating workflow chain");

    let workflow_id =
        cyberclaw_core::ids::WorkflowId::from_string(format!("wf-chain-{}", uuid::Uuid::new_v4()))
            .map_err(|e| ApiError::InternalError(format!("Failed to create workflow ID: {}", e)))?;

    let steps = wire_chain_dependencies(&req.tasks);

    let definition = cyberclaw_workflow::WorkflowDefinition {
        id: workflow_id.clone(),
        name: req.name,
        version: req.version,
        description: Some("Task chain auto-generated by /admin/workflows/chain".to_string()),
        steps,
        metadata: req.metadata,
    };

    state
        .workflow_engine
        .register_workflow(definition)
        .await
        .map_err(|e| ApiError::InternalError(format!("Failed to register chain: {}", e)))?;

    let context = cyberclaw_workflow::WorkflowContext {
        variables: HashMap::new(),
        step_results: HashMap::new(),
        error_messages: Vec::new(),
        step_retry_counts: HashMap::new(),
    };
    let instance_id = state
        .workflow_engine
        .create_instance(&workflow_id, context)
        .await
        .map_err(|e| ApiError::InternalError(format!("Failed to create chain instance: {}", e)))?;

    info!(workflow_id = %workflow_id, instance_id = %instance_id, "Chain registered");

    emit_workflow_audit(
        &state,
        claims.as_ref(),
        "workflow.create_chain",
        format!("workflow:{instance_id}"),
        serde_json::json!({
            "workflow_id": workflow_id.as_str(),
            "instance_id": instance_id,
            "task_count": req.tasks.len(),
        }),
    )
    .await;

    Ok(Json(CreateWorkflowResponse {
        success: true,
        workflow_id: workflow_id.as_str().to_string(),
        instance_id,
    }))
}

/// GET /api/v1/workflows/:id - 获取工作流状态
async fn get_workflow_status(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<WorkflowStatusResponse>, ApiError> {
    info!(instance_id = %id, "Getting workflow status");

    let instance = state
        .workflow_engine
        .get_instance(&id)
        .await
        .map_err(|e| match e {
            cyberclaw_workflow::WorkflowError::WorkflowNotFound(_) => {
                ApiError::NotFound(format!("Workflow instance {} not found", id))
            }
            other => ApiError::InternalError(format!("Failed to get workflow: {}", other)),
        })?;

    Ok(Json(WorkflowStatusResponse {
        instance_id: instance.id,
        workflow_id: instance.workflow_id.as_str().to_string(),
        status: format!("{:?}", instance.status),
        current_step: instance.current_step,
        started_at: instance.started_at.to_rfc3339(),
        completed_at: instance.completed_at.map(|t| t.to_rfc3339()),
    }))
}

/// POST /api/v1/workflows/:id/pause - 暂停工作流
async fn pause_workflow(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    claims: Option<axum::Extension<Claims>>,
) -> Result<Json<WorkflowActionResponse>, ApiError> {
    info!(instance_id = %id, "Pausing workflow");

    state
        .workflow_engine
        .pause_instance(&id)
        .await
        .map_err(|e| match e {
            cyberclaw_workflow::WorkflowError::WorkflowNotFound(_) => {
                ApiError::NotFound(format!("Workflow instance {} not found", id))
            }
            cyberclaw_workflow::WorkflowError::InvalidState(msg) => {
                ApiError::InvalidRequest(format!("Cannot pause workflow: {}", msg))
            }
            other => ApiError::InternalError(format!("Failed to pause workflow: {}", other)),
        })?;

    emit_workflow_audit(
        &state,
        claims.as_ref(),
        "workflow.pause",
        format!("workflow:{id}"),
        serde_json::json!({ "instance_id": id }),
    )
    .await;

    Ok(Json(WorkflowActionResponse {
        success: true,
        message: format!("Workflow {} paused", id),
    }))
}

/// POST /api/v1/workflows/:id/resume - 恢复工作流
async fn resume_workflow(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    claims: Option<axum::Extension<Claims>>,
) -> Result<Json<WorkflowActionResponse>, ApiError> {
    info!(instance_id = %id, "Resuming workflow");

    state
        .workflow_engine
        .resume_instance(&id)
        .await
        .map_err(|e| match e {
            cyberclaw_workflow::WorkflowError::WorkflowNotFound(_) => {
                ApiError::NotFound(format!("Workflow instance {} not found", id))
            }
            cyberclaw_workflow::WorkflowError::InvalidState(msg) => {
                ApiError::InvalidRequest(format!("Cannot resume workflow: {}", msg))
            }
            other => ApiError::InternalError(format!("Failed to resume workflow: {}", other)),
        })?;

    emit_workflow_audit(
        &state,
        claims.as_ref(),
        "workflow.resume",
        format!("workflow:{id}"),
        serde_json::json!({ "instance_id": id }),
    )
    .await;

    Ok(Json(WorkflowActionResponse {
        success: true,
        message: format!("Workflow {} resumed", id),
    }))
}

/// POST /api/v1/workflows/:id/cancel - 取消工作流
async fn cancel_workflow(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    claims: Option<axum::Extension<Claims>>,
) -> Result<Json<WorkflowActionResponse>, ApiError> {
    info!(instance_id = %id, "Cancelling workflow");

    state
        .workflow_engine
        .cancel_instance(&id)
        .await
        .map_err(|e| match e {
            cyberclaw_workflow::WorkflowError::WorkflowNotFound(_) => {
                ApiError::NotFound(format!("Workflow instance {} not found", id))
            }
            other => ApiError::InternalError(format!("Failed to cancel workflow: {}", other)),
        })?;

    emit_workflow_audit(
        &state,
        claims.as_ref(),
        "workflow.cancel",
        format!("workflow:{id}"),
        serde_json::json!({ "instance_id": id }),
    )
    .await;

    Ok(Json(WorkflowActionResponse {
        success: true,
        message: format!("Workflow {} cancelled", id),
    }))
}

/// GET /api/v1/workflows/:id/triggers - 列出工作流的触发器
async fn list_workflow_triggers(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<TriggerListResponse>, ApiError> {
    info!(workflow_id = %id, "Listing workflow triggers");

    let registrations = state.trigger_registry.get_triggers(&id).await;

    let triggers: Vec<TriggerInfo> = registrations
        .into_iter()
        .map(|reg| {
            let trigger_type = match &reg.trigger {
                cyberclaw_workflow::WorkflowTrigger::Manual => "manual".to_string(),
                cyberclaw_workflow::WorkflowTrigger::OnExecutionComplete { .. } => {
                    "on_execution_complete".to_string()
                }
                cyberclaw_workflow::WorkflowTrigger::OnWebhook { .. } => "on_webhook".to_string(),
                cyberclaw_workflow::WorkflowTrigger::Cron { .. } => "cron".to_string(),
                cyberclaw_workflow::WorkflowTrigger::OnEvent { .. } => "on_event".to_string(),
            };
            TriggerInfo {
                trigger_id: reg.trigger_id,
                trigger_type,
                enabled: reg.enabled,
                created_at: reg.created_at.to_rfc3339(),
            }
        })
        .collect();

    let total = triggers.len();

    Ok(Json(TriggerListResponse {
        workflow_id: id,
        triggers,
        total,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_chain_dependencies_links_each_to_previous() {
        // BT-40 — verify A -> B -> C dependency wiring.
        let tasks = vec![
            ChainTaskInput {
                id: "a".to_string(),
                name: "Task A".to_string(),
                config: HashMap::new(),
            },
            ChainTaskInput {
                id: "b".to_string(),
                name: "Task B".to_string(),
                config: HashMap::new(),
            },
            ChainTaskInput {
                id: "c".to_string(),
                name: "Task C".to_string(),
                config: HashMap::new(),
            },
        ];
        let steps = wire_chain_dependencies(&tasks);
        assert_eq!(steps.len(), 3);
        assert!(steps[0].dependencies.is_empty(), "first task has no deps");
        assert_eq!(steps[1].dependencies, vec!["a".to_string()]);
        assert_eq!(steps[2].dependencies, vec!["b".to_string()]);
    }

    #[test]
    fn wire_chain_dependencies_empty_input() {
        let steps = wire_chain_dependencies(&[]);
        assert!(steps.is_empty());
    }

    #[test]
    fn create_chain_request_deserializes() {
        let json = r#"{
            "name": "demo-chain",
            "tasks": [
                {"id": "gen", "name": "Generate"},
                {"id": "test", "name": "Test", "config": {"timeout_ms": 5000}},
                {"id": "report", "name": "Report"}
            ]
        }"#;
        let req: CreateChainRequest = serde_json::from_str(json).expect("deserializes");
        assert_eq!(req.name, "demo-chain");
        assert_eq!(req.tasks.len(), 3);
        assert_eq!(req.tasks[0].id, "gen");
        assert!(req.tasks[1].config.contains_key("timeout_ms"));
    }

    #[test]
    fn test_create_workflow_request_defaults() {
        let json = r#"{"name": "test-workflow"}"#;
        let req: CreateWorkflowRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name, "test-workflow");
        assert_eq!(req.version, "1.0.0");
        assert!(req.description.is_none());
        assert!(req.steps.is_empty());
        assert!(req.metadata.is_empty());
        assert!(req.variables.is_empty());
    }

    #[test]
    fn test_create_workflow_response_serialization() {
        let response = CreateWorkflowResponse {
            success: true,
            workflow_id: "wf-001".to_string(),
            instance_id: "inst-001".to_string(),
        };
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["success"], true);
        assert_eq!(json["workflow_id"], "wf-001");
        assert_eq!(json["instance_id"], "inst-001");
    }

    #[test]
    fn test_workflow_status_response_serialization() {
        let response = WorkflowStatusResponse {
            instance_id: "inst-001".to_string(),
            workflow_id: "wf-001".to_string(),
            status: "Running".to_string(),
            current_step: Some("step-1".to_string()),
            started_at: "2026-04-12T00:00:00Z".to_string(),
            completed_at: None,
        };
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["instance_id"], "inst-001");
        assert_eq!(json["status"], "Running");
        assert_eq!(json["current_step"], "step-1");
        assert!(json.get("completed_at").is_none());
    }

    #[test]
    fn test_workflow_action_response_serialization() {
        let response = WorkflowActionResponse {
            success: true,
            message: "Workflow paused".to_string(),
        };
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["success"], true);
        assert_eq!(json["message"], "Workflow paused");
    }

    #[test]
    fn test_trigger_list_response_serialization() {
        let response = TriggerListResponse {
            workflow_id: "wf-001".to_string(),
            triggers: vec![TriggerInfo {
                trigger_id: "trig-001".to_string(),
                trigger_type: "cron".to_string(),
                enabled: true,
                created_at: "2026-04-12T00:00:00Z".to_string(),
            }],
            total: 1,
        };
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["total"], 1);
        assert_eq!(json["triggers"][0]["trigger_type"], "cron");
    }

    #[test]
    fn test_workflow_step_input_defaults() {
        let json = r#"{"id": "step-1", "name": "First Step"}"#;
        let step: WorkflowStepInput = serde_json::from_str(json).unwrap();
        assert_eq!(step.id, "step-1");
        assert_eq!(step.name, "First Step");
        assert_eq!(step.step_type, "task");
        assert!(step.config.is_empty());
        assert!(step.dependencies.is_empty());
    }
}
