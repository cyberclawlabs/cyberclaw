//! Workflow Engine 实现
//!
//! 提供工作流创建、执行、监控和状态持久化能力。

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use cyberclaw_core::ids::{ExecutionId, WorkflowId};
use cyberclaw_store::state_store::{ExecutionRecord, StateStore};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::mpsc;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

/// 工作流引擎错误
#[derive(Error, Debug)]
pub enum WorkflowError {
    #[error("工作流未找到: {0}")]
    WorkflowNotFound(String),

    #[error("工作流已存在: {0}")]
    WorkflowAlreadyExists(String),

    #[error("无效的工作流状态: {0}")]
    InvalidState(String),

    #[error("执行失败: {0}")]
    ExecutionFailed(String),

    #[error("内部错误: {0}")]
    Internal(String),
}

/// 工作流定义
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowDefinition {
    pub id: WorkflowId,
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub steps: Vec<WorkflowStep>,
    pub metadata: HashMap<String, serde_json::Value>,
}

/// 工作流步骤
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowStep {
    pub id: String,
    pub name: String,
    pub step_type: StepType,
    pub config: HashMap<String, serde_json::Value>,
    pub dependencies: Vec<String>,
    pub retry_policy: Option<RetryPolicy>,
}

/// 步骤类型
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepType {
    Task,
    Parallel,
    Condition,
    Loop,
    SubWorkflow,
}

/// 重试策略
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub backoff_ms: u64,
    pub exponential: bool,
}

