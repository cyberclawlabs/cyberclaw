//! SkillRuntime trait and MinimalSkillRuntime implementation.
//!
//! The `SkillRuntime` trait defines the core abstraction for skill registration
//! and context retrieval. Direct skill invocation via `invoke()` is deprecated
//! in favour of the Connector -> Capability execution path. Use
//! `get_skill_context()` to obtain the skill's prompt, tool declarations, and
//! references, then route execution through the appropriate Connector.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio::time::{timeout, Duration};
use tracing::{debug, warn};

use cyberclaw_core::ids::SkillId;
use cyberclaw_core::validation::Validate;

use crate::context::SkillContext;
use crate::error::SkillRuntimeError;
use crate::handler::SkillHandler;
use crate::loaders::SkillMetadata;

/// Configuration for the skill runtime, including timeout settings.
#[derive(Debug, Clone)]
pub struct SkillRuntimeConfig {
    /// Maximum duration allowed for a single skill invocation.
    pub execution_timeout: Duration,
}

impl Default for SkillRuntimeConfig {
    fn default() -> Self {
        Self {
            execution_timeout: Duration::from_secs(300), // 5-minute default timeout
        }
    }
}

impl SkillRuntimeConfig {
    /// Create a new config with the given execution timeout.
    pub fn with_timeout(execution_timeout: Duration) -> Self {
        Self { execution_timeout }
    }
}

impl Validate for SkillRuntimeConfig {
    fn validate(&self) -> cyberclaw_core::validation::ValidationResult {
        use cyberclaw_core::validation::ValidationError;

        if self.execution_timeout.is_zero() {
            return Err(ValidationError::new("execution_timeout must be > 0"));
        }
        // Validate timeout is reasonable (not too long)
        const MAX_TIMEOUT_SECS: u64 = 3600; // 1 hour max
        if self.execution_timeout.as_secs() > MAX_TIMEOUT_SECS {
            return Err(ValidationError::new(format!(
                "execution_timeout too long: {} seconds (max {} seconds)",
                self.execution_timeout.as_secs(),
                MAX_TIMEOUT_SECS
            )));
        }
        Ok(())
    }
}

/// Core trait for a skill runtime that can register skills and provide context.
///
/// In CyberClaw's architecture, Skills provide methods, knowledge, and templates
/// but do **not** directly execute capabilities. All execution flows through
/// `Connector -> Capability`. Use [`get_skill_context`](SkillRuntime::get_skill_context)
/// to retrieve the skill's context for injection into the agentic loop.
#[async_trait]
pub trait SkillRuntime: Send + Sync {
    /// Invoke a registered skill by its ID with the given JSON input.
    ///
    /// # Deprecation
    ///
    /// Direct skill invocation violates CyberClaw's execution model where all
    /// capability execution must flow through `Connector -> Capability`. Use
    /// [`get_skill_context`](SkillRuntime::get_skill_context) to retrieve the
    /// skill's context and route execution through the appropriate Connector.
    ///
    /// # Errors
    /// Returns [`SkillRuntimeError::SkillNotFound`] if no skill is registered for `skill_id`.
    /// Returns [`SkillRuntimeError::InvocationFailed`] if the handler returns an error.
    /// Returns [`SkillRuntimeError::ExecutionTimeout`] if the invocation exceeds the configured timeout.
    #[deprecated(
        since = "0.2.0",
        note = "Use Connector -> Capability path instead. Skills provide context, not execution. See get_skill_context()."
    )]
    async fn invoke(
        &self,
        skill_id: &SkillId,
        input: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value>;

    /// Retrieve the context provided by a registered skill.
    ///
    /// Returns `None` if no skill is registered for `skill_id`. The returned
    /// [`SkillContext`] contains the skill's prompt extension, tool declarations,
    /// and reference documents for injection into the agentic loop.
    async fn get_skill_context(&self, skill_id: &SkillId) -> Option<SkillContext>;

    /// Register a skill handler under the given ID.
    ///
    /// If a handler is already registered for `skill_id`, it will be replaced.
    async fn register_skill(&self, skill_id: SkillId, handler: Arc<dyn SkillHandler>);

    /// Unregister a skill by its ID.
    ///
    /// Returns `true` if the skill was registered and has been removed.
    /// Returns `false` if no skill was registered under `skill_id`.
    ///
    /// # Errors
    ///
    /// Returns an error if the runtime does not support dynamic unregistration.
    async fn unregister_skill(&self, skill_id: &SkillId) -> anyhow::Result<bool>;
}

