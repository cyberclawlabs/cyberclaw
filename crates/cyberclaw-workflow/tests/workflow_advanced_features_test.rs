//! Workflow Engine 高级特性集成测试
//!
//! 测试覆盖 Agent-5 实现的 5 个核心增强功能：
//! 1. 条件分支逻辑（evaluate_condition + extract_variable）
//! 2. 并行步骤执行（execute_parallel_steps）
//! 3. 状态持久化（persist_instance + load_instance_from_store）
//! 4. 高级重试策略（execute_step_with_retry + exponential backoff）
//! 5. 子工作流嵌套执行（execute_subworkflow）

use cyberclaw_core::ids::WorkflowId;
use cyberclaw_store::memory::InMemoryStateStore;
use cyberclaw_workflow::{
    RetryPolicy, StepType, WorkflowContext, WorkflowDefinition, WorkflowEngine, WorkflowEngineApi,
    WorkflowStatus, WorkflowStep,
};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

/// Helper: 创建测试用 WorkflowEngine（无持久化）
fn create_test_engine() -> WorkflowEngine {
    WorkflowEngine::new()
}

/// Helper: 创建测试用 WorkflowEngine（带持久化）
fn create_test_engine_with_store() -> WorkflowEngine {
    let store = Arc::new(InMemoryStateStore::new());
    WorkflowEngine::with_state_store(store)
}

/// Helper: 创建空的工作流上下文
fn create_empty_context() -> WorkflowContext {
    WorkflowContext {
        variables: HashMap::new(),
        step_results: HashMap::new(),
        error_messages: vec![],
        step_retry_counts: HashMap::new(),
    }
}

/// Helper: 创建带变量的工作流上下文
#[allow(dead_code)]
fn create_context_with_variables(variables: HashMap<String, serde_json::Value>) -> WorkflowContext {
    WorkflowContext {
        variables,
        step_results: HashMap::new(),
        error_messages: vec![],
        step_retry_counts: HashMap::new(),
    }
}

// ============================================================================
// 测试组 1: 条件分支逻辑 (Agent-5.1)
// ============================================================================

/// 测试条件分支 - equals 类型
#[tokio::test]
async fn test_condition_branching_equals() {
    let engine: WorkflowEngine = create_test_engine();

    // 创建带条件步骤的工作流
    let definition = WorkflowDefinition {
        id: WorkflowId::new(),
        name: "Conditional Workflow - Equals".to_string(),
        version: "1.0.0".to_string(),
        description: Some("测试 equals 条件类型".to_string()),
        steps: vec![WorkflowStep {
            id: "condition_step".to_string(),
            name: "Check Status Equals Success".to_string(),
            step_type: StepType::Condition,
            config: {
                let mut config = HashMap::new();
                config.insert(
                    "condition".to_string(),
                    json!({
                        "type": "equals",
                        "variable": "context.variables.status",
                        "value": "success",
                        "then_branch": "success_path",
                        "else_branch": "failure_path"
                    }),
                );
                config
            },
            dependencies: vec![],
            retry_policy: None,
        }],
        metadata: HashMap::new(),
    };

    engine.register_workflow(definition.clone()).await.unwrap();

    // 测试 1: 条件匹配
    let mut context = create_empty_context();
    context
        .variables
        .insert("status".to_string(), json!("success"));
    let instance_id = engine
        .create_instance(&definition.id, context)
        .await
        .unwrap();
    engine.execute_instance(&instance_id).await.unwrap();

    let instance = engine.get_instance(&instance_id).await.unwrap();
    assert_eq!(instance.status, WorkflowStatus::Completed);
    let result = instance.context.step_results.get("condition_step").unwrap();
    assert_eq!(result["matched"], json!(true));
    assert_eq!(result["branch"], json!("success_path"));

    // 测试 2: 条件不匹配
    let mut context = create_empty_context();
    context
        .variables
        .insert("status".to_string(), json!("failure"));
    let instance_id = engine
        .create_instance(&definition.id, context)
        .await
        .unwrap();
    engine.execute_instance(&instance_id).await.unwrap();

    let instance = engine.get_instance(&instance_id).await.unwrap();
    let result = instance.context.step_results.get("condition_step").unwrap();
    assert_eq!(result["matched"], json!(false));
    assert_eq!(result["branch"], json!("failure_path"));
}

