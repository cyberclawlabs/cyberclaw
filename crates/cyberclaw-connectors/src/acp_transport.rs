//! # ACP Transport Layer
//!
//! 定义与外部 Agent 运行时通信的传输层抽象。
//! 提供 `AcpTransport` trait 和用于测试的 `MockTransport` 实现。

use crate::acp_runtime::{
    ExternalRuntime, PermissionProfile, SecretRef, SessionArtifact, SessionEvent, SessionState,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

/// 外部 Agent 会话的启动配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnConfig {
    /// 目标运行时类型
    pub runtime: ExternalRuntime,
    /// 工作目录
    pub cwd: Option<String>,
    /// 模型覆盖
    pub model: Option<String>,
    /// 权限配置
    pub permission_profile: PermissionProfile,
    /// 初始提示词
    pub initial_prompt: Option<String>,
    /// 凭证引用（从密钥存储解析，不序列化到事件/产物中）
    pub credentials: Option<SecretRef>,
}

/// 与外部 Agent 运行时通信的传输层 trait
#[async_trait]
pub trait AcpTransport: Send + Sync {
    /// 创建新会话，返回会话 ID
    async fn spawn_session(&self, config: SpawnConfig) -> anyhow::Result<String>;
    /// 发送提示词到活跃会话
    async fn send_prompt(&self, session_id: &str, prompt: &str) -> anyhow::Result<()>;
    /// 发送后续消息
    async fn send_followup(&self, session_id: &str, message: &str) -> anyhow::Result<()>;
    /// 中断当前操作
    async fn interrupt(&self, session_id: &str) -> anyhow::Result<()>;
    /// 获取会话状态
    async fn get_status(&self, session_id: &str) -> anyhow::Result<SessionState>;
    /// 收集会话产物
    async fn collect_artifacts(&self, session_id: &str) -> anyhow::Result<Vec<SessionArtifact>>;
    /// 停止会话
    async fn stop_session(&self, session_id: &str) -> anyhow::Result<()>;
    /// 订阅会话事件流
    async fn subscribe_events(
        &self,
        session_id: &str,
    ) -> anyhow::Result<mpsc::Receiver<SessionEvent>>;
}

/// 用于测试的 Mock 传输层实现
#[derive(Debug)]
pub struct MockTransport {
    /// 模拟的会话状态
    sessions: Arc<RwLock<HashMap<String, SessionState>>>,
    /// 下一个会话 ID 计数器
    next_id: Arc<RwLock<u64>>,
}

impl MockTransport {
    /// 创建新的 MockTransport
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            next_id: Arc::new(RwLock::new(1)),
        }
    }
}

impl Default for MockTransport {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AcpTransport for MockTransport {
    async fn spawn_session(&self, _config: SpawnConfig) -> anyhow::Result<String> {
        let mut counter = self.next_id.write().await;
        let id = format!("mock-session-{}", *counter);
        *counter += 1;
        self.sessions
            .write()
            .await
            .insert(id.clone(), SessionState::Active);
        Ok(id)
    }

    async fn send_prompt(&self, session_id: &str, _prompt: &str) -> anyhow::Result<()> {
        let sessions = self.sessions.read().await;
        if sessions.contains_key(session_id) {
            Ok(())
        } else {
            anyhow::bail!("Session not found: {}", session_id)
        }
    }

    async fn send_followup(&self, session_id: &str, _message: &str) -> anyhow::Result<()> {
        let sessions = self.sessions.read().await;
        if sessions.contains_key(session_id) {
            Ok(())
        } else {
            anyhow::bail!("Session not found: {}", session_id)
        }
    }

    async fn interrupt(&self, session_id: &str) -> anyhow::Result<()> {
        let mut sessions = self.sessions.write().await;
        if sessions.contains_key(session_id) {
            sessions.insert(session_id.to_string(), SessionState::Paused);
            Ok(())
        } else {
            anyhow::bail!("Session not found: {}", session_id)
        }
    }

    async fn get_status(&self, session_id: &str) -> anyhow::Result<SessionState> {
        let sessions = self.sessions.read().await;
        sessions
            .get(session_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Session not found: {}", session_id))
    }

    async fn collect_artifacts(&self, session_id: &str) -> anyhow::Result<Vec<SessionArtifact>> {
        let sessions = self.sessions.read().await;
        if sessions.contains_key(session_id) {
            Ok(vec![])
        } else {
            anyhow::bail!("Session not found: {}", session_id)
        }
    }

    async fn stop_session(&self, session_id: &str) -> anyhow::Result<()> {
        let mut sessions = self.sessions.write().await;
        if sessions.contains_key(session_id) {
            sessions.insert(session_id.to_string(), SessionState::Cancelled);
            Ok(())
        } else {
            anyhow::bail!("Session not found: {}", session_id)
        }
    }

    async fn subscribe_events(
        &self,
        session_id: &str,
    ) -> anyhow::Result<mpsc::Receiver<SessionEvent>> {
        let sessions = self.sessions.read().await;
        if sessions.contains_key(session_id) {
            let (_tx, rx) = mpsc::channel(16);
            Ok(rx)
        } else {
            anyhow::bail!("Session not found: {}", session_id)
        }
    }
}