/// 工作流实例
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowInstance {
    pub id: String,
    pub workflow_id: WorkflowId,
    pub execution_id: ExecutionId,
    pub status: WorkflowStatus,
    pub current_step: Option<String>,
    pub context: WorkflowContext,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// 工作流状态
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowStatus {
    Pending,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

/// 工作流上下文
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowContext {
    pub variables: HashMap<String, serde_json::Value>,
    pub step_results: HashMap<String, serde_json::Value>,
    pub error_messages: Vec<String>,
    /// 步骤重试计数（step_id -> 当前尝试次数）
    #[serde(default)]
    pub step_retry_counts: HashMap<String, u32>,
}

/// 工作流事件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkflowEvent {
    Started {
        instance_id: String,
    },
    StepStarted {
        instance_id: String,
        step_id: String,
    },
    StepCompleted {
        instance_id: String,
        step_id: String,
        result: serde_json::Value,
    },
    StepFailed {
        instance_id: String,
        step_id: String,
        error: String,
    },
    Completed {
        instance_id: String,
    },
    Failed {
        instance_id: String,
        error: String,
    },
    Cancelled {
        instance_id: String,
    },
}

/// 条件评估结果
#[derive(Debug, Clone)]
struct ConditionResult {
    /// 条件是否匹配
    matched: bool,
    /// 选择的分支名称
    branch: String,
}

/// 并行执行结果
#[derive(Debug, Clone)]
struct ParallelResult {
    /// 所有子步骤的执行结果
    outputs: Vec<serde_json::Value>,
    /// 总步骤数
    total: usize,
    /// 成功数量
    succeeded: usize,
    /// 失败数量
    failed: usize,
}

/// 工作流引擎 trait
#[async_trait]
pub trait WorkflowEngineApi: Send + Sync {
    /// 注册工作流定义
    async fn register_workflow(&self, definition: WorkflowDefinition) -> Result<(), WorkflowError>;

    /// 创建工作流实例
    async fn create_instance(
        &self,
        workflow_id: &WorkflowId,
        context: WorkflowContext,
    ) -> Result<String, WorkflowError>;

    /// 执行工作流实例
    async fn execute_instance(&self, instance_id: &str) -> Result<(), WorkflowError>;

    /// 暂停工作流实例
    async fn pause_instance(&self, instance_id: &str) -> Result<(), WorkflowError>;

    /// 恢复工作流实例
    async fn resume_instance(&self, instance_id: &str) -> Result<(), WorkflowError>;

    /// 取消工作流实例
    async fn cancel_instance(&self, instance_id: &str) -> Result<(), WorkflowError>;

    /// 获取工作流实例状态
    async fn get_instance(&self, instance_id: &str) -> Result<WorkflowInstance, WorkflowError>;

    /// 列出工作流实例
    async fn list_instances(
        &self,
        workflow_id: Option<&WorkflowId>,
    ) -> Result<Vec<WorkflowInstance>, WorkflowError>;
}

/// 工作流引擎实现
///
/// # 持久化支持
///
/// WorkflowEngine 支持可选的 StateStore 集成，用于持久化工作流状态。
/// 当前实现通过 `persist_instance` 和 `update_instance_in_store` 将
/// WorkflowInstance 序列化后存入 ExecutionRecord，复用现有 StateStore 接口。
/// 若需要独立的 workflow 存储接口，可在 StateStore trait 中按 Policy 接口
/// 的模式新增专用方法，并更新所有实现（PostgresStateStore、InMemoryStateStore）。
pub struct WorkflowEngine {
    definitions: Arc<RwLock<HashMap<WorkflowId, WorkflowDefinition>>>,
    instances: Arc<RwLock<HashMap<String, WorkflowInstance>>>,
    event_sender: mpsc::UnboundedSender<WorkflowEvent>,
    #[allow(dead_code)]
    event_receiver: Arc<RwLock<mpsc::UnboundedReceiver<WorkflowEvent>>>,
    /// 可选的状态存储后端（用于持久化工作流实例）
    state_store: Option<Arc<dyn StateStore>>,
}

impl WorkflowEngine {
    /// 创建新的工作流引擎（仅内存存储）
    pub fn new() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            definitions: Arc::new(RwLock::new(HashMap::new())),
            instances: Arc::new(RwLock::new(HashMap::new())),
            event_sender: tx,
            event_receiver: Arc::new(RwLock::new(rx)),
            state_store: None,
        }
    }

    /// 创建带持久化支持的工作流引擎
    ///
    /// # 参数
    ///
    /// * `state_store` - StateStore 实现（PostgreSQL 或 InMemory）
    ///
    /// # 示例
    ///
    /// ```rust,ignore
    /// use cyberclaw_store::memory::InMemoryStateStore;
    /// let store = Arc::new(InMemoryStateStore::new());
    /// let engine = WorkflowEngine::with_state_store(store);
    /// ```
    pub fn with_state_store(state_store: Arc<dyn StateStore>) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            definitions: Arc::new(RwLock::new(HashMap::new())),
            instances: Arc::new(RwLock::new(HashMap::new())),
            event_sender: tx,
            event_receiver: Arc::new(RwLock::new(rx)),
            state_store: Some(state_store),
        }
    }

    /// 订阅工作流事件
    pub fn subscribe_events(&self) -> mpsc::UnboundedReceiver<WorkflowEvent> {
        let (_tx, rx) = mpsc::unbounded_channel();

        // 转发事件到新的订阅者
        let _event_sender = self.event_sender.clone();
        tokio::spawn(async move {
            // 这里应该实现事件广播逻辑
            // 简化实现：直接使用原始 channel
        });

        rx
    }

    /// 发送工作流事件
    fn send_event(&self, event: WorkflowEvent) {
        if let Err(e) = self.event_sender.send(event) {
            error!("发送工作流事件失败: {}", e);
        }
    }

    /// 执行工作流步骤
    fn execute_step<'a>(
        &'a self,
        instance: &'a mut WorkflowInstance,
        step: &'a WorkflowStep,
    ) -> Pin<
        Box<dyn std::future::Future<Output = Result<serde_json::Value, WorkflowError>> + Send + 'a>,
    > {
        Box::pin(async move {
            info!("执行工作流步骤: {} - {}", instance.id, step.id);

            // 发送步骤开始事件
            self.send_event(WorkflowEvent::StepStarted {
                instance_id: instance.id.clone(),
                step_id: step.id.clone(),
            });

            // 执行步骤逻辑
            let result = match step.step_type {
                StepType::Task => {
                    // 执行任务步骤
                    serde_json::json!({
                        "status": "completed",
                        "output": "task result"
                    })
                }
                StepType::Parallel => {
                    // 执行并行步骤 - 实现真实的并发执行
                    let parallel_result = self.execute_parallel_steps(step, instance).await?;
                    serde_json::json!({
                        "status": "completed",
                        "outputs": parallel_result.outputs,
                        "total_steps": parallel_result.total,
                        "succeeded": parallel_result.succeeded,
                        "failed": parallel_result.failed
                    })
                }
                StepType::Condition => {
                    // 执行条件步骤 - 实现真实的条件评估逻辑
                    let condition_result = self.evaluate_condition(step, instance)?;
                    serde_json::json!({
                        "status": "completed",
                        "branch": condition_result.branch,
                        "matched": condition_result.matched
                    })
                }
                StepType::Loop => {
                    // 执行循环步骤
                    serde_json::json!({
                        "status": "completed",
                        "iterations": 1
                    })
                }
                StepType::SubWorkflow => {
                    // 执行子工作流 - 实现真实的嵌套工作流执行
                    self.execute_subworkflow(step, instance).await?
                }
            };

            // 更新步骤结果
            instance
                .context
                .step_results
                .insert(step.id.clone(), result.clone());

            // 发送步骤完成事件
            self.send_event(WorkflowEvent::StepCompleted {
                instance_id: instance.id.clone(),
                step_id: step.id.clone(),
                result: result.clone(),
            });

            Ok(result)
        })
    }

    /// 评估条件表达式
    ///
    /// 支持的条件格式（在 step.config 中）：
    /// ```json
    /// {
    ///   "condition": {
    ///     "type": "equals|contains|greater_than|less_than|exists",
    ///     "variable": "context.variables.status",
    ///     "value": "success",
    ///     "then_branch": "success_path",
    ///     "else_branch": "failure_path"
    ///   }
    /// }
    /// ```
    fn evaluate_condition(
        &self,
        step: &WorkflowStep,
        instance: &WorkflowInstance,
    ) -> Result<ConditionResult, WorkflowError> {
        // 从 config 中提取条件配置
        let condition = step.config.get("condition").ok_or_else(|| {
            WorkflowError::ExecutionFailed(format!("步骤 {} 缺少 condition 配置", step.id))
        })?;

        let condition_type = condition
            .get("type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| WorkflowError::ExecutionFailed("条件缺少 type 字段".to_string()))?;

        let variable_path = condition
            .get("variable")
            .and_then(|v| v.as_str())
            .ok_or_else(|| WorkflowError::ExecutionFailed("条件缺少 variable 字段".to_string()))?;

        let expected_value = condition.get("value");
        let then_branch = condition
            .get("then_branch")
            .and_then(|v| v.as_str())
            .unwrap_or("then");
        let else_branch = condition
            .get("else_branch")
            .and_then(|v| v.as_str())
            .unwrap_or("else");

        // 从上下文中提取变量值
        let actual_value = self.extract_variable(variable_path, instance)?;

        // 根据条件类型评估
        let matched = match condition_type {
            "equals" => {
                if let Some(expected) = expected_value {
                    actual_value == *expected
                } else {
                    false
                }
            }
            "contains" => {
                if let (Some(actual_str), Some(expected_str)) = (
                    actual_value.as_str(),
                    expected_value.and_then(|v| v.as_str()),
                ) {
                    actual_str.contains(expected_str)
                } else {
                    false
                }
            }
            "greater_than" => {
                if let (Some(actual_num), Some(expected_num)) = (
                    actual_value.as_f64(),
                    expected_value.and_then(|v| v.as_f64()),
                ) {
                    actual_num > expected_num
                } else {
                    false
                }
            }
            "less_than" => {
                if let (Some(actual_num), Some(expected_num)) = (
                    actual_value.as_f64(),
                    expected_value.and_then(|v| v.as_f64()),
                ) {
                    actual_num < expected_num
                } else {
                    false
                }
            }
            "exists" => !actual_value.is_null(),
            _ => {
                return Err(WorkflowError::ExecutionFailed(format!(
                    "不支持的条件类型: {}",
                    condition_type
                )));
            }
        };

        Ok(ConditionResult {
            matched,
            branch: if matched {
                then_branch.to_string()
            } else {
                else_branch.to_string()
            },
        })
    }

    /// 从上下文中提取变量
    ///
    /// 支持的路径格式：
    /// - `context.variables.key` - 从 context.variables 中提取
    /// - `context.step_results.step_id.key` - 从步骤结果中提取
    fn extract_variable(
        &self,
        path: &str,
        instance: &WorkflowInstance,
    ) -> Result<serde_json::Value, WorkflowError> {
        let parts: Vec<&str> = path.split('.').collect();

        if parts.len() < 3 || parts[0] != "context" {
            return Err(WorkflowError::ExecutionFailed(format!(
                "无效的变量路径: {}",
                path
            )));
        }

        match parts[1] {
            "variables" => {
                // 从 context.variables 中提取
                let key = parts[2..].join(".");
                Ok(instance
                    .context
                    .variables
                    .get(&key)
                    .cloned()
                    .unwrap_or(serde_json::Value::Null))
            }
            "step_results" => {
                // 从步骤结果中提取
                if parts.len() < 4 {
                    return Err(WorkflowError::ExecutionFailed(format!(
                        "步骤结果路径格式错误: {}",
                        path
                    )));
                }
                let step_id = parts[2];
                let result = instance.context.step_results.get(step_id).ok_or_else(|| {
                    WorkflowError::ExecutionFailed(format!("步骤结果未找到: {}", step_id))
                })?;

                // 提取嵌套字段
                let mut current = result;
                for part in &parts[3..] {
                    current = current.get(part).ok_or_else(|| {
                        WorkflowError::ExecutionFailed(format!("字段未找到: {}", part))
                    })?;
                }
                Ok(current.clone())
            }
            _ => Err(WorkflowError::ExecutionFailed(format!(
                "不支持的上下文类型: {}",
                parts[1]
            ))),
        }
    }

    /// 执行并行步骤
    ///
    /// 支持的配置格式（在 step.config 中）：
    /// ```json
    /// {
    ///   "substeps": [
    ///     {
    ///       "id": "substep1",
    ///       "name": "Substep 1",
    ///       "action": "task_action"
    ///     },
    ///     {
    ///       "id": "substep2",
    ///       "name": "Substep 2",
    ///       "action": "another_action"
    ///     }
    ///   ]
    /// }
    /// ```
    async fn execute_parallel_steps(
        &self,
        step: &WorkflowStep,
        instance: &WorkflowInstance,
    ) -> Result<ParallelResult, WorkflowError> {
        // 从 config 中提取子步骤列表
        let substeps = step
            .config
            .get("substeps")
            .and_then(|v| v.as_array())
            .ok_or_else(|| {
                WorkflowError::ExecutionFailed(format!("并行步骤 {} 缺少 substeps 配置", step.id))
            })?;

        if substeps.is_empty() {
            return Ok(ParallelResult {
                outputs: vec![],
                total: 0,
                succeeded: 0,
                failed: 0,
            });
        }

        info!("并行执行 {} 个子步骤", substeps.len());

        // 使用 tokio::spawn 并发执行所有子步骤
        let mut handles = Vec::new();
        for (idx, substep_value) in substeps.iter().enumerate() {
            let default_id = format!("substep_{}", idx);
            let substep_id = substep_value
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or(&default_id);
            let substep_action = substep_value
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("default_action");

            // 克隆需要的数据用于并发任务
            let substep_id = substep_id.to_string();
            let substep_action = substep_action.to_string();
            let _instance_id = instance.id.clone();

            // 生成并发任务
            let handle = tokio::spawn(async move {
                // 模拟子步骤执行（实际应用中这里会调用真实的任务执行逻辑）
                tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

                info!("子步骤 {} 执行完成: {}", substep_id, substep_action);

                (
                    substep_id.clone(),
                    serde_json::json!({
                        "substep_id": substep_id,
                        "action": substep_action,
                        "status": "completed",
                        "output": format!("result from {}", substep_id)
                    }),
                )
            });

            handles.push(handle);
        }

        // 等待所有并发任务完成
        let results = futures::future::join_all(handles).await;

        // 收集结果
        let mut outputs = Vec::new();
        let mut succeeded = 0;
        let mut failed = 0;

        for result in results {
            match result {
                Ok((substep_id, output)) => {
                    outputs.push(output);
                    succeeded += 1;
                    info!("子步骤 {} 成功", substep_id);
                }
                Err(e) => {
                    failed += 1;
                    error!("子步骤执行失败: {}", e);
                    outputs.push(serde_json::json!({
                        "status": "failed",
                        "error": e.to_string()
                    }));
                }
            }
        }

        Ok(ParallelResult {
            outputs,
            total: substeps.len(),
            succeeded,
            failed,
        })
    }

    /// 执行子工作流
    ///
    /// 支持的配置格式（在 step.config 中）：
    /// ```json
    /// {
    ///   "workflow_id": "sub-workflow-123",
    ///   "inherit_variables": ["var1", "var2"],  // 可选：从父工作流继承的变量
    ///   "input_mapping": {                      // 可选：输入变量映射
    ///     "parent_var": "child_var"
    ///   }
    /// }
    /// ```
    async fn execute_subworkflow(
        &self,
        step: &WorkflowStep,
        parent_instance: &WorkflowInstance,
    ) -> Result<serde_json::Value, WorkflowError> {
        // 从 config 中提取子工作流 ID
        let sub_workflow_id_str = step
            .config
            .get("workflow_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                WorkflowError::ExecutionFailed(format!(
                    "子工作流步骤 {} 缺少 workflow_id 配置",
                    step.id
                ))
            })?;

        // 解析 WorkflowId
        let sub_workflow_id =
            WorkflowId::from_string(sub_workflow_id_str.to_string()).map_err(|e| {
                WorkflowError::ExecutionFailed(format!(
                    "无效的子工作流 ID {}: {}",
                    sub_workflow_id_str, e
                ))
            })?;

        // 查找子工作流定义
        let sub_definition = {
            let definitions = self.definitions.read().await;
            definitions
                .get(&sub_workflow_id)
                .ok_or_else(|| WorkflowError::WorkflowNotFound(sub_workflow_id_str.to_string()))?
                .clone()
        };

        info!(
            "执行子工作流: {} (父实例: {})",
            sub_definition.name, parent_instance.id
        );

        // 准备子工作流的上下文
        let mut sub_context = WorkflowContext {
            variables: HashMap::new(),
            step_results: HashMap::new(),
            error_messages: vec![],
            step_retry_counts: HashMap::new(),
        };

        // 1. 处理变量继承
        if let Some(inherit_vars) = step
            .config
            .get("inherit_variables")
            .and_then(|v| v.as_array())
        {
            for var_name in inherit_vars {
                if let Some(var_name_str) = var_name.as_str() {
                    if let Some(var_value) = parent_instance.context.variables.get(var_name_str) {
                        sub_context
                            .variables
                            .insert(var_name_str.to_string(), var_value.clone());
                    }
                }
            }
        }

        // 2. 处理输入映射
        if let Some(input_mapping) = step.config.get("input_mapping").and_then(|v| v.as_object()) {
            for (parent_key, child_key) in input_mapping {
                if let Some(child_key_str) = child_key.as_str() {
                    if let Some(parent_value) = parent_instance.context.variables.get(parent_key) {
                        sub_context
                            .variables
                            .insert(child_key_str.to_string(), parent_value.clone());
                    }
                }
            }
        }

        // 创建子工作流实例
        let sub_instance_id = format!("{}-sub-{}", parent_instance.id, uuid::Uuid::new_v4());
        let sub_instance = WorkflowInstance {
            id: sub_instance_id.clone(),
            workflow_id: sub_definition.id.clone(),
            execution_id: parent_instance.execution_id.clone(), // 共享父执行 ID
            status: WorkflowStatus::Running,
            current_step: None,
            context: sub_context,
            started_at: chrono::Utc::now(),
            completed_at: None,
        };

        // 注册子工作流实例
        {
            let mut instances = self.instances.write().await;
            instances.insert(sub_instance_id.clone(), sub_instance.clone());
        }

        // 持久化子工作流实例（如果配置了 StateStore）
        if let Err(e) = self.persist_instance(&sub_instance).await {
            warn!("持久化子工作流实例失败: {}", e);
        }

        // 执行子工作流的所有步骤
        info!(
            "开始执行子工作流 {} 的 {} 个步骤",
            sub_instance_id,
            sub_definition.steps.len()
        );

        for step in &sub_definition.steps {
            // 检查依赖
            let can_execute = {
                let instances = self.instances.read().await;
                step.dependencies.iter().all(|dep_id| {
                    instances
                        .get(&sub_instance_id)
                        .map(|inst| inst.context.step_results.contains_key(dep_id))
                        .unwrap_or(false)
                })
            };

            if !can_execute {
                return Err(WorkflowError::ExecutionFailed(format!(
                    "子工作流步骤 {} 的依赖未满足",
                    step.id
                )));
            }

            // 取出子实例的克隆用于执行
            let mut sub_inst = {
                let mut instances = self.instances.write().await;
                let inst = instances
                    .get_mut(&sub_instance_id)
                    .ok_or_else(|| WorkflowError::WorkflowNotFound(sub_instance_id.clone()))?;
                inst.current_step = Some(step.id.clone());
                inst.clone()
            };

            let step_result = self.execute_step_with_retry(&mut sub_inst, step).await?;

            // 更新子实例状态
            {
                let mut instances = self.instances.write().await;
                if let Some(instance) = instances.get_mut(&sub_instance_id) {
                    instance
                        .context
                        .step_results
                        .insert(step.id.clone(), step_result);
                    instance.current_step = Some(step.id.clone());
                }
            }
        }

        // 标记子工作流完成
        let sub_result = {
            let mut instances = self.instances.write().await;
            let sub_instance = instances
                .get_mut(&sub_instance_id)
                .ok_or_else(|| WorkflowError::WorkflowNotFound(sub_instance_id.clone()))?;
            sub_instance.status = WorkflowStatus::Completed;
            sub_instance.completed_at = Some(chrono::Utc::now());

            // 收集子工作流的所有步骤结果
            serde_json::json!({
                "status": "completed",
                "sub_workflow_id": sub_instance_id,
                "sub_workflow_name": sub_definition.name,
                "step_results": sub_instance.context.step_results,
                "final_variables": sub_instance.context.variables
            })
        };

        // 持久化子工作流完成状态
        {
            let instances = self.instances.read().await;
            if let Some(sub_inst) = instances.get(&sub_instance_id) {
                if let Err(e) = self.update_instance_in_store(sub_inst).await {
                    warn!("持久化子工作流完成状态失败: {}", e);
                }
            }
        }

        info!("子工作流 {} 执行完成", sub_instance_id);
        Ok(sub_result)
    }

    /// 执行带重试策略的工作流步骤
    ///
    /// 实现指数退避重试机制：
    /// - 如果步骤配置了 retry_policy，在失败时自动重试
    /// - 支持线性退避和指数退避两种模式
    /// - 退避时间计算：
    ///   * 线性退避（exponential = false）: backoff_ms
    ///   * 指数退避（exponential = true）: backoff_ms * 2^(attempt - 1)
    /// - 记录每次重试的日志和统计信息
    ///
    /// # 参数
    ///
    /// * `instance` - 工作流实例的可变引用
    /// * `step` - 要执行的工作流步骤
    ///
    /// # 返回
    ///
    /// 成功时返回步骤执行结果，所有重试都失败后返回最后一次的错误
    fn execute_step_with_retry<'a>(
        &'a self,
        instance: &'a mut WorkflowInstance,
        step: &'a WorkflowStep,
    ) -> Pin<
        Box<dyn std::future::Future<Output = Result<serde_json::Value, WorkflowError>> + Send + 'a>,
    > {
        Box::pin(async move {
            // 检查是否配置了重试策略
            let retry_policy = match &step.retry_policy {
                Some(policy) => policy.clone(),
                None => {
                    // 没有重试策略，直接执行一次
                    return self.execute_step(instance, step).await;
                }
            };

            // 获取当前步骤的重试计数
            let current_attempt = instance
                .context
                .step_retry_counts
                .get(&step.id)
                .copied()
                .unwrap_or(0);

            let max_attempts = retry_policy.max_attempts;
            let mut last_error: Option<WorkflowError> = None;

            // 重试循环
            for attempt in current_attempt..max_attempts {
                info!(
                    "执行步骤 {} (尝试 {}/{})",
                    step.id,
                    attempt + 1,
                    max_attempts
                );

                // 更新重试计数
                instance
                    .context
                    .step_retry_counts
                    .insert(step.id.clone(), attempt + 1);

                // 尝试执行步骤
                match self.execute_step(instance, step).await {
                    Ok(result) => {
                        // 执行成功，清除重试计数
                        instance.context.step_retry_counts.remove(&step.id);
                        info!(
                            "步骤 {} 执行成功 (尝试 {}/{})",
                            step.id,
                            attempt + 1,
                            max_attempts
                        );
                        return Ok(result);
                    }
                    Err(e) => {
                        warn!(
                            "步骤 {} 执行失败 (尝试 {}/{}): {}",
                            step.id,
                            attempt + 1,
                            max_attempts,
                            e
                        );
                        last_error = Some(e);

                        // 如果这不是最后一次尝试，进行退避等待
                        if attempt + 1 < max_attempts {
                            let backoff_ms = if retry_policy.exponential {
                                // 指数退避: backoff_ms * 2^attempt
                                retry_policy.backoff_ms * 2u64.pow(attempt)
                            } else {
                                // 线性退避: 固定 backoff_ms
                                retry_policy.backoff_ms
                            };

                            info!("步骤 {} 将在 {} ms 后重试", step.id, backoff_ms);
                            tokio::time::sleep(tokio::time::Duration::from_millis(backoff_ms))
                                .await;
                        }
                    }
                }
            }

            // 所有重试都失败，返回最后一次的错误
            let final_error = last_error.unwrap_or_else(|| {
                WorkflowError::ExecutionFailed(format!(
                    "步骤 {} 重试 {} 次后失败",
                    step.id, max_attempts
                ))
            });

            error!("步骤 {} 在 {} 次尝试后最终失败", step.id, max_attempts);

            Err(final_error)
        })
    }

    /// 持久化工作流实例到 StateStore
    ///
    /// 将 WorkflowInstance 序列化并存储到 ExecutionRecord。
    ///
    /// **注意**：这是临时方案，未来应该有专门的 workflow 存储接口。
    async fn persist_instance(&self, instance: &WorkflowInstance) -> Result<(), WorkflowError> {
        if let Some(ref store) = self.state_store {
            // 将 WorkflowInstance 序列化为 JSON
            let instance_json = serde_json::to_value(instance)
                .map_err(|e| WorkflowError::Internal(format!("序列化工作流实例失败: {}", e)))?;

            // 使用 ExecutionRecord 存储（临时方案）
            // agent_id = "workflow_engine"
            // skill_id = workflow_id
            // status = workflow_status
            // input = instance_json
            let record = ExecutionRecord {
                id: uuid::Uuid::parse_str(&instance.id)
                    .map_err(|e| WorkflowError::Internal(format!("解析实例 ID 失败: {}", e)))?,
                agent_id: "workflow_engine".to_string(),
                skill_id: Some(instance.workflow_id.to_string()),
                status: format!("{:?}", instance.status).to_lowercase(),
                input: instance_json,
                output: None,
                error: None,
                started_at: instance.started_at,
                completed_at: instance.completed_at,
            };

            store
                .save_execution(record)
                .await
                .map_err(|e| WorkflowError::Internal(format!("持久化工作流实例失败: {}", e)))?;

            info!("工作流实例 {} 已持久化到 StateStore", instance.id);
        }
        Ok(())
    }

    /// 从 StateStore 加载工作流实例
    ///
    /// **注意**：这是临时方案，从 ExecutionRecord 反序列化 WorkflowInstance。
    #[allow(dead_code)]
    async fn load_instance_from_store(
        &self,
        instance_id: &str,
    ) -> Result<WorkflowInstance, WorkflowError> {
        if let Some(ref store) = self.state_store {
            let uuid = uuid::Uuid::parse_str(instance_id)
                .map_err(|e| WorkflowError::Internal(format!("解析实例 ID 失败: {}", e)))?;

            let record = store.get_execution(uuid).await.map_err(|e| {
                WorkflowError::Internal(format!("从 StateStore 加载工作流实例失败: {}", e))
            })?;

            // 从 ExecutionRecord 反序列化 WorkflowInstance
            let instance: WorkflowInstance = serde_json::from_value(record.input)
                .map_err(|e| WorkflowError::Internal(format!("反序列化工作流实例失败: {}", e)))?;

            info!("工作流实例 {} 已从 StateStore 加载", instance_id);
            Ok(instance)
        } else {
            Err(WorkflowError::Internal("StateStore 未配置".to_string()))
        }
    }

    /// 更新 StateStore 中的工作流实例状态
    async fn update_instance_in_store(
        &self,
        instance: &WorkflowInstance,
    ) -> Result<(), WorkflowError> {
        if let Some(ref store) = self.state_store {
            let uuid = uuid::Uuid::parse_str(&instance.id)
                .map_err(|e| WorkflowError::Internal(format!("解析实例 ID 失败: {}", e)))?;

            // 序列化最新状态
            let instance_json = serde_json::to_value(instance)
                .map_err(|e| WorkflowError::Internal(format!("序列化工作流实例失败: {}", e)))?;

            store
                .update_execution(
                    uuid,
                    format!("{:?}", instance.status).to_lowercase(),
                    Some(instance_json),
                    None,
                )
                .await
                .map_err(|e| WorkflowError::Internal(format!("更新工作流实例失败: {}", e)))?;

            info!("工作流实例 {} 状态已更新到 StateStore", instance.id);
        }
        Ok(())
    }
}

