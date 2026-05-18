use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use cyberclaw_core::ids::ExecutionId;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::shared_state_store::SharedStateStore;

// ============================================================================
// 迭代状态数据模型
// ============================================================================

/// 单次迭代的完整状态快照（扩展版，支持 No-progress 检测）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutopilotIterationState {
    /// 迭代序号（从 1 开始）
    pub iteration_number: u32,
    /// 状态哈希值（用于无进展检测）
    pub state_hash: String,
    /// 迭代开始时间
    pub started_at: DateTime<Utc>,
    /// 迭代结束时间（可选）
    pub completed_at: Option<DateTime<Utc>>,
    /// 迭代是否成功完成
    pub success: bool,
    /// 迭代产生的 artifact 数量
    pub artifact_count: u32,
    /// 迭代执行的操作类型（如 "plan", "execute", "review"）
    pub operation_type: Option<String>,
    /// 错误信息（如果失败）
    pub error: Option<String>,
    /// 当前迭代的连续失败次数（用于防止无限重试）
    pub failure_count: u32,
}

impl AutopilotIterationState {
    /// 创建新的迭代状态
    pub fn new(iteration_number: u32, state_hash: String) -> Self {
        Self {
            iteration_number,
            state_hash,
            started_at: Utc::now(),
            completed_at: None,
            success: false,
            artifact_count: 0,
            operation_type: None,
            error: None,
            failure_count: 0,
        }
    }

    /// 标记迭代完成
    pub fn mark_completed(&mut self, success: bool, error: Option<String>) {
        self.completed_at = Some(Utc::now());
        self.success = success;
        self.error = error;
    }
}

/// 迭代历史摘要信息（扩展版）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutopilotIterationSummary {
    pub iteration_number: u32,
    pub state_hash: String,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub success: bool,
    pub duration_ms: Option<u64>,
}

impl From<AutopilotIterationState> for AutopilotIterationSummary {
    fn from(state: AutopilotIterationState) -> Self {
        let duration_ms = state
            .completed_at
            .map(|completed| (completed - state.started_at).num_milliseconds().max(0) as u64);

        Self {
            iteration_number: state.iteration_number,
            state_hash: state.state_hash,
            started_at: state.started_at,
            completed_at: state.completed_at,
            success: state.success,
            duration_ms,
        }
    }
}

// ============================================================================
// IterationTracker Trait
// ============================================================================

/// 迭代追踪器 Trait（扩展版，支持历史和 No-progress 检测）
#[async_trait::async_trait]
pub trait AutopilotIterationTracker: Send + Sync {
    /// 获取当前迭代次数
    async fn current_iteration(&self, run_id: &ExecutionId) -> Result<u32>;

    /// 递增迭代计数，返回新的迭代次数
    async fn increment(&self, run_id: &ExecutionId) -> Result<u32>;

    /// 获取完整迭代历史
    async fn get_history(&self, run_id: &ExecutionId) -> Result<Vec<AutopilotIterationSummary>>;

    /// 检测是否卡住（无进展）
    async fn detect_stuck(&self, run_id: &ExecutionId) -> Result<bool>;

    /// 记录迭代状态
    async fn record_iteration(
        &self,
        run_id: &ExecutionId,
        state: AutopilotIterationState,
    ) -> Result<()>;

    /// 获取最近 N 次迭代的哈希值
    async fn get_last_n_hashes(&self, run_id: &ExecutionId, n: usize) -> Result<Vec<String>>;

    /// 递增当前迭代的失败计数，返回新的失败次数
    async fn increment_failure(&self, run_id: &ExecutionId) -> Result<u32>;

    /// 获取当前迭代的失败次数
    async fn get_failure_count(&self, run_id: &ExecutionId) -> Result<u32>;

    /// 清理已完成 Run 的历史（可选）
    async fn cleanup_run(&self, run_id: &ExecutionId) -> Result<()>;
}

