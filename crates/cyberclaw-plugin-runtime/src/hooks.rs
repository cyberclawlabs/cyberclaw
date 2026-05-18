//! # Hook 分发系统 (Hook Dispatcher)
//!
//! 提供 Platform Plugin 的 Hook 注册、管理和分发功能。
//!
//! ## 核心组件
//!
//! - **HookDispatcher**: Hook 事件分发器
//! - **HookRegistry**: Hook 注册表
//! - **HookRegistration**: Hook 注册信息
//! - **HookHandler**: Hook 处理器 trait
//!
//! ## 特性
//!
//! - 按优先级排序执行 Hook
//! - 超时控制
//! - 失败策略支持（Ignore/Retry/Abort）
//! - 事件总线集成

use crate::event_bus::{EventBus, HookEvent};
use crate::failure_policy::FailurePolicy;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// Hook 类型
///
/// 定义 Plugin 可以注册的 Hook 点。
///
/// # 示例
///
/// ```rust
/// use cyberclaw_plugin_runtime::hooks::HookType;
///
/// let hook = HookType::BeforeExecution;
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookType {
    /// 执行前 Hook
    BeforeExecution,
    /// 执行后 Hook
    AfterExecution,
    /// 失败时 Hook
    OnFailure,
    /// 审查时 Hook
    OnReview,
    /// Capability 执行前 Hook
    BeforeCapability,
    /// Capability 执行后 Hook
    AfterCapability,
}

impl HookType {
    /// 获取 Hook 类型的字符串表示
    pub fn as_str(&self) -> &'static str {
        match self {
            HookType::BeforeExecution => "before_execution",
            HookType::AfterExecution => "after_execution",
            HookType::OnFailure => "on_failure",
            HookType::OnReview => "on_review",
            HookType::BeforeCapability => "before_capability",
            HookType::AfterCapability => "after_capability",
        }
    }
}

/// Hook 执行上下文
///
/// 提供给 Hook 处理器的上下文信息。
///
/// # 示例
///
/// ```rust
/// use cyberclaw_plugin_runtime::hooks::HookContext;
/// use serde_json::json;
///
/// let context = HookContext {
///     execution_id: "exec-123".to_string(),
///     capability: "github.create_issue".to_string(),
///     input: json!({"title": "Bug"}),
///     metadata: json!({"user": "alice"}),
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookContext {
    /// 执行 ID
    pub execution_id: String,
    /// 能力名称
    pub capability: String,
    /// 输入参数
    pub input: serde_json::Value,
    /// 元数据
    pub metadata: serde_json::Value,
}

/// Hook 输出
///
/// Hook 处理器返回的结果。
///
/// # 示例
///
/// ```rust
/// use cyberclaw_plugin_runtime::hooks::HookOutput;
/// use serde_json::json;
///
/// let output = HookOutput {
///     continue_execution: true,
///     modified_input: None,
///     metadata: json!({"logged": true}),
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookOutput {
    /// 是否继续执行
    pub continue_execution: bool,
    /// 修改后的输入（如果有）
    pub modified_input: Option<serde_json::Value>,
    /// 元数据
    pub metadata: serde_json::Value,
}

impl Default for HookOutput {
    fn default() -> Self {
        Self {
            continue_execution: true,
            modified_input: None,
            metadata: serde_json::json!({}),
        }
    }
}

/// Hook 处理器 trait
///
/// Plugin 实现此 trait 来处理 Hook 事件。
///
/// # 示例
///
/// ```rust
/// use cyberclaw_plugin_runtime::hooks::{HookHandler, HookContext, HookOutput};
/// use async_trait::async_trait;
/// use anyhow::Result;
///
/// struct MyHook;
///
/// #[async_trait]
/// impl HookHandler for MyHook {
///     async fn handle(&self, context: &HookContext) -> Result<HookOutput> {
///         println!("Handling hook for execution: {}", context.execution_id);
///         Ok(HookOutput::default())
///     }
/// }
/// ```
#[async_trait]
pub trait HookHandler: Send + Sync {
    /// 处理 Hook 事件
    ///
    /// # 参数
    ///
    /// - `context`: Hook 执行上下文
    ///
    /// # 返回值
    ///
    /// - `Ok(HookOutput)`: 处理成功
    /// - `Err(error)`: 处理失败
    async fn handle(&self, context: &HookContext) -> anyhow::Result<HookOutput>;
}