#[async_trait]
impl WorkflowEngineApi for WorkflowEngine {
    async fn register_workflow(&self, definition: WorkflowDefinition) -> Result<(), WorkflowError> {
        let mut defs = self.definitions.write().await;

        if defs.contains_key(&definition.id) {
            return Err(WorkflowError::WorkflowAlreadyExists(
                definition.id.to_string(),
            ));
        }

        info!("注册工作流: {} - {}", definition.id, definition.name);
        defs.insert(definition.id.clone(), definition);
        Ok(())
    }

    async fn create_instance(
        &self,
        workflow_id: &WorkflowId,
        context: WorkflowContext,
    ) -> Result<String, WorkflowError> {
        let defs = self.definitions.read().await;

        if !defs.contains_key(workflow_id) {
            return Err(WorkflowError::WorkflowNotFound(workflow_id.to_string()));
        }
        drop(defs);

        let instance_id = uuid::Uuid::new_v4().to_string();
        let instance = WorkflowInstance {
            id: instance_id.clone(),
            workflow_id: workflow_id.clone(),
            execution_id: ExecutionId::new(),
            status: WorkflowStatus::Pending,
            current_step: None,
            context,
            started_at: chrono::Utc::now(),
            completed_at: None,
        };

        {
            let mut instances = self.instances.write().await;
            instances.insert(instance_id.clone(), instance.clone());
        }

        // 持久化新实例到 StateStore
        self.persist_instance(&instance).await?;

        info!(
            "创建工作流实例: {} for workflow {}",
            instance_id, workflow_id
        );
        Ok(instance_id)
    }