// ============================================================================
// InMemoryIterationTracker 实现
// ============================================================================

/// 内存实现的迭代追踪器
///
/// 特性：
/// - 自动持久化到 SharedStateStore
/// - 单个 Run 最多保留 1000 次迭代历史
/// - 支持 No-progress 检测（连续 N 次 hash 相同）
pub struct InMemoryIterationTracker {
    /// Run -> 迭代历史（使用 VecDeque 实现滑动窗口）
    iterations: Arc<RwLock<BTreeMap<ExecutionId, VecDeque<AutopilotIterationState>>>>,
    /// No-progress 检测阈值（连续 N 次 hash 相同）
    stuck_threshold: u32,
    /// 单个 Run 最大历史记录数
    max_history_per_run: usize,
    /// SharedStateStore 引用（用于持久化）
    state_store: Option<Arc<dyn SharedStateStore>>,
}

impl InMemoryIterationTracker {
    /// 创建新的迭代追踪器
    pub fn new() -> Self {
        Self {
            iterations: Arc::new(RwLock::new(BTreeMap::new())),
            stuck_threshold: 3,
            max_history_per_run: 1000,
            state_store: None,
        }
    }

    /// 创建带 StateStore 的追踪器
    pub fn with_state_store(state_store: Arc<dyn SharedStateStore>) -> Self {
        Self {
            iterations: Arc::new(RwLock::new(BTreeMap::new())),
            stuck_threshold: 3,
            max_history_per_run: 1000,
            state_store: Some(state_store),
        }
    }

    /// 自定义 No-progress 阈值
    pub fn with_stuck_threshold(mut self, threshold: u32) -> Self {
        self.stuck_threshold = threshold;
        self
    }

    /// 自定义最大历史记录数
    pub fn with_max_history(mut self, max: usize) -> Self {
        self.max_history_per_run = max;
        self
    }

    /// 持久化到 StateStore
    async fn persist_to_store(&self, run_id: &ExecutionId) -> Result<()> {
        if let Some(store) = &self.state_store {
            let key = format!("autopilot:iterations:{}", run_id.as_str());
            let iterations = self.iterations.read().await;

            if let Some(history) = iterations.get(run_id) {
                let serialized =
                    serde_json::to_vec(history).context("Failed to serialize iteration history")?;

                // 使用版本 0 强制覆盖（简化版本控制）
                store
                    .put(key, serialized, 0)
                    .await
                    .context("Failed to persist iteration history to StateStore")?;
            }
        }
        Ok(())
    }

    /// 从 StateStore 恢复历史
    pub async fn restore_from_store(&self, run_id: &ExecutionId) -> Result<()> {
        if let Some(store) = &self.state_store {
            let key = format!("autopilot:iterations:{}", run_id.as_str());

            if let Some(entry) = store.get(&key).await? {
                let history: VecDeque<AutopilotIterationState> =
                    serde_json::from_slice(&entry.value)
                        .context("Failed to deserialize iteration history")?;

                let mut iterations = self.iterations.write().await;
                iterations.insert(run_id.clone(), history);
            }
        }
        Ok(())
    }

    /// 应用历史记录数量限制（滑动窗口）
    fn enforce_history_limit(&self, history: &mut VecDeque<AutopilotIterationState>) {
        while history.len() > self.max_history_per_run {
            history.pop_front(); // 删除最旧的记录
        }
    }
}

impl Default for InMemoryIterationTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl AutopilotIterationTracker for InMemoryIterationTracker {
    async fn current_iteration(&self, run_id: &ExecutionId) -> Result<u32> {
        let iterations = self.iterations.read().await;

        Ok(iterations
            .get(run_id)
            .and_then(|history| history.back())
            .map(|state| state.iteration_number)
            .unwrap_or(0))
    }