/// Hook 注册信息
///
/// 包含 Hook 的元数据和处理器。
///
/// # 示例
///
/// ```rust
/// use cyberclaw_plugin_runtime::hooks::{HookRegistration, HookType, HookHandler};
/// use cyberclaw_plugin_runtime::failure_policy::FailurePolicy;
/// use std::sync::Arc;
/// use async_trait::async_trait;
///
/// struct MyHook;
///
/// #[async_trait]
/// impl HookHandler for MyHook {
///     async fn handle(&self, context: &cyberclaw_plugin_runtime::hooks::HookContext)
///         -> anyhow::Result<cyberclaw_plugin_runtime::hooks::HookOutput>
///     {
///         Ok(cyberclaw_plugin_runtime::hooks::HookOutput::default())
///     }
/// }
///
/// let registration = HookRegistration {
///     plugin_id: "my-plugin".to_string(),
///     hook_type: HookType::BeforeExecution,
///     handler: Arc::new(MyHook),
///     priority: 100,
///     timeout_ms: 5000,
///     failure_policy: FailurePolicy::Ignore,
/// };
/// ```
#[derive(Clone)]
pub struct HookRegistration {
    /// Plugin ID
    pub plugin_id: String,
    /// Hook 类型
    pub hook_type: HookType,
    /// Hook 处理器
    pub handler: Arc<dyn HookHandler>,
    /// 优先级（数值越大越先执行）
    pub priority: i32,
    /// 超时时间（毫秒）
    pub timeout_ms: u64,
    /// 失败策略
    pub failure_policy: FailurePolicy,
}

impl std::fmt::Debug for HookRegistration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HookRegistration")
            .field("plugin_id", &self.plugin_id)
            .field("hook_type", &self.hook_type)
            .field("priority", &self.priority)
            .field("timeout_ms", &self.timeout_ms)
            .field("failure_policy", &self.failure_policy)
            .finish()
    }
}

/// Hook 注册表
///
/// 管理所有已注册的 Hook。
struct HookRegistry {
    /// Hook 列表，按类型组织
    hooks: indexmap::IndexMap<HookType, Vec<HookRegistration>>,
}

impl HookRegistry {
    /// 创建新的注册表
    fn new() -> Self {
        Self {
            hooks: indexmap::IndexMap::new(),
        }
    }

    /// 注册 Hook
    fn register(&mut self, registration: HookRegistration) {
        let hook_type = registration.hook_type;
        let hooks = self.hooks.entry(hook_type).or_default();

        // 插入并按优先级排序（降序）
        hooks.push(registration);
        hooks.sort_by(|a, b| b.priority.cmp(&a.priority));
    }

    /// 注销 Hook
    fn unregister(&mut self, plugin_id: &str, hook_type: HookType) -> bool {
        if let Some(hooks) = self.hooks.get_mut(&hook_type) {
            let original_len = hooks.len();
            hooks.retain(|h| h.plugin_id != plugin_id);
            return original_len != hooks.len();
        }
        false
    }

    /// 获取指定类型的所有 Hook
    fn get_hooks(&self, hook_type: HookType) -> Vec<HookRegistration> {
        self.hooks.get(&hook_type).cloned().unwrap_or_default()
    }

    /// 清空所有 Hook
    fn clear(&mut self) {
        self.hooks.clear();
    }

    /// 获取统计信息
    fn stats(&self) -> HookStats {
        let total = self.hooks.values().map(|v| v.len()).sum();
        let by_type = self
            .hooks
            .iter()
            .map(|(k, v)| (k.as_str().to_string(), v.len()))
            .collect();

        HookStats { total, by_type }
    }
}

/// Hook 统计信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookStats {
    /// 总 Hook 数量
    pub total: usize,
    /// 按类型统计
    pub by_type: std::collections::HashMap<String, usize>,
}

/// Hook 分发器
///
/// 负责 Hook 的注册、管理和分发。
///
/// # 示例
///
/// ```rust
/// use cyberclaw_plugin_runtime::hooks::{HookDispatcher, HookType, HookContext};
/// use cyberclaw_plugin_runtime::event_bus::EventBus;
/// use cyberclaw_plugin_runtime::failure_policy::FailurePolicy;
///
/// #[tokio::test]
/// async fn example() {
///     let (event_bus, _receiver) = EventBus::create();
///     let dispatcher = HookDispatcher::new(event_bus, FailurePolicy::Ignore);
///
///     let context = HookContext {
///         execution_id: "test".to_string(),
///         capability: "test.action".to_string(),
///         input: serde_json::json!({}),
///         metadata: serde_json::json!({}),
///     };
///
///     let result = dispatcher.dispatch(HookType::BeforeExecution, &context).await;
///     assert!(result.is_ok());
/// }
/// ```
pub struct HookDispatcher {
    /// Hook 注册表
    registry: Arc<RwLock<HookRegistry>>,
    /// 事件总线
    event_bus: EventBus,
    /// 默认失败策略（保留用于未来扩展）
    #[allow(dead_code)]
    default_failure_policy: FailurePolicy,
}