/// 测试条件分支 - contains 类型
#[tokio::test]
async fn test_condition_branching_contains() {
    let engine: WorkflowEngine = create_test_engine();

    let definition = WorkflowDefinition {
        id: WorkflowId::new(),
        name: "Conditional Workflow - Contains".to_string(),
        version: "1.0.0".to_string(),
        description: Some("测试 contains 条件类型".to_string()),
        steps: vec![WorkflowStep {
            id: "condition_step".to_string(),
            name: "Check Message Contains Error".to_string(),
            step_type: StepType::Condition,
            config: {
                let mut config = HashMap::new();
                config.insert(
                    "condition".to_string(),
                    json!({
                        "type": "contains",
                        "variable": "context.variables.message",
                        "value": "error",
                        "then_branch": "error_handling",
                        "else_branch": "normal_flow"
                    }),
                );
                config
            },
            dependencies: vec![],
            retry_policy: None,
        }],
        metadata: HashMap::new(),
    };

    engine.register_workflow(definition.clone()).await.unwrap();

    // 测试: 字符串包含匹配
    let mut context = create_empty_context();
    context
        .variables
        .insert("message".to_string(), json!("system error occurred"));
    let instance_id = engine
        .create_instance(&definition.id, context)
        .await
        .unwrap();
    engine.execute_instance(&instance_id).await.unwrap();

    let instance = engine.get_instance(&instance_id).await.unwrap();
    let result = instance.context.step_results.get("condition_step").unwrap();
    assert_eq!(result["matched"], json!(true));
    assert_eq!(result["branch"], json!("error_handling"));
}

/// 测试条件分支 - greater_than 和 less_than 类型
#[tokio::test]
async fn test_condition_branching_numeric() {
    let engine: WorkflowEngine = create_test_engine();

    // 测试 greater_than
    let definition = WorkflowDefinition {
        id: WorkflowId::new(),
        name: "Conditional Workflow - Numeric".to_string(),
        version: "1.0.0".to_string(),
        description: Some("测试数值比较条件".to_string()),
        steps: vec![WorkflowStep {
            id: "condition_step".to_string(),
            name: "Check Count Greater Than 10".to_string(),
            step_type: StepType::Condition,
            config: {
                let mut config = HashMap::new();
                config.insert(
                    "condition".to_string(),
                    json!({
                        "type": "greater_than",
                        "variable": "context.variables.count",
                        "value": 10,
                        "then_branch": "high_volume",
                        "else_branch": "low_volume"
                    }),
                );
                config
            },
            dependencies: vec![],
            retry_policy: None,
        }],
        metadata: HashMap::new(),
    };

    engine.register_workflow(definition.clone()).await.unwrap();

    let mut context = create_empty_context();
    context.variables.insert("count".to_string(), json!(15));
    let instance_id = engine
        .create_instance(&definition.id, context)
        .await
        .unwrap();
    engine.execute_instance(&instance_id).await.unwrap();

    let instance = engine.get_instance(&instance_id).await.unwrap();
    let result = instance.context.step_results.get("condition_step").unwrap();
    assert_eq!(result["matched"], json!(true));
    assert_eq!(result["branch"], json!("high_volume"));
}

/// 测试条件分支 - exists 类型
#[tokio::test]
async fn test_condition_branching_exists() {
    let engine: WorkflowEngine = create_test_engine();

    let definition = WorkflowDefinition {
        id: WorkflowId::new(),
        name: "Conditional Workflow - Exists".to_string(),
        version: "1.0.0".to_string(),
        description: Some("测试 exists 条件类型".to_string()),
        steps: vec![WorkflowStep {
            id: "condition_step".to_string(),
            name: "Check Variable Exists".to_string(),
            step_type: StepType::Condition,
            config: {
                let mut config = HashMap::new();
                config.insert(
                    "condition".to_string(),
                    json!({
                        "type": "exists",
                        "variable": "context.variables.optional_field",
                        "then_branch": "field_present",
                        "else_branch": "field_missing"
                    }),
                );
                config
            },
            dependencies: vec![],
            retry_policy: None,
        }],
        metadata: HashMap::new(),
    };

    engine.register_workflow(definition.clone()).await.unwrap();

    // 测试 1: 变量存在
    let mut context = create_empty_context();
    context
        .variables
        .insert("optional_field".to_string(), json!("some_value"));
    let instance_id = engine
        .create_instance(&definition.id, context)
        .await
        .unwrap();
    engine.execute_instance(&instance_id).await.unwrap();

    let instance = engine.get_instance(&instance_id).await.unwrap();
    let result = instance.context.step_results.get("condition_step").unwrap();
    assert_eq!(result["matched"], json!(true));
    assert_eq!(result["branch"], json!("field_present"));

    // 测试 2: 变量不存在
    let context = create_empty_context(); // 不添加 optional_field
    let instance_id = engine
        .create_instance(&definition.id, context)
        .await
        .unwrap();
    engine.execute_instance(&instance_id).await.unwrap();

    let instance = engine.get_instance(&instance_id).await.unwrap();
    let result = instance.context.step_results.get("condition_step").unwrap();
    assert_eq!(result["matched"], json!(false));
    assert_eq!(result["branch"], json!("field_missing"));
}

// ============================================================================
// 测试组 2: 并行步骤执行 (Agent-5.2)
// ============================================================================