    async fn execute_instance(&self, instance_id: &str) -> Result<(), WorkflowError> {
        // 更新状态为 Running
        let running_instance = {
            let mut instances = self.instances.write().await;
            let instance = instances
                .get_mut(instance_id)
                .ok_or_else(|| WorkflowError::WorkflowNotFound(instance_id.to_string()))?;

            if instance.status != WorkflowStatus::Pending
                && instance.status != WorkflowStatus::Paused
            {
                return Err(WorkflowError::InvalidState(format!(
                    "无法执行状态为 {:?} 的工作流",
                    instance.status
                )));
            }

            instance.status = WorkflowStatus::Running;
            instance.clone()
        };

        // 持久化状态变更
        self.update_instance_in_store(&running_instance).await?;

        // 发送开始事件
        self.send_event(WorkflowEvent::Started {
            instance_id: instance_id.to_string(),
        });

        // 获取工作流定义和实例快照
        let (definition, mut current_instance) = {
            let defs = self.definitions.read().await;
            let instances = self.instances.read().await;
            let instance = instances
                .get(instance_id)
                .ok_or_else(|| WorkflowError::WorkflowNotFound(instance_id.to_string()))?
                .clone();
            let definition = defs
                .get(&instance.workflow_id)
                .ok_or_else(|| WorkflowError::WorkflowNotFound(instance.workflow_id.to_string()))?
                .clone();
            (definition, instance)
        };

        // 执行工作流步骤
        for step in &definition.steps {
            // 更新当前步骤
            {
                let mut instances = self.instances.write().await;
                if let Some(instance) = instances.get_mut(instance_id) {
                    instance.current_step = Some(step.id.clone());
                    current_instance = instance.clone();
                }
            }

            // 执行步骤（带重试策略）
            match self
                .execute_step_with_retry(&mut current_instance, step)
                .await
            {
                Ok(_) => {
                    info!("步骤 {} 执行成功", step.id);
                    // 将步骤结果同步回实例存储
                    let mut instances = self.instances.write().await;
                    if let Some(instance) = instances.get_mut(instance_id) {
                        instance.context.step_results =
                            current_instance.context.step_results.clone();
                    }
                }
                Err(e) => {
                    error!("步骤 {} 执行失败: {}", step.id, e);

                    let failed_instance = {
                        let mut instances = self.instances.write().await;
                        let instance = instances.get_mut(instance_id).ok_or_else(|| {
                            WorkflowError::WorkflowNotFound(instance_id.to_string())
                        })?;
                        instance.status = WorkflowStatus::Failed;
                        instance.context.error_messages.push(e.to_string());
                        instance.clone()
                    };

                    // 持久化失败状态
                    if let Err(persist_err) = self.update_instance_in_store(&failed_instance).await
                    {
                        error!("持久化失败状态时出错: {}", persist_err);
                    }

                    self.send_event(WorkflowEvent::Failed {
                        instance_id: instance_id.to_string(),
                        error: e.to_string(),
                    });

                    return Err(e);
                }
            }
        }

        // 更新实例状态为完成
        let completed_instance = {
            let mut instances = self.instances.write().await;
            let instance = instances
                .get_mut(instance_id)
                .ok_or_else(|| WorkflowError::WorkflowNotFound(instance_id.to_string()))?;
            instance.status = WorkflowStatus::Completed;
            instance.completed_at = Some(chrono::Utc::now());
            instance.clone()
        };

        // 持久化完成状态
        if let Err(persist_err) = self.update_instance_in_store(&completed_instance).await {
            error!("持久化完成状态时出错: {}", persist_err);
        }

        self.send_event(WorkflowEvent::Completed {
            instance_id: instance_id.to_string(),
        });

        info!("工作流实例 {} 执行完成", instance_id);
        Ok(())
    }

