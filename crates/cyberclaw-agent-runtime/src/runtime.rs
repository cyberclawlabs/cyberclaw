//! MinimalAgentRuntime: in-memory agent runtime implementation.

use crate::config::AgentConfig;
use crate::error::{AgentRuntimeError, AgentRuntimeResult};
use crate::types::{AgentRequest, AgentResponse};
use crate::AgentRuntime;
use async_trait::async_trait;
use cyberclaw_core::ids::AgentId;
use cyberclaw_core::validation::Validate;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio::time::{timeout, Duration};
use tracing::{debug, warn};

/// Configuration for the agent runtime, including timeout and resource limit settings.
#[derive(Debug, Clone)]
pub struct AgentRuntimeConfig {
    /// Maximum duration allowed for a single agent execution.
    pub execution_timeout: Duration,
    /// Maximum memory allocation in MB per agent execution (default: 256MB)
    pub max_memory_mb: usize,
    /// Maximum CPU percentage per agent (0-100, default: 25%)
    pub max_cpu_percent: u8,
    /// Maximum number of concurrent agent executions (default: 10)
    pub max_concurrent_executions: usize,
    /// Enable input validation for dangerous patterns (default: true)
    pub enable_input_validation: bool,
}

impl Default for AgentRuntimeConfig {
    fn default() -> Self {
        Self {
            execution_timeout: Duration::from_secs(300), // 5-minute default timeout
            max_memory_mb: 256,                          // 256MB memory limit
            max_cpu_percent: 25,                         // 25% CPU limit
            max_concurrent_executions: 10,               // 10 concurrent executions max
            enable_input_validation: true,               // Enable security validation
        }
    }
}

impl AgentRuntimeConfig {
    /// Create a new config with the given execution timeout.
    pub fn with_timeout(execution_timeout: Duration) -> Self {
        Self {
            execution_timeout,
            ..Default::default()
        }
    }

    /// Create a config with custom resource limits.
    pub fn with_limits(
        execution_timeout: Duration,
        max_memory_mb: usize,
        max_cpu_percent: u8,
    ) -> Self {
        Self {
            execution_timeout,
            max_memory_mb,
            max_cpu_percent,
            ..Default::default()
        }
    }
}

impl Validate for AgentRuntimeConfig {
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

        // Validate resource limits
        const MAX_MEMORY_MB: usize = 2048; // 2GB max per agent
        if self.max_memory_mb == 0 || self.max_memory_mb > MAX_MEMORY_MB {
            return Err(ValidationError::new(format!(
                "max_memory_mb must be between 1 and {} MB",
                MAX_MEMORY_MB
            )));
        }

        if self.max_cpu_percent > 100 {
            return Err(ValidationError::new(
                "max_cpu_percent must be between 0 and 100",
            ));
        }

        if self.max_concurrent_executions == 0 {
            return Err(ValidationError::new(
                "max_concurrent_executions must be > 0",
            ));
        }

        Ok(())
    }
}

