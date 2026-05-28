//! Mock SkillRuntime for testing.

use crate::context::SkillContext;
use crate::handler::SkillHandler;
use crate::loaders::SkillMetadata;
use crate::runtime::SkillRuntime;
use async_trait::async_trait;
use cyberclaw_core::ids::SkillId;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Recorded invocation for test assertions.
#[derive(Debug, Clone)]
pub struct SkillInvocationRecord {
    pub skill_id: SkillId,
    pub input: serde_json::Value,
}

/// A controllable mock skill runtime used in tests.
///
/// By default every `invoke` call succeeds with an echo of the input.
/// Set `fail_with` to make all invocations return an error.
#[derive(Default)]
pub struct MockSkillRuntime {
    /// When `Some`, all `invoke` calls return this error.
    pub fail_with: Arc<RwLock<Option<String>>>,
    /// All invocations received so far (for assertion in tests).
    pub recorded: Arc<RwLock<Vec<SkillInvocationRecord>>>,
}

impl MockSkillRuntime {
    /// Create a new mock that succeeds every call.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a mock that always fails with the given message.
    pub fn with_failure(message: impl Into<String>) -> Self {
        Self {
            fail_with: Arc::new(RwLock::new(Some(message.into()))),
            recorded: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Return all invocations recorded so far.
    ///
    /// Uses `blocking_read()` which parks the current thread until the read lock
    /// is available. This is safe to call from synchronous test helper methods
    /// because:
    ///   1. Tests call this only after all async tasks that could hold a write
    ///      lock have completed (i.e., after `.await` points).
    ///   2. `RwLock::blocking_read()` never panics — it blocks until the lock is
    ///      acquired, unlike `try_read()` which returns `Err` when the lock is
    ///      currently held.
    pub fn get_recorded(&self) -> Vec<SkillInvocationRecord> {
        self.recorded.blocking_read().clone()
    }
}

#[async_trait]
impl SkillRuntime for MockSkillRuntime {
    #[allow(deprecated)]
    async fn invoke(
        &self,
        skill_id: &SkillId,
        input: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        self.recorded.write().await.push(SkillInvocationRecord {
            skill_id: skill_id.clone(),
            input: input.clone(),
        });

        let fail_msg = self.fail_with.read().await.clone();
        if let Some(msg) = fail_msg {
            anyhow::bail!("MockSkillRuntime: {}", msg);
        }

        Ok(serde_json::json!({ "mock": true, "echo": input }))
    }

    async fn get_skill_context(&self, skill_id: &SkillId) -> Option<SkillContext> {
        // Mock always returns a minimal context.
        Some(SkillContext {
            metadata: SkillMetadata {
                name: skill_id.as_str().to_string(),
                version: "0.0.0-mock".to_string(),
                description: "Mock skill context".to_string(),
                author: None,
                homepage: None,
                tags: vec![],
                tools: vec![],
                scripts: vec![],
                assets: vec![],
                prompt_body: None,
                platforms: vec![],
                triggers: vec![],
                required_toolsets: vec![],
                source_ecosystem: None,
                allowed_capabilities: vec![],
            },
            prompt_extension: String::new(),
            tool_declarations: vec![],
            references: vec![],
            scripts: vec![],
            assets: vec![],
            origin_format: String::new(),
        })
    }

    async fn register_skill(&self, _skill_id: SkillId, _handler: Arc<dyn SkillHandler>) {
        // No-op in mock
    }

    async fn unregister_skill(&self, _skill_id: &SkillId) -> anyhow::Result<bool> {
        // No-op in mock; always reports success
        Ok(true)
    }
}