/// In-memory skill runtime backed by a `HashMap` protected by a `RwLock`.
///
/// This is the default implementation suitable for single-process use.
///
/// When dropped, the runtime signals shutdown to any background tasks
/// and aborts them, ensuring clean resource release.
pub struct MinimalSkillRuntime {
    registry: RwLock<HashMap<String, Arc<dyn SkillHandler>>>,
    /// Metadata registry, keyed by skill ID string.
    metadata_registry: RwLock<HashMap<String, SkillMetadata>>,
    config: SkillRuntimeConfig,
    /// Shutdown flag shared with any background tasks.
    shutdown: Arc<AtomicBool>,
    /// Handles to background tasks that must be aborted on drop.
    background_tasks: Vec<JoinHandle<()>>,
}

impl Default for MinimalSkillRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl MinimalSkillRuntime {
    /// Create an empty runtime with no registered skills and default configuration.
    pub fn new() -> Self {
        Self {
            registry: RwLock::new(HashMap::new()),
            metadata_registry: RwLock::new(HashMap::new()),
            config: SkillRuntimeConfig::default(),
            shutdown: Arc::new(AtomicBool::new(false)),
            background_tasks: Vec::new(),
        }
    }

    /// Create an empty runtime with the given configuration.
    pub fn with_config(config: SkillRuntimeConfig) -> Self {
        Self {
            registry: RwLock::new(HashMap::new()),
            metadata_registry: RwLock::new(HashMap::new()),
            config,
            shutdown: Arc::new(AtomicBool::new(false)),
            background_tasks: Vec::new(),
        }
    }

    /// Register metadata for a skill.
    ///
    /// This is used alongside `register_skill` to associate metadata with a
    /// skill handler, enabling `get_skill_context` to return meaningful context.
    pub async fn register_skill_metadata(&self, skill_id: &SkillId, metadata: SkillMetadata) {
        let key = skill_id.as_str().to_string();
        debug!(skill_id = %key, "registering skill metadata");
        self.metadata_registry.write().await.insert(key, metadata);
    }

    /// Returns the number of registered skills.
    pub async fn skill_count(&self) -> usize {
        self.registry.read().await.len()
    }

    /// Returns `true` if a skill is registered for the given ID.
    pub async fn has_skill(&self, skill_id: &SkillId) -> bool {
        self.registry.read().await.contains_key(skill_id.as_str())
    }

    /// Returns `true` if the runtime has been shut down.
    pub fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::Acquire)
    }
}

impl Drop for MinimalSkillRuntime {
    fn drop(&mut self) {
        // Signal shutdown to all background tasks.
        self.shutdown.store(true, Ordering::Release);

        // Abort all tracked background task handles.
        for task in &self.background_tasks {
            task.abort();
        }

        debug!(
            task_count = self.background_tasks.len(),
            "MinimalSkillRuntime dropped: shutdown signalled, background tasks aborted"
        );
    }
}

