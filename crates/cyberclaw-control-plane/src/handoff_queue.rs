//! HandoffQueue — Multi-Agent Handoff 请求队列
//!
//! Sprint 21 T3: `HandoffQueue` trait + `InMemoryHandoffQueue` 内存实现。
//! 语义上平行于 S15 `InMemoryClarifyQueue`（随机访问 HashMap），
//! 但生命周期状态机为 Initiated→Authorized→Accepted/Declined/Expired。
//!
//! # 关键设计决策
//! - 使用 `RwLock`（clarify_queue 用 Mutex）：`get` / `list_recent` 走读锁，
//!   `enqueue` / `update_status` / `expire_stale` 走写锁，减少读争用。
//! - `expire_stale` 只处理 `is_pending()` 条目，已终态不再修改。
//! - `update_status` 仅接受合法目标状态 (Authorized / Accepted / Declined / Expired)；
//!   其余目标或非法来源状态返回 `HandoffError::Internal("invalid state transition")`.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use cyberclaw_core::handoff::{HandoffError, HandoffRequest, HandoffStatus};
use cyberclaw_core::ids::HandoffId;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

// ---------------------------------------------------------------------------
// HandoffQueue trait
// ---------------------------------------------------------------------------

/// 异步 HandoffQueue 接口。所有方法均为 async，实现必须是 `Send + Sync`。
///
/// # 方法语义
///
/// | 方法 | 说明 |
/// |---|---|
/// | `enqueue` | 验证 + 唯一性检查后插入队列 |
/// | `get` | 按 `HandoffId` 随机访问（只读） |
/// | `update_status` | 按合法状态机转换改变状态 |
/// | `list_recent` | 按 `initiated_at` 降序，截取 `limit` 条 |
/// | `expire_stale` | TTL 扫描，返回过期条目数 |
#[async_trait]
pub trait HandoffQueue: Send + Sync {
    /// 入队 handoff 请求。先执行 `req.validate()`，重复 `handoff_id` 返回 `HandoffError::Internal`.
    async fn enqueue(&self, req: HandoffRequest) -> Result<(), HandoffError>;

    /// 按 id 查询。不存在返回 `None`。
    async fn get(&self, id: &HandoffId) -> Option<HandoffRequest>;

    /// 根据 `status` 参数更新目标条目的状态。
    /// - 目标状态 `Initiated` 无意义 → 返回 `HandoffError::Internal`
    /// - 条目不存在 → `HandoffError::NotFound`
    /// - 非法来源状态（如 `Expired → Authorized`）→ `HandoffError::Internal`
    async fn update_status(
        &self,
        id: &HandoffId,
        status: HandoffStatus,
    ) -> Result<(), HandoffError>;

    /// 返回所有条目按 `initiated_at` 降序排列，截取前 `limit` 条。
    async fn list_recent(&self, limit: usize) -> Vec<HandoffRequest>;

    /// 对所有 `is_pending()` 且 `now - initiated_at > ttl_seconds` 的条目调用 `mark_expired(now)`。
    /// 返回本次标记为 Expired 的条目数。
    async fn expire_stale(&self, now: DateTime<Utc>) -> usize;

    /// Persist the allocated `SessionId` on an accepted handoff for idempotent replay.
    ///
    /// Default no-op so existing implementations don't need to change.
    /// `InMemoryHandoffQueue` overrides this to store the session id in the map.
    async fn set_target_session(
        &self,
        id: &HandoffId,
        session_id: cyberclaw_core::ids::SessionId,
    ) -> Result<(), HandoffError> {
        let _ = (id, session_id);
        Ok(())
    }

    /// Sprint 21 T8: reverse lookup from `target_session_id` to the originating
    /// handoff request. Used by `read_prior_context` to prepend a
    /// `<handoff_briefing>` block when the current execution session was
    /// created via an accepted handoff.
    ///
    /// Default returns `None` so existing implementations don't need to change.
    /// `InMemoryHandoffQueue` overrides this with a linear scan of stored entries.
    async fn find_by_target_session(
        &self,
        session_id: &cyberclaw_core::ids::SessionId,
    ) -> Option<HandoffRequest> {
        let _ = session_id;
        None
    }
}

// ---------------------------------------------------------------------------
// InMemoryHandoffQueue
// ---------------------------------------------------------------------------

/// `HandoffQueue` 的内存实现。
///
/// 内部使用 `RwLock<HashMap<HandoffId, HandoffRequest>>`，读操作走共享锁。
/// 不持久化，重启后队列清空（SQLite 持久化为 v2 计划项）。
#[derive(Debug)]
pub struct InMemoryHandoffQueue {
    inner: Arc<RwLock<HashMap<HandoffId, HandoffRequest>>>,
}

