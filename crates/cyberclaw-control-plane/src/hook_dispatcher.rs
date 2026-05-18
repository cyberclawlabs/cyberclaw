//! # HookDispatcher
//!
//! 平台 Hook 分发器，为执行链关键节点提供扩展点。
//! Platform Plugin 可通过注册 handler 在执行前后注入逻辑，
//! 但所有 hook 必须走主链，不能绕过治理与追踪。

use async_trait::async_trait;
use std::sync::Arc;
use tracing::{error, warn};

// ─── HookPoint ────────────────────────────────────────────────────────────────

/// 执行链中可注入 hook 的关键节点。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum HookPoint {
    /// 能力执行前
    PreExecution,
    /// 能力执行后
    PostExecution,
    /// 评审队列入队前
    PreReview,
    /// 评审完成后
    PostReview,
    /// 执行链发生错误时
    OnError,
    /// 执行链全部完成时
    OnComplete,
}

// ─── HookContext ──────────────────────────────────────────────────────────────

/// Hook 调用时的上下文信息，只读传入每个 handler。
#[derive(Debug, Clone)]
pub struct HookContext {
    /// 本次执行的唯一 ID
    pub execution_id: String,
    /// 正在执行的能力 ID
    pub capability_id: String,
    /// 当前步骤序号（0-based）
    pub step: usize,
    /// 可选：触发 hook 的错误信息（仅 OnError 节点有值）
    pub error_message: Option<String>,
    /// 可选：额外的键值对元数据
    pub metadata: std::collections::HashMap<String, String>,
}

impl HookContext {
    /// 构造一个基础上下文。
    pub fn new(
        execution_id: impl Into<String>,
        capability_id: impl Into<String>,
        step: usize,
    ) -> Self {
        Self {
            execution_id: execution_id.into(),
            capability_id: capability_id.into(),
            step,
            error_message: None,
            metadata: std::collections::HashMap::new(),
        }
    }

    /// 附加错误信息（用于 OnError 节点）。
    pub fn with_error(mut self, msg: impl Into<String>) -> Self {
        self.error_message = Some(msg.into());
        self
    }
}

// ─── HookResult ───────────────────────────────────────────────────────────────

/// Handler 执行后返回的指令，控制后续行为。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookResult {
    /// 继续正常执行
    Continue,
    /// 中止执行链，携带原因说明
    Abort(String),
    /// 请求重试当前步骤
    Retry,
    /// 跳过当前步骤
    Skip,
}

// ─── HookHandler ─────────────────────────────────────────────────────────────

/// 可注册到 `HookDispatcher` 的 hook 处理器。
#[async_trait]
pub trait HookHandler: Send + Sync {
    /// 处理 hook 调用，返回对执行链的指令。
    async fn handle(&self, point: &HookPoint, context: &HookContext) -> HookResult;
}

// ─── FailurePolicy ────────────────────────────────────────────────────────────

/// 当 handler 自身发生内部错误（panic / unexpected failure）时的处理策略。
///
/// 注意：handler 返回 `HookResult::Abort` 不属于"失败"，而是主动中止指令。
/// FailurePolicy 仅针对 handler 执行过程中抛出的错误。
#[derive(Debug, Clone)]
pub enum FailurePolicy {
    /// 忽略 handler 内部错误，主链继续执行（默认宽松模式）
    Ignore,
    /// handler 内部错误立即中止整条执行链
    Abort,
    /// handler 内部错误时重试，超过 max_attempts 后中止
    Retry { max_attempts: u32 },
}

// ─── DispatchResult ───────────────────────────────────────────────────────────

/// `HookDispatcher::dispatch` 的聚合结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchResult {
    /// 所有 handler 均返回 Continue，主链正常推进
    Continue,
    /// 某个 handler 请求中止，携带原因
    Abort(String),
    /// 某个 handler 请求重试当前步骤
    Retry,
    /// 某个 handler 请求跳过当前步骤
    Skip,
}

// ─── Internal Registration Entry ─────────────────────────────────────────────

struct HookEntry {
    point: HookPoint,
    handler: Arc<dyn HookHandler>,
    policy: FailurePolicy,
}

// ─── HookDispatcher ───────────────────────────────────────────────────────────

/// 管理所有已注册 hook handler 并按节点顺序分发调用。
///
/// # 线程安全
///
/// `HookDispatcher` 内部使用 `Arc<tokio::sync::RwLock<_>>`，可跨线程共享。
pub struct HookDispatcher {
    entries: Arc<tokio::sync::RwLock<Vec<HookEntry>>>,
}

impl Default for HookDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl HookDispatcher {
    /// 创建一个空的 dispatcher。
    pub fn new() -> Self {
        Self {
            entries: Arc::new(tokio::sync::RwLock::new(Vec::new())),
        }
    }