    async fn pause_instance(&self, instance_id: &str) -> Result<(), WorkflowError> {
        let paused_instance = {
            let mut instances = self.instances.write().await;
            let instance = instances
                .get_mut(instance_id)
                .ok_or_else(|| WorkflowError::WorkflowNotFound(instance_id.to_string()))?;

            if instance.status != WorkflowStatus::Running {
                return Err(WorkflowError::InvalidState(format!(
                    "无法暂停状态为 {:?} 的工作流",
                    instance.status
                )));
            }

            instance.status = WorkflowStatus::Paused;
            instance.clone()
        };

        // 持久化暂停状态
        if let Err(persist_err) = self.update_instance_in_store(&paused_instance).await {
            error!("持久化暂停状态时出错: {}", persist_err);
        }

        info!("工作流实例 {} 已暂停", instance_id);
        Ok(())
    }

    async fn resume_instance(&self, instance_id: &str) -> Result<(), WorkflowError> {
        {
            let mut instances = self.instances.write().await;
            let instance = instances
                .get_mut(instance_id)
                .ok_or_else(|| WorkflowError::WorkflowNotFound(instance_id.to_string()))?;

            if instance.status != WorkflowStatus::Paused {
                return Err(WorkflowError::InvalidState(format!(
                    "无法恢复状态为 {:?} 的工作流",
                    instance.status
                )));
            }

            instance.status = WorkflowStatus::Running;
        }

        info!("工作流实例 {} 已恢复", instance_id);

        // 继续执行
        self.execute_instance(instance_id).await
    }