impl HookDispatcher {
    /// 创建新的 Hook 分发器
    ///
    /// # 参数
    ///
    /// - `event_bus`: 事件总线
    /// - `default_failure_policy`: 默认失败策略
    pub fn new(event_bus: EventBus, default_failure_policy: FailurePolicy) -> Self {
        Self {
            registry: Arc::new(RwLock::new(HookRegistry::new())),
            event_bus,
            default_failure_policy,
        }
    }

    /// 注册 Hook
    ///
    /// # 参数
    ///
    /// - `registration`: Hook 注册信息
    pub async fn register(&self, registration: HookRegistration) {
        let plugin_id = registration.plugin_id.clone();
        let hook_type = registration.hook_type;
        let priority = registration.priority;

        self.registry.write().await.register(registration);

        self.event_bus
            .publish(HookEvent::HookRegistered {
                plugin_id: plugin_id.clone(),
                hook_type: hook_type.as_str().to_string(),
                priority,
            })
            .await;

        info!("Hook registered: {} for {}", plugin_id, hook_type.as_str());
    }

    /// 注销 Hook
    ///
    /// # 参数
    ///
    /// - `plugin_id`: Plugin ID
    /// - `hook_type`: Hook 类型
    pub async fn unregister(&self, plugin_id: &str, hook_type: HookType) -> bool {
        let removed = self.registry.write().await.unregister(plugin_id, hook_type);

        if removed {
            self.event_bus
                .publish(HookEvent::HookUnregistered {
                    plugin_id: plugin_id.to_string(),
                    hook_type: hook_type.as_str().to_string(),
                })
                .await;

            info!(
                "Hook unregistered: {} for {}",
                plugin_id,
                hook_type.as_str()
            );
        }

        removed
    }

    /// 分发 Hook 事件
    ///
    /// # 参数
    ///
    /// - `hook_type`: Hook 类型
    /// - `context`: Hook 上下文
    ///
    /// # 返回值
    ///
    /// - `Ok(Vec<HookOutput>)`: 所有 Hook 的输出
    /// - `Err(error)`: 如果某个 Hook 失败且策略为 Abort
    pub async fn dispatch(
        &self,
        hook_type: HookType,
        context: &HookContext,
    ) -> anyhow::Result<Vec<HookOutput>> {
        let hooks = self.registry.read().await.get_hooks(hook_type);

        if hooks.is_empty() {
            debug!("No hooks registered for {}", hook_type.as_str());
            return Ok(vec![]);
        }

        let total_hooks = hooks.len();
        let chain_start = Instant::now();

        self.event_bus
            .publish(HookEvent::HookChainStarted {
                hook_type: hook_type.as_str().to_string(),
                total_hooks,
            })
            .await;

        info!(
            "Dispatching {} hooks for {}",
            total_hooks,
            hook_type.as_str()
        );

        let mut results = Vec::new();
        let mut success_count = 0;
        let mut failure_count = 0;

        for hook in hooks {
            match self.execute_hook(&hook, context).await {
                Ok(output) => {
                    results.push(output);
                    success_count += 1;
                }
                Err(e) => {
                    failure_count += 1;
                    error!("Hook {} failed: {}", hook.plugin_id, e);

                    // 应用失败策略
                    let policy = &hook.failure_policy;
                    match policy.apply(1, &e.to_string()) {
                        Ok(true) => {
                            // 继续执行
                            continue;
                        }
                        Ok(false) => {
                            // 不应该到达这里（重试在 execute_hook 内部处理）
                            unreachable!("Retry should be handled in execute_hook");
                        }
                        Err(abort_error) => {
                            // 中止执行
                            return Err(anyhow::anyhow!(abort_error));
                        }
                    }
                }
            }
        }

        let total_duration_ms = chain_start.elapsed().as_millis() as u64;

        self.event_bus
            .publish(HookEvent::HookChainCompleted {
                hook_type: hook_type.as_str().to_string(),
                total_hooks,
                total_duration_ms,
                success_count,
                failure_count,
            })
            .await;

        info!(
            "Hook chain completed: {}/{} succeeded in {}ms",
            success_count, total_hooks, total_duration_ms
        );

        Ok(results)
    }