#[async_trait]
impl SkillRuntime for MinimalSkillRuntime {
    #[allow(deprecated)]
    async fn invoke(
        &self,
        skill_id: &SkillId,
        input: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let handler = {
            let registry = self.registry.read().await;
            registry.get(skill_id.as_str()).cloned()
        };

        match handler {
            Some(h) => {
                debug!(skill_id = %skill_id, "invoking skill (deprecated path)");
                let timeout_secs = self.config.execution_timeout.as_secs();
                let invoke_future = h.handle(input);
                timeout(self.config.execution_timeout, invoke_future)
                    .await
                    .map_err(|_| {
                        warn!(skill_id = %skill_id, timeout_secs, "skill invocation timed out");
                        anyhow::Error::from(SkillRuntimeError::ExecutionTimeout {
                            skill_id: skill_id.clone(),
                            timeout_secs,
                        })
                    })?
                    .map_err(|e| {
                        SkillRuntimeError::InvocationFailed {
                            skill_id: skill_id.clone(),
                            source: e,
                        }
                        .into()
                    })
            }
            None => {
                warn!(skill_id = %skill_id, "skill not found");
                Err(SkillRuntimeError::SkillNotFound(skill_id.clone()).into())
            }
        }
    }

    async fn get_skill_context(&self, skill_id: &SkillId) -> Option<SkillContext> {
        let key = skill_id.as_str();
        let has_handler = self.registry.read().await.contains_key(key);
        if !has_handler {
            return None;
        }

        let metadata = {
            let meta_reg = self.metadata_registry.read().await;
            meta_reg.get(key).cloned().unwrap_or_else(|| SkillMetadata {
                name: skill_id.as_str().to_string(),
                version: "0.0.0".to_string(),
                description: String::new(),
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
            })
        };

        // Convert SkillToolDeclarations from metadata into ToolDeclarations.
        let tool_declarations = metadata.tools.clone();
        let scripts = metadata.scripts.clone();
        let assets = metadata.assets.clone();

        // v1.8 Bug D fix: 之前 prompt_extension 永远是空字符串 →
        // LLM 看不到 SKILL.md body 的 actionable content. 现在从 metadata
        // 取 loader 解析时填入的 prompt_body (Anthropic / hermes pattern).
        // 见 docs/research/skill-md-body-pipeline-gap-2026-05-28.md
        let prompt_extension = metadata.prompt_body.clone().unwrap_or_default();

        Some(SkillContext {
            metadata,
            prompt_extension,
            tool_declarations,
            references: vec![],
            scripts,
            assets,
            origin_format: String::new(),
        })
    }

    async fn register_skill(&self, skill_id: SkillId, handler: Arc<dyn SkillHandler>) {
        let key = skill_id.into_string();
        debug!(skill_id = %key, "registering skill");
        self.registry.write().await.insert(key, handler);
    }

    async fn unregister_skill(&self, skill_id: &SkillId) -> anyhow::Result<bool> {
        let key = skill_id.as_str();
        let removed = self.registry.write().await.remove(key).is_some();
        // Also remove associated metadata.
        self.metadata_registry.write().await.remove(key);
        if removed {
            debug!(skill_id = %key, "unregistered skill");
        } else {
            warn!(skill_id = %key, "unregister_skill: skill not found");
        }
        Ok(removed)
    }
}

#[cfg(test)]
#[allow(deprecated)]
mod tests {
    use super::*;
    use crate::loaders::SkillMetadata;
    use crate::skills::EchoSkill;
    use anyhow::anyhow;

    #[tokio::test]
    async fn test_register_and_invoke() {
        let runtime = MinimalSkillRuntime::new();
        let skill_id = SkillId::from_string("echo".to_string()).unwrap();

        runtime
            .register_skill(skill_id.clone(), Arc::new(EchoSkill))
            .await;

        assert!(runtime.has_skill(&skill_id).await);
        assert_eq!(runtime.skill_count().await, 1);

        let input = serde_json::json!({ "msg": "hello" });
        let output = runtime.invoke(&skill_id, input.clone()).await.unwrap();
        assert_eq!(output, input);
    }