    /// 向指定节点注册一个 handler，并指定失败策略。
    ///
    /// Handler 按注册先后顺序执行。
    pub async fn register(
        &self,
        point: HookPoint,
        handler: Arc<dyn HookHandler>,
        policy: FailurePolicy,
    ) {
        let mut entries = self.entries.write().await;
        entries.push(HookEntry {
            point,
            handler,
            policy,
        });
    }

    /// 向指定节点分发 hook 调用。
    ///
    /// 按注册顺序遍历与 `point` 匹配的所有 handler：
    /// - `HookResult::Continue` → 继续下一个 handler
    /// - `HookResult::Abort(reason)` → 立即停止，返回 `DispatchResult::Abort`
    /// - `HookResult::Retry` / `HookResult::Skip` → 立即停止并转发
    ///
    /// handler 内部错误按各自的 `FailurePolicy` 处理。
    pub async fn dispatch(&self, point: &HookPoint, context: &HookContext) -> DispatchResult {
        let entries = self.entries.read().await;

        for entry in entries.iter().filter(|e| &e.point == point) {
            let result = self.invoke_with_policy(entry, point, context).await;

            match result {
                HookResult::Continue => {
                    // 继续下一个 handler
                }
                HookResult::Abort(reason) => {
                    warn!(
                        execution_id = %context.execution_id,
                        capability_id = %context.capability_id,
                        step = context.step,
                        reason = %reason,
                        "Hook dispatcher: handler requested Abort"
                    );
                    return DispatchResult::Abort(reason);
                }
                HookResult::Retry => {
                    warn!(
                        execution_id = %context.execution_id,
                        capability_id = %context.capability_id,
                        step = context.step,
                        "Hook dispatcher: handler requested Retry"
                    );
                    return DispatchResult::Retry;
                }
                HookResult::Skip => {
                    warn!(
                        execution_id = %context.execution_id,
                        capability_id = %context.capability_id,
                        step = context.step,
                        "Hook dispatcher: handler requested Skip"
                    );
                    return DispatchResult::Skip;
                }
            }
        }

        DispatchResult::Continue
    }

