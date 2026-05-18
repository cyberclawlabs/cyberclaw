//! Runtime trait abstractions for capability execution
//!
//! Defines the core ProcessRuntime trait for executing capabilities in isolated environments.

use super::config::ProcessConfig;
use super::mode::RuntimeMode;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Result of process execution containing output, exit status, and metrics
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessResult {
    /// Exit code from the subprocess (0 = success)
    pub exit_code: i32,

    /// Standard output captured from the subprocess
    ///
    /// Empty if `ProcessConfig.capture_stdout` was false
    pub stdout: String,

    /// Standard error captured from the subprocess
    ///
    /// Empty if `ProcessConfig.capture_stderr` was false
    pub stderr: String,

    /// Whether the process was terminated due to timeout
    pub timed_out: bool,

    /// Whether the process was killed with SIGKILL (after SIGTERM grace period)
    pub force_killed: bool,

    /// Execution duration in milliseconds
    pub duration_ms: u64,

    /// Additional metadata about the execution
    pub metadata: HashMap<String, String>,
}

impl ProcessResult {
    /// Returns true if the process exited successfully (exit_code == 0 and not timed out)
    pub fn is_success(&self) -> bool {
        self.exit_code == 0 && !self.timed_out
    }

    /// Returns true if the process failed (non-zero exit code or timed out)
    pub fn is_failure(&self) -> bool {
        !self.is_success()
    }

    /// Get a human-readable description of the execution outcome
    pub fn outcome_description(&self) -> String {
        if self.timed_out {
            format!(
                "Process timed out after {}ms{}",
                self.duration_ms,
                if self.force_killed {
                    " (force killed)"
                } else {
                    ""
                }
            )
        } else if self.exit_code == 0 {
            format!("Process succeeded in {}ms", self.duration_ms)
        } else {
            format!(
                "Process failed with exit code {} after {}ms",
                self.exit_code, self.duration_ms
            )
        }
    }
}

/// Async trait for runtime execution engines
///
/// # Implementations
///
/// - `NativeRuntime`: Direct execution in current process (no isolation)
/// - `ProcessExecutor`: Subprocess execution with timeout/limits
/// - `ContainerRuntime`: Docker/podman container execution (future)
///
/// # Examples
///
/// ```no_run
/// use cyberclaw_connectors::runtime::{ProcessRuntime, ProcessConfig, ProcessExecutor};
/// use std::path::PathBuf;
///
/// #[tokio::main]
/// async fn main() -> anyhow::Result<()> {
///     let executor = ProcessExecutor::new();
///     let config = ProcessConfig::builder()
///         .working_directory(PathBuf::from("/tmp"))
///         .build();
///
///     let result = executor.execute_command(
///         "ls",
///         vec!["-la".to_string()],
///         config
///     ).await?;
///
///     println!("Output: {}", result.stdout);
///     Ok(())
/// }
/// ```
#[async_trait]
pub trait ProcessRuntime: Send + Sync {
    /// Returns the runtime mode supported by this implementation
    fn mode(&self) -> RuntimeMode;

    /// Execute a command with arguments in an isolated runtime
    ///
    /// # Arguments
    ///
    /// - `command`: The command to execute (e.g., "cargo", "ls", "/usr/bin/python3")
    /// - `args`: Command-line arguments (e.g., vec!["test", "--workspace"])
    /// - `config`: Runtime configuration (timeout, cwd, env, limits)
    ///
    /// # Returns
    ///
    /// - `Ok(ProcessResult)`: Execution completed (may have non-zero exit code)
    /// - `Err(anyhow::Error)`: Execution could not start or runtime error occurred
    ///
    /// # Security
    ///
    /// Implementations MUST:
    /// - Enforce timeout (SIGTERM → SIGKILL graceful shutdown)
    /// - Validate command path (no path traversal, command injection)
    /// - Sanitize environment variables (no secrets in env)
    /// - Apply resource limits (memory, file descriptors)
    /// - Capture stdout/stderr to prevent information leakage
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - Command validation fails (invalid path, injection attempt)
    /// - Config validation fails (timeout bounds, working directory)
    /// - Process spawn fails (command not found, permission denied)
    /// - I/O error during output capture
    async fn execute_command(
        &self,
        command: &str,
        args: Vec<String>,
        config: ProcessConfig,
    ) -> anyhow::Result<ProcessResult>;