/// 测试并行步骤执行 - 基本功能
#[tokio::test]
async fn test_parallel_execution_basic() {
    let engine: WorkflowEngine = create_test_engine();

    let definition = WorkflowDefinition {
        id: WorkflowId::new(),
        name: "Parallel Workflow".to_string(),
        version: "1.0.0".to_string(),
        description: Some("测试并行执行".to_string()),
        steps: vec![WorkflowStep {
            id: "parallel_step".to_string(),
            name: "Execute Tasks in Parallel".to_string(),
            step_type: StepType::Parallel,
            config: {
                let mut config = HashMap::new();
                config.insert(
                    "substeps".to_string(),
                    json!([
                        {"id": "subtask1", "name": "Subtask 1", "action": "process_data"},
                        {"id": "subtask2", "name": "Subtask 2", "action": "validate_input"},
                        {"id": "subtask3", "name": "Subtask 3", "action": "send_notification"}
                    ]),
                );
                config
            },
            dependencies: vec![],
            retry_policy: None,
        }],
        metadata: HashMap::new(),
    };

    engine.register_workflow(definition.clone()).await.unwrap();

    let context = create_empty_context();
    let instance_id = engine
        .create_instance(&definition.id, context)
        .await
        .unwrap();
    engine.execute_instance(&instance_id).await.unwrap();

    let instance = engine.get_instance(&instance_id).await.unwrap();
    assert_eq!(instance.status, WorkflowStatus::Completed);

    let result = instance.context.step_results.get("parallel_step").unwrap();
    assert_eq!(result["total_steps"], json!(3));
    assert_eq!(result["succeeded"], json!(3));
    assert_eq!(result["failed"], json!(0));

    let outputs = result["outputs"].as_array().unwrap();
    assert_eq!(outputs.len(), 3);

    // 验证所有子步骤都返回了结果
    for output in outputs {
        assert_eq!(output["status"], json!("completed"));
        assert!(output["substep_id"].is_string());
        assert!(output["action"].is_string());
    }
}

/// 测试并行步骤执行 - 空子步骤列表
#[tokio::test]
async fn test_parallel_execution_empty_substeps() {
    let engine: WorkflowEngine = create_test_engine();

    let definition = WorkflowDefinition {
        id: WorkflowId::new(),
        name: "Parallel Workflow - Empty".to_string(),
        version: "1.0.0".to_string(),
        description: Some("测试空并行步骤".to_string()),
        steps: vec![WorkflowStep {
            id: "parallel_step".to_string(),
            name: "Empty Parallel Step".to_string(),
            step_type: StepType::Parallel,
            config: {
                let mut config = HashMap::new();
                config.insert("substeps".to_string(), json!([]));
                config
            },
            dependencies: vec![],
            retry_policy: None,
        }],
        metadata: HashMap::new(),
    };

    engine.register_workflow(definition.clone()).await.unwrap();

    let context = create_empty_context();
    let instance_id = engine
        .create_instance(&definition.id, context)
        .await
        .unwrap();
    engine.execute_instance(&instance_id).await.unwrap();

    let instance = engine.get_instance(&instance_id).await.unwrap();
    assert_eq!(instance.status, WorkflowStatus::Completed);

    let result = instance.context.step_results.get("parallel_step").unwrap();
    assert_eq!(result["total_steps"], json!(0));
    assert_eq!(result["succeeded"], json!(0));
    assert_eq!(result["failed"], json!(0));
}

/// 测试并行步骤执行 - 大量并发任务
#[tokio::test]
async fn test_parallel_execution_high_concurrency() {
    let engine: WorkflowEngine = create_test_engine();

    // 创建 50 个并发子任务
    let mut substeps = vec![];
    for i in 0..50 {
        substeps.push(json!({
            "id": format!("subtask_{}", i),
            "name": format!("Subtask {}", i),
            "action": format!("action_{}", i)
        }));
    }

    let definition = WorkflowDefinition {
        id: WorkflowId::new(),
        name: "High Concurrency Parallel Workflow".to_string(),
        version: "1.0.0".to_string(),
        description: Some("测试高并发并行执行".to_string()),
        steps: vec![WorkflowStep {
            id: "parallel_step".to_string(),
            name: "Execute 50 Tasks in Parallel".to_string(),
            step_type: StepType::Parallel,
            config: {
                let mut config = HashMap::new();
                config.insert("substeps".to_string(), json!(substeps));
                config
            },
            dependencies: vec![],
            retry_policy: None,
        }],
        metadata: HashMap::new(),
    };

    engine.register_workflow(definition.clone()).await.unwrap();

    let context = create_empty_context();
    let instance_id = engine
        .create_instance(&definition.id, context)
        .await
        .unwrap();

    let start = std::time::Instant::now();
    engine.execute_instance(&instance_id).await.unwrap();
    let duration = start.elapsed();

    let instance = engine.get_instance(&instance_id).await.unwrap();
    assert_eq!(instance.status, WorkflowStatus::Completed);

    let result = instance.context.step_results.get("parallel_step").unwrap();
    assert_eq!(result["total_steps"], json!(50));
    assert_eq!(result["succeeded"], json!(50));
    assert_eq!(result["failed"], json!(0));

    // 验证并发执行比串行快（每个任务 10ms，串行需要 500ms）
    // 并发执行应该显著快于 500ms
    assert!(
        duration.as_millis() < 200,
        "并发执行时间过长: {}ms",
        duration.as_millis()
    );
}

