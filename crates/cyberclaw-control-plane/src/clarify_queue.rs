//! ClarifyQueue — Agent→User 结构化澄清请求队列
//!
//! 独立于 ReviewQueue 的语义：clarify 按 id 随机访问（HashMap），
//! review 按 FIFO 顺序处理（VecDeque）。两者不共享存储。

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use cyberclaw_core::clarify::{
    ClarifyAnswer, ClarifyError, ClarifyQueue, ClarifyRequest, ClarifyStatus,
};
use cyberclaw_core::ids::ClarifyId;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

// ClarifyQueue trait 定义在 cyberclaw_core::clarify，此处直接实现

// ---------------------------------------------------------------------------
// InMemoryClarifyQueue
// ---------------------------------------------------------------------------

/// ClarifyQueue 的内存实现
///
/// 使用 `Mutex<HashMap<ClarifyId, ClarifyRequest>>`，支持 O(1) id 查找。
/// 与 InMemoryReviewQueue 语义分离，不共享存储。
#[derive(Debug)]
pub struct InMemoryClarifyQueue {
    requests: Arc<Mutex<HashMap<ClarifyId, ClarifyRequest>>>,
}

impl InMemoryClarifyQueue {
    /// 创建一个新的空队列
    pub fn new() -> Self {
        Self {
            requests: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl Default for InMemoryClarifyQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ClarifyQueue for InMemoryClarifyQueue {
    async fn enqueue(&self, req: ClarifyRequest) -> Result<(), ClarifyError> {
        // 1. 结构验证
        req.validate()?;

        let mut map = self.requests.lock().await;

        // 2. id 唯一性检查
        if map.contains_key(&req.id) {
            return Err(ClarifyError::InvalidForm {
                reason: "duplicate clarify_id".to_string(),
            });
        }

        // 3. 插入
        map.insert(req.id.clone(), req);
        Ok(())
    }

    async fn resolve(
        &self,
        id: &ClarifyId,
        answer: ClarifyAnswer,
    ) -> Result<ClarifyRequest, ClarifyError> {
        let mut map = self.requests.lock().await;

        let req = map.get_mut(id).ok_or(ClarifyError::NotFound)?;

        match req.status {
            ClarifyStatus::TimedOut => Err(ClarifyError::AlreadyTimedOut),
            ClarifyStatus::Resolved => {
                // 幂等：返回已存的 request，不重新 apply answer
                Ok(req.clone())
            }
            ClarifyStatus::Pending => {
                req.status = ClarifyStatus::Resolved;
                req.answers = Some(answer.answers);
                req.resolved_at = Some(Utc::now());
                Ok(req.clone())
            }
        }
    }

    async fn mark_timeout(&self, id: &ClarifyId) -> Result<ClarifyRequest, ClarifyError> {
        let mut map = self.requests.lock().await;

        let req = map.get_mut(id).ok_or(ClarifyError::NotFound)?;

        match req.status {
            ClarifyStatus::Resolved => Err(ClarifyError::AlreadyResolved),
            ClarifyStatus::TimedOut => {
                // 幂等：直接返回
                Ok(req.clone())
            }
            ClarifyStatus::Pending => {
                req.status = ClarifyStatus::TimedOut;
                Ok(req.clone())
            }
        }
    }

    async fn get(&self, id: &ClarifyId) -> Option<ClarifyRequest> {
        let map = self.requests.lock().await;
        map.get(id).cloned()
    }

    async fn list_pending(&self) -> Vec<ClarifyRequest> {
        let map = self.requests.lock().await;
        map.values()
            .filter(|r| r.status == ClarifyStatus::Pending)
            .cloned()
            .collect()
    }

    async fn list_resolved(&self, since: DateTime<Utc>) -> Vec<ClarifyRequest> {
        let map = self.requests.lock().await;
        let mut resolved: Vec<ClarifyRequest> = map
            .values()
            .filter(|r| {
                r.status == ClarifyStatus::Resolved
                    && r.resolved_at.map(|t| t >= since).unwrap_or(false)
            })
            .cloned()
            .collect();
        // 按 resolved_at 升序
        resolved.sort_by_key(|r| r.resolved_at);
        resolved
    }

    async fn list_timed_out(
        &self,
        limit: usize,
    ) -> Result<Vec<ClarifyRequest>, cyberclaw_core::clarify::ClarifyError> {
        let map = self.requests.lock().await;
        let mut timed_out: Vec<ClarifyRequest> = map
            .values()
            .filter(|r| r.status == ClarifyStatus::TimedOut)
            .cloned()
            .collect();
        // 按 created_at 降序（最新优先）
        timed_out.sort_by_key(|r| std::cmp::Reverse(r.created_at));
        timed_out.truncate(limit);
        Ok(timed_out)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use cyberclaw_core::clarify::{ClarifyOption, ClarifyQuestion};
    use cyberclaw_core::ids::AgentId;

    // --- 辅助函数 ---

    fn make_option(label: &str, description: &str) -> ClarifyOption {
        ClarifyOption {
            label: label.to_string(),
            description: description.to_string(),
            preview: None,
        }
    }

    fn make_valid_request() -> ClarifyRequest {
        let now = Utc::now();
        ClarifyRequest {
            id: ClarifyId::new(),
            conversation_id: "test-conv-001".to_string(),
            agent_id: AgentId::from_string("test-agent".to_string()).unwrap(),
            questions: vec![ClarifyQuestion {
                question: "Which staging?".to_string(),
                options: vec![
                    make_option("staging-a", "Use A"),
                    make_option("staging-b", "Use B"),
                ],
                multi_select: false,
            }],
            source: None,
            created_at: now,
            expires_at: now + chrono::Duration::seconds(300),
            status: ClarifyStatus::Pending,
            answers: None,
            resolved_at: None,
        }
    }

    fn make_answer(question: &str, choice: &str) -> ClarifyAnswer {
        let mut a = ClarifyAnswer::new();
        a.insert(question, choice);
        a
    }

    // --- 测试 1: 入队接受合法请求 ---

    #[tokio::test]
    async fn test_enqueue_accepts_valid_request() {
        let queue = InMemoryClarifyQueue::new();
        let req = make_valid_request();
        queue.enqueue(req).await.unwrap();
        let pending = queue.list_pending().await;
        assert_eq!(pending.len(), 1);
    }

    // --- 测试 2: 零 options 被拒绝 ---

    #[tokio::test]
    async fn test_enqueue_rejects_invalid_form() {
        let queue = InMemoryClarifyQueue::new();
        let now = Utc::now();
        // 只有 1 个 option — 违反 >= 2 的约束
        let req = ClarifyRequest {
            id: ClarifyId::new(),
            conversation_id: "test-conv-001".to_string(),
            agent_id: AgentId::from_string("test-agent".to_string()).unwrap(),
            questions: vec![ClarifyQuestion {
                question: "Which staging?".to_string(),
                options: vec![make_option("staging-a", "Use A")], // 只有 1 个
                multi_select: false,
            }],
            source: None,
            created_at: now,
            expires_at: now + chrono::Duration::seconds(300),
            status: ClarifyStatus::Pending,
            answers: None,
            resolved_at: None,
        };
        let err = queue.enqueue(req).await.unwrap_err();
        assert!(matches!(err, ClarifyError::InvalidForm { .. }));
    }

    // --- 测试 3: 同一 id 二次入队失败 ---

    #[tokio::test]
    async fn test_enqueue_rejects_duplicate_id() {
        let queue = InMemoryClarifyQueue::new();
        let req = make_valid_request();
        let id = req.id.clone();

        queue.enqueue(req.clone()).await.unwrap();

        // 构造相同 id 的第二个请求
        let req2 = ClarifyRequest { id, ..req };
        let err = queue.enqueue(req2).await.unwrap_err();
        assert!(
            matches!(err, ClarifyError::InvalidForm { ref reason } if reason.contains("duplicate")),
            "expected duplicate error, got: {:?}",
            err
        );
    }

    // --- 测试 4: resolve happy path ---

    #[tokio::test]
    async fn test_resolve_happy_path() {
        let queue = InMemoryClarifyQueue::new();
        let req = make_valid_request();
        let id = req.id.clone();
        queue.enqueue(req).await.unwrap();

        let answer = make_answer("Which staging?", "staging-a");
        let resolved = queue.resolve(&id, answer.clone()).await.unwrap();

        assert_eq!(resolved.status, ClarifyStatus::Resolved);
        assert!(resolved.resolved_at.is_some());
        // 答案落回 request
        let stored_answers = resolved.answers.unwrap();
        assert_eq!(stored_answers.get("Which staging?").unwrap(), "staging-a");

        // get 也反映最新状态
        let fetched = queue.get(&id).await.unwrap();
        assert_eq!(fetched.status, ClarifyStatus::Resolved);
    }

    // --- 测试 5: resolve 不存在的 id ---

    #[tokio::test]
    async fn test_resolve_not_found() {
        let queue = InMemoryClarifyQueue::new();
        let fake_id = ClarifyId::new();
        let answer = make_answer("Which staging?", "staging-a");
        let err = queue.resolve(&fake_id, answer).await.unwrap_err();
        assert_eq!(err, ClarifyError::NotFound);
    }

    // --- 测试 6: resolve 已 Resolved 的 id 幂等 ---

    #[tokio::test]
    async fn test_resolve_idempotent() {
        let queue = InMemoryClarifyQueue::new();
        let req = make_valid_request();
        let id = req.id.clone();
        queue.enqueue(req).await.unwrap();

        let answer1 = make_answer("Which staging?", "staging-a");
        let first = queue.resolve(&id, answer1).await.unwrap();
        assert_eq!(first.status, ClarifyStatus::Resolved);

        // 第二次用不同 answer，应返回原 request 而不报错
        let answer2 = make_answer("Which staging?", "staging-b");
        let second = queue.resolve(&id, answer2).await.unwrap();
        assert_eq!(second.status, ClarifyStatus::Resolved);
        // 原答案保持不变
        assert_eq!(
            second.answers.unwrap().get("Which staging?").unwrap(),
            "staging-a"
        );
    }

    // --- 测试 7: 先 mark_timeout 再 resolve 应返回 AlreadyTimedOut ---

    #[tokio::test]
    async fn test_resolve_already_timedout_returns_error() {
        let queue = InMemoryClarifyQueue::new();
        let req = make_valid_request();
        let id = req.id.clone();
        queue.enqueue(req).await.unwrap();

        queue.mark_timeout(&id).await.unwrap();

        let answer = make_answer("Which staging?", "staging-a");
        let err = queue.resolve(&id, answer).await.unwrap_err();
        assert_eq!(err, ClarifyError::AlreadyTimedOut);
    }

    // --- 测试 8: mark_timeout happy path ---

    #[tokio::test]
    async fn test_mark_timeout_happy_path() {
        let queue = InMemoryClarifyQueue::new();
        let req = make_valid_request();
        let id = req.id.clone();
        queue.enqueue(req).await.unwrap();

        let result = queue.mark_timeout(&id).await.unwrap();
        assert_eq!(result.status, ClarifyStatus::TimedOut);

        let fetched = queue.get(&id).await.unwrap();
        assert_eq!(fetched.status, ClarifyStatus::TimedOut);
    }

    // --- 测试 9: mark_timeout 已 Resolved 的返回 AlreadyResolved ---

    #[tokio::test]
    async fn test_mark_timeout_already_resolved_returns_error() {
        let queue = InMemoryClarifyQueue::new();
        let req = make_valid_request();
        let id = req.id.clone();
        queue.enqueue(req).await.unwrap();

        let answer = make_answer("Which staging?", "staging-a");
        queue.resolve(&id, answer).await.unwrap();

        let err = queue.mark_timeout(&id).await.unwrap_err();
        assert_eq!(err, ClarifyError::AlreadyResolved);
    }

    // --- 测试 10: list_pending 只返回 Pending 状态 ---

    #[tokio::test]
    async fn test_list_pending_filters_status() {
        let queue = InMemoryClarifyQueue::new();

        let req1 = make_valid_request();
        let req2 = make_valid_request();
        let id2 = req2.id.clone();

        queue.enqueue(req1).await.unwrap();
        queue.enqueue(req2).await.unwrap();

        // 将 req2 resolve
        let answer = make_answer("Which staging?", "staging-b");
        queue.resolve(&id2, answer).await.unwrap();

        let pending = queue.list_pending().await;
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].status, ClarifyStatus::Pending);
    }

    // --- 测试 11: list_resolved since 过滤生效 ---

    #[tokio::test]
    async fn test_list_timed_out_returns_only_timed_out_entries() {
        let queue = InMemoryClarifyQueue::new();

        // Enqueue 3 requests in different states.
        let req_pending = make_valid_request();
        let req_resolved = make_valid_request();
        let req_timed_out = make_valid_request();

        let id_resolved = req_resolved.id.clone();
        let id_timed_out = req_timed_out.id.clone();

        queue.enqueue(req_pending).await.unwrap();
        queue.enqueue(req_resolved).await.unwrap();
        queue.enqueue(req_timed_out).await.unwrap();

        // Transition states.
        queue
            .resolve(&id_resolved, make_answer("Which staging?", "staging-a"))
            .await
            .unwrap();
        queue.mark_timeout(&id_timed_out).await.unwrap();

        let result = queue.list_timed_out(100).await.unwrap();
        assert_eq!(result.len(), 1, "only one TimedOut entry expected");
        assert_eq!(result[0].id, id_timed_out);
        assert_eq!(result[0].status, ClarifyStatus::TimedOut);
    }

    #[tokio::test]
    async fn test_list_timed_out_respects_limit() {
        let queue = InMemoryClarifyQueue::new();

        // Enqueue 5 requests and mark all as timed out.
        for _ in 0..5 {
            let req = make_valid_request();
            let id = req.id.clone();
            queue.enqueue(req).await.unwrap();
            queue.mark_timeout(&id).await.unwrap();
        }

        let result = queue.list_timed_out(2).await.unwrap();
        assert_eq!(result.len(), 2, "limit=2 must return exactly 2 entries");
    }

    #[tokio::test]
    async fn test_list_timed_out_empty_when_none_timed_out() {
        let queue = InMemoryClarifyQueue::new();

        // Enqueue only Pending requests — no timeouts.
        queue.enqueue(make_valid_request()).await.unwrap();
        queue.enqueue(make_valid_request()).await.unwrap();

        let result = queue.list_timed_out(100).await.unwrap();
        assert!(
            result.is_empty(),
            "expected empty Vec when no TimedOut entries exist"
        );
    }

    #[tokio::test]
    async fn test_list_resolved_since_filter() {
        let queue = InMemoryClarifyQueue::new();

        let req1 = make_valid_request();
        let req2 = make_valid_request();
        let id1 = req1.id.clone();
        let id2 = req2.id.clone();

        queue.enqueue(req1).await.unwrap();
        queue.enqueue(req2).await.unwrap();

        let before_resolve = Utc::now();

        let answer = make_answer("Which staging?", "staging-a");
        queue.resolve(&id1, answer.clone()).await.unwrap();
        queue.resolve(&id2, answer).await.unwrap();

        // since = before_resolve，两个都应在范围内
        let resolved_all = queue.list_resolved(before_resolve).await;
        assert_eq!(resolved_all.len(), 2);

        // since = 未来时间，应返回空
        let future = Utc::now() + chrono::Duration::hours(1);
        let resolved_empty = queue.list_resolved(future).await;
        assert_eq!(resolved_empty.len(), 0);
    }
}