impl InMemoryHandoffQueue {
    /// 创建空队列。
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl Default for InMemoryHandoffQueue {
    fn default() -> Self {
        Self::new()
    }
}

/// 判断状态转换 `from → to` 是否合法。
///
/// 合法路径（来自 design.md §2.1 状态机）：
/// ```text
///   Initiated  → Authorized | Declined | Expired
///   Authorized → Accepted   | Declined | Expired
/// ```
/// 所有终态 (Accepted / Declined / Expired) → 任何目标 = 非法。
fn is_valid_transition(from: HandoffStatus, to: HandoffStatus) -> bool {
    matches!(
        (from, to),
        (HandoffStatus::Initiated, HandoffStatus::Authorized)
            | (HandoffStatus::Initiated, HandoffStatus::Declined)
            | (HandoffStatus::Initiated, HandoffStatus::Expired)
            | (HandoffStatus::Authorized, HandoffStatus::Accepted)
            | (HandoffStatus::Authorized, HandoffStatus::Declined)
            | (HandoffStatus::Authorized, HandoffStatus::Expired)
    )
}

#[async_trait]
impl HandoffQueue for InMemoryHandoffQueue {
    async fn enqueue(&self, req: HandoffRequest) -> Result<(), HandoffError> {
        // 1. 结构验证（先验证再锁）
        req.validate()?;

        let mut map = self.inner.write().await;

        // 2. 唯一性检查
        if map.contains_key(&req.handoff_id) {
            return Err(HandoffError::Internal("duplicate handoff_id".to_string()));
        }

        // 3. 插入
        map.insert(req.handoff_id.clone(), req);
        Ok(())
    }

    async fn get(&self, id: &HandoffId) -> Option<HandoffRequest> {
        let map = self.inner.read().await;
        map.get(id).cloned()
    }

    async fn update_status(
        &self,
        id: &HandoffId,
        status: HandoffStatus,
    ) -> Result<(), HandoffError> {
        // Initiated 作为目标无意义（只有 enqueue 才能产生 Initiated 状态）
        if status == HandoffStatus::Initiated {
            return Err(HandoffError::Internal(
                "cannot set status to Initiated via update_status".to_string(),
            ));
        }

        let now = Utc::now();
        let mut map = self.inner.write().await;

        let req = map.get_mut(id).ok_or(HandoffError::NotFound)?;

        let from = req.status;
        if !is_valid_transition(from, status) {
            return Err(HandoffError::Internal(format!(
                "invalid state transition: {:?} → {:?}",
                from, status
            )));
        }

        match status {
            HandoffStatus::Authorized => req.mark_authorized(now),
            HandoffStatus::Accepted => req.mark_accepted(now),
            HandoffStatus::Declined => req.mark_declined(now),
            HandoffStatus::Expired => req.mark_expired(now),
            HandoffStatus::Initiated => unreachable!("guarded above"),
        }

        Ok(())
    }

    async fn list_recent(&self, limit: usize) -> Vec<HandoffRequest> {
        let map = self.inner.read().await;
        let mut items: Vec<HandoffRequest> = map.values().cloned().collect();
        // 按 initiated_at 降序（最新在前）
        items.sort_by_key(|r| std::cmp::Reverse(r.initiated_at));
        items.truncate(limit);
        items
    }

    async fn expire_stale(&self, now: DateTime<Utc>) -> usize {
        let mut map = self.inner.write().await;
        let mut count = 0usize;

        for req in map.values_mut() {
            if !req.is_pending() {
                continue;
            }
            let elapsed_secs = (now - req.initiated_at).num_seconds();
            let ttl = req.ttl_seconds as i64;
            if elapsed_secs > ttl {
                req.mark_expired(now);
                count += 1;
            }
        }

        count
    }

    async fn set_target_session(
        &self,
        id: &HandoffId,
        session_id: cyberclaw_core::ids::SessionId,
    ) -> Result<(), HandoffError> {
        let mut map = self.inner.write().await;
        let req = map.get_mut(id).ok_or(HandoffError::NotFound)?;
        req.target_session_id = Some(session_id);
        Ok(())
    }

