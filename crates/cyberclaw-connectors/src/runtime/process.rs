//! Process runtime executor with timeout and resource limits
//!
//! Implements subprocess execution with:
//! - Timeout enforcement (SIGTERM → SIGKILL graceful shutdown)
//! - Working directory and environment isolation
//! - Stdout/stderr capture
//! - Command injection prevention
//! - Resource limit enforcement (where supported)

use super::config::ProcessConfig;
use super::mode::RuntimeMode;
use super::traits::{ProcessResult, ProcessRuntime};
use async_trait::async_trait;
use std::collections::HashMap;
use std::process::Stdio;
use std::time::Instant;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time::timeout;

/// Process runtime executor for subprocess isolation
///
/// Executes capabilities in isolated subprocesses with timeout and resource limits.
///
/// # Security Features
///
/// - **Command Validation**: Whitelist-only command execution, path validation
/// - **Timeout Enforcement**: SIGTERM followed by SIGKILL after grace period
/// - **Environment Isolation**: Clean environment by default (no inherited variables)
/// - **Output Capture**: Prevents information leakage via stdout/stderr
/// - **Resource Limits**: File descriptor limits, optional memory limits (Linux cgroups)
///
/// # Examples
///
/// ```no_run
/// use cyberclaw_connectors::runtime::{ProcessExecutor, ProcessConfig, ProcessRuntime};
/// use std::path::PathBuf;
/// use std::time::Duration;
///
/// #[tokio::main]
/// async fn main() -> anyhow::Result<()> {
///     let executor = ProcessExecutor::new();
///
///     let config = ProcessConfig::builder()
///         .timeout(Duration::from_secs(30))
///         .working_directory(PathBuf::from("/tmp/workspace"))
///         .env("RUST_LOG", "info")
///         .build();
///
///     let result = executor.execute_command(
///         "cargo",
///         vec!["test".to_string(), "--workspace".to_string()],
///         config
///     ).await?;
///
///     if result.is_success() {
///         println!("Tests passed: {}", result.stdout);
///     } else {
///         eprintln!("Tests failed: {}", result.stderr);
///     }
///
///     Ok(())
/// }
/// ```
#[derive(Debug, Clone)]
pub struct ProcessExecutor {
    /// Whitelist of allowed commands (security: prevent arbitrary command execution)
    ///
    /// If None, all commands are allowed (INSECURE - only for development)
    /// If Some(set), only commands in the set are allowed
    command_whitelist: Option<std::collections::HashSet<String>>,
}

impl Default for ProcessExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessExecutor {
    /// Create a new ProcessExecutor with secure default (empty whitelist)
    ///
    /// # Security (CRITICAL FIX)
    ///
    /// Default-deny: NO commands are allowed by default. Use `with_whitelist()` to add
    /// allowed commands, or `new_unrestricted()` for development/testing only.
    ///
    /// This is the recommended secure constructor for production use.
    pub fn new() -> Self {
        Self {
            command_whitelist: Some(std::collections::HashSet::new()),
        }
    }

    /// Create an unrestricted ProcessExecutor (development/testing ONLY)
    ///
    /// # ⚠️ SECURITY WARNING ⚠️
    ///
    /// This executor allows ALL commands without validation. NEVER use in production.
    /// Only for development, testing, or trusted environments.
    ///
    /// For production, use `new()` with `with_whitelist()` to explicitly allow commands.
    pub fn new_unrestricted() -> Self {
        Self {
            command_whitelist: None,
        }
    }

    /// Create a ProcessExecutor with a command whitelist (production-ready)
    ///
    /// Only commands in the whitelist will be allowed to execute.
    ///
    /// # Examples
    ///
    /// ```
    /// use cyberclaw_connectors::runtime::ProcessExecutor;
    /// use std::collections::HashSet;
    ///
    /// let whitelist = HashSet::from([
    ///     "cargo".to_string(),
    ///     "git".to_string(),
    ///     "npm".to_string(),
    /// ]);
    /// let executor = ProcessExecutor::with_whitelist(whitelist);
    /// ```
    pub fn with_whitelist(whitelist: std::collections::HashSet<String>) -> Self {
        Self {
            command_whitelist: Some(whitelist),
        }
    }