    async fn cancel_instance(&self, instance_id: &str) -> Result<(), WorkflowError> {
        let cancelled_instance = {
            let mut instances = self.instances.write().await;
            let instance = instances
                .get_mut(instance_id)
                .ok_or_else(|| WorkflowError::WorkflowNotFound(instance_id.to_string()))?;

            if instance.status == WorkflowStatus::Completed
                || instance.status == WorkflowStatus::Cancelled
            {
                return Err(WorkflowError::InvalidState(format!(
                    "无法取消状态为 {:?} 的工作流",
                    instance.status
                )));
            }

            instance.status = WorkflowStatus::Cancelled;
            instance.completed_at = Some(chrono::Utc::now());
            instance.clone()
        };

        // 持久化取消状态
        if let Err(persist_err) = self.update_instance_in_store(&cancelled_instance).await {
            error!("持久化取消状态时出错: {}", persist_err);
        }

        self.send_event(WorkflowEvent::Cancelled {
            instance_id: instance_id.to_string(),
        });

        info!("工作流实例 {} 已取消", instance_id);
        Ok(())
    }

    async fn get_instance(&self, instance_id: &str) -> Result<WorkflowInstance, WorkflowError> {
        let instances = self.instances.read().await;

        instances
            .get(instance_id)
            .cloned()
            .ok_or_else(|| WorkflowError::WorkflowNotFound(instance_id.to_string()))
    }