    async fn find_by_target_session(
        &self,
        session_id: &cyberclaw_core::ids::SessionId,
    ) -> Option<HandoffRequest> {
        let map = self.inner.read().await;
        // Linear scan — typical handoff count per active session is O(1) since
        // each session is created from at most one handoff. If this ever grows
        // a secondary `session_id → handoff_id` index can be added.
        map.values()
            .find(|req| req.target_session_id.as_ref() == Some(session_id))
            .cloned()
    }
}

// ---------------------------------------------------------------------------
// HandoffSink adapter
// ---------------------------------------------------------------------------

/// Adapter: `InMemoryHandoffQueue` implements `HandoffSink` so that
/// `HandoffConnector` (in `cyberclaw-connectors`) can be constructed with the
/// real in-process queue without introducing a circular dependency.
///
/// `cyberclaw-control-plane` already depends on `cyberclaw-connectors`, so the
/// impl lives here. The connector crate defines the minimal `HandoffSink`
/// trait; we bridge it to `HandoffQueue::enqueue` with a one-liner.
///
/// See `.spec-workflow/specs/s21-multi-agent-handoff/design.md` §4.
#[async_trait]
impl cyberclaw_connectors::handoff::HandoffSink for InMemoryHandoffQueue {
    async fn enqueue(
        &self,
        req: cyberclaw_core::handoff::HandoffRequest,
    ) -> Result<(), cyberclaw_core::handoff::HandoffError> {
        <Self as HandoffQueue>::enqueue(self, req).await
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use cyberclaw_core::handoff::HANDOFF_TTL_DEFAULT_SECS;
    use cyberclaw_core::ids::{AgentId, HandoffId};

    /// 构造最小合法请求，`initiated_at` 由调用者指定。
    fn make_request(
        id: &str,
        from: &str,
        to: &str,
        initiated_at: DateTime<Utc>,
        ttl_seconds: u32,
    ) -> HandoffRequest {
        let ttl = if ttl_seconds == 0 {
            HANDOFF_TTL_DEFAULT_SECS
        } else {
            ttl_seconds
        };
        HandoffRequest {
            handoff_id: HandoffId::from_string(id.to_string()).unwrap(),
            from_agent_id: AgentId::from_string(from.to_string()).unwrap(),
            to_agent_id: AgentId::from_string(to.to_string()).unwrap(),
            conversation_id: "conv_test".to_string(),
            reason: "unit test handoff".to_string(),
            briefing_text: "some briefing".to_string(),
            context_artifacts: vec![],
            status: HandoffStatus::Initiated,
            initiated_at,
            decided_at: None,
            ttl_seconds: ttl,
            initiated_by_execution: None,
            target_session_id: None,
        }
    }

    fn default_req(id: &str) -> HandoffRequest {
        make_request(
            id,
            "agent_a",
            "agent_b",
            Utc::now(),
            HANDOFF_TTL_DEFAULT_SECS,
        )
    }

    // --- 测试 1: enqueue 后 get 返回同一条目 ---

    #[tokio::test]
    async fn enqueue_and_get_roundtrip() {
        let queue = InMemoryHandoffQueue::new();
        let req = default_req("ho_001");
        let id = req.handoff_id.clone();

        queue.enqueue(req.clone()).await.unwrap();

        let fetched = queue.get(&id).await.unwrap();
        assert_eq!(fetched.handoff_id.to_string(), req.handoff_id.to_string());
        assert_eq!(fetched.status, HandoffStatus::Initiated);
        assert_eq!(fetched.reason, req.reason);
    }

    // --- 测试 2: 相同 id 二次 enqueue 返回 Internal 错误 ---

    #[tokio::test]
    async fn enqueue_duplicate_returns_internal_error() {
        let queue = InMemoryHandoffQueue::new();
        let req = default_req("ho_dup");

        queue.enqueue(req.clone()).await.unwrap();
        let err = queue.enqueue(req).await.unwrap_err();

        assert!(
            matches!(&err, HandoffError::Internal(msg) if msg.contains("duplicate handoff_id")),
            "expected Internal(duplicate), got: {:?}",
            err
        );
    }

    // --- 测试 3: 自我 handoff 触发 validate() → SelfHandoff 错误 ---

    #[tokio::test]
    async fn enqueue_invalid_request_returns_validation_error() {
        let queue = InMemoryHandoffQueue::new();
        // from == to → SelfHandoff
        let req = make_request(
            "ho_self",
            "agent_a",
            "agent_a",
            Utc::now(),
            HANDOFF_TTL_DEFAULT_SECS,
        );
        let err = queue.enqueue(req).await.unwrap_err();
        assert_eq!(err, HandoffError::SelfHandoff);
    }

    // --- 测试 4: update_status 不存在的 id → NotFound ---

    #[tokio::test]
    async fn update_status_not_found() {
        let queue = InMemoryHandoffQueue::new();
        let fake_id = HandoffId::from_string("ho_ghost".to_string()).unwrap();
        let err = queue
            .update_status(&fake_id, HandoffStatus::Authorized)
            .await
            .unwrap_err();
        assert_eq!(err, HandoffError::NotFound);
    }

    // --- 测试 5: Initiated → Authorized → Accepted 正常路径 ---

    #[tokio::test]
    async fn update_status_authorized_then_accepted_happy_path() {
        let queue = InMemoryHandoffQueue::new();
        let req = default_req("ho_happy");
        let id = req.handoff_id.clone();

        queue.enqueue(req).await.unwrap();

        // Initiated → Authorized
        queue
            .update_status(&id, HandoffStatus::Authorized)
            .await
            .unwrap();
        let stored = queue.get(&id).await.unwrap();
        assert_eq!(stored.status, HandoffStatus::Authorized);
        assert!(stored.decided_at.is_some());

        // Authorized → Accepted
        queue
            .update_status(&id, HandoffStatus::Accepted)
            .await
            .unwrap();
        let stored = queue.get(&id).await.unwrap();
        assert_eq!(stored.status, HandoffStatus::Accepted);
        assert!(!stored.is_pending());
    }

    // --- 测试 6: list_recent 按 initiated_at 降序，limit 截断 ---

    #[tokio::test]
    async fn list_recent_respects_order_desc_and_limit() {
        let queue = InMemoryHandoffQueue::new();
        let base = Utc::now();

        // 插入 5 条，initiated_at 各差 1 秒
        for i in 0..5usize {
            let req = make_request(
                &format!("ho_{:02}", i),
                "agent_a",
                "agent_b",
                base + Duration::seconds(i as i64),
                HANDOFF_TTL_DEFAULT_SECS,
            );
            queue.enqueue(req).await.unwrap();
        }

        let recent = queue.list_recent(3).await;
        assert_eq!(recent.len(), 3);

        // 最新的应排在最前（ho_04 initiated_at 最大）
        assert!(recent[0].initiated_at >= recent[1].initiated_at);
        assert!(recent[1].initiated_at >= recent[2].initiated_at);
        // 最前的应该是 ho_04
        assert_eq!(recent[0].handoff_id.to_string(), "ho_04");
    }

    // --- 测试 7: limit 大于条目数，返回全部 ---

    #[tokio::test]
    async fn list_recent_limit_larger_than_store() {
        let queue = InMemoryHandoffQueue::new();
        for i in 0..3usize {
            queue
                .enqueue(default_req(&format!("ho_lt_{}", i)))
                .await
                .unwrap();
        }
        let all = queue.list_recent(100).await;
        assert_eq!(all.len(), 3);
    }

    // --- 测试 8: expire_stale 标记过期 pending 条目，count=1 ---

    #[tokio::test]
    async fn expire_stale_marks_pending_past_ttl() {
        let queue = InMemoryHandoffQueue::new();
        let past = Utc::now() - Duration::seconds(120);
        let req = make_request("ho_expired", "agent_a", "agent_b", past, 30);
        queue.enqueue(req).await.unwrap();

        // now = past + 120s, ttl = 30s → 超时
        let now = Utc::now();
        let count = queue.expire_stale(now).await;
        assert_eq!(count, 1);

        let stored = queue
            .get(&HandoffId::from_string("ho_expired".to_string()).unwrap())
            .await
            .unwrap();
        assert_eq!(stored.status, HandoffStatus::Expired);
        assert!(stored.decided_at.is_some());
    }

    // --- 测试 9: expire_stale 不处理未超时条目，count=0 ---

    #[tokio::test]
    async fn expire_stale_leaves_fresh_entries_alone() {
        let queue = InMemoryHandoffQueue::new();
        let req = make_request(
            "ho_fresh",
            "agent_a",
            "agent_b",
            Utc::now() - Duration::seconds(60),
            3600,
        );
        queue.enqueue(req).await.unwrap();

        // now = initiated_at + 60s, ttl = 3600s → 未超时
        let count = queue.expire_stale(Utc::now()).await;
        assert_eq!(count, 0);

        let stored = queue
            .get(&HandoffId::from_string("ho_fresh".to_string()).unwrap())
            .await
            .unwrap();
        assert_eq!(stored.status, HandoffStatus::Initiated);
    }

    // --- 测试 10: Sprint 21 T8 — find_by_target_session 反查命中 ---

    #[tokio::test]
    async fn find_by_target_session_returns_match_when_set() {
        use cyberclaw_core::ids::SessionId;

        let queue = InMemoryHandoffQueue::new();
        let req = default_req("ho_fbts_match");
        let id = req.handoff_id.clone();
        queue.enqueue(req).await.unwrap();

        // 设置 target_session_id 后能反查
        let session = SessionId::from_string("sess_abc".to_string()).unwrap();
        queue
            .set_target_session(&id, session.clone())
            .await
            .unwrap();

        let found = queue.find_by_target_session(&session).await;
        assert!(found.is_some());
        assert_eq!(found.unwrap().handoff_id.to_string(), "ho_fbts_match");

        // 未设置 session 的 handoff 不会被命中
        let other = SessionId::from_string("sess_other".to_string()).unwrap();
        assert!(queue.find_by_target_session(&other).await.is_none());
    }
}