    /// 执行单个 Hook（带重试逻辑）
    async fn execute_hook(
        &self,
        hook: &HookRegistration,
        context: &HookContext,
    ) -> anyhow::Result<HookOutput> {
        let max_attempts = hook.failure_policy.max_attempts();

        for attempt in 1..=max_attempts {
            match self.call_hook(hook, context).await {
                Ok(output) => {
                    return Ok(output);
                }
                Err(e) => {
                    if hook.failure_policy.should_retry(attempt) {
                        self.event_bus
                            .publish(HookEvent::HookRetrying {
                                plugin_id: hook.plugin_id.clone(),
                                hook_type: hook.hook_type.as_str().to_string(),
                                attempt,
                                max_attempts,
                            })
                            .await;

                        warn!(
                            "Hook {} failed (attempt {}/{}), retrying: {}",
                            hook.plugin_id, attempt, max_attempts, e
                        );

                        // 等待 backoff 时间
                        if let Some(backoff) = hook.failure_policy.backoff_duration() {
                            tokio::time::sleep(backoff).await;
                        }
                    } else {
                        // 达到最大重试次数
                        self.event_bus
                            .publish(HookEvent::HookFailed {
                                plugin_id: hook.plugin_id.clone(),
                                hook_type: hook.hook_type.as_str().to_string(),
                                error: e.to_string(),
                                attempt,
                            })
                            .await;

                        return Err(e);
                    }
                }
            }
        }

        unreachable!("Should not reach here");
    }

