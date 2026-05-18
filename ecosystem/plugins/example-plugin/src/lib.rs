//! Example CyberClaw Plugin
//!
//! 这是一个示例 Plugin，演示了如何：
//! - 实现 Plugin 入口函数
//! - 注册和处理 Hooks
//! - 管理 Plugin 生命周期
//! - 实现安全策略

use cyberclaw_plugin_runtime::{
    Plugin, PluginApi, PluginManifest, HookRegistration, HookHandler,
    HookContext, HookOutput, HookType, Result, Error,
};
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::RwLock;
use serde_json::json;
use tracing::{info, warn, error, instrument};
use chrono::Utc;

/// Plugin 初始化函数 - 必须导出此函数
#[no_mangle]
pub extern "C" fn cyberclaw_plugin_init(
    manifest: &PluginManifest,
) -> Result<Box<dyn PluginApi>> {
    // 初始化日志
    tracing::info!("Initializing Example Plugin v{}", manifest.plugin.version);

    // 验证 manifest
    validate_manifest(manifest)?;

    // 创建 Plugin 实例
    let plugin = ExamplePlugin::new(manifest)?;

    Ok(Box::new(plugin))
}

/// 验证 Plugin Manifest
fn validate_manifest(manifest: &PluginManifest) -> Result<()> {
    if manifest.plugin.id.is_empty() {
        return Err(Error::InvalidManifest("Plugin ID is required".into()));
    }

    if manifest.plugin.version.is_empty() {
        return Err(Error::InvalidManifest("Plugin version is required".into()));
    }

    Ok(())
}

/// Example Plugin 实现
pub struct ExamplePlugin {
    id: String,
    hooks: Vec<HookRegistration>,
    state: Arc<RwLock<PluginState>>,
}

/// Plugin 内部状态
#[derive(Debug, Default)]
struct PluginState {
    execution_count: u64,
    failure_count: u64,
    last_execution_id: Option<String>,
    metrics: PluginMetrics,
}

/// Plugin 指标
#[derive(Debug, Default)]
struct PluginMetrics {
    total_hook_calls: u64,
    total_duration_ms: u64,
    errors: Vec<String>,
}

impl ExamplePlugin {
    pub fn new(manifest: &PluginManifest) -> Result<Self> {
        // 创建 Hook 注册
        let hooks = vec![
            HookRegistration {
                plugin_id: manifest.plugin.id.clone(),
                hook_type: HookType::BeforeExecution,
                handler: Arc::new(BeforeExecutionHandler::new()),
                priority: 100,
                timeout_ms: 5000,
            },
            HookRegistration {
                plugin_id: manifest.plugin.id.clone(),
                hook_type: HookType::AfterExecution,
                handler: Arc::new(AfterExecutionHandler::new()),
                priority: 100,
                timeout_ms: 5000,
            },
            HookRegistration {
                plugin_id: manifest.plugin.id.clone(),
                hook_type: HookType::OnFailure,
                handler: Arc::new(OnFailureHandler::new()),
                priority: 50,
                timeout_ms: 3000,
            },
            HookRegistration {
                plugin_id: manifest.plugin.id.clone(),
                hook_type: HookType::OnReview,
                handler: Arc::new(OnReviewHandler::new()),
                priority: 75,
                timeout_ms: 5000,
            },
        ];

        Ok(Self {
            id: manifest.plugin.id.clone(),
            hooks,
            state: Arc::new(RwLock::new(PluginState::default())),
        })
    }

    async fn update_metrics(&self, duration_ms: u64) {
        let mut state = self.state.write().await;
        state.metrics.total_hook_calls += 1;
        state.metrics.total_duration_ms += duration_ms;
    }
}

#[async_trait]
impl PluginApi for ExamplePlugin {
    fn id(&self) -> &str {
        &self.id
    }

    fn hooks(&self) -> &[HookRegistration] {
        &self.hooks
    }

    #[instrument(skip(self))]
    async fn start(&mut self) -> Result<()> {
        info!("Starting Example Plugin");

        // 初始化资源
        let mut state = self.state.write().await;
        state.execution_count = 0;
        state.failure_count = 0;

        info!("Example Plugin started successfully");
        Ok(())
    }

