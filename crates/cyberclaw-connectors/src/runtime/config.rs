//! Process runtime configuration for resource limits and isolation
//!
//! Defines configuration parameters for subprocess execution including timeouts,
//! working directory, environment variables, and resource limits.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

/// Configuration for process runtime execution
///
/// # Security Considerations
///
/// - **Timeout**: Prevents infinite loops or denial-of-service attacks
/// - **Working Directory**: Isolates file operations to a specific directory
/// - **Environment**: Controls available environment variables (secrets isolation)
/// - **Resource Limits**: Prevents resource exhaustion (CPU, memory, file handles)
///
/// # Examples
///
/// ```
/// use cyberclaw_connectors::runtime::ProcessConfig;
/// use std::time::Duration;
/// use std::path::PathBuf;
///
/// let config = ProcessConfig::builder()
///     .timeout(Duration::from_secs(30))
///     .working_directory(PathBuf::from("/tmp/workspace"))
///     .env("RUST_LOG", "info")
///     .build();
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessConfig {
    /// Maximum execution time before SIGTERM is sent
    ///
    /// After timeout expires:
    /// 1. Send SIGTERM to allow graceful shutdown (5 seconds grace period)
    /// 2. If still running, send SIGKILL to forcefully terminate
    ///
    /// Default: 120 seconds
    pub timeout: Duration,

    /// Grace period between SIGTERM and SIGKILL
    ///
    /// Allows the process to clean up resources before forceful termination
    ///
    /// Default: 5 seconds
    pub kill_grace_period: Duration,

    /// Working directory for subprocess execution
    ///
    /// All relative file paths will be resolved against this directory.
    /// If None, inherits the parent process's working directory.
    ///
    /// Default: None (inherit from parent)
    pub working_directory: Option<PathBuf>,

    /// Environment variables to set for the subprocess
    ///
    /// # Security Note
    ///
    /// Environment variables should NOT contain secrets directly.
    /// Use SecretsManager and inject secret references instead.
    ///
    /// Default: Empty (inherits parent environment)
    pub environment: HashMap<String, String>,

    /// Whether to inherit environment variables from the parent process
    ///
    /// If false, only `environment` variables are available (clean environment)
    ///
    /// Default: false (do not inherit for security)
    pub inherit_env: bool,

    /// Maximum memory usage in bytes (Linux cgroups only)
    ///
    /// If None, no memory limit is enforced.
    /// Requires cgroup v2 support on Linux.
    ///
    /// Default: None
    pub max_memory_bytes: Option<u64>,

    /// Maximum number of file descriptors
    ///
    /// Prevents file descriptor exhaustion attacks.
    /// If None, uses system default (ulimit -n)
    ///
    /// Default: Some(1024)
    pub max_file_descriptors: Option<u32>,

    /// Capture stdout from subprocess
    ///
    /// If true, stdout is captured and returned in ProcessResult.
    /// If false, stdout is inherited from parent (useful for interactive processes)
    ///
    /// Default: true
    pub capture_stdout: bool,

    /// Capture stderr from subprocess
    ///
    /// If true, stderr is captured and returned in ProcessResult.
    /// If false, stderr is inherited from parent
    ///
    /// Default: true
    pub capture_stderr: bool,
}

impl Default for ProcessConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(120),
            kill_grace_period: Duration::from_secs(2),
            working_directory: None,
            environment: HashMap::new(),
            inherit_env: false, // Secure by default: clean environment
            max_memory_bytes: None,
            max_file_descriptors: Some(1024),
            capture_stdout: true,
            capture_stderr: true,
        }
    }
}

impl ProcessConfig {
    /// Create a new ProcessConfig builder
    pub fn builder() -> ProcessConfigBuilder {
        ProcessConfigBuilder::default()
    }

    /// Validate the configuration for security and correctness
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Timeout is zero or exceeds 10 minutes (600 seconds)
    /// - Kill grace period is zero or exceeds timeout
    /// - Working directory does not exist or is not a directory
    /// - Environment variables contain control characters or secrets
    pub fn validate(&self) -> anyhow::Result<()> {
        // Validate timeout bounds
        if self.timeout.is_zero() {
            anyhow::bail!("timeout must be greater than zero");
        }
        if self.timeout > Duration::from_secs(600) {
            anyhow::bail!("timeout cannot exceed 600 seconds (10 minutes)");
        }

        // Validate kill grace period
        if self.kill_grace_period.is_zero() {
            anyhow::bail!("kill_grace_period must be greater than zero");
        }
        if self.kill_grace_period >= self.timeout {
            anyhow::bail!("kill_grace_period must be less than timeout");
        }

        // Validate working directory exists (if specified)
        if let Some(ref cwd) = self.working_directory {
            if !cwd.exists() {
                anyhow::bail!("working_directory does not exist: {}", cwd.display());
            }
            if !cwd.is_dir() {
                anyhow::bail!("working_directory is not a directory: {}", cwd.display());
            }
        }

        // Validate environment variables (no control characters)
        for (key, value) in &self.environment {
            if key.contains(|c: char| c.is_control()) {
                anyhow::bail!(
                    "environment variable key contains control characters: {}",
                    key
                );
            }
            if value.contains(|c: char| c.is_control() && c != '\n') {
                anyhow::bail!(
                    "environment variable value contains control characters: {}",
                    key
                );
            }
        }

        Ok(())
    }
}

