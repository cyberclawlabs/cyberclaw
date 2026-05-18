//! Mock AgentRuntime for testing.

use crate::config::AgentConfig;
use crate::error::{AgentRuntimeError, AgentRuntimeResult};
use crate::types::{AgentRequest, AgentResponse};
use crate::AgentRuntime;
use async_trait::async_trait;
use cyberclaw_core::ids::AgentId;
use std::sync::Arc;
use tokio::sync::RwLock;

/// A controllable mock agent runtime used in tests.
///
/// By default every call to `execute` succeeds with an echo response.
/// Set `fail_with` to make executions return an error.
#[derive(Default)]
pub struct MockAgentRuntime {
    /// When `Some`, all `execute` calls return this error.
    pub fail_with: Arc<RwLock<Option<String>>>,
    /// Simulated delay in milliseconds for each `execute` call.
    pub delay_ms: u64,
    /// All requests received so far (for assertion in tests).
    pub recorded: Arc<RwLock<Vec<AgentRequest>>>,
}

impl MockAgentRuntime {
    /// Create a new mock that succeeds every call.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a mock that always fails with the given message.
    pub fn with_failure(message: impl Into<String>) -> Self {
        Self {
            fail_with: Arc::new(RwLock::new(Some(message.into()))),
            delay_ms: 0,
            recorded: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Create a mock with a simulated execution delay.
    pub fn with_delay(delay_ms: u64) -> Self {
        Self {
            delay_ms,
            ..Default::default()
        }
    }

    /// Return all requests recorded so far.
    ///
    /// Uses `blocking_read()` which parks the current thread until the read lock
    /// is available. This is safe to call from synchronous test helper methods
    /// because:
    ///   1. Tests call this only after all async tasks that could hold a write
    ///      lock have completed (i.e., after `.await` points).
    ///   2. `RwLock::blocking_read()` never panics — it blocks until the lock is
    ///      acquired, unlike `try_read()` which returns `Err` when the lock is
    ///      currently held.
    pub fn get_recorded(&self) -> Vec<AgentRequest> {
        self.recorded.blocking_read().clone()
    }
}

#[async_trait]
impl AgentRuntime for MockAgentRuntime {
    async fn execute(&self, request: AgentRequest) -> AgentRuntimeResult<AgentResponse> {
        self.recorded.write().await.push(request.clone());

        if self.delay_ms > 0 {
            tokio::time::sleep(tokio::time::Duration::from_millis(self.delay_ms)).await;
        }

        let fail_msg = self.fail_with.read().await.clone();
        if let Some(msg) = fail_msg {
            let agent_id = request.agent_id.clone();
            return Err(AgentRuntimeError::ExecutionFailed {
                agent_id,
                source: anyhow::anyhow!("MockAgentRuntime: {}", msg),
            });
        }

        let agent_id = request.agent_id.clone();
        Ok(AgentResponse::ok(
            agent_id,
            format!("mock: {}", request.input),
        ))
    }

    async fn load_config(&self, agent_id: &AgentId) -> AgentRuntimeResult<AgentConfig> {
        Ok(AgentConfig::new(
            agent_id.clone(),
            "MockAgent",
            "Mock agent config",
        ))
    }
}