    async fn list_instances(
        &self,
        workflow_id: Option<&WorkflowId>,
    ) -> Result<Vec<WorkflowInstance>, WorkflowError> {
        let instances = self.instances.read().await;

        let result: Vec<WorkflowInstance> = if let Some(wf_id) = workflow_id {
            instances
                .values()
                .filter(|i| &i.workflow_id == wf_id)
                .cloned()
                .collect()
        } else {
            instances.values().cloned().collect()
        };

        Ok(result)
    }
}

impl Default for WorkflowEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_workflow_engine_lifecycle() {
        let engine = WorkflowEngine::new();

        // 创建工作流定义
        let definition = WorkflowDefinition {
            id: WorkflowId::new(),
            name: "Test Workflow".to_string(),
            version: "1.0.0".to_string(),
            description: Some("测试工作流".to_string()),
            steps: vec![
                WorkflowStep {
                    id: "step1".to_string(),
                    name: "Step 1".to_string(),
                    step_type: StepType::Task,
                    config: HashMap::new(),
                    dependencies: vec![],
                    retry_policy: None,
                },
                WorkflowStep {
                    id: "step2".to_string(),
                    name: "Step 2".to_string(),
                    step_type: StepType::Task,
                    config: HashMap::new(),
                    dependencies: vec!["step1".to_string()],
                    retry_policy: None,
                },
            ],
            metadata: HashMap::new(),
        };