    #[tokio::test]
    async fn test_invoke_unknown_skill_returns_error() {
        let runtime = MinimalSkillRuntime::new();
        let skill_id = SkillId::from_string("nonexistent".to_string()).unwrap();

        let result = runtime.invoke(&skill_id, serde_json::json!({})).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("nonexistent"));
    }

    #[tokio::test]
    async fn test_register_overwrites_existing() {
        let runtime = MinimalSkillRuntime::new();
        let skill_id = SkillId::from_string("overwrite".to_string()).unwrap();

        runtime
            .register_skill(skill_id.clone(), Arc::new(EchoSkill))
            .await;
        runtime
            .register_skill(skill_id.clone(), Arc::new(EchoSkill))
            .await;

        // Count should still be 1 (replaced, not duplicated)
        assert_eq!(runtime.skill_count().await, 1);
    }

    /// A skill handler that deliberately sleeps longer than the configured timeout.
    struct SlowSkill;

    #[async_trait]
    impl SkillHandler for SlowSkill {
        async fn handle(&self, _input: serde_json::Value) -> anyhow::Result<serde_json::Value> {
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok(serde_json::json!({}))
        }
    }

    #[tokio::test]
    async fn test_invoke_times_out() {
        // Use a very short timeout so the test completes quickly.
        let config = SkillRuntimeConfig::with_timeout(Duration::from_millis(50));
        let runtime = MinimalSkillRuntime::with_config(config);
        let skill_id = SkillId::from_string("slow".to_string()).unwrap();

        runtime
            .register_skill(skill_id.clone(), Arc::new(SlowSkill))
            .await;

        let result = runtime.invoke(&skill_id, serde_json::json!({})).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("timed out"),
            "expected timeout error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_with_config_custom_timeout() {
        let config = SkillRuntimeConfig::with_timeout(Duration::from_secs(10));
        let runtime = MinimalSkillRuntime::with_config(config);
        let skill_id = SkillId::from_string("echo".to_string()).unwrap();

        runtime
            .register_skill(skill_id.clone(), Arc::new(EchoSkill))
            .await;

        let input = serde_json::json!({ "msg": "custom-timeout" });
        let output = runtime.invoke(&skill_id, input.clone()).await.unwrap();
        assert_eq!(output, input);
    }

    // Suppress unused import warning for anyhow in non-test code paths
    fn _use_anyhow() -> anyhow::Error {
        anyhow!("unused")
    }

    #[tokio::test]
    async fn test_drop_sets_shutdown_flag() {
        // Capture the shutdown Arc before dropping the runtime.
        let shutdown_flag = {
            let runtime = MinimalSkillRuntime::new();
            let flag = runtime.shutdown.clone();
            assert!(
                !flag.load(std::sync::atomic::Ordering::Acquire),
                "shutdown should be false before drop"
            );
            flag
            // runtime is dropped here at end of block
        };
        // After drop, the shutdown flag must be set to true.
        assert!(
            shutdown_flag.load(std::sync::atomic::Ordering::Acquire),
            "shutdown flag must be true after runtime is dropped"
        );
    }

    #[tokio::test]
    async fn test_drop_aborts_background_tasks() {
        let mut runtime = MinimalSkillRuntime::new();

        // Spawn a long-running task and track it in the runtime.
        let handle = tokio::spawn(async {
            tokio::time::sleep(Duration::from_secs(3600)).await;
        });
        runtime.background_tasks.push(handle);

        // Retrieve the shutdown flag before dropping.
        let shutdown_flag = runtime.shutdown.clone();

        // Drop the runtime; this should abort the long-running task.
        drop(runtime);

        assert!(
            shutdown_flag.load(std::sync::atomic::Ordering::Acquire),
            "shutdown flag must be true after drop"
        );
    }

    #[tokio::test]
    async fn test_is_shutdown_returns_false_before_drop() {
        let runtime = MinimalSkillRuntime::new();
        assert!(
            !runtime.is_shutdown(),
            "is_shutdown() must return false on a live runtime"
        );
    }

    #[tokio::test]
    async fn test_with_config_drop_sets_shutdown_flag() {
        let shutdown_flag = {
            let config = SkillRuntimeConfig::with_timeout(Duration::from_secs(10));
            let runtime = MinimalSkillRuntime::with_config(config);
            let flag = runtime.shutdown.clone();
            assert!(!flag.load(std::sync::atomic::Ordering::Acquire));
            flag
        };
        assert!(
            shutdown_flag.load(std::sync::atomic::Ordering::Acquire),
            "shutdown flag must be true after with_config runtime is dropped"
        );
    }

    #[test]
    fn test_skill_runtime_config_validation_valid() {
        let config = SkillRuntimeConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_skill_runtime_config_validation_zero_timeout() {
        let config = SkillRuntimeConfig {
            execution_timeout: Duration::ZERO,
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("execution_timeout"));
    }

    #[test]
    fn test_skill_runtime_config_validation_timeout_too_long() {
        let config = SkillRuntimeConfig {
            execution_timeout: Duration::from_secs(7200), // 2 hours, exceeds max
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("too long"));
    }

    #[test]
    fn test_skill_runtime_config_with_timeout() {
        let config = SkillRuntimeConfig::with_timeout(Duration::from_secs(120));
        assert!(config.validate().is_ok());
        assert_eq!(config.execution_timeout.as_secs(), 120);
    }

    #[tokio::test]
    async fn test_get_skill_context_returns_none_for_unknown() {
        let runtime = MinimalSkillRuntime::new();
        let skill_id = SkillId::from_string("unknown".to_string()).unwrap();

        let ctx = runtime.get_skill_context(&skill_id).await;
        assert!(ctx.is_none());
    }

    #[tokio::test]
    async fn test_get_skill_context_with_handler_only() {
        let runtime = MinimalSkillRuntime::new();
        let skill_id = SkillId::from_string("echo".to_string()).unwrap();

        runtime
            .register_skill(skill_id.clone(), Arc::new(EchoSkill))
            .await;

        // Without explicit metadata, get_skill_context should still return
        // a context with default/derived metadata.
        let ctx = runtime.get_skill_context(&skill_id).await;
        assert!(ctx.is_some());
        let ctx = ctx.unwrap();
        assert_eq!(ctx.metadata.name, "echo");
        assert_eq!(ctx.metadata.version, "0.0.0");
    }

    #[tokio::test]
    async fn test_get_skill_context_with_metadata() {
        let runtime = MinimalSkillRuntime::new();
        let skill_id = SkillId::from_string("rich".to_string()).unwrap();

        runtime
            .register_skill(skill_id.clone(), Arc::new(EchoSkill))
            .await;

        runtime
            .register_skill_metadata(
                &skill_id,
                SkillMetadata {
                    name: "Rich Skill".to_string(),
                    version: "2.0.0".to_string(),
                    description: "A skill with full metadata".to_string(),
                    author: Some("tester".to_string()),
                    homepage: None,
                    tags: vec!["test".to_string()],
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
            )
            .await;

        let ctx = runtime.get_skill_context(&skill_id).await;
        assert!(ctx.is_some());
        let ctx = ctx.unwrap();
        assert_eq!(ctx.metadata.name, "Rich Skill");
        assert_eq!(ctx.metadata.version, "2.0.0");
        assert_eq!(ctx.metadata.description, "A skill with full metadata");
        assert_eq!(ctx.metadata.author.as_deref(), Some("tester"));
    }

    #[tokio::test]
    async fn test_unregister_removes_metadata() {
        let runtime = MinimalSkillRuntime::new();
        let skill_id = SkillId::from_string("cleanup".to_string()).unwrap();

        runtime
            .register_skill(skill_id.clone(), Arc::new(EchoSkill))
            .await;
        runtime
            .register_skill_metadata(
                &skill_id,
                SkillMetadata {
                    name: "Cleanup".to_string(),
                    version: "1.0.0".to_string(),
                    description: "Will be removed".to_string(),
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
            )
            .await;

        assert!(runtime.get_skill_context(&skill_id).await.is_some());

        runtime.unregister_skill(&skill_id).await.unwrap();

        assert!(runtime.get_skill_context(&skill_id).await.is_none());
    }
}