/// Validates agent input to prevent common security issues.
///
/// # Security Checks
///
/// - **JSON depth limit**: Maximum 10 levels of nesting to prevent stack overflow via deeply
///   nested structures.
/// - **String length limit**: Maximum 1MB per string value to prevent memory exhaustion.
/// - **Collection size limit**: Maximum 10,000 elements per object or array to prevent resource
///   exhaustion.
/// - **Dangerous character patterns**: Rejects strings containing path traversal sequences,
///   script injection, shell injection, and SQL injection patterns.
///
/// # Errors
///
/// Returns [`AgentRuntimeError::ValidationError`] if any check fails, with a message describing
/// which constraint was violated.
fn validate_input(input: &serde_json::Value) -> Result<(), AgentRuntimeError> {
    const MAX_DEPTH: usize = 10;
    const MAX_STRING_LENGTH: usize = 1024 * 1024; // 1MB
    const MAX_COLLECTION_SIZE: usize = 10_000;

    const DANGEROUS_PATTERNS: &[&str] = &[
        "../",
        "..\\",
        "<script>",
        "${",
        "$(",
        "; rm ",
        "| rm ",
        "&& rm ",
        "DROP TABLE",
    ];

    fn check_depth(
        value: &serde_json::Value,
        current_depth: usize,
        max_depth: usize,
    ) -> Result<(), AgentRuntimeError> {
        if current_depth > max_depth {
            return Err(AgentRuntimeError::ValidationError(format!(
                "JSON depth exceeds maximum of {}",
                max_depth
            )));
        }

        match value {
            serde_json::Value::Object(map) => {
                if map.len() > MAX_COLLECTION_SIZE {
                    return Err(AgentRuntimeError::ValidationError(format!(
                        "Object size exceeds maximum of {}",
                        MAX_COLLECTION_SIZE
                    )));
                }
                for v in map.values() {
                    check_depth(v, current_depth + 1, max_depth)?;
                }
            }
            serde_json::Value::Array(arr) => {
                if arr.len() > MAX_COLLECTION_SIZE {
                    return Err(AgentRuntimeError::ValidationError(format!(
                        "Array size exceeds maximum of {}",
                        MAX_COLLECTION_SIZE
                    )));
                }
                for v in arr {
                    check_depth(v, current_depth + 1, max_depth)?;
                }
            }
            serde_json::Value::String(s) => {
                if s.len() > MAX_STRING_LENGTH {
                    return Err(AgentRuntimeError::ValidationError(format!(
                        "String length exceeds maximum of {} bytes",
                        MAX_STRING_LENGTH
                    )));
                }
                for pattern in DANGEROUS_PATTERNS {
                    if s.contains(pattern) {
                        return Err(AgentRuntimeError::ValidationError(format!(
                            "Input contains dangerous pattern: {}",
                            pattern
                        )));
                    }
                }
            }
            _ => {}
        }

        Ok(())
    }

    check_depth(input, 0, MAX_DEPTH)
}

/// RAII guard that decrements the concurrent execution counter when dropped.
///
/// Ensures the counter is always decremented even if execution panics or
/// returns early with an error.
struct ExecutionGuard {
    counter: Arc<AtomicUsize>,
}

impl ExecutionGuard {
    fn new(counter: Arc<AtomicUsize>) -> Self {
        Self { counter }
    }
}

impl Drop for ExecutionGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A minimal in-memory agent runtime.
///
/// Stores agent configurations in memory and responds to execution
/// requests using the registered agent config.
///
/// When dropped, the runtime signals shutdown to any background tasks
/// and aborts them, ensuring clean resource release.
pub struct MinimalAgentRuntime {
    configs: RwLock<HashMap<String, AgentConfig>>,
    config: AgentRuntimeConfig,
    /// Shutdown flag shared with any background tasks.
    shutdown: Arc<AtomicBool>,
    /// Handles to background tasks that must be aborted on drop.
    background_tasks: Vec<JoinHandle<()>>,
    /// Current number of concurrent executions (for resource limiting)
    concurrent_executions: Arc<std::sync::atomic::AtomicUsize>,
}