    async fn increment(&self, run_id: &ExecutionId) -> Result<u32> {
        let mut iterations = self.iterations.write().await;

        let history = iterations
            .entry(run_id.clone())
            .or_insert_with(VecDeque::new);

        let new_iteration = history
            .back()
            .map(|state| state.iteration_number + 1)
            .unwrap_or(1);

        // 创建新的迭代状态并记录
        let state = AutopilotIterationState::new(new_iteration, String::new());

        // 添加到历史
        history.push_back(state);

        // 限制历史大小
        while history.len() > self.max_history_per_run {
            history.pop_front();
        }

        Ok(new_iteration)
    }

    async fn get_history(&self, run_id: &ExecutionId) -> Result<Vec<AutopilotIterationSummary>> {
        let iterations = self.iterations.read().await;

        Ok(iterations
            .get(run_id)
            .map(|history| {
                history
                    .iter()
                    .map(|state| AutopilotIterationSummary::from(state.clone()))
                    .collect()
            })
            .unwrap_or_default())
    }

    async fn detect_stuck(&self, run_id: &ExecutionId) -> Result<bool> {
        // 边界情况：threshold 必须 >= 2 才有意义
        if self.stuck_threshold <= 1 {
            return Ok(false); // 单次迭代无法判定为"卡住"
        }

        let hashes = self
            .get_last_n_hashes(run_id, self.stuck_threshold as usize)
            .await?;

        // 如果历史记录不足，无法判定
        if hashes.len() < self.stuck_threshold as usize {
            return Ok(false);
        }

        // 检查最近 N 次迭代的 state_hash 是否完全相同
        // 使用 windows(2) 比较相邻元素
        let all_same = hashes.windows(2).all(|w| w[0] == w[1]);

        if all_same {
            tracing::warn!(
                "No-progress detected for run_id {}: {} consecutive identical state hashes",
                run_id.as_str(),
                self.stuck_threshold
            );
        }

        Ok(all_same)
    }

    async fn record_iteration(
        &self,
        run_id: &ExecutionId,
        state: AutopilotIterationState,
    ) -> Result<()> {
        // 1. 更新内存状态
        let mut iterations = self.iterations.write().await;

        let history = iterations
            .entry(run_id.clone())
            .or_insert_with(VecDeque::new);
        history.push_back(state);

        // 2. 应用历史记录数量限制
        self.enforce_history_limit(history);

        // 3. 释放锁后持久化
        drop(iterations);

        // 4. 异步持久化到 StateStore
        self.persist_to_store(run_id).await?;

        Ok(())
    }

    async fn get_last_n_hashes(&self, run_id: &ExecutionId, n: usize) -> Result<Vec<String>> {
        let iterations = self.iterations.read().await;

        Ok(iterations
            .get(run_id)
            .map(|history| {
                history
                    .iter()
                    .rev() // 从最新往旧遍历
                    .take(n)
                    .map(|state| state.state_hash.clone())
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev() // 恢复时间顺序（旧 -> 新）
                    .collect()
            })
            .unwrap_or_default())
    }

    async fn increment_failure(&self, run_id: &ExecutionId) -> Result<u32> {
        let mut iterations = self.iterations.write().await;

        let history = iterations
            .get_mut(run_id)
            .ok_or_else(|| anyhow::anyhow!("Run not found: {}", run_id.as_str()))?;

        let current = history
            .back_mut()
            .ok_or_else(|| anyhow::anyhow!("No iterations found for run: {}", run_id.as_str()))?;

        current.failure_count += 1;
        Ok(current.failure_count)
    }

    async fn get_failure_count(&self, run_id: &ExecutionId) -> Result<u32> {
        let iterations = self.iterations.read().await;

        Ok(iterations
            .get(run_id)
            .and_then(|history| history.back())
            .map(|state| state.failure_count)
            .unwrap_or(0))
    }