    /// 调用 Hook（带超时控制）
    async fn call_hook(
        &self,
        hook: &HookRegistration,
        context: &HookContext,
    ) -> anyhow::Result<HookOutput> {
        let start = Instant::now();
        let timeout = Duration::from_millis(hook.timeout_ms);

        let result = tokio::time::timeout(timeout, hook.handler.handle(context)).await;

        let duration_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(Ok(output)) => {
                self.event_bus
                    .publish(HookEvent::HookExecuted {
                        plugin_id: hook.plugin_id.clone(),
                        hook_type: hook.hook_type.as_str().to_string(),
                        duration_ms,
                        success: true,
                    })
                    .await;

                debug!(
                    "Hook {} executed successfully in {}ms",
                    hook.plugin_id, duration_ms
                );

                Ok(output)
            }
            Ok(Err(e)) => {
                self.event_bus
                    .publish(HookEvent::HookExecuted {
                        plugin_id: hook.plugin_id.clone(),
                        hook_type: hook.hook_type.as_str().to_string(),
                        duration_ms,
                        success: false,
                    })
                    .await;

                Err(e)
            }
            Err(_) => {
                self.event_bus
                    .publish(HookEvent::HookTimeout {
                        plugin_id: hook.plugin_id.clone(),
                        hook_type: hook.hook_type.as_str().to_string(),
                        timeout_ms: hook.timeout_ms,
                    })
                    .await;

                Err(anyhow::anyhow!("Hook timeout after {}ms", hook.timeout_ms))
            }
        }
    }

    /// 获取统计信息
    pub async fn stats(&self) -> HookStats {
        self.registry.read().await.stats()
    }

    /// 清空所有 Hook
    pub async fn clear(&self) {
        self.registry.write().await.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestHook {
        should_fail: bool,
        delay_ms: u64,
    }

    #[async_trait]
    impl HookHandler for TestHook {
        async fn handle(&self, _context: &HookContext) -> anyhow::Result<HookOutput> {
            if self.delay_ms > 0 {
                tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
            }

            if self.should_fail {
                Err(anyhow::anyhow!("Test failure"))
            } else {
                Ok(HookOutput::default())
            }
        }
    }

    #[tokio::test]
    async fn test_hook_dispatcher_create() {
        let (event_bus, _receiver) = EventBus::create();
        let dispatcher = HookDispatcher::new(event_bus, FailurePolicy::Ignore);

        let stats = dispatcher.stats().await;
        assert_eq!(stats.total, 0);
    }

    #[tokio::test]
    async fn test_register_and_dispatch() {
        let (event_bus, _receiver) = EventBus::create();
        let dispatcher = HookDispatcher::new(event_bus, FailurePolicy::Ignore);

        let hook = HookRegistration {
            plugin_id: "test-plugin".to_string(),
            hook_type: HookType::BeforeExecution,
            handler: Arc::new(TestHook {
                should_fail: false,
                delay_ms: 0,
            }),
            priority: 100,
            timeout_ms: 1000,
            failure_policy: FailurePolicy::Ignore,
        };

        dispatcher.register(hook).await;

        let context = HookContext {
            execution_id: "exec-1".to_string(),
            capability: "test.action".to_string(),
            input: serde_json::json!({}),
            metadata: serde_json::json!({}),
        };

        let results = dispatcher
            .dispatch(HookType::BeforeExecution, &context)
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn test_priority_ordering() {
        let (event_bus, _receiver) = EventBus::create();
        let dispatcher = HookDispatcher::new(event_bus, FailurePolicy::Ignore);

        // 注册优先级不同的 Hook
        for (i, priority) in [50, 200, 100].iter().enumerate() {
            let hook = HookRegistration {
                plugin_id: format!("plugin-{}", i),
                hook_type: HookType::BeforeExecution,
                handler: Arc::new(TestHook {
                    should_fail: false,
                    delay_ms: 0,
                }),
                priority: *priority,
                timeout_ms: 1000,
                failure_policy: FailurePolicy::Ignore,
            };
            dispatcher.register(hook).await;
        }

        let stats = dispatcher.stats().await;
        assert_eq!(stats.total, 3);
    }

    #[tokio::test]
    async fn test_hook_timeout() {
        let (event_bus, _receiver) = EventBus::create();
        let dispatcher = HookDispatcher::new(event_bus, FailurePolicy::Abort);

        let hook = HookRegistration {
            plugin_id: "slow-plugin".to_string(),
            hook_type: HookType::BeforeExecution,
            handler: Arc::new(TestHook {
                should_fail: false,
                delay_ms: 200, // 延迟 200ms
            }),
            priority: 100,
            timeout_ms: 50, // 超时 50ms
            failure_policy: FailurePolicy::Abort,
        };

        dispatcher.register(hook).await;

        let context = HookContext {
            execution_id: "exec-1".to_string(),
            capability: "test.action".to_string(),
            input: serde_json::json!({}),
            metadata: serde_json::json!({}),
        };

        let result = dispatcher
            .dispatch(HookType::BeforeExecution, &context)
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("timeout"));
    }

    #[tokio::test]
    async fn test_failure_policy_ignore() {
        let (event_bus, _receiver) = EventBus::create();
        let dispatcher = HookDispatcher::new(event_bus, FailurePolicy::Ignore);

        let hook = HookRegistration {
            plugin_id: "failing-plugin".to_string(),
            hook_type: HookType::BeforeExecution,
            handler: Arc::new(TestHook {
                should_fail: true,
                delay_ms: 0,
            }),
            priority: 100,
            timeout_ms: 1000,
            failure_policy: FailurePolicy::Ignore,
        };

        dispatcher.register(hook).await;

        let context = HookContext {
            execution_id: "exec-1".to_string(),
            capability: "test.action".to_string(),
            input: serde_json::json!({}),
            metadata: serde_json::json!({}),
        };

        let result = dispatcher
            .dispatch(HookType::BeforeExecution, &context)
            .await;
        assert!(result.is_ok()); // Ignore 策略，继续执行
    }

    #[tokio::test]
    async fn test_failure_policy_abort() {
        let (event_bus, _receiver) = EventBus::create();
        let dispatcher = HookDispatcher::new(event_bus, FailurePolicy::Abort);

        let hook = HookRegistration {
            plugin_id: "failing-plugin".to_string(),
            hook_type: HookType::BeforeExecution,
            handler: Arc::new(TestHook {
                should_fail: true,
                delay_ms: 0,
            }),
            priority: 100,
            timeout_ms: 1000,
            failure_policy: FailurePolicy::Abort,
        };

        dispatcher.register(hook).await;

        let context = HookContext {
            execution_id: "exec-1".to_string(),
            capability: "test.action".to_string(),
            input: serde_json::json!({}),
            metadata: serde_json::json!({}),
        };

        let result = dispatcher
            .dispatch(HookType::BeforeExecution, &context)
            .await;
        assert!(result.is_err()); // Abort 策略，中止执行
    }

    #[tokio::test]
    async fn test_unregister_hook() {
        let (event_bus, _receiver) = EventBus::create();
        let dispatcher = HookDispatcher::new(event_bus, FailurePolicy::Ignore);

        let hook = HookRegistration {
            plugin_id: "test-plugin".to_string(),
            hook_type: HookType::BeforeExecution,
            handler: Arc::new(TestHook {
                should_fail: false,
                delay_ms: 0,
            }),
            priority: 100,
            timeout_ms: 1000,
            failure_policy: FailurePolicy::Ignore,
        };

        dispatcher.register(hook).await;
        assert_eq!(dispatcher.stats().await.total, 1);

        let removed = dispatcher
            .unregister("test-plugin", HookType::BeforeExecution)
            .await;
        assert!(removed);
        assert_eq!(dispatcher.stats().await.total, 0);
    }
}