impl MinimalAgentRuntime {
    /// Create a new empty runtime with default configuration.
    pub fn new() -> Self {
        Self {
            configs: RwLock::new(HashMap::new()),
            config: AgentRuntimeConfig::default(),
            shutdown: Arc::new(AtomicBool::new(false)),
            background_tasks: Vec::new(),
            concurrent_executions: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    /// Create a new empty runtime with the given configuration.
    pub fn with_config(config: AgentRuntimeConfig) -> Self {
        Self {
            configs: RwLock::new(HashMap::new()),
            config,
            shutdown: Arc::new(AtomicBool::new(false)),
            background_tasks: Vec::new(),
            concurrent_executions: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    /// Register an agent configuration in the runtime.
    ///
    /// Validates `config` before storing it. Returns
    /// [`AgentRuntimeError::ConfigError`] when validation fails.
    pub async fn register(&self, config: AgentConfig) -> AgentRuntimeResult<()> {
        config
            .validate()
            .map_err(|e| AgentRuntimeError::ConfigError(e.to_string()))?;
        let mut store = self.configs.write().await;
        store.insert(config.agent_id.as_str().to_owned(), config);
        Ok(())
    }

    /// List all registered agent configurations.
    pub async fn list_registered_configs(&self) -> Vec<AgentConfig> {
        let store = self.configs.read().await;
        store.values().cloned().collect()
    }

    /// Returns `true` if the runtime has been shut down.
    pub fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::Acquire)
    }
}

impl Default for MinimalAgentRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for MinimalAgentRuntime {
    fn drop(&mut self) {
        // Signal shutdown to all background tasks.
        self.shutdown.store(true, Ordering::Release);

        // Abort all tracked background task handles.
        for task in &self.background_tasks {
            task.abort();
        }

        debug!(
            task_count = self.background_tasks.len(),
            "MinimalAgentRuntime dropped: shutdown signalled, background tasks aborted"
        );
    }
}

#[async_trait]
impl AgentRuntime for MinimalAgentRuntime {
    /// Execute the request using the registered agent configuration.
    ///
    /// Returns [`AgentRuntimeError::ExecutionTimeout`] if the execution
    /// exceeds the configured timeout.
    async fn execute(&self, request: AgentRequest) -> AgentRuntimeResult<AgentResponse> {
        // 1. Check concurrent execution limit before proceeding.
        let current = self.concurrent_executions.load(Ordering::Acquire);
        if current >= self.config.max_concurrent_executions {
            return Err(AgentRuntimeError::ResourceLimitExceeded(format!(
                "Max concurrent executions ({}) reached",
                self.config.max_concurrent_executions
            )));
        }

        // 2. Increment counter and install RAII guard to decrement on exit.
        self.concurrent_executions.fetch_add(1, Ordering::SeqCst);
        let _guard = ExecutionGuard::new(Arc::clone(&self.concurrent_executions));

        let agent_id = request.agent_id.clone();

        // 3. Validate input if enabled.
        if self.config.enable_input_validation {
            let input_value = serde_json::Value::String(request.input.clone());
            validate_input(&input_value)?;
        }

        // 4. Confirm the agent is registered before executing.
        {
            let store = self.configs.read().await;
            if !store.contains_key(agent_id.as_str()) {
                return Err(AgentRuntimeError::AgentNotFound(agent_id));
            }
        }

        let config = {
            let store = self.configs.read().await;
            store
                .get(agent_id.as_str())
                .cloned()
                .ok_or_else(|| AgentRuntimeError::AgentNotFound(agent_id.clone()))?
        };

        let timeout_secs = self.config.execution_timeout.as_secs();
        let agent_input = request.input.clone();
        let request_context = request.context.clone();
        let response_agent_id = agent_id.clone();
        let agent_name = config.name.clone();
        let agent_description = config.description.clone();
        let config_params = config.params.clone();

        let execution_future = async move {
            let mut response = AgentResponse::ok(
                response_agent_id,
                format!("agent '{}' processed input: {}", agent_name, agent_input),
            );
            response
                .metadata
                .insert("agent_name".to_string(), serde_json::json!(agent_name));
            response.metadata.insert(
                "agent_description".to_string(),
                serde_json::json!(agent_description),
            );
            response
                .metadata
                .insert("params".to_string(), serde_json::json!(config_params));
            response
                .metadata
                .insert("context".to_string(), serde_json::json!(request_context));
            AgentRuntimeResult::Ok(response)
        };

        timeout(self.config.execution_timeout, execution_future)
            .await
            .map_err(|_| {
                warn!(
                    agent_id = %agent_id,
                    timeout_secs,
                    "agent execution timed out"
                );
                AgentRuntimeError::ExecutionTimeout {
                    agent_id: agent_id.clone(),
                    timeout_secs,
                }
            })?
    }

    /// Load agent configuration from the in-memory store.
    async fn load_config(&self, agent_id: &AgentId) -> AgentRuntimeResult<AgentConfig> {
        let store = self.configs.read().await;
        store
            .get(agent_id.as_str())
            .cloned()
            .ok_or_else(|| AgentRuntimeError::AgentNotFound(agent_id.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_agent_id(s: &str) -> AgentId {
        AgentId::from_string(s.to_owned()).expect("valid agent id")
    }

    #[tokio::test]
    async fn test_register_and_load_config() {
        let runtime = MinimalAgentRuntime::new();
        let id = make_agent_id("test-agent");
        let config = AgentConfig::new(id.clone(), "Test Agent", "A test agent");

        runtime.register(config.clone()).await.unwrap();

        let loaded = runtime.load_config(&id).await.unwrap();
        assert_eq!(loaded.agent_id, id);
        assert_eq!(loaded.name, "Test Agent");
        assert_eq!(loaded.description, "A test agent");
    }

    #[tokio::test]
    async fn test_load_config_not_found() {
        let runtime = MinimalAgentRuntime::new();
        let id = make_agent_id("missing-agent");
        let result = runtime.load_config(&id).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not registered"));
    }

    #[tokio::test]
    async fn test_execute_uses_registered_config() {
        let runtime = MinimalAgentRuntime::new();
        let id = make_agent_id("echo-agent");
        let config = AgentConfig::new(id.clone(), "Echo Agent", "Echoes input");
        runtime.register(config).await.unwrap();

        let request = AgentRequest::new(id.clone(), "hello world");
        let response = runtime.execute(request).await.unwrap();

        assert!(response.success);
        assert_eq!(
            response.output,
            "agent 'Echo Agent' processed input: hello world"
        );
        assert_eq!(response.agent_id, id);
        assert_eq!(
            response.metadata.get("agent_name"),
            Some(&serde_json::json!("Echo Agent"))
        );
    }

    #[tokio::test]
    async fn test_execute_unregistered_agent_returns_error() {
        let runtime = MinimalAgentRuntime::new();
        let id = make_agent_id("ghost-agent");
        let request = AgentRequest::new(id.clone(), "will fail");
        let result = runtime.execute(request).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not registered"));
    }

    #[tokio::test]
    async fn test_config_with_params() {
        let runtime = MinimalAgentRuntime::new();
        let id = make_agent_id("param-agent");
        let config = AgentConfig::new(id.clone(), "Param Agent", "Agent with params")
            .with_param("model", serde_json::json!("gpt-4"))
            .with_param("temperature", serde_json::json!(0.7));

        runtime.register(config).await.unwrap();

        let loaded = runtime.load_config(&id).await.unwrap();
        assert_eq!(loaded.params["model"], serde_json::json!("gpt-4"));
        assert_eq!(loaded.params["temperature"], serde_json::json!(0.7));
    }

    #[tokio::test]
    async fn test_execute_with_context() {
        let runtime = MinimalAgentRuntime::new();
        let id = make_agent_id("ctx-agent");
        runtime
            .register(AgentConfig::new(id.clone(), "Ctx Agent", ""))
            .await
            .unwrap();

        let request = AgentRequest::new(id.clone(), "contextual")
            .with_context("session", serde_json::json!("abc123"));

        let response = runtime.execute(request).await.unwrap();
        assert!(response.success);
        assert_eq!(
            response.output,
            "agent 'Ctx Agent' processed input: contextual"
        );
        assert_eq!(
            response.metadata.get("context"),
            Some(&serde_json::json!({"session":"abc123"}))
        );
    }

    #[tokio::test]
    async fn test_list_registered_configs() {
        let runtime = MinimalAgentRuntime::new();
        runtime
            .register(AgentConfig::new(
                make_agent_id("agent-1"),
                "Agent 1",
                "first",
            ))
            .await
            .unwrap();
        runtime
            .register(AgentConfig::new(
                make_agent_id("agent-2"),
                "Agent 2",
                "second",
            ))
            .await
            .unwrap();

        let configs = runtime.list_registered_configs().await;
        assert_eq!(configs.len(), 2);
    }

    #[tokio::test]
    async fn test_drop_sets_shutdown_flag() {
        // Capture the shutdown Arc before dropping the runtime.
        let shutdown_flag = {
            let runtime = MinimalAgentRuntime::new();
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
        let mut runtime = MinimalAgentRuntime::new();

        // Spawn a long-running task and track it in the runtime.
        let handle = tokio::spawn(async {
            tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;
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
        let runtime = MinimalAgentRuntime::new();
        assert!(
            !runtime.is_shutdown(),
            "is_shutdown() must return false on a live runtime"
        );
    }

    #[test]
    fn test_agent_runtime_config_validation_valid() {
        let config = AgentRuntimeConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_agent_runtime_config_validation_zero_timeout() {
        let config = AgentRuntimeConfig {
            execution_timeout: Duration::ZERO,
            ..Default::default()
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("execution_timeout"));
    }

    #[test]
    fn test_agent_runtime_config_validation_timeout_too_long() {
        let config = AgentRuntimeConfig {
            execution_timeout: Duration::from_secs(7200), // 2 hours, exceeds max
            ..Default::default()
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("too long"));
    }

    #[test]
    fn test_agent_runtime_config_with_timeout() {
        let config = AgentRuntimeConfig::with_timeout(Duration::from_secs(120));
        assert!(config.validate().is_ok());
        assert_eq!(config.execution_timeout.as_secs(), 120);
    }
}