    /// Execute a shell script in an isolated runtime
    ///
    /// # Arguments
    ///
    /// - `script`: Shell script content to execute
    /// - `shell`: Shell interpreter (e.g., "bash", "sh", "zsh")
    /// - `config`: Runtime configuration
    ///
    /// # Security Warning
    ///
    /// Shell script execution is inherently risky. Implementations MUST:
    /// - Sanitize script content (no command injection)
    /// - Use restricted shell environments (no network access for high-risk scripts)
    /// - Apply strict timeout enforcement
    /// - Log all script executions to audit trail
    ///
    /// For high-risk scripts (RiskLevel::High or Critical), prefer `execute_command`
    /// with explicit argument passing over shell script execution.
    async fn execute_shell_script(
        &self,
        script: &str,
        shell: &str,
        config: ProcessConfig,
    ) -> anyhow::Result<ProcessResult>;

    /// Check if this runtime implementation is available on the current system
    ///
    /// # Returns
    ///
    /// - `Ok(())`: Runtime is available and functional
    /// - `Err(anyhow::Error)`: Runtime is not available (e.g., Docker not installed)
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use cyberclaw_connectors::runtime::{ProcessRuntime, ProcessExecutor};
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let executor = ProcessExecutor::new();
    ///     if executor.check_availability().await.is_ok() {
    ///         println!("Process runtime is available");
    ///     }
    /// }
    /// ```
    async fn check_availability(&self) -> anyhow::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_process_result_is_success() {
        let result = ProcessResult {
            exit_code: 0,
            stdout: "output".to_string(),
            stderr: String::new(),
            timed_out: false,
            force_killed: false,
            duration_ms: 100,
            metadata: HashMap::new(),
        };
        assert!(result.is_success());
        assert!(!result.is_failure());
    }

    #[test]
    fn test_process_result_failure_exit_code() {
        let result = ProcessResult {
            exit_code: 1,
            stdout: String::new(),
            stderr: "error".to_string(),
            timed_out: false,
            force_killed: false,
            duration_ms: 50,
            metadata: HashMap::new(),
        };
        assert!(!result.is_success());
        assert!(result.is_failure());
    }

    #[test]
    fn test_process_result_timeout() {
        let result = ProcessResult {
            exit_code: 137, // SIGKILL exit code
            stdout: String::new(),
            stderr: String::new(),
            timed_out: true,
            force_killed: true,
            duration_ms: 5000,
            metadata: HashMap::new(),
        };
        assert!(!result.is_success());
        assert!(result.is_failure());
        assert!(result.timed_out);
    }

    #[test]
    fn test_outcome_description() {
        let result = ProcessResult {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
            timed_out: false,
            force_killed: false,
            duration_ms: 200,
            metadata: HashMap::new(),
        };
        assert_eq!(result.outcome_description(), "Process succeeded in 200ms");

        let result_timeout = ProcessResult {
            exit_code: 137,
            stdout: String::new(),
            stderr: String::new(),
            timed_out: true,
            force_killed: true,
            duration_ms: 5000,
            metadata: HashMap::new(),
        };
        assert_eq!(
            result_timeout.outcome_description(),
            "Process timed out after 5000ms (force killed)"
        );
    }

    #[test]
    fn test_serde_round_trip() {
        let result = ProcessResult {
            exit_code: 0,
            stdout: "test output".to_string(),
            stderr: "test error".to_string(),
            timed_out: false,
            force_killed: false,
            duration_ms: 150,
            metadata: HashMap::from([("key".to_string(), "value".to_string())]),
        };
        let json = serde_json::to_string(&result).unwrap();
        let deserialized: ProcessResult = serde_json::from_str(&json).unwrap();
        assert_eq!(result, deserialized);
    }
}