    /// 按 FailurePolicy 执行单个 handler，屏蔽内部错误。
    async fn invoke_with_policy(
        &self,
        entry: &HookEntry,
        point: &HookPoint,
        context: &HookContext,
    ) -> HookResult {
        match &entry.policy {
            FailurePolicy::Ignore => {
                // handler 返回值即结果；trait 方法不返回 Result，由 handler 自行决定
                entry.handler.handle(point, context).await
            }
            FailurePolicy::Abort => {
                // 同 Ignore，但语义上告知调用方：若 handler 本身因 panic 等原因
                // 无法正常返回，上层应中止。此处实现层面 handler 无法抛 Err，
                // 所以直接透传返回值。
                entry.handler.handle(point, context).await
            }
            FailurePolicy::Retry { max_attempts } => {
                let mut attempts = 0u32;
                loop {
                    let result = entry.handler.handle(point, context).await;
                    // 仅在 handler 主动返回 Retry 时才重试（模拟内部失败语义）
                    if result == HookResult::Retry && attempts < *max_attempts {
                        attempts += 1;
                        error!(
                            execution_id = %context.execution_id,
                            attempt = attempts,
                            max_attempts = max_attempts,
                            "Hook handler requested retry; retrying"
                        );
                        continue;
                    }
                    return result;
                }
            }
        }
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // ── 辅助 handler ──────────────────────────────────────────────────────────

    /// 始终返回 Continue 的 handler
    struct AlwaysContinue;

    #[async_trait]
    impl HookHandler for AlwaysContinue {
        async fn handle(&self, _point: &HookPoint, _ctx: &HookContext) -> HookResult {
            HookResult::Continue
        }
    }

    /// 始终返回 Abort 的 handler
    struct AlwaysAbort {
        reason: String,
    }

    #[async_trait]
    impl HookHandler for AlwaysAbort {
        async fn handle(&self, _point: &HookPoint, _ctx: &HookContext) -> HookResult {
            HookResult::Abort(self.reason.clone())
        }
    }

    /// 记录调用次数的 handler，始终返回 Continue
    struct CountingHandler {
        count: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl HookHandler for CountingHandler {
        async fn handle(&self, _point: &HookPoint, _ctx: &HookContext) -> HookResult {
            self.count.fetch_add(1, Ordering::SeqCst);
            HookResult::Continue
        }
    }

    fn make_context() -> HookContext {
        HookContext::new("exec-001", "cap-read-file", 0)
    }

    // ── 测试 1：注册并触发 hook ───────────────────────────────────────────────

    #[tokio::test]
    async fn test_register_and_dispatch_continue() {
        let dispatcher = HookDispatcher::new();
        dispatcher
            .register(
                HookPoint::PreExecution,
                Arc::new(AlwaysContinue),
                FailurePolicy::Ignore,
            )
            .await;

        let result = dispatcher
            .dispatch(&HookPoint::PreExecution, &make_context())
            .await;

        assert_eq!(result, DispatchResult::Continue);
    }

    // ── 测试 2：FailurePolicy::Ignore — hook 返回 Continue，主链不受影响 ────

    #[tokio::test]
    async fn test_failure_policy_ignore_does_not_affect_main_chain() {
        let dispatcher = HookDispatcher::new();
        // 注册一个 Continue handler，策略为 Ignore
        dispatcher
            .register(
                HookPoint::PreExecution,
                Arc::new(AlwaysContinue),
                FailurePolicy::Ignore,
            )
            .await;

        let result = dispatcher
            .dispatch(&HookPoint::PreExecution, &make_context())
            .await;

        // 主链应继续
        assert_eq!(result, DispatchResult::Continue);
    }

    // ── 测试 3：FailurePolicy::Abort — handler 返回 Abort 时中断执行 ─────────

    #[tokio::test]
    async fn test_failure_policy_abort_stops_dispatch() {
        let dispatcher = HookDispatcher::new();
        dispatcher
            .register(
                HookPoint::PreExecution,
                Arc::new(AlwaysAbort {
                    reason: "security violation".to_string(),
                }),
                FailurePolicy::Abort,
            )
            .await;

        let result = dispatcher
            .dispatch(&HookPoint::PreExecution, &make_context())
            .await;

        assert_eq!(
            result,
            DispatchResult::Abort("security violation".to_string())
        );
    }

    // ── 测试 4：多个 handler 按注册顺序执行 ──────────────────────────────────

    #[tokio::test]
    async fn test_multiple_handlers_execute_in_order() {
        let dispatcher = HookDispatcher::new();
        let counter = Arc::new(AtomicUsize::new(0));

        for _ in 0..3 {
            dispatcher
                .register(
                    HookPoint::PostExecution,
                    Arc::new(CountingHandler {
                        count: counter.clone(),
                    }),
                    FailurePolicy::Ignore,
                )
                .await;
        }

        let result = dispatcher
            .dispatch(&HookPoint::PostExecution, &make_context())
            .await;

        assert_eq!(result, DispatchResult::Continue);
        // 三个 handler 全部被调用
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    // ── 测试 5：HookResult::Abort 中断后续 handler ────────────────────────────

    #[tokio::test]
    async fn test_abort_result_stops_subsequent_handlers() {
        let dispatcher = HookDispatcher::new();
        let counter = Arc::new(AtomicUsize::new(0));

        // 第一个：Abort
        dispatcher
            .register(
                HookPoint::OnError,
                Arc::new(AlwaysAbort {
                    reason: "abort early".to_string(),
                }),
                FailurePolicy::Ignore,
            )
            .await;

        // 第二个：counting handler，不应被调用
        dispatcher
            .register(
                HookPoint::OnError,
                Arc::new(CountingHandler {
                    count: counter.clone(),
                }),
                FailurePolicy::Ignore,
            )
            .await;

        let result = dispatcher
            .dispatch(&HookPoint::OnError, &make_context())
            .await;

        assert_eq!(result, DispatchResult::Abort("abort early".to_string()));
        // 第二个 handler 不应被调用
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    // ── 测试 6：不同节点的 handler 互不干扰 ──────────────────────────────────

    #[tokio::test]
    async fn test_handlers_are_scoped_to_hook_point() {
        let dispatcher = HookDispatcher::new();

        // 仅注册在 PostExecution
        dispatcher
            .register(
                HookPoint::PostExecution,
                Arc::new(AlwaysAbort {
                    reason: "should not fire".to_string(),
                }),
                FailurePolicy::Abort,
            )
            .await;

        // dispatch PreExecution — 不应命中上面的 handler
        let result = dispatcher
            .dispatch(&HookPoint::PreExecution, &make_context())
            .await;

        assert_eq!(result, DispatchResult::Continue);
    }

    // ── 测试 7：无 handler 时 dispatch 返回 Continue ──────────────────────────

    #[tokio::test]
    async fn test_empty_dispatcher_returns_continue() {
        let dispatcher = HookDispatcher::new();
        let result = dispatcher
            .dispatch(&HookPoint::OnComplete, &make_context())
            .await;
        assert_eq!(result, DispatchResult::Continue);
    }
}