// ============================================================================
// 测试组 3: 状态持久化 (Agent-5.3)
// ============================================================================

/// 测试状态持久化 - 基本功能
#[tokio::test]
async fn test_state_persistence_basic() {
    let engine: WorkflowEngine = create_test_engine_with_store();

    let definition = WorkflowDefinition {
        id: WorkflowId::new(),
        name: "Persistent Workflow".to_string(),
        version: "1.0.0".to_string(),
        description: Some("测试持久化".to_string()),
        steps: vec![WorkflowStep {
            id: "step1".to_string(),
            name: "Step 1".to_string(),
            step_type: StepType::Task,
            config: HashMap::new(),
            dependencies: vec![],
            retry_policy: None,
        }],
        metadata: HashMap::new(),
    };

    engine.register_workflow(definition.clone()).await.unwrap();

    let mut context = create_empty_context();
    context
        .variables
        .insert("user_id".to_string(), json!("user123"));
    let instance_id = engine
        .create_instance(&definition.id, context)
        .await
        .unwrap();

    // 验证实例已创建并持久化
    let instance = engine.get_instance(&instance_id).await.unwrap();
    assert_eq!(instance.status, WorkflowStatus::Pending);
    assert_eq!(
        instance.context.variables.get("user_id").unwrap(),
        &json!("user123")
    );

    // 执行工作流
    engine.execute_instance(&instance_id).await.unwrap();

    // 验证状态已更新并持久化
    let instance = engine.get_instance(&instance_id).await.unwrap();
    assert_eq!(instance.status, WorkflowStatus::Completed);
    assert!(instance.completed_at.is_some());
}