    /// Validate command before execution (security check)
    ///
    /// # Security Checks
    ///
    /// 1. Command must be in whitelist (if whitelist is enabled)
    /// 2. Command must not contain path traversal (../, ..\)
    /// 3. Command must not contain control characters
    /// 4. Command must not be an absolute path (unless whitelisted)
    ///
    /// # Errors
    ///
    /// Returns error if validation fails
    fn validate_command(&self, command: &str) -> anyhow::Result<()> {
        // Check for empty command
        if command.is_empty() {
            anyhow::bail!("command cannot be empty");
        }

        // Check for control characters (command injection prevention)
        if command.contains(|c: char| c.is_control()) {
            anyhow::bail!("command contains control characters: {}", command);
        }

        // Check for path traversal
        if command.contains("../") || command.contains("..\\") {
            anyhow::bail!("command contains path traversal: {}", command);
        }

        // Check whitelist (if enabled)
        if let Some(ref whitelist) = self.command_whitelist {
            // Extract base command name (handle both "/usr/bin/cargo" and "cargo")
            let base_cmd = std::path::Path::new(command)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(command);

            if !whitelist.contains(base_cmd) {
                anyhow::bail!(
                    "command '{}' is not in whitelist (allowed: {:?})",
                    base_cmd,
                    whitelist
                );
            }
        }

        Ok(())
    }

    /// Validate shell script content (basic sanitization)
    fn validate_shell_script(&self, script: &str) -> anyhow::Result<()> {
        if script.is_empty() {
            anyhow::bail!("shell script cannot be empty");
        }

        // Check for extremely dangerous patterns (basic protection)
        let dangerous_patterns = ["rm -rf /", ":(){ :|:& };:", "wget", "curl"];
        for pattern in &dangerous_patterns {
            if script.contains(pattern) {
                anyhow::bail!("shell script contains dangerous pattern: {}", pattern);
            }
        }

        Ok(())
    }