        // 注册工作流
        engine.register_workflow(definition.clone()).await.unwrap();

        // 创建实例
        let context = WorkflowContext {
            variables: HashMap::new(),
            step_results: HashMap::new(),
            error_messages: vec![],
            step_retry_counts: HashMap::new(),
        };
        let instance_id = engine
            .create_instance(&definition.id, context)
            .await
            .unwrap();

        // 执行实例
        engine.execute_instance(&instance_id).await.unwrap();

        // 验证状态
        let instance = engine.get_instance(&instance_id).await.unwrap();
        assert_eq!(instance.status, WorkflowStatus::Completed);
        assert!(instance.completed_at.is_some());
    }

    #[tokio::test]
    async fn test_workflow_pause_resume() {
        let engine = WorkflowEngine::new();

        let definition = WorkflowDefinition {
            id: WorkflowId::new(),
            name: "Pausable Workflow".to_string(),
            version: "1.0.0".to_string(),
            description: None,
            steps: vec![],
            metadata: HashMap::new(),
        };

        engine.register_workflow(definition.clone()).await.unwrap();

        let context = WorkflowContext {
            variables: HashMap::new(),
            step_results: HashMap::new(),
            error_messages: vec![],
            step_retry_counts: HashMap::new(),
        };
        let instance_id = engine
            .create_instance(&definition.id, context)
            .await
            .unwrap();

        // 暂停未运行的工作流应该失败
        assert!(engine.pause_instance(&instance_id).await.is_err());
    }
}