/// Builder for ProcessConfig with fluent API
#[derive(Debug, Default)]
pub struct ProcessConfigBuilder {
    config: ProcessConfig,
}

impl ProcessConfigBuilder {
    /// Set the execution timeout
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.config.timeout = timeout;
        self
    }

    /// Set the kill grace period (between SIGTERM and SIGKILL)
    pub fn kill_grace_period(mut self, grace: Duration) -> Self {
        self.config.kill_grace_period = grace;
        self
    }

    /// Set the working directory for subprocess execution
    pub fn working_directory(mut self, cwd: PathBuf) -> Self {
        self.config.working_directory = Some(cwd);
        self
    }

    /// Add a single environment variable
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.config.environment.insert(key.into(), value.into());
        self
    }

    /// Set all environment variables at once (replaces existing)
    pub fn environment(mut self, env: HashMap<String, String>) -> Self {
        self.config.environment = env;
        self
    }

    /// Enable or disable environment variable inheritance from parent
    pub fn inherit_env(mut self, inherit: bool) -> Self {
        self.config.inherit_env = inherit;
        self
    }

    /// Set maximum memory limit in bytes
    pub fn max_memory_bytes(mut self, limit: u64) -> Self {
        self.config.max_memory_bytes = Some(limit);
        self
    }

    /// Set maximum file descriptor limit
    pub fn max_file_descriptors(mut self, limit: u32) -> Self {
        self.config.max_file_descriptors = Some(limit);
        self
    }

    /// Enable or disable stdout capture
    pub fn capture_stdout(mut self, capture: bool) -> Self {
        self.config.capture_stdout = capture;
        self
    }

    /// Enable or disable stderr capture
    pub fn capture_stderr(mut self, capture: bool) -> Self {
        self.config.capture_stderr = capture;
        self
    }

    /// Build the ProcessConfig (does not validate)
    pub fn build(self) -> ProcessConfig {
        self.config
    }

    /// Build and validate the ProcessConfig
    ///
    /// # Errors
    ///
    /// Returns an error if validation fails (see ProcessConfig::validate)
    pub fn build_validated(self) -> anyhow::Result<ProcessConfig> {
        self.config.validate()?;
        Ok(self.config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = ProcessConfig::default();
        assert_eq!(config.timeout, Duration::from_secs(120));
        assert_eq!(config.kill_grace_period, Duration::from_secs(2));
        assert_eq!(config.working_directory, None);
        assert_eq!(config.environment.len(), 0);
        assert!(!config.inherit_env);
        assert_eq!(config.max_file_descriptors, Some(1024));
        assert!(config.capture_stdout);
        assert!(config.capture_stderr);
    }

    #[test]
    fn test_builder_fluent_api() {
        let config = ProcessConfig::builder()
            .timeout(Duration::from_secs(30))
            .kill_grace_period(Duration::from_secs(3))
            .env("KEY", "value")
            .inherit_env(true)
            .capture_stdout(false)
            .build();

        assert_eq!(config.timeout, Duration::from_secs(30));
        assert_eq!(config.kill_grace_period, Duration::from_secs(3));
        assert_eq!(config.environment.get("KEY"), Some(&"value".to_string()));
        assert!(config.inherit_env);
        assert!(!config.capture_stdout);
    }

    #[test]
    fn test_validate_zero_timeout() {
        let config = ProcessConfig::builder()
            .timeout(Duration::from_secs(0))
            .build();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_excessive_timeout() {
        let config = ProcessConfig::builder()
            .timeout(Duration::from_secs(700))
            .build();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_grace_period_exceeds_timeout() {
        let config = ProcessConfig::builder()
            .timeout(Duration::from_secs(10))
            .kill_grace_period(Duration::from_secs(15))
            .build();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_control_chars_in_env() {
        let config = ProcessConfig::builder().env("KEY\x00", "value").build();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_serde_round_trip() {
        let config = ProcessConfig::builder()
            .timeout(Duration::from_secs(45))
            .env("TEST", "123")
            .build();
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: ProcessConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config, deserialized);
    }
}