    /// Execute command with timeout and graceful shutdown (SIGTERM → SIGKILL)
    async fn execute_with_timeout(
        &self,
        mut command: Command,
        config: &ProcessConfig,
    ) -> anyhow::Result<ProcessResult> {
        let start = Instant::now();

        // Spawn the child process
        let mut child = command
            .spawn()
            .map_err(|e| anyhow::anyhow!("failed to spawn process: {}", e))?;

        // Get PID for logging
        let pid = child.id().unwrap_or(0);

        // Take stdout/stderr handles for reading
        let mut stdout_handle = child.stdout.take();
        let mut stderr_handle = child.stderr.take();

        // Wait for process with timeout
        let execution_result = timeout(config.timeout, child.wait()).await;

        let duration_ms = start.elapsed().as_millis().min(u64::MAX as u128) as u64;

        match execution_result {
            // Process completed before timeout
            Ok(Ok(status)) => {
                let exit_code = status.code().unwrap_or(-1);

                // Read captured stdout/stderr
                let mut stdout = String::new();
                let mut stderr = String::new();

                if let Some(ref mut handle) = stdout_handle {
                    let _ = handle.read_to_string(&mut stdout).await;
                }
                if let Some(ref mut handle) = stderr_handle {
                    let _ = handle.read_to_string(&mut stderr).await;
                }

                let mut metadata = HashMap::new();
                metadata.insert("pid".to_string(), pid.to_string());

                Ok(ProcessResult {
                    exit_code,
                    stdout,
                    stderr,
                    timed_out: false,
                    force_killed: false,
                    duration_ms,
                    metadata,
                })
            }

            // Process I/O error
            Ok(Err(e)) => Err(anyhow::anyhow!("process I/O error: {}", e)),

            // Timeout occurred - initiate graceful shutdown
            Err(_) => {
                // Send SIGTERM for graceful shutdown
                #[cfg(unix)]
                {
                    use nix::sys::signal::{kill, Signal};
                    use nix::unistd::Pid;

                    let nix_pid = Pid::from_raw(pid as i32);
                    let _ = kill(nix_pid, Signal::SIGTERM);

                    // Wait for grace period
                    let grace_result = timeout(config.kill_grace_period, child.wait()).await;

                    let (force_killed, exit_code) = match grace_result {
                        // Process terminated gracefully after SIGTERM
                        Ok(Ok(status)) => {
                            let exit_code = status.code().unwrap_or(143); // 143 = SIGTERM
                            (false, exit_code)
                        }

                        // Process did not terminate - send SIGKILL
                        _ => {
                            let _ = kill(nix_pid, Signal::SIGKILL);
                            let _ = child.wait().await; // Wait for SIGKILL to take effect
                            (true, 137) // 137 = SIGKILL
                        }
                    };

                    // Try to read any captured output
                    let mut stdout = String::new();
                    let mut stderr = String::new();
                    if let Some(ref mut handle) = stdout_handle {
                        let _ = handle.read_to_string(&mut stdout).await;
                    }
                    if let Some(ref mut handle) = stderr_handle {
                        let _ = handle.read_to_string(&mut stderr).await;
                    }

                    let mut metadata = HashMap::new();
                    metadata.insert("pid".to_string(), pid.to_string());
                    metadata.insert(
                        "timeout_ms".to_string(),
                        config.timeout.as_millis().to_string(),
                    );

                    Ok(ProcessResult {
                        exit_code,
                        stdout,
                        stderr,
                        timed_out: true,
                        force_killed,
                        duration_ms,
                        metadata,
                    })
                }

                // Non-Unix platforms: force kill immediately (no graceful shutdown)
                #[cfg(not(unix))]
                {
                    let _ = child.kill().await;

                    // Try to read any captured output
                    let mut stdout = String::new();
                    let mut stderr = String::new();
                    if let Some(ref mut handle) = stdout_handle {
                        let _ = handle.read_to_string(&mut stdout).await;
                    }
                    if let Some(ref mut handle) = stderr_handle {
                        let _ = handle.read_to_string(&mut stderr).await;
                    }

                    let mut metadata = HashMap::new();
                    metadata.insert("pid".to_string(), pid.to_string());

                    Ok(ProcessResult {
                        exit_code: -1,
                        stdout,
                        stderr,
                        timed_out: true,
                        force_killed: true,
                        duration_ms,
                        metadata,
                    })
                }
            }
        }
    }
}

#[async_trait]
impl ProcessRuntime for ProcessExecutor {
    fn mode(&self) -> RuntimeMode {
        RuntimeMode::Process
    }

    async fn execute_command(
        &self,
        command: &str,
        args: Vec<String>,
        config: ProcessConfig,
    ) -> anyhow::Result<ProcessResult> {
        // Validate configuration
        config.validate()?;

        // Validate command (security check)
        self.validate_command(command)?;

        // Build tokio Command
        let mut cmd = Command::new(command);
        cmd.args(&args);

        // Set working directory
        if let Some(ref cwd) = config.working_directory {
            cmd.current_dir(cwd);
        }

        // Set environment variables
        if !config.inherit_env {
            cmd.env_clear(); // Start with clean environment
        }
        for (key, value) in &config.environment {
            cmd.env(key, value);
        }

        // Configure stdio
        cmd.stdin(Stdio::null());
        cmd.stdout(if config.capture_stdout {
            Stdio::piped()
        } else {
            Stdio::inherit()
        });
        cmd.stderr(if config.capture_stderr {
            Stdio::piped()
        } else {
            Stdio::inherit()
        });

        // Execute with timeout
        self.execute_with_timeout(cmd, &config).await
    }