    async fn cleanup_run(&self, run_id: &ExecutionId) -> Result<()> {
        // 1. 清理内存
        let mut iterations = self.iterations.write().await;
        iterations.remove(run_id);
        drop(iterations);

        // 2. 清理 StateStore
        if let Some(store) = &self.state_store {
            let key = format!("autopilot:iterations:{}", run_id.as_str());
            store.delete(&key).await?;
        }

        Ok(())
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared_state_store::InMemorySharedStateStore;

    fn test_execution_id(id: &str) -> ExecutionId {
        ExecutionId::from_string(id.to_string()).unwrap()
    }

    // ------------------------------------------------------------------------
    // 基础功能测试
    // ------------------------------------------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn test_current_iteration_empty() {
        let tracker = InMemoryIterationTracker::new();
        let run_id = test_execution_id("run-1");

        let current = tracker.current_iteration(&run_id).await.unwrap();
        assert_eq!(current, 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_increment_creates_new_run() {
        let tracker = InMemoryIterationTracker::new();
        let run_id = test_execution_id("run-1");

        let new_iter = tracker.increment(&run_id).await.unwrap();
        assert_eq!(new_iter, 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_increment_multiple_times() {
        let tracker = InMemoryIterationTracker::new();
        let run_id = test_execution_id("run-1");

        assert_eq!(tracker.increment(&run_id).await.unwrap(), 1);
        assert_eq!(tracker.increment(&run_id).await.unwrap(), 2);
        assert_eq!(tracker.increment(&run_id).await.unwrap(), 3);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_record_iteration_basic() {
        let tracker = InMemoryIterationTracker::new();
        let run_id = test_execution_id("run-1");

        let state = AutopilotIterationState::new(1, "hash-1".to_string());
        tracker.record_iteration(&run_id, state).await.unwrap();

        let current = tracker.current_iteration(&run_id).await.unwrap();
        assert_eq!(current, 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_get_history() {
        let tracker = InMemoryIterationTracker::new();
        let run_id = test_execution_id("run-1");

        tracker
            .record_iteration(
                &run_id,
                AutopilotIterationState::new(1, "hash-1".to_string()),
            )
            .await
            .unwrap();
        tracker
            .record_iteration(
                &run_id,
                AutopilotIterationState::new(2, "hash-2".to_string()),
            )
            .await
            .unwrap();
        tracker
            .record_iteration(
                &run_id,
                AutopilotIterationState::new(3, "hash-3".to_string()),
            )
            .await
            .unwrap();

        let history = tracker.get_history(&run_id).await.unwrap();
        assert_eq!(history.len(), 3);
        assert_eq!(history[0].iteration_number, 1);
        assert_eq!(history[1].iteration_number, 2);
        assert_eq!(history[2].iteration_number, 3);
    }

    // ------------------------------------------------------------------------
    // No-progress 检测测试
    // ------------------------------------------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn test_detect_stuck_insufficient_data() {
        let tracker = InMemoryIterationTracker::new();
        let run_id = test_execution_id("run-1");

        // 只记录 2 次迭代（阈值是 3）
        tracker
            .record_iteration(
                &run_id,
                AutopilotIterationState::new(1, "hash-1".to_string()),
            )
            .await
            .unwrap();
        tracker
            .record_iteration(
                &run_id,
                AutopilotIterationState::new(2, "hash-1".to_string()),
            )
            .await
            .unwrap();

        let stuck = tracker.detect_stuck(&run_id).await.unwrap();
        assert!(!stuck); // 数据不足，不判定为卡住
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_detect_stuck_all_same() {
        let tracker = InMemoryIterationTracker::new();
        let run_id = test_execution_id("run-1");

        // 连续 3 次相同 hash
        tracker
            .record_iteration(
                &run_id,
                AutopilotIterationState::new(1, "hash-1".to_string()),
            )
            .await
            .unwrap();
        tracker
            .record_iteration(
                &run_id,
                AutopilotIterationState::new(2, "hash-1".to_string()),
            )
            .await
            .unwrap();
        tracker
            .record_iteration(
                &run_id,
                AutopilotIterationState::new(3, "hash-1".to_string()),
            )
            .await
            .unwrap();

        let stuck = tracker.detect_stuck(&run_id).await.unwrap();
        assert!(stuck); // 应该判定为卡住
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_detect_stuck_different_hashes() {
        let tracker = InMemoryIterationTracker::new();
        let run_id = test_execution_id("run-1");

        // 每次 hash 不同
        tracker
            .record_iteration(
                &run_id,
                AutopilotIterationState::new(1, "hash-1".to_string()),
            )
            .await
            .unwrap();
        tracker
            .record_iteration(
                &run_id,
                AutopilotIterationState::new(2, "hash-2".to_string()),
            )
            .await
            .unwrap();
        tracker
            .record_iteration(
                &run_id,
                AutopilotIterationState::new(3, "hash-3".to_string()),
            )
            .await
            .unwrap();

        let stuck = tracker.detect_stuck(&run_id).await.unwrap();
        assert!(!stuck); // 正常进展
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_detect_stuck_recovery() {
        let tracker = InMemoryIterationTracker::new();
        let run_id = test_execution_id("run-1");

        // 先卡住（3 次相同）
        tracker
            .record_iteration(
                &run_id,
                AutopilotIterationState::new(1, "hash-1".to_string()),
            )
            .await
            .unwrap();
        tracker
            .record_iteration(
                &run_id,
                AutopilotIterationState::new(2, "hash-1".to_string()),
            )
            .await
            .unwrap();
        tracker
            .record_iteration(
                &run_id,
                AutopilotIterationState::new(3, "hash-1".to_string()),
            )
            .await
            .unwrap();

        assert!(tracker.detect_stuck(&run_id).await.unwrap());

        // 然后恢复（新的 hash）
        tracker
            .record_iteration(
                &run_id,
                AutopilotIterationState::new(4, "hash-2".to_string()),
            )
            .await
            .unwrap();

        let stuck = tracker.detect_stuck(&run_id).await.unwrap();
        assert!(!stuck); // 最近 3 次是 hash-1, hash-1, hash-2，不全相同
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_get_last_n_hashes() {
        let tracker = InMemoryIterationTracker::new();
        let run_id = test_execution_id("run-1");

        tracker
            .record_iteration(
                &run_id,
                AutopilotIterationState::new(1, "hash-1".to_string()),
            )
            .await
            .unwrap();
        tracker
            .record_iteration(
                &run_id,
                AutopilotIterationState::new(2, "hash-2".to_string()),
            )
            .await
            .unwrap();
        tracker
            .record_iteration(
                &run_id,
                AutopilotIterationState::new(3, "hash-3".to_string()),
            )
            .await
            .unwrap();
        tracker
            .record_iteration(
                &run_id,
                AutopilotIterationState::new(4, "hash-4".to_string()),
            )
            .await
            .unwrap();

        let hashes = tracker.get_last_n_hashes(&run_id, 3).await.unwrap();
        assert_eq!(hashes, vec!["hash-2", "hash-3", "hash-4"]);
    }

    // ------------------------------------------------------------------------
    // 内存限制测试
    // ------------------------------------------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn test_enforce_history_limit() {
        let tracker = InMemoryIterationTracker::new().with_max_history(5);
        let run_id = test_execution_id("run-1");

        // 插入 10 条记录
        for i in 1..=10 {
            tracker
                .record_iteration(
                    &run_id,
                    AutopilotIterationState::new(i, format!("hash-{}", i)),
                )
                .await
                .unwrap();
        }

        let history = tracker.get_history(&run_id).await.unwrap();
        assert_eq!(history.len(), 5); // 只保留最新 5 条
        assert_eq!(history[0].iteration_number, 6); // 最旧的是第 6 次迭代
        assert_eq!(history[4].iteration_number, 10); // 最新的是第 10 次
    }

    // ------------------------------------------------------------------------
    // StateStore 持久化测试
    // ------------------------------------------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn test_persist_to_state_store() {
        let store = Arc::new(InMemorySharedStateStore::new());
        let tracker = InMemoryIterationTracker::with_state_store(store.clone());
        let run_id = test_execution_id("run-1");

        tracker
            .record_iteration(
                &run_id,
                AutopilotIterationState::new(1, "hash-1".to_string()),
            )
            .await
            .unwrap();

        // 验证 StateStore 中有数据
        let key = format!("autopilot:iterations:{}", run_id.as_str());
        let entry = store.get(&key).await.unwrap();
        assert!(entry.is_some());

        let persisted: VecDeque<AutopilotIterationState> =
            serde_json::from_slice(&entry.unwrap().value).unwrap();
        assert_eq!(persisted.len(), 1);
        assert_eq!(persisted[0].iteration_number, 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_restore_from_state_store() {
        let store = Arc::new(InMemorySharedStateStore::new());
        let tracker1 = InMemoryIterationTracker::with_state_store(store.clone());
        let run_id = test_execution_id("run-1");

        // 第一个 tracker 写入数据
        tracker1
            .record_iteration(
                &run_id,
                AutopilotIterationState::new(1, "hash-1".to_string()),
            )
            .await
            .unwrap();
        tracker1
            .record_iteration(
                &run_id,
                AutopilotIterationState::new(2, "hash-2".to_string()),
            )
            .await
            .unwrap();

        // 第二个 tracker 从 StateStore 恢复
        let tracker2 = InMemoryIterationTracker::with_state_store(store.clone());
        tracker2.restore_from_store(&run_id).await.unwrap();

        let history = tracker2.get_history(&run_id).await.unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].iteration_number, 1);
        assert_eq!(history[1].iteration_number, 2);
    }

    // ------------------------------------------------------------------------
    // 清理测试
    // ------------------------------------------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn test_cleanup_run() {
        let store = Arc::new(InMemorySharedStateStore::new());
        let tracker = InMemoryIterationTracker::with_state_store(store.clone());
        let run_id = test_execution_id("run-1");

        tracker
            .record_iteration(
                &run_id,
                AutopilotIterationState::new(1, "hash-1".to_string()),
            )
            .await
            .unwrap();

        // 验证数据存在
        assert_eq!(tracker.get_history(&run_id).await.unwrap().len(), 1);

        // 清理
        tracker.cleanup_run(&run_id).await.unwrap();

        // 验证内存和 StateStore 都已清理
        assert_eq!(tracker.get_history(&run_id).await.unwrap().len(), 0);

        let key = format!("autopilot:iterations:{}", run_id.as_str());
        assert!(store.get(&key).await.unwrap().is_none());
    }

    // ------------------------------------------------------------------------
    // 并发测试
    // ------------------------------------------------------------------------

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_concurrent_record_iteration() {
        let tracker = Arc::new(InMemoryIterationTracker::new());
        let run_id = test_execution_id("run-concurrent");

        // 并发记录 100 次迭代
        let mut handles = vec![];
        for i in 1..=100 {
            let tracker_clone = tracker.clone();
            let run_id_clone = run_id.clone();
            let handle = tokio::spawn(async move {
                tracker_clone
                    .record_iteration(
                        &run_id_clone,
                        AutopilotIterationState::new(i, format!("hash-{}", i)),
                    )
                    .await
                    .unwrap();
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.await.unwrap();
        }

        let history = tracker.get_history(&run_id).await.unwrap();
        assert_eq!(history.len(), 100);
    }

    // ------------------------------------------------------------------------
    // Stuck Detection Edge Case Tests (MEDIUM-4)
    // ------------------------------------------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn test_detect_stuck_threshold_one() {
        // Edge case: threshold=1 should never detect stuck
        let tracker = InMemoryIterationTracker::new().with_stuck_threshold(1);
        let run_id = test_execution_id("run-edge");

        tracker
            .record_iteration(
                &run_id,
                AutopilotIterationState::new(1, "hash-1".to_string()),
            )
            .await
            .unwrap();

        let stuck = tracker.detect_stuck(&run_id).await.unwrap();
        assert!(
            !stuck,
            "threshold=1 should never detect stuck (vacuous truth fix)"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_detect_stuck_threshold_two_identical() {
        // Normal case: threshold=2 with identical hashes should detect stuck
        let tracker = InMemoryIterationTracker::new().with_stuck_threshold(2);
        let run_id = test_execution_id("run-normal");

        tracker
            .record_iteration(
                &run_id,
                AutopilotIterationState::new(1, "hash-same".to_string()),
            )
            .await
            .unwrap();
        tracker
            .record_iteration(
                &run_id,
                AutopilotIterationState::new(2, "hash-same".to_string()),
            )
            .await
            .unwrap();

        let stuck = tracker.detect_stuck(&run_id).await.unwrap();
        assert!(
            stuck,
            "threshold=2 with identical hashes should detect stuck"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_detect_stuck_threshold_two_different() {
        // Normal case: threshold=2 with different hashes should NOT detect stuck
        let tracker = InMemoryIterationTracker::new().with_stuck_threshold(2);
        let run_id = test_execution_id("run-progress");

        tracker
            .record_iteration(
                &run_id,
                AutopilotIterationState::new(1, "hash-1".to_string()),
            )
            .await
            .unwrap();
        tracker
            .record_iteration(
                &run_id,
                AutopilotIterationState::new(2, "hash-2".to_string()),
            )
            .await
            .unwrap();

        let stuck = tracker.detect_stuck(&run_id).await.unwrap();
        assert!(
            !stuck,
            "threshold=2 with different hashes should NOT detect stuck"
        );
    }

    // ------------------------------------------------------------------------
    // Failure Tracking Tests (MEDIUM-6)
    // ------------------------------------------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn test_failure_tracking() {
        let tracker = InMemoryIterationTracker::new();
        let run_id = test_execution_id("run-failures");

        // Start iteration 1
        tracker
            .record_iteration(
                &run_id,
                AutopilotIterationState::new(1, "hash-1".to_string()),
            )
            .await
            .unwrap();

        // Initial failure count should be 0
        assert_eq!(tracker.get_failure_count(&run_id).await.unwrap(), 0);

        // Increment failures
        assert_eq!(tracker.increment_failure(&run_id).await.unwrap(), 1);
        assert_eq!(tracker.increment_failure(&run_id).await.unwrap(), 2);
        assert_eq!(tracker.increment_failure(&run_id).await.unwrap(), 3);

        // Verify current failure count
        assert_eq!(tracker.get_failure_count(&run_id).await.unwrap(), 3);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_failure_tracking_error_no_run() {
        let tracker = InMemoryIterationTracker::new();
        let run_id = test_execution_id("run-nonexistent");

        // Should error when run doesn't exist
        let result = tracker.increment_failure(&run_id).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Run not found"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_failure_tracking_resets_on_new_iteration() {
        let tracker = InMemoryIterationTracker::new();
        let run_id = test_execution_id("run-reset");

        // Iteration 1 with failures
        tracker
            .record_iteration(
                &run_id,
                AutopilotIterationState::new(1, "hash-1".to_string()),
            )
            .await
            .unwrap();
        tracker.increment_failure(&run_id).await.unwrap();
        tracker.increment_failure(&run_id).await.unwrap();
        assert_eq!(tracker.get_failure_count(&run_id).await.unwrap(), 2);

        // Iteration 2 should start with failure_count=0
        tracker
            .record_iteration(
                &run_id,
                AutopilotIterationState::new(2, "hash-2".to_string()),
            )
            .await
            .unwrap();
        assert_eq!(tracker.get_failure_count(&run_id).await.unwrap(), 0);
    }
}
