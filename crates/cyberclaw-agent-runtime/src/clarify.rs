//! ClarifyCoordinator — 桥接 agent loop 与 ClarifyQueue
//!
//! agent loop 调用 `ask()` 时，同步 enqueue 请求到 queue 并注册 oneshot waiter；
//! 当 HTTP 层调用 `notify_resolved()` 时唤醒 waiter。
//!
//! # 并发安全
//!
//! - `waiters` 使用 `tokio::sync::Mutex`
//! - Mutex 锁在 insert/remove 后立即释放，**不跨 .await 持锁**
//! - oneshot channel 承担 waiter 通知职责

use cyberclaw_core::clarify::{ClarifyAnswer, ClarifyError, ClarifyQueue, ClarifyRequest};
use cyberclaw_core::ids::ClarifyId;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{oneshot, Mutex};

// ---------------------------------------------------------------------------
// ClarifyCoordinator
// ---------------------------------------------------------------------------

/// 协调 agent loop 等待澄清答案的组件。
///
/// - `ask()`: enqueue → 注册 oneshot waiter → 等待超时或被唤醒
/// - `notify_resolved()`: 由 HTTP 层在用户提交答案后调用，唤醒等待中的 ask()
pub struct ClarifyCoordinator {
    queue: Arc<dyn ClarifyQueue>,
    waiters: Mutex<HashMap<ClarifyId, oneshot::Sender<ClarifyAnswer>>>,
}

impl ClarifyCoordinator {
    /// 创建一个新的 ClarifyCoordinator
    pub fn new(queue: Arc<dyn ClarifyQueue>) -> Self {
        Self {
            queue,
            waiters: Mutex::new(HashMap::new()),
        }
    }