    #[instrument(skip(self))]
    async fn stop(&mut self) -> Result<()> {
        info!("Stopping Example Plugin");

        // 清理资源
        let state = self.state.read().await;
        info!(
            "Plugin statistics - Executions: {}, Failures: {}",
            state.execution_count,
            state.failure_count
        );

        Ok(())
    }

    async fn health_check(&self) -> Result<cyberclaw_plugin_runtime::HealthStatus> {
        let state = self.state.read().await;

        // 检查错误率
        let error_rate = if state.execution_count > 0 {
            state.failure_count as f64 / state.execution_count as f64
        } else {
            0.0
        };

        let status = if error_rate > 0.5 {
            cyberclaw_plugin_runtime::HealthStatus::Unhealthy
        } else if error_rate > 0.2 {
            cyberclaw_plugin_runtime::HealthStatus::Degraded
        } else {
            cyberclaw_plugin_runtime::HealthStatus::Healthy
        };

        Ok(status)
    }

    async fn metrics(&self) -> Result<cyberclaw_plugin_runtime::Metrics> {
        let state = self.state.read().await;

        Ok(cyberclaw_plugin_runtime::Metrics {
            counters: vec![
                ("execution_count".to_string(), state.execution_count as i64),
                ("failure_count".to_string(), state.failure_count as i64),
                ("hook_calls".to_string(), state.metrics.total_hook_calls as i64),
            ],
            gauges: vec![],
            histograms: vec![],
        })
    }
}

// ==================== Hook Handlers ====================

/// BeforeExecution Hook Handler
struct BeforeExecutionHandler {
    audit_log: Arc<RwLock<Vec<AuditEntry>>>,
}

impl BeforeExecutionHandler {
    fn new() -> Self {
        Self {
            audit_log: Arc::new(RwLock::new(Vec::new())),
        }
    }
}

#[async_trait]
impl HookHandler for BeforeExecutionHandler {
    #[instrument(skip(self, context))]
    async fn handle(&self, context: &HookContext) -> Result<HookOutput> {
        let start = std::time::Instant::now();

        info!("BeforeExecution hook triggered for: {}", context.execution_id);

        // 记录审计日志
        let entry = AuditEntry {
            timestamp: Utc::now(),
            execution_id: context.execution_id.clone(),
            phase: "before_execution".to_string(),
            params: context.params.clone(),
        };

        self.audit_log.write().await.push(entry);

        // 参数验证
        if let Some(sensitive) = context.params.get("sensitive_data") {
            warn!("Sensitive data detected in parameters");
            // 可以选择拒绝执行或清理数据
        }

        // 注入额外参数
        let mut modified_params = context.params.clone();
        modified_params["plugin_processed"] = json!(true);
        modified_params["plugin_timestamp"] = json!(Utc::now().to_rfc3339());

        let duration_ms = start.elapsed().as_millis() as u64;

        Ok(HookOutput {
            modified_params: Some(modified_params),
            metadata: vec![
                ("plugin".to_string(), "example-plugin".to_string()),
                ("hook".to_string(), "before_execution".to_string()),
                ("duration_ms".to_string(), duration_ms.to_string()),
            ],
        })
    }
}

/// AfterExecution Hook Handler
struct AfterExecutionHandler;

impl AfterExecutionHandler {
    fn new() -> Self {
        Self
    }
}

#[async_trait]
impl HookHandler for AfterExecutionHandler {
    async fn handle(&self, context: &HookContext) -> Result<HookOutput> {
        info!("AfterExecution hook triggered for: {}", context.execution_id);

        // 处理执行结果
        if let Some(result) = &context.result {
            if result.get("error").is_some() {
                error!("Execution failed with error: {:?}", result["error"]);
            } else {
                info!("Execution completed successfully");
            }
        }

        // 清理临时资源
        // cleanup_temp_resources(&context.execution_id).await?;

        Ok(HookOutput {
            modified_params: None,
            metadata: vec![
                ("plugin".to_string(), "example-plugin".to_string()),
                ("hook".to_string(), "after_execution".to_string()),
            ],
        })
    }
}

/// OnFailure Hook Handler
struct OnFailureHandler {
    notification_service: Arc<NotificationService>,
}

impl OnFailureHandler {
    fn new() -> Self {
        Self {
            notification_service: Arc::new(NotificationService::new()),
        }
    }
}