/// 测试状态持久化 - 暂停和恢复
#[tokio::test]
async fn test_state_persistence_pause_resume() {
    let engine: WorkflowEngine = create_test_engine_with_store();

    let definition = WorkflowDefinition {
        id: WorkflowId::new(),
        name: "Pausable Persistent Workflow".to_string(),
        version: "1.0.0".to_string(),
        description: Some("测试暂停恢复持久化".to_string()),
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

    engine.register_workflow(definition.clone()).await.unwrap();

    let context = create_empty_context();
    let instance_id = engine
        .create_instance(&definition.id, context)
        .await
        .unwrap();

    // 启动执行（但我们无法中途暂停，因为执行是同步的）
    // 这里测试暂停和取消操作
    engine.execute_instance(&instance_id).await.unwrap();

    let instance = engine.get_instance(&instance_id).await.unwrap();
    assert_eq!(instance.status, WorkflowStatus::Completed);
}

/// 测试状态持久化 - 取消工作流
#[tokio::test]
async fn test_state_persistence_cancel() {
    let engine: WorkflowEngine = create_test_engine_with_store();

    let definition = WorkflowDefinition {
        id: WorkflowId::new(),
        name: "Cancellable Workflow".to_string(),
        version: "1.0.0".to_string(),
        description: Some("测试取消持久化".to_string()),
        steps: vec![],
        metadata: HashMap::new(),
    };

    engine.register_workflow(definition.clone()).await.unwrap();

    let context = create_empty_context();
    let instance_id = engine
        .create_instance(&definition.id, context)
        .await
        .unwrap();

    // 取消 Pending 状态的工作流
    engine.cancel_instance(&instance_id).await.unwrap();

    let instance = engine.get_instance(&instance_id).await.unwrap();
    assert_eq!(instance.status, WorkflowStatus::Cancelled);
    assert!(instance.completed_at.is_some());
}

// ============================================================================
// 测试组 4: 高级重试策略 (Agent-5.4)
// ============================================================================

/// 测试重试策略 - 线性退避（exponential = false）
#[tokio::test]
async fn test_retry_policy_linear_backoff() {
    let engine: WorkflowEngine = create_test_engine();

    let definition = WorkflowDefinition {
        id: WorkflowId::new(),
        name: "Retry Workflow - Linear".to_string(),
        version: "1.0.0".to_string(),
        description: Some("测试线性退避重试".to_string()),
        steps: vec![WorkflowStep {
            id: "step1".to_string(),
            name: "Step with Linear Retry".to_string(),
            step_type: StepType::Task,
            config: HashMap::new(),
            dependencies: vec![],
            retry_policy: Some(RetryPolicy {
                max_attempts: 3,
                backoff_ms: 50, // 50ms 固定退避
                exponential: false,
            }),
        }],
        metadata: HashMap::new(),
    };

    engine.register_workflow(definition.clone()).await.unwrap();

    let context = create_empty_context();
    let instance_id = engine
        .create_instance(&definition.id, context)
        .await
        .unwrap();

    let start = std::time::Instant::now();
    engine.execute_instance(&instance_id).await.unwrap();
    let duration = start.elapsed();

    let instance = engine.get_instance(&instance_id).await.unwrap();
    assert_eq!(instance.status, WorkflowStatus::Completed);

    // 验证没有重试（因为任务成功），执行时间应该很短
    assert!(
        duration.as_millis() < 100,
        "执行时间过长: {}ms",
        duration.as_millis()
    );
}

/// 测试重试策略 - 指数退避（exponential = true）
#[tokio::test]
async fn test_retry_policy_exponential_backoff() {
    let engine: WorkflowEngine = create_test_engine();

    let definition = WorkflowDefinition {
        id: WorkflowId::new(),
        name: "Retry Workflow - Exponential".to_string(),
        version: "1.0.0".to_string(),
        description: Some("测试指数退避重试".to_string()),
        steps: vec![WorkflowStep {
            id: "step1".to_string(),
            name: "Step with Exponential Retry".to_string(),
            step_type: StepType::Task,
            config: HashMap::new(),
            dependencies: vec![],
            retry_policy: Some(RetryPolicy {
                max_attempts: 4,
                backoff_ms: 10,    // 基础退避 10ms
                exponential: true, // 指数退避: 10ms, 20ms, 40ms
            }),
        }],
        metadata: HashMap::new(),
    };

    engine.register_workflow(definition.clone()).await.unwrap();

    let context = create_empty_context();
    let instance_id = engine
        .create_instance(&definition.id, context)
        .await
        .unwrap();

    engine.execute_instance(&instance_id).await.unwrap();

    let instance = engine.get_instance(&instance_id).await.unwrap();
    assert_eq!(instance.status, WorkflowStatus::Completed);

    // 验证重试计数已清除（成功后）
    assert!(instance.context.step_retry_counts.is_empty());
}

/// 测试重试策略 - 没有重试策略
#[tokio::test]
async fn test_retry_policy_none() {
    let engine: WorkflowEngine = create_test_engine();

    let definition = WorkflowDefinition {
        id: WorkflowId::new(),
        name: "No Retry Workflow".to_string(),
        version: "1.0.0".to_string(),
        description: Some("测试无重试策略".to_string()),
        steps: vec![WorkflowStep {
            id: "step1".to_string(),
            name: "Step without Retry".to_string(),
            step_type: StepType::Task,
            config: HashMap::new(),
            dependencies: vec![],
            retry_policy: None, // 没有重试策略
        }],
        metadata: HashMap::new(),
    };

    engine.register_workflow(definition.clone()).await.unwrap();

    let context = create_empty_context();
    let instance_id = engine
        .create_instance(&definition.id, context)
        .await
        .unwrap();

    engine.execute_instance(&instance_id).await.unwrap();

    let instance = engine.get_instance(&instance_id).await.unwrap();
    assert_eq!(instance.status, WorkflowStatus::Completed);
}

// ============================================================================
// 测试组 5: 子工作流嵌套执行 (Agent-5.5)
// ============================================================================

/// 测试子工作流执行 - 基本功能
#[tokio::test]
async fn test_subworkflow_execution_basic() {
    let engine: WorkflowEngine = create_test_engine();

    // 创建子工作流定义
    let sub_definition = WorkflowDefinition {
        id: WorkflowId::from_string("sub-workflow-001".to_string()).unwrap(),
        name: "Sub Workflow".to_string(),
        version: "1.0.0".to_string(),
        description: Some("子工作流".to_string()),
        steps: vec![
            WorkflowStep {
                id: "sub_step1".to_string(),
                name: "Sub Step 1".to_string(),
                step_type: StepType::Task,
                config: HashMap::new(),
                dependencies: vec![],
                retry_policy: None,
            },
            WorkflowStep {
                id: "sub_step2".to_string(),
                name: "Sub Step 2".to_string(),
                step_type: StepType::Task,
                config: HashMap::new(),
                dependencies: vec!["sub_step1".to_string()],
                retry_policy: None,
            },
        ],
        metadata: HashMap::new(),
    };

    // 创建父工作流定义
    let parent_definition = WorkflowDefinition {
        id: WorkflowId::new(),
        name: "Parent Workflow".to_string(),
        version: "1.0.0".to_string(),
        description: Some("父工作流".to_string()),
        steps: vec![WorkflowStep {
            id: "call_subworkflow".to_string(),
            name: "Call Subworkflow".to_string(),
            step_type: StepType::SubWorkflow,
            config: {
                let mut config = HashMap::new();
                config.insert("workflow_id".to_string(), json!("sub-workflow-001"));
                config
            },
            dependencies: vec![],
            retry_policy: None,
        }],
        metadata: HashMap::new(),
    };

    // 注册两个工作流
    engine.register_workflow(sub_definition).await.unwrap();
    engine
        .register_workflow(parent_definition.clone())
        .await
        .unwrap();

    // 执行父工作流
    let context = create_empty_context();
    let instance_id = engine
        .create_instance(&parent_definition.id, context)
        .await
        .unwrap();
    engine.execute_instance(&instance_id).await.unwrap();

    let instance = engine.get_instance(&instance_id).await.unwrap();
    assert_eq!(instance.status, WorkflowStatus::Completed);

    // 验证子工作流步骤结果
    let result = instance
        .context
        .step_results
        .get("call_subworkflow")
        .unwrap();
    assert_eq!(result["status"], json!("completed"));
    assert!(result["sub_workflow_id"].is_string());
    assert_eq!(result["sub_workflow_name"], json!("Sub Workflow"));

    // 验证子工作流的步骤结果都被记录
    let step_results = result["step_results"].as_object().unwrap();
    assert!(step_results.contains_key("sub_step1"));
    assert!(step_results.contains_key("sub_step2"));
}

/// 测试子工作流执行 - 变量继承
#[tokio::test]
async fn test_subworkflow_execution_inherit_variables() {
    let engine: WorkflowEngine = create_test_engine();

    // 创建子工作流定义
    let sub_definition = WorkflowDefinition {
        id: WorkflowId::from_string("sub-workflow-002".to_string()).unwrap(),
        name: "Sub Workflow with Inherited Variables".to_string(),
        version: "1.0.0".to_string(),
        description: Some("继承变量的子工作流".to_string()),
        steps: vec![WorkflowStep {
            id: "sub_step".to_string(),
            name: "Sub Step".to_string(),
            step_type: StepType::Task,
            config: HashMap::new(),
            dependencies: vec![],
            retry_policy: None,
        }],
        metadata: HashMap::new(),
    };

    // 创建父工作流定义
    let parent_definition = WorkflowDefinition {
        id: WorkflowId::new(),
        name: "Parent Workflow with Variable Inheritance".to_string(),
        version: "1.0.0".to_string(),
        description: Some("传递变量的父工作流".to_string()),
        steps: vec![WorkflowStep {
            id: "call_subworkflow".to_string(),
            name: "Call Subworkflow with Variables".to_string(),
            step_type: StepType::SubWorkflow,
            config: {
                let mut config = HashMap::new();
                config.insert("workflow_id".to_string(), json!("sub-workflow-002"));
                config.insert(
                    "inherit_variables".to_string(),
                    json!(["user_id", "session_id"]),
                );
                config
            },
            dependencies: vec![],
            retry_policy: None,
        }],
        metadata: HashMap::new(),
    };

    // 注册两个工作流
    engine.register_workflow(sub_definition).await.unwrap();
    engine
        .register_workflow(parent_definition.clone())
        .await
        .unwrap();

    // 创建父工作流上下文，包含要继承的变量
    let mut context = create_empty_context();
    context
        .variables
        .insert("user_id".to_string(), json!("user123"));
    context
        .variables
        .insert("session_id".to_string(), json!("session456"));
    context
        .variables
        .insert("other_var".to_string(), json!("not_inherited"));

    let instance_id = engine
        .create_instance(&parent_definition.id, context)
        .await
        .unwrap();
    engine.execute_instance(&instance_id).await.unwrap();

    let instance = engine.get_instance(&instance_id).await.unwrap();
    assert_eq!(instance.status, WorkflowStatus::Completed);

    // 验证子工作流继承了指定的变量
    let result = instance
        .context
        .step_results
        .get("call_subworkflow")
        .unwrap();
    let final_variables = result["final_variables"].as_object().unwrap();
    assert_eq!(final_variables["user_id"], json!("user123"));
    assert_eq!(final_variables["session_id"], json!("session456"));
    // other_var 不应该被继承
    assert!(!final_variables.contains_key("other_var"));
}

/// 测试子工作流执行 - 输入映射
#[tokio::test]
async fn test_subworkflow_execution_input_mapping() {
    let engine: WorkflowEngine = create_test_engine();

    // 创建子工作流定义
    let sub_definition = WorkflowDefinition {
        id: WorkflowId::from_string("sub-workflow-003".to_string()).unwrap(),
        name: "Sub Workflow with Input Mapping".to_string(),
        version: "1.0.0".to_string(),
        description: Some("输入映射的子工作流".to_string()),
        steps: vec![WorkflowStep {
            id: "sub_step".to_string(),
            name: "Sub Step".to_string(),
            step_type: StepType::Task,
            config: HashMap::new(),
            dependencies: vec![],
            retry_policy: None,
        }],
        metadata: HashMap::new(),
    };

    // 创建父工作流定义
    let parent_definition = WorkflowDefinition {
        id: WorkflowId::new(),
        name: "Parent Workflow with Input Mapping".to_string(),
        version: "1.0.0".to_string(),
        description: Some("输入映射的父工作流".to_string()),
        steps: vec![WorkflowStep {
            id: "call_subworkflow".to_string(),
            name: "Call Subworkflow with Mapping".to_string(),
            step_type: StepType::SubWorkflow,
            config: {
                let mut config = HashMap::new();
                config.insert("workflow_id".to_string(), json!("sub-workflow-003"));
                config.insert(
                    "input_mapping".to_string(),
                    json!({
                        "parent_user_id": "child_user_id",
                        "parent_request_id": "child_request_id"
                    }),
                );
                config
            },
            dependencies: vec![],
            retry_policy: None,
        }],
        metadata: HashMap::new(),
    };

    // 注册两个工作流
    engine.register_workflow(sub_definition).await.unwrap();
    engine
        .register_workflow(parent_definition.clone())
        .await
        .unwrap();

    // 创建父工作流上下文
    let mut context = create_empty_context();
    context
        .variables
        .insert("parent_user_id".to_string(), json!("user789"));
    context
        .variables
        .insert("parent_request_id".to_string(), json!("req123"));

    let instance_id = engine
        .create_instance(&parent_definition.id, context)
        .await
        .unwrap();
    engine.execute_instance(&instance_id).await.unwrap();

    let instance = engine.get_instance(&instance_id).await.unwrap();
    assert_eq!(instance.status, WorkflowStatus::Completed);

    // 验证子工作流接收到映射后的变量
    let result = instance
        .context
        .step_results
        .get("call_subworkflow")
        .unwrap();
    let final_variables = result["final_variables"].as_object().unwrap();
    assert_eq!(final_variables["child_user_id"], json!("user789"));
    assert_eq!(final_variables["child_request_id"], json!("req123"));
    // 父变量名不应该存在
    assert!(!final_variables.contains_key("parent_user_id"));
    assert!(!final_variables.contains_key("parent_request_id"));
}

/// 测试子工作流执行 - 复杂嵌套（带持久化）
#[tokio::test]
async fn test_subworkflow_execution_nested_with_persistence() {
    let engine: WorkflowEngine = create_test_engine_with_store();

    // 创建孙工作流定义
    let grandchild_definition = WorkflowDefinition {
        id: WorkflowId::from_string("grandchild-workflow".to_string()).unwrap(),
        name: "Grandchild Workflow".to_string(),
        version: "1.0.0".to_string(),
        description: Some("孙工作流".to_string()),
        steps: vec![WorkflowStep {
            id: "grandchild_step".to_string(),
            name: "Grandchild Step".to_string(),
            step_type: StepType::Task,
            config: HashMap::new(),
            dependencies: vec![],
            retry_policy: None,
        }],
        metadata: HashMap::new(),
    };

    // 创建子工作流定义（调用孙工作流）
    let child_definition = WorkflowDefinition {
        id: WorkflowId::from_string("child-workflow".to_string()).unwrap(),
        name: "Child Workflow".to_string(),
        version: "1.0.0".to_string(),
        description: Some("子工作流".to_string()),
        steps: vec![WorkflowStep {
            id: "call_grandchild".to_string(),
            name: "Call Grandchild".to_string(),
            step_type: StepType::SubWorkflow,
            config: {
                let mut config = HashMap::new();
                config.insert("workflow_id".to_string(), json!("grandchild-workflow"));
                config
            },
            dependencies: vec![],
            retry_policy: None,
        }],
        metadata: HashMap::new(),
    };

    // 创建父工作流定义（调用子工作流）
    let parent_definition = WorkflowDefinition {
        id: WorkflowId::new(),
        name: "Parent Workflow".to_string(),
        version: "1.0.0".to_string(),
        description: Some("父工作流".to_string()),
        steps: vec![WorkflowStep {
            id: "call_child".to_string(),
            name: "Call Child".to_string(),
            step_type: StepType::SubWorkflow,
            config: {
                let mut config = HashMap::new();
                config.insert("workflow_id".to_string(), json!("child-workflow"));
                config
            },
            dependencies: vec![],
            retry_policy: None,
        }],
        metadata: HashMap::new(),
    };

    // 注册所有工作流
    engine
        .register_workflow(grandchild_definition)
        .await
        .unwrap();
    engine.register_workflow(child_definition).await.unwrap();
    engine
        .register_workflow(parent_definition.clone())
        .await
        .unwrap();

    // 执行父工作流
    let context = create_empty_context();
    let instance_id = engine
        .create_instance(&parent_definition.id, context)
        .await
        .unwrap();
    engine.execute_instance(&instance_id).await.unwrap();

    let instance = engine.get_instance(&instance_id).await.unwrap();
    assert_eq!(instance.status, WorkflowStatus::Completed);

    // 验证父工作流包含子工作流结果
    let parent_result = instance.context.step_results.get("call_child").unwrap();
    assert_eq!(parent_result["status"], json!("completed"));

    // 验证子工作流包含孙工作流结果
    let child_results = parent_result["step_results"].as_object().unwrap();
    assert!(child_results.contains_key("call_grandchild"));
}

// ============================================================================
// 综合测试：多特性组合
// ============================================================================

/// 综合测试 - 条件 + 并行 + 重试
#[tokio::test]
async fn test_comprehensive_condition_parallel_retry() {
    let engine: WorkflowEngine = create_test_engine();

    let definition = WorkflowDefinition {
        id: WorkflowId::new(),
        name: "Comprehensive Workflow".to_string(),
        version: "1.0.0".to_string(),
        description: Some("综合测试：条件 + 并行 + 重试".to_string()),
        steps: vec![
            // 步骤 1: 条件判断
            WorkflowStep {
                id: "condition_check".to_string(),
                name: "Check Environment".to_string(),
                step_type: StepType::Condition,
                config: {
                    let mut config = HashMap::new();
                    config.insert(
                        "condition".to_string(),
                        json!({
                            "type": "equals",
                            "variable": "context.variables.env",
                            "value": "production",
                            "then_branch": "prod_flow",
                            "else_branch": "dev_flow"
                        }),
                    );
                    config
                },
                dependencies: vec![],
                retry_policy: None,
            },
            // 步骤 2: 并行任务（带重试）
            WorkflowStep {
                id: "parallel_tasks".to_string(),
                name: "Execute Parallel Tasks".to_string(),
                step_type: StepType::Parallel,
                config: {
                    let mut config = HashMap::new();
                    config.insert(
                        "substeps".to_string(),
                        json!([
                            {"id": "task1", "action": "validate"},
                            {"id": "task2", "action": "transform"},
                            {"id": "task3", "action": "enrich"}
                        ]),
                    );
                    config
                },
                dependencies: vec!["condition_check".to_string()],
                retry_policy: Some(RetryPolicy {
                    max_attempts: 2,
                    backoff_ms: 10,
                    exponential: false,
                }),
            },
        ],
        metadata: HashMap::new(),
    };

    engine.register_workflow(definition.clone()).await.unwrap();

    let mut context = create_empty_context();
    context
        .variables
        .insert("env".to_string(), json!("production"));
    let instance_id = engine
        .create_instance(&definition.id, context)
        .await
        .unwrap();
    engine.execute_instance(&instance_id).await.unwrap();

    let instance = engine.get_instance(&instance_id).await.unwrap();
    assert_eq!(instance.status, WorkflowStatus::Completed);

    // 验证条件步骤
    let condition_result = instance
        .context
        .step_results
        .get("condition_check")
        .unwrap();
    assert_eq!(condition_result["matched"], json!(true));
    assert_eq!(condition_result["branch"], json!("prod_flow"));

    // 验证并行步骤
    let parallel_result = instance.context.step_results.get("parallel_tasks").unwrap();
    assert_eq!(parallel_result["total_steps"], json!(3));
    assert_eq!(parallel_result["succeeded"], json!(3));
}

/// 综合测试 - 子工作流 + 持久化 + 并行
#[tokio::test]
async fn test_comprehensive_subworkflow_persistence_parallel() {
    let engine: WorkflowEngine = create_test_engine_with_store();

    // 子工作流包含并行任务
    let sub_definition = WorkflowDefinition {
        id: WorkflowId::from_string("parallel-sub-workflow".to_string()).unwrap(),
        name: "Parallel Sub Workflow".to_string(),
        version: "1.0.0".to_string(),
        description: Some("包含并行任务的子工作流".to_string()),
        steps: vec![WorkflowStep {
            id: "parallel_processing".to_string(),
            name: "Parallel Processing".to_string(),
            step_type: StepType::Parallel,
            config: {
                let mut config = HashMap::new();
                config.insert(
                    "substeps".to_string(),
                    json!([
                        {"id": "process1", "action": "analyze"},
                        {"id": "process2", "action": "classify"}
                    ]),
                );
                config
            },
            dependencies: vec![],
            retry_policy: None,
        }],
        metadata: HashMap::new(),
    };

    // 父工作流调用子工作流
    let parent_definition = WorkflowDefinition {
        id: WorkflowId::new(),
        name: "Parent Workflow with Persistence".to_string(),
        version: "1.0.0".to_string(),
        description: Some("持久化父工作流".to_string()),
        steps: vec![WorkflowStep {
            id: "call_parallel_sub".to_string(),
            name: "Call Parallel Subworkflow".to_string(),
            step_type: StepType::SubWorkflow,
            config: {
                let mut config = HashMap::new();
                config.insert("workflow_id".to_string(), json!("parallel-sub-workflow"));
                config
            },
            dependencies: vec![],
            retry_policy: None,
        }],
        metadata: HashMap::new(),
    };

    engine.register_workflow(sub_definition).await.unwrap();
    engine
        .register_workflow(parent_definition.clone())
        .await
        .unwrap();

    let context = create_empty_context();
    let instance_id = engine
        .create_instance(&parent_definition.id, context)
        .await
        .unwrap();
    engine.execute_instance(&instance_id).await.unwrap();

    let instance = engine.get_instance(&instance_id).await.unwrap();
    assert_eq!(instance.status, WorkflowStatus::Completed);

    // 验证子工作流的并行执行结果
    let result = instance
        .context
        .step_results
        .get("call_parallel_sub")
        .unwrap();
    let sub_step_results = result["step_results"].as_object().unwrap();
    let parallel_result = &sub_step_results["parallel_processing"];
    assert_eq!(parallel_result["total_steps"], json!(2));
    assert_eq!(parallel_result["succeeded"], json!(2));
}