    /// 向 queue 提交澄清请求，并同步等待用户回答或超时。
    ///
    /// # 流程
    ///
    /// 1. 调用 `queue.enqueue(req)` — 失败即返回
    /// 2. 创建 oneshot channel，将 Sender 存入 waiters
    /// 3. `tokio::time::timeout(timeout, receiver.await)`
    ///    - 收到 answer → `Ok(answer)`
    ///    - 超时 → best-effort `queue.mark_timeout()` + 清理 waiters → `Err(AlreadyTimedOut)`
    ///    - channel closed → `Err(InvalidForm { reason: "coordinator dropped waiter" })`
    pub async fn ask(
        &self,
        req: ClarifyRequest,
        timeout: Duration,
    ) -> Result<ClarifyAnswer, ClarifyError> {
        let id = req.id.clone();

        // 步骤 1: enqueue
        self.queue.enqueue(req).await?;

        // 步骤 2: 注册 waiter（锁住 → insert → 立即释放）
        let (tx, rx) = oneshot::channel::<ClarifyAnswer>();
        {
            let mut waiters = self.waiters.lock().await;
            waiters.insert(id.clone(), tx);
        } // lock 在这里释放，不跨 .await

        // 步骤 3: 等待答案或超时
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(answer)) => Ok(answer),
            Ok(Err(_)) => {
                // oneshot Sender 已被 drop（不应在正常路径发生）
                {
                    let mut waiters = self.waiters.lock().await;
                    waiters.remove(&id);
                }
                Err(ClarifyError::InvalidForm {
                    reason: "coordinator dropped waiter".to_string(),
                })
            }
            Err(_timeout) => {
                // 超时：best-effort 更新 queue 状态
                let _ = self.queue.mark_timeout(&id).await;
                // 清理 waiters 防止内存泄漏
                {
                    let mut waiters = self.waiters.lock().await;
                    waiters.remove(&id);
                }
                Err(ClarifyError::AlreadyTimedOut)
            }
        }
    }

    /// 通知协调器用户已提交答案，唤醒对应的 `ask()` 等待者。
    ///
    /// - 若 waiter 存在：发送 answer 唤醒 ask()
    /// - 若 waiter 不存在（进程重启或超时已清理）：静默忽略，不 panic
    /// - 若 send 失败（receiver 已 drop）：静默忽略
    pub async fn notify_resolved(&self, id: &ClarifyId, answer: ClarifyAnswer) {
        let sender = {
            let mut waiters = self.waiters.lock().await;
            waiters.remove(id)
        }; // lock 在这里释放

        if let Some(tx) = sender {
            // send 失败（receiver 已 drop）时忽略
            let _ = tx.send(answer);
        }
        // 若 sender 不存在（超时已清理、进程重启等），静默忽略
    }

    /// 返回当前 waiters 数量（仅用于测试验证）
    #[cfg(test)]
    pub(crate) async fn waiters_len(&self) -> usize {
        self.waiters.lock().await.len()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use chrono::{DateTime, Utc};
    use cyberclaw_core::clarify::{ClarifyOption, ClarifyQuestion, ClarifyStatus};
    use cyberclaw_core::ids::AgentId;
    use std::collections::HashMap;
    use tokio::sync::Mutex as TokioMutex;

    // --- 内联 MockClarifyQueue（避免依赖 control-plane）---

    #[derive(Debug)]
    struct MockClarifyQueue {
        requests: TokioMutex<HashMap<ClarifyId, ClarifyRequest>>,
    }

    impl MockClarifyQueue {
        fn new() -> Self {
            Self {
                requests: TokioMutex::new(HashMap::new()),
            }
        }
    }

    #[async_trait]
    impl ClarifyQueue for MockClarifyQueue {
        async fn enqueue(&self, req: ClarifyRequest) -> Result<(), ClarifyError> {
            req.validate()?;
            let mut map = self.requests.lock().await;
            if map.contains_key(&req.id) {
                return Err(ClarifyError::InvalidForm {
                    reason: "duplicate clarify_id".to_string(),
                });
            }
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
                ClarifyStatus::Resolved => Ok(req.clone()),
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
                ClarifyStatus::TimedOut => Ok(req.clone()),
                ClarifyStatus::Pending => {
                    req.status = ClarifyStatus::TimedOut;
                    Ok(req.clone())
                }
            }
        }

        async fn get(&self, id: &ClarifyId) -> Option<ClarifyRequest> {
            self.requests.lock().await.get(id).cloned()
        }

        async fn list_pending(&self) -> Vec<ClarifyRequest> {
            self.requests
                .lock()
                .await
                .values()
                .filter(|r| r.status == ClarifyStatus::Pending)
                .cloned()
                .collect()
        }

        async fn list_resolved(&self, since: DateTime<Utc>) -> Vec<ClarifyRequest> {
            self.requests
                .lock()
                .await
                .values()
                .filter(|r| {
                    r.status == ClarifyStatus::Resolved
                        && r.resolved_at.map(|t| t >= since).unwrap_or(false)
                })
                .cloned()
                .collect()
        }
    }

    // --- 辅助函数 ---

    fn build_test_request(id_suffix: &str) -> ClarifyRequest {
        let now = Utc::now();
        ClarifyRequest {
            id: ClarifyId::new(),
            conversation_id: format!("test-conv-coord-{}", id_suffix),
            agent_id: AgentId::from_string("test-agent-coord".to_string()).unwrap(),
            questions: vec![ClarifyQuestion {
                question: "Which environment?".to_string(),
                options: vec![
                    ClarifyOption {
                        label: "staging".to_string(),
                        description: "Use the staging environment".to_string(),
                        preview: None,
                    },
                    ClarifyOption {
                        label: "production".to_string(),
                        description: "Use the production environment".to_string(),
                        preview: None,
                    },
                ],
                multi_select: false,
            }],
            source: None,
            created_at: now,
            expires_at: now + chrono::Duration::seconds(900),
            status: ClarifyStatus::Pending,
            answers: None,
            resolved_at: None,
        }
    }

    fn make_coordinator() -> Arc<ClarifyCoordinator> {
        let queue = Arc::new(MockClarifyQueue::new());
        Arc::new(ClarifyCoordinator::new(queue))
    }

    fn make_answer(question: &str, choice: &str) -> ClarifyAnswer {
        let mut a = ClarifyAnswer::new();
        a.insert(question, choice);
        a
    }

    // --- 测试 1: 正常唤醒路径 ---

    #[tokio::test]
    async fn test_ask_happy_wake_normal() {
        let coordinator = make_coordinator();
        let req = build_test_request("001");
        let id = req.id.clone();

        let coord_clone = Arc::clone(&coordinator);
        let handle =
            tokio::spawn(async move { coord_clone.ask(req, Duration::from_secs(5)).await });

        // 等待 ask 注册 waiter
        tokio::time::sleep(Duration::from_millis(20)).await;

        let answer = make_answer("Which environment?", "staging");
        coordinator.notify_resolved(&id, answer.clone()).await;

        let result = handle.await.unwrap();
        assert!(result.is_ok(), "ask() should succeed: {:?}", result);
        let received = result.unwrap();
        assert_eq!(
            received.answers.get("Which environment?").unwrap(),
            "staging"
        );
    }

    // --- 测试 2: 超时路径 ---

    #[tokio::test]
    async fn test_ask_times_out() {
        let queue = Arc::new(MockClarifyQueue::new());
        let coordinator = Arc::new(ClarifyCoordinator::new(
            Arc::clone(&queue) as Arc<dyn ClarifyQueue>
        ));

        let req = build_test_request("002");
        let id = req.id.clone();

        // 极短超时，100ms，不 notify_resolved
        let result = coordinator.ask(req, Duration::from_millis(100)).await;

        assert!(
            matches!(result, Err(ClarifyError::AlreadyTimedOut)),
            "expected AlreadyTimedOut, got: {:?}",
            result
        );

        // queue 中 status 应为 TimedOut
        let stored = queue.get(&id).await;
        assert!(stored.is_some(), "request should still be in queue");
        assert_eq!(
            stored.unwrap().status,
            ClarifyStatus::TimedOut,
            "queue status should be TimedOut"
        );
    }

    // --- 测试 3: 先 notify_resolved 一个不存在的 id，不 panic ---

    #[tokio::test]
    async fn test_notify_before_ask_ignored() {
        let coordinator = make_coordinator();
        let fake_id = ClarifyId::new();

        // 先 notify 一个不存在的 id — 不应 panic
        let answer = make_answer("Which environment?", "staging");
        coordinator.notify_resolved(&fake_id, answer).await;

        // 之后 ask 同一（新的）请求仍然能正常工作
        let req = build_test_request("003");
        let id = req.id.clone();

        let coord_clone = Arc::clone(&coordinator);
        let handle =
            tokio::spawn(async move { coord_clone.ask(req, Duration::from_secs(5)).await });

        tokio::time::sleep(Duration::from_millis(20)).await;

        let answer2 = make_answer("Which environment?", "production");
        coordinator.notify_resolved(&id, answer2).await;

        let result = handle.await.unwrap();
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap().answers.get("Which environment?").unwrap(),
            "production"
        );
    }

    // --- 测试 4: 双重 notify，第二次静默失败不 panic ---

    #[tokio::test]
    async fn test_double_notify_second_ignored() {
        let coordinator = make_coordinator();
        let req = build_test_request("004");
        let id = req.id.clone();

        let coord_clone = Arc::clone(&coordinator);
        let handle =
            tokio::spawn(async move { coord_clone.ask(req, Duration::from_secs(5)).await });

        tokio::time::sleep(Duration::from_millis(20)).await;

        let answer1 = make_answer("Which environment?", "staging");
        coordinator.notify_resolved(&id, answer1).await;

        // 等待 ask 完成后 waiter 已被移除
        tokio::time::sleep(Duration::from_millis(20)).await;

        // 第二次 notify — 不应 panic，静默忽略
        let answer2 = make_answer("Which environment?", "production");
        coordinator.notify_resolved(&id, answer2).await;

        let result = handle.await.unwrap();
        assert!(result.is_ok());
        // 收到的是第一次 notify 的答案
        assert_eq!(
            result.unwrap().answers.get("Which environment?").unwrap(),
            "staging"
        );
    }

    // --- 测试 5: 超时后 waiters 中不遗留该 id ---

    #[tokio::test]
    async fn test_waiter_cleanup_on_timeout() {
        let coordinator = make_coordinator();
        let req = build_test_request("005");

        // 超时后 waiters 应为空
        let result = coordinator.ask(req, Duration::from_millis(50)).await;

        assert!(matches!(result, Err(ClarifyError::AlreadyTimedOut)));

        // 超时后 coordinator.waiters 里不应遗留 sender
        assert_eq!(
            coordinator.waiters_len().await,
            0,
            "waiters should be empty after timeout"
        );

        // 验证"超时后 notify 不死锁"——notify 应立即返回，不挂起
        let fake_id = ClarifyId::new();
        let answer = make_answer("Which environment?", "staging");
        // 此调用必须立即完成，不阻塞
        tokio::time::timeout(
            Duration::from_millis(100),
            coordinator.notify_resolved(&fake_id, answer),
        )
        .await
        .expect("notify_resolved should not block");
    }
}