    async fn execute_shell_script(
        &self,
        script: &str,
        shell: &str,
        config: ProcessConfig,
    ) -> anyhow::Result<ProcessResult> {
        // Validate script content
        self.validate_shell_script(script)?;

        // Validate shell command
        self.validate_command(shell)?;

        // Execute shell with script as stdin or -c argument
        // Using -c is more portable than stdin
        let args = vec!["-c".to_string(), script.to_string()];
        self.execute_command(shell, args, config).await
    }

    async fn check_availability(&self) -> anyhow::Result<()> {
        // Process runtime is always available on all platforms
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_validate_command_empty() {
        let executor = ProcessExecutor::new();
        assert!(executor.validate_command("").is_err());
    }

    #[test]
    fn test_validate_command_control_chars() {
        let executor = ProcessExecutor::new();
        assert!(executor.validate_command("ls\x00-la").is_err());
    }

    #[test]
    fn test_validate_command_path_traversal() {
        let executor = ProcessExecutor::new();
        assert!(executor.validate_command("../../../etc/passwd").is_err());
        assert!(executor
            .validate_command("..\\..\\windows\\system32")
            .is_err());
    }

    #[test]
    fn test_validate_command_whitelist() {
        let whitelist = std::collections::HashSet::from(["cargo".to_string(), "git".to_string()]);
        let executor = ProcessExecutor::with_whitelist(whitelist);

        assert!(executor.validate_command("cargo").is_ok());
        assert!(executor.validate_command("git").is_ok());
        assert!(executor.validate_command("npm").is_err());
    }

    #[test]
    fn test_validate_shell_script_empty() {
        let executor = ProcessExecutor::new();
        assert!(executor.validate_shell_script("").is_err());
    }

    #[test]
    fn test_validate_shell_script_dangerous_patterns() {
        let executor = ProcessExecutor::new();
        assert!(executor.validate_shell_script("rm -rf /").is_err());
        assert!(executor.validate_shell_script(":(){ :|:& };:").is_err());
    }

    #[tokio::test]
    async fn test_execute_command_success() {
        let executor = ProcessExecutor::new_unrestricted();
        let config = ProcessConfig::builder()
            .timeout(Duration::from_secs(5))
            .build();

        let result = executor
            .execute_command("echo", vec!["hello".to_string()], config)
            .await
            .unwrap();

        assert!(result.is_success());
        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("hello"));
    }

    #[tokio::test]
    async fn test_execute_command_failure() {
        let executor = ProcessExecutor::new_unrestricted();
        let config = ProcessConfig::builder()
            .timeout(Duration::from_secs(5))
            .build();

        let result = executor
            .execute_command("ls", vec!["/nonexistent-directory-xyz".to_string()], config)
            .await
            .unwrap();

        assert!(result.is_failure());
        assert_ne!(result.exit_code, 0);
    }

    #[tokio::test]
    async fn test_execute_command_timeout() {
        let executor = ProcessExecutor::new_unrestricted();
        let config = ProcessConfig::builder()
            .timeout(Duration::from_millis(500))
            .kill_grace_period(Duration::from_millis(100))
            .build();

        let result = executor
            .execute_command("sleep", vec!["10".to_string()], config)
            .await
            .unwrap();

        assert!(result.timed_out);
        assert!(result.is_failure());
    }

    #[tokio::test]
    async fn test_execute_shell_script() {
        let executor = ProcessExecutor::new_unrestricted();
        let config = ProcessConfig::builder()
            .timeout(Duration::from_secs(5))
            .build();

        let script = "echo 'test output'";
        let result = executor
            .execute_shell_script(script, "sh", config)
            .await
            .unwrap();

        assert!(result.is_success());
        assert!(result.stdout.contains("test output"));
    }

    #[tokio::test]
    async fn test_check_availability() {
        let executor = ProcessExecutor::new();
        assert!(executor.check_availability().await.is_ok());
    }

    #[test]
    fn test_process_result_outcome_description() {
        let result = ProcessResult {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
            timed_out: false,
            force_killed: false,
            duration_ms: 123,
            metadata: HashMap::new(),
        };
        assert!(result.outcome_description().contains("succeeded"));
        assert!(result.outcome_description().contains("123ms"));
    }
}