#[async_trait]
impl HookHandler for OnFailureHandler {
    async fn handle(&self, context: &HookContext) -> Result<HookOutput> {
        error!("OnFailure hook triggered for: {}", context.execution_id);

        // 发送失败通知
        if let Some(error) = &context.error {
            self.notification_service
                .send_failure_notification(&context.execution_id, error)
                .await?;
        }

        // 记录失败详情
        let failure_details = json!({
            "execution_id": context.execution_id,
            "error": context.error,
            "timestamp": Utc::now().to_rfc3339(),
            "params": context.params,
        });

        // 可以选择重试
        let should_retry = self.should_retry(&context.error);

        Ok(HookOutput {
            modified_params: if should_retry {
                Some(json!({ "retry": true }))
            } else {
                None
            },
            metadata: vec![
                ("plugin".to_string(), "example-plugin".to_string()),
                ("hook".to_string(), "on_failure".to_string()),
                ("should_retry".to_string(), should_retry.to_string()),
            ],
        })
    }
}

impl OnFailureHandler {
    fn should_retry(&self, error: &Option<String>) -> bool {
        // 根据错误类型决定是否重试
        if let Some(err) = error {
            err.contains("timeout") || err.contains("connection")
        } else {
            false
        }
    }
}

/// OnReview Hook Handler
struct OnReviewHandler;

impl OnReviewHandler {
    fn new() -> Self {
        Self
    }
}

#[async_trait]
impl HookHandler for OnReviewHandler {
    async fn handle(&self, context: &HookContext) -> Result<HookOutput> {
        info!("OnReview hook triggered for: {}", context.execution_id);

        // 执行审核逻辑
        let review_result = self.perform_review(&context.params).await?;

        Ok(HookOutput {
            modified_params: Some(json!({
                "review_result": review_result,
                "reviewed_at": Utc::now().to_rfc3339(),
            })),
            metadata: vec![
                ("plugin".to_string(), "example-plugin".to_string()),
                ("hook".to_string(), "on_review".to_string()),
                ("review_result".to_string(), review_result.clone()),
            ],
        })
    }
}

impl OnReviewHandler {
    async fn perform_review(&self, params: &serde_json::Value) -> Result<String> {
        // 实现审核逻辑
        if params.get("risk_level").and_then(|v| v.as_str()) == Some("high") {
            Ok("rejected".to_string())
        } else {
            Ok("approved".to_string())
        }
    }
}

// ==================== 辅助结构 ====================

#[derive(Debug, Clone)]
struct AuditEntry {
    timestamp: chrono::DateTime<Utc>,
    execution_id: String,
    phase: String,
    params: serde_json::Value,
}

struct NotificationService;

impl NotificationService {
    fn new() -> Self {
        Self
    }

    async fn send_failure_notification(
        &self,
        execution_id: &str,
        error: &str,
    ) -> Result<()> {
        // 实现通知逻辑（例如发送到 Slack、Email 等）
        info!(
            "Sending failure notification for execution {}: {}",
            execution_id, error
        );
        Ok(())
    }
}

// ==================== 测试 ====================

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_plugin_initialization() {
        let manifest = create_test_manifest();
        let result = cyberclaw_plugin_init(&manifest);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_before_execution_hook() {
        let handler = BeforeExecutionHandler::new();
        let context = HookContext {
            execution_id: "test-001".to_string(),
            phase: HookType::BeforeExecution,
            params: json!({"test": true}),
            result: None,
            error: None,
            metadata: Default::default(),
        };

        let result = handler.handle(&context).await.unwrap();
        assert!(result.modified_params.is_some());
        assert!(result.modified_params.unwrap()["plugin_processed"].as_bool().unwrap());
    }

    fn create_test_manifest() -> PluginManifest {
        PluginManifest {
            plugin: cyberclaw_plugin_runtime::PluginInfo {
                id: "test-plugin".to_string(),
                name: "Test Plugin".to_string(),
                version: "0.1.0".to_string(),
                description: "Test".to_string(),
                authors: vec![],
            },
            library: cyberclaw_plugin_runtime::LibraryConfig {
                path: "test.so".to_string(),
                entry_point: "init".to_string(),
            },
            hooks: Default::default(),
            capabilities: Default::default(),
            resources: Default::default(),
            metadata: Default::default(),
        }
    }
}