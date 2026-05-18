// tests/integration/plugin_runtime/hook_dispatch_tests.rs
// Hook 分发和失败策略测试

use anyhow::Result;
use cyberclaw_plugin_runtime::{
    HookDispatcher, HookRegistration, HookType, HookContext,
    FailurePolicy, HookHandler, HookResult,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::RwLock;

/// Mock Hook Handler 用于测试
struct MockHookHandler {
    call_count: Arc<AtomicUsize>,
    should_fail: bool,
    delay_ms: u64,
}

impl MockHookHandler {
    fn new() -> Self {
        Self {
            call_count: Arc::new(AtomicUsize::new(0)),
            should_fail: false,
            delay_ms: 0,
        }
    }

    fn with_failure() -> Self {
        Self {
            call_count: Arc::new(AtomicUsize::new(0)),
            should_fail: true,
            delay_ms: 0,
        }
    }

    fn with_delay(delay_ms: u64) -> Self {
        Self {
            call_count: Arc::new(AtomicUsize::new(0)),
            should_fail: false,
            delay_ms,
        }
    }

    fn call_count(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl HookHandler for MockHookHandler {
    async fn handle(&self, _context: &HookContext) -> Result<HookResult> {
        self.call_count.fetch_add(1, Ordering::SeqCst);

        if self.delay_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(self.delay_ms)).await;
        }

        if self.should_fail {
            Err(anyhow::anyhow!("Hook handler failed"))
        } else {
            Ok(HookResult::Continue)
        }
    }
}

/// 测试基本 Hook 分发
#[tokio::test]
async fn test_basic_hook_dispatch() -> Result<()> {
    let dispatcher = HookDispatcher::new();
    let handler = Arc::new(MockHookHandler::new());

    // 注册 Hook
    let registration = HookRegistration {
        plugin_id: "test_plugin".into(),
        hook_type: HookType::BeforeExecution,
        handler: handler.clone() as Arc<dyn HookHandler>,
        priority: 100,
        timeout_ms: 5000,
    };

    dispatcher.register(registration).await?;

    // 创建 Hook 上下文
    let context = HookContext::new("test_execution");

    // 分发 Hook
    let result = dispatcher.dispatch(HookType::BeforeExecution, &context).await?;

    // 验证 Hook 被调用
    assert_eq!(handler.call_count(), 1);
    assert_eq!(result, HookResult::Continue);

    Ok(())
}

/// 测试多个 Hook 按优先级执行
#[tokio::test]
async fn test_hook_priority_ordering() -> Result<()> {
    let dispatcher = HookDispatcher::new();
    let call_order = Arc::new(RwLock::new(Vec::new()));

    // 创建多个具有不同优先级的 Hook
    for priority in [300, 100, 200] {
        let order = call_order.clone();
        let handler = Arc::new(MockHookHandler::new());

        let registration = HookRegistration {
            plugin_id: format!("plugin_{}", priority),
            hook_type: HookType::BeforeExecution,
            handler: Arc::new(move |_: &HookContext| {
                let order = order.clone();
                let priority = priority;
                async move {
                    order.write().await.push(priority);
                    Ok(HookResult::Continue)
                }
            }) as Arc<dyn HookHandler>,
            priority,
            timeout_ms: 5000,
        };

        dispatcher.register(registration).await?;
    }

    let context = HookContext::new("test");
    dispatcher.dispatch(HookType::BeforeExecution, &context).await?;

    // 验证执行顺序（优先级从低到高）
    let order = call_order.read().await.clone();
    assert_eq!(order, vec![100, 200, 300]);

    Ok(())
}

/// 测试 FailurePolicy::Ignore
#[tokio::test]
async fn test_failure_policy_ignore() -> Result<()> {
    let dispatcher = HookDispatcher::with_failure_policy(FailurePolicy::Ignore);

    // 注册会失败的 Hook
    let failing_handler = Arc::new(MockHookHandler::with_failure());
    dispatcher.register(HookRegistration {
        plugin_id: "failing_plugin".into(),
        hook_type: HookType::BeforeExecution,
        handler: failing_handler.clone() as Arc<dyn HookHandler>,
        priority: 100,
        timeout_ms: 5000,
    }).await?;

    // 注册正常的 Hook
    let normal_handler = Arc::new(MockHookHandler::new());
    dispatcher.register(HookRegistration {
        plugin_id: "normal_plugin".into(),
        hook_type: HookType::BeforeExecution,
        handler: normal_handler.clone() as Arc<dyn HookHandler>,
        priority: 200,
        timeout_ms: 5000,
    }).await?;

    let context = HookContext::new("test");

    // 分发应该成功（忽略失败）
    let result = dispatcher.dispatch(HookType::BeforeExecution, &context).await;
    assert!(result.is_ok());

    // 验证两个 Hook 都被调用
    assert_eq!(failing_handler.call_count(), 1);
    assert_eq!(normal_handler.call_count(), 1);

    Ok(())
}

/// 测试 FailurePolicy::Retry
#[tokio::test]
async fn test_failure_policy_retry() -> Result<()> {
    let dispatcher = HookDispatcher::with_failure_policy(FailurePolicy::Retry { max_attempts: 3 });

    // 创建第二次成功的 Handler
    struct RetryHandler {
        attempt_count: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl HookHandler for RetryHandler {
        async fn handle(&self, _context: &HookContext) -> Result<HookResult> {
            let attempt = self.attempt_count.fetch_add(1, Ordering::SeqCst) + 1;
            if attempt < 2 {
                Err(anyhow::anyhow!("Attempt {} failed", attempt))
            } else {
                Ok(HookResult::Continue)
            }
        }
    }

    let handler = Arc::new(RetryHandler {
        attempt_count: Arc::new(AtomicUsize::new(0)),
    });

    dispatcher.register(HookRegistration {
        plugin_id: "retry_plugin".into(),
        hook_type: HookType::BeforeExecution,
        handler: handler.clone() as Arc<dyn HookHandler>,
        priority: 100,
        timeout_ms: 5000,
    }).await?;

    let context = HookContext::new("test");
    let result = dispatcher.dispatch(HookType::BeforeExecution, &context).await?;

    // 验证重试成功
    assert_eq!(result, HookResult::Continue);
    assert_eq!(handler.attempt_count.load(Ordering::SeqCst), 2); // 失败1次，成功1次

    Ok(())
}

/// 测试 FailurePolicy::Abort
#[tokio::test]
async fn test_failure_policy_abort() -> Result<()> {
    let dispatcher = HookDispatcher::with_failure_policy(FailurePolicy::Abort);

    // 注册会失败的 Hook
    let failing_handler = Arc::new(MockHookHandler::with_failure());
    dispatcher.register(HookRegistration {
        plugin_id: "failing_plugin".into(),
        hook_type: HookType::BeforeExecution,
        handler: failing_handler as Arc<dyn HookHandler>,
        priority: 100,
        timeout_ms: 5000,
    }).await?;

    // 注册正常的 Hook（不应该被执行）
    let normal_handler = Arc::new(MockHookHandler::new());
    dispatcher.register(HookRegistration {
        plugin_id: "normal_plugin".into(),
        hook_type: HookType::BeforeExecution,
        handler: normal_handler.clone() as Arc<dyn HookHandler>,
        priority: 200,
        timeout_ms: 5000,
    }).await?;

    let context = HookContext::new("test");

    // 分发应该失败
    let result = dispatcher.dispatch(HookType::BeforeExecution, &context).await;
    assert!(result.is_err());

    // 验证第二个 Hook 没有被执行（因为第一个失败后中止）
    assert_eq!(normal_handler.call_count(), 0);

    Ok(())
}

/// 测试 Hook 超时
#[tokio::test]
async fn test_hook_timeout() -> Result<()> {
    let dispatcher = HookDispatcher::new();

    // 注册会超时的 Hook
    let slow_handler = Arc::new(MockHookHandler::with_delay(2000)); // 2秒延迟

    dispatcher.register(HookRegistration {
        plugin_id: "slow_plugin".into(),
        hook_type: HookType::BeforeExecution,
        handler: slow_handler as Arc<dyn HookHandler>,
        priority: 100,
        timeout_ms: 500, // 500ms 超时
    }).await?;

    let context = HookContext::new("test");

    // 分发应该因超时失败
    let start = std::time::Instant::now();
    let result = dispatcher.dispatch(HookType::BeforeExecution, &context).await;
    let duration = start.elapsed();

    assert!(result.is_err());
    assert!(duration.as_millis() < 1000); // 应该在1秒内超时

    Ok(())
}

/// 测试不同类型的 Hook
#[tokio::test]
async fn test_different_hook_types() -> Result<()> {
    let dispatcher = HookDispatcher::new();
    let handlers = std::collections::HashMap::new();

    // 为每种 Hook 类型注册 Handler
    for hook_type in [
        HookType::BeforeExecution,
        HookType::AfterExecution,
        HookType::OnFailure,
        HookType::OnReview,
        HookType::BeforeCapability,
        HookType::AfterCapability,
    ] {
        let handler = Arc::new(MockHookHandler::new());
        handlers.insert(hook_type, handler.clone());

        dispatcher.register(HookRegistration {
            plugin_id: format!("plugin_{:?}", hook_type),
            hook_type,
            handler: handler as Arc<dyn HookHandler>,
            priority: 100,
            timeout_ms: 5000,
        }).await?;
    }

    // 分发每种类型的 Hook
    for (hook_type, handler) in handlers {
        let context = HookContext::new("test");
        dispatcher.dispatch(hook_type, &context).await?;

        // 验证对应的 Handler 被调用
        assert_eq!(handler.call_count(), 1);
    }

    Ok(())
}

/// 测试 Hook 结果组合
#[tokio::test]
async fn test_hook_result_combination() -> Result<()> {
    let dispatcher = HookDispatcher::new();

    // 注册返回 Modify 的 Hook
    struct ModifyHandler {
        modification: String,
    }

    #[async_trait::async_trait]
    impl HookHandler for ModifyHandler {
        async fn handle(&self, context: &HookContext) -> Result<HookResult> {
            Ok(HookResult::Modify {
                key: "test_key".to_string(),
                value: self.modification.clone(),
            })
        }
    }

    let handler1 = Arc::new(ModifyHandler {
        modification: "value1".to_string(),
    });

    let handler2 = Arc::new(ModifyHandler {
        modification: "value2".to_string(),
    });

    dispatcher.register(HookRegistration {
        plugin_id: "plugin1".into(),
        hook_type: HookType::BeforeExecution,
        handler: handler1 as Arc<dyn HookHandler>,
        priority: 100,
        timeout_ms: 5000,
    }).await?;

    dispatcher.register(HookRegistration {
        plugin_id: "plugin2".into(),
        hook_type: HookType::BeforeExecution,
        handler: handler2 as Arc<dyn HookHandler>,
        priority: 200,
        timeout_ms: 5000,
    }).await?;

    let context = HookContext::new("test");
    let result = dispatcher.dispatch(HookType::BeforeExecution, &context).await?;

    // 验证结果包含所有修改
    match result {
        HookResult::Combined(modifications) => {
            assert_eq!(modifications.len(), 2);
        }
        _ => panic!("Expected Combined result"),
    }

    Ok(())
}

/// 测试 Hook 注销
#[tokio::test]
async fn test_hook_unregistration() -> Result<()> {
    let dispatcher = HookDispatcher::new();
    let handler = Arc::new(MockHookHandler::new());

    // 注册 Hook
    let registration_id = dispatcher.register(HookRegistration {
        plugin_id: "test_plugin".into(),
        hook_type: HookType::BeforeExecution,
        handler: handler.clone() as Arc<dyn HookHandler>,
        priority: 100,
        timeout_ms: 5000,
    }).await?;

    // 第一次分发
    let context = HookContext::new("test");
    dispatcher.dispatch(HookType::BeforeExecution, &context).await?;
    assert_eq!(handler.call_count(), 1);

    // 注销 Hook
    dispatcher.unregister(registration_id).await?;

    // 第二次分发（Hook 已注销，不应被调用）
    dispatcher.dispatch(HookType::BeforeExecution, &context).await?;
    assert_eq!(handler.call_count(), 1); // 仍然是1

    Ok(())
}

/// 测试并发 Hook 执行
#[tokio::test]
async fn test_concurrent_hook_execution() -> Result<()> {
    let dispatcher = HookDispatcher::new();
    let concurrent_count = Arc::new(AtomicUsize::new(0));
    let max_concurrent = Arc::new(AtomicUsize::new(0));

    // 创建会并发执行的 Handler
    struct ConcurrentHandler {
        concurrent_count: Arc<AtomicUsize>,
        max_concurrent: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl HookHandler for ConcurrentHandler {
        async fn handle(&self, _context: &HookContext) -> Result<HookResult> {
            let current = self.concurrent_count.fetch_add(1, Ordering::SeqCst) + 1;

            // 更新最大并发数
            loop {
                let max = self.max_concurrent.load(Ordering::SeqCst);
                if current <= max || self.max_concurrent.compare_exchange(
                    max, current, Ordering::SeqCst, Ordering::SeqCst
                ).is_ok() {
                    break;
                }
            }

            // 模拟工作
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            self.concurrent_count.fetch_sub(1, Ordering::SeqCst);
            Ok(HookResult::Continue)
        }
    }

    // 注册多个可并发的 Hook
    for i in 0..5 {
        let handler = Arc::new(ConcurrentHandler {
            concurrent_count: concurrent_count.clone(),
            max_concurrent: max_concurrent.clone(),
        });

        dispatcher.register(HookRegistration {
            plugin_id: format!("concurrent_plugin_{}", i),
            hook_type: HookType::BeforeExecution,
            handler: handler as Arc<dyn HookHandler>,
            priority: 100, // 相同优先级，可并发
            timeout_ms: 5000,
        }).await?;
    }

    let context = HookContext::new("test");
    dispatcher.dispatch(HookType::BeforeExecution, &context).await?;

    // 验证有并发执行
    let max = max_concurrent.load(Ordering::SeqCst);
    assert!(max > 1, "Expected concurrent execution, but max concurrent was {}", max);

    Ok(())
}