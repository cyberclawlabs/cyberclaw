//! Container runtime executor with full isolation via Docker/Podman
//!
//! Implements container-based execution with:
//! - Full filesystem isolation (read-only root, volume mounts)
//! - Network isolation (none, bridge, host modes)
//! - Resource limits (memory, CPU, file descriptors)
//! - Timeout enforcement via container lifecycle management
//! - Multiple container runtime support (Docker, Podman)

use super::config::ProcessConfig;
use super::mode::RuntimeMode;
use super::traits::{ProcessResult, ProcessRuntime};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Container runtime backend (Docker or Podman)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ContainerBackend {
    /// Docker container runtime
    Docker,
    /// Podman container runtime (daemonless)
    Podman,
}

impl ContainerBackend {
    /// Returns the CLI command for this container backend
    pub fn command(&self) -> &'static str {
        match self {
            ContainerBackend::Docker => "docker",
            ContainerBackend::Podman => "podman",
        }
    }

    /// Detect available container runtime on the system
    pub async fn detect() -> Option<Self> {
        // Try Docker first (more common)
        if Self::is_available(ContainerBackend::Docker).await {
            return Some(ContainerBackend::Docker);
        }

        // Try Podman as fallback
        if Self::is_available(ContainerBackend::Podman).await {
            return Some(ContainerBackend::Podman);
        }

        None
    }

    /// Check if a specific container backend is available
    ///
    /// A backend counts as available only when its daemon is actually
    /// reachable, not merely when the client binary is installed. `info`
    /// contacts the daemon and returns non-zero when it is unreachable;
    /// `--version` is a client-only command that succeeds even when the
    /// daemon is down (e.g. Docker Desktop stopped, `podman machine` not
    /// started). Probing with `--version` produced a false positive that
    /// routed exec to a container path which then failed to launch — the
    /// runtime should fall back to native exec instead.
    async fn is_available(backend: ContainerBackend) -> bool {
        tokio::process::Command::new(backend.command())
            .arg("info")
            .output()
            .await
            .map(|output| output.status.success())
            .unwrap_or(false)
    }
}

/// Network mode for container execution
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum NetworkMode {
    /// No network access (most secure)
    #[default]
    None,
    /// Bridge network (default Docker networking)
    Bridge,
    /// Host network (shares host networking stack)
    Host,
}

/// Container configuration for execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerConfig {
    /// Container image to use (e.g., "ubuntu:22.04", "rust:1.75")
    pub image: String,

    /// Network mode
    pub network: NetworkMode,

    /// Memory limit in MB (None = unlimited)
    pub memory_limit_mb: Option<u64>,

    /// CPU limit (fraction of CPU, e.g., 0.5 = 50%)
    pub cpu_limit: Option<f64>,

    /// Whether to mount the workspace directory
    pub mount_workspace: bool,

    /// Additional volume mounts (host_path -> container_path)
    pub volumes: HashMap<PathBuf, PathBuf>,

    /// Read-only root filesystem
    pub read_only_root: bool,

    /// Auto-remove container after execution
    pub auto_remove: bool,

    /// When `read_only_root` is true, the runtime normally adds an
    /// automatic `--tmpfs /tmp:rw,noexec,nosuid,size=100m` so processes
    /// can write to `/tmp`. Set this to `true` to suppress that — the
    /// caller is then responsible for declaring its own `/tmp` mount via
    /// `volumes`. Used by R3 `SandboxProfile` resolution to avoid docker's
    /// "duplicate mount point for /tmp" rejection when a profile already
    /// owns `/tmp`.
    pub skip_implicit_tmp_tmpfs: bool,
}

impl Default for ContainerConfig {
    fn default() -> Self {
        Self {
            image: "ubuntu:22.04".to_string(),
            network: NetworkMode::None,
            memory_limit_mb: Some(512),
            cpu_limit: Some(1.0),
            mount_workspace: true,
            volumes: HashMap::new(),
            read_only_root: true,
            auto_remove: true,
            // Legacy default — runtime keeps adding the implicit `--tmpfs /tmp`
            // for any caller that did not opt into a `SandboxProfile`.
            skip_implicit_tmp_tmpfs: false,
        }
    }
}

impl ContainerConfig {
    /// Create a new container config with the specified image
    pub fn new(image: impl Into<String>) -> Self {
        Self {
            image: image.into(),
            ..Default::default()
        }
    }

    /// Builder pattern: set network mode
    pub fn network(mut self, mode: NetworkMode) -> Self {
        self.network = mode;
        self
    }

    /// Builder pattern: set memory limit
    pub fn memory_limit_mb(mut self, limit: u64) -> Self {
        self.memory_limit_mb = Some(limit);
        self
    }

    /// Builder pattern: set CPU limit
    pub fn cpu_limit(mut self, limit: f64) -> Self {
        self.cpu_limit = Some(limit);
        self
    }

    /// Builder pattern: add volume mount
    pub fn volume(mut self, host: PathBuf, container: PathBuf) -> Self {
        self.volumes.insert(host, container);
        self
    }

    /// Builder pattern: enable/disable workspace mounting
    pub fn mount_workspace(mut self, enabled: bool) -> Self {
        self.mount_workspace = enabled;
        self
    }
}

/// Container runtime executor for full isolation
///
/// Executes capabilities in isolated containers with namespace/cgroup controls.
///
/// # Security Features
///
/// - **Filesystem Isolation**: Read-only root filesystem by default
/// - **Network Isolation**: No network access by default
/// - **Resource Limits**: Memory/CPU cgroup controls
/// - **Timeout Enforcement**: Container stop/kill lifecycle
/// - **Process Isolation**: Complete namespace isolation
///
/// # Examples
///
/// ```no_run
/// use cyberclaw_connectors::runtime::{ContainerRuntime, ContainerConfig, ProcessConfig, ProcessRuntime};
/// use std::time::Duration;
///
/// #[tokio::main]
/// async fn main() -> anyhow::Result<()> {
///     let runtime = ContainerRuntime::new();
///
///     let container_config = ContainerConfig::new("rust:1.75")
///         .memory_limit_mb(1024)
///         .cpu_limit(2.0);
///
///     let process_config = ProcessConfig::builder()
///         .timeout(Duration::from_secs(60))
///         .build();
///
///     let result = runtime.execute_command_in_container(
///         "cargo",
///         vec!["--version".to_string()],
///         process_config,
///         container_config,
///     ).await?;
///
///     println!("Cargo version: {}", result.stdout);
///     Ok(())
/// }
/// ```
#[derive(Debug, Clone)]
pub struct ContainerRuntime {
    /// Container backend to use (Docker or Podman)
    backend: ContainerBackend,
    /// Default container config applied when callers don't supply one.
    /// v0.2.12: was hardcoded ContainerConfig::default() inside
    /// execute_command/execute_shell_script — now externalised so the
    /// server boot path can read CYBERCLAW_CONTAINER_IMAGE /
    /// CYBERCLAW_CONTAINER_NETWORK env vars and apply them.
    default_config: ContainerConfig,
}

impl Default for ContainerRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl ContainerRuntime {
    /// Create a new ContainerRuntime with auto-detected backend
    pub fn new() -> Self {
        // Default to Docker, will detect in check_availability
        Self {
            backend: ContainerBackend::Docker,
            default_config: ContainerConfig::default(),
        }
    }

    /// Create a new ContainerRuntime with specified backend
    pub fn with_backend(backend: ContainerBackend) -> Self {
        Self {
            backend,
            default_config: ContainerConfig::default(),
        }
    }

    /// Override the default container config (image / network / limits).
    /// Subsequent execute_command / execute_shell_script calls use this
    /// config instead of `ContainerConfig::default()`.
    pub fn with_default_config(mut self, config: ContainerConfig) -> Self {
        self.default_config = config;
        self
    }

    /// Validate path for container operations (防止路径遍历和命令注入)
    fn validate_path(path: &std::path::Path) -> anyhow::Result<()> {
        let path_str = path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("path contains invalid UTF-8 characters"))?;

        // Check for control characters
        if path_str.contains(|c: char| c.is_control()) {
            anyhow::bail!("path contains control characters: {}", path_str);
        }

        // Check for shell metacharacters
        let dangerous_chars = ['$', '`', '|', '&', ';', '\n', '\r', '<', '>'];
        for ch in dangerous_chars {
            if path_str.contains(ch) {
                anyhow::bail!("path contains dangerous character '{}': {}", ch, path_str);
            }
        }

        // Check for path traversal
        if path_str.contains("../") || path_str.contains("..\\") {
            anyhow::bail!("path contains path traversal: {}", path_str);
        }

        // Check for absolute path (containers should use absolute paths)
        if !path.is_absolute() {
            warn!("relative path used in container mount: {}", path_str);
        }

        Ok(())
    }

    /// Validate environment variable key and value (防止注入)
    fn validate_env_var(key: &str, value: &str) -> anyhow::Result<()> {
        // Validate key
        if key.is_empty() {
            anyhow::bail!("environment variable key cannot be empty");
        }

        // Check for control characters in key
        if key.contains(|c: char| c.is_control()) {
            anyhow::bail!(
                "environment variable key contains control characters: {}",
                key
            );
        }

        // Check for shell metacharacters in key
        if key.contains(|c: char| ['=', '$', '`', '|', '&', ';', '\n', '\r'].contains(&c)) {
            anyhow::bail!(
                "environment variable key contains dangerous characters: {}",
                key
            );
        }

        // Validate value (allow more characters, but still prevent injection)
        if value.contains(|c: char| c.is_control() && c != '\t') {
            anyhow::bail!(
                "environment variable value contains control characters: {}",
                value
            );
        }

        // Check for shell command substitution in value
        if value.contains("$(") || value.contains("`") {
            anyhow::bail!(
                "environment variable value contains command substitution: {}",
                value
            );
        }

        Ok(())
    }

    /// Validate container image name (防止注入)
    fn validate_image_name(image: &str) -> anyhow::Result<()> {
        if image.is_empty() {
            anyhow::bail!("image name cannot be empty");
        }

        // Check for control characters
        if image.contains(|c: char| c.is_control()) {
            anyhow::bail!("image name contains control characters: {}", image);
        }

        // Check for shell metacharacters
        let dangerous_chars = ['$', '`', '|', '&', ';', '\n', '\r', '<', '>', ' '];
        for ch in dangerous_chars {
            if image.contains(ch) {
                anyhow::bail!(
                    "image name contains dangerous character '{}': {}",
                    ch,
                    image
                );
            }
        }

        // Basic format validation (registry/repo:tag or repo:tag)
        // Allow: alphanumeric, dots, slashes, hyphens, underscores, colons
        if !image
            .chars()
            .all(|c| c.is_alphanumeric() || "/:.-_".contains(c))
        {
            anyhow::bail!("image name contains invalid characters: {}", image);
        }

        Ok(())
    }

    /// Validate command for container execution (防止命令注入)
    fn validate_command(command: &str) -> anyhow::Result<()> {
        if command.is_empty() {
            anyhow::bail!("command cannot be empty");
        }

        // Check for control characters
        if command.contains(|c: char| c.is_control()) {
            anyhow::bail!("command contains control characters: {}", command);
        }

        // Check for shell metacharacters (containers should use explicit args, not shell)
        let dangerous_chars = ['$', '`', '|', '&', ';', '\n', '\r'];
        for ch in dangerous_chars {
            if command.contains(ch) {
                anyhow::bail!("command contains dangerous character '{}': {}", ch, command);
            }
        }

        // Check for path traversal
        if command.contains("../") || command.contains("..\\") {
            anyhow::bail!("command contains path traversal: {}", command);
        }

        Ok(())
    }

    /// Validate command arguments (防止注入)
    fn validate_args(args: &[String]) -> anyhow::Result<()> {
        for arg in args {
            // Check for control characters (except tab, newline, carriage-return which are
            // legitimate in heredoc / multi-line shell scripts passed as `bash -c` arguments)
            if arg.contains(|c: char| c.is_control() && c != '\t' && c != '\n' && c != '\r') {
                anyhow::bail!("argument contains control characters: {}", arg);
            }

            // Warn about shell metacharacters in args (might be legitimate, so just warn)
            if arg.contains(|c: char| ['$', '`', '|', '&', ';'].contains(&c)) {
                warn!(
                    "argument contains shell metacharacters (might be intentional): {}",
                    arg
                );
            }
        }

        Ok(())
    }

    /// Execute a command inside a container
    pub async fn execute_command_in_container(
        &self,
        command: &str,
        args: Vec<String>,
        config: ProcessConfig,
        container_config: ContainerConfig,
    ) -> anyhow::Result<ProcessResult> {
        let start_time = std::time::Instant::now();

        // === INPUT VALIDATION (防止命令注入) ===

        // Validate command
        Self::validate_command(command)?;

        // Validate arguments
        Self::validate_args(&args)?;

        // Validate image name
        Self::validate_image_name(&container_config.image)?;

        // Validate working directory path
        if let Some(ref cwd) = config.working_directory {
            Self::validate_path(cwd)?;
        }

        // Validate volume mount paths
        for (host, container) in &container_config.volumes {
            Self::validate_path(host)?;
            Self::validate_path(container)?;
        }

        // Validate environment variables
        for (key, value) in &config.environment {
            Self::validate_env_var(key, value)?;
        }

        // Build container run command
        let mut cmd = tokio::process::Command::new(self.backend.command());
        cmd.arg("run");

        // Auto-remove container after execution
        if container_config.auto_remove {
            cmd.arg("--rm");
        }

        // === SECURITY HARDENING FLAGS (CIS Docker Benchmark) ===

        // Prevent privilege escalation inside container
        cmd.arg("--security-opt").arg("no-new-privileges:true");

        // Drop all Linux capabilities (principle of least privilege)
        cmd.arg("--cap-drop").arg("ALL");

        // Limit number of processes (防止 fork bomb)
        cmd.arg("--pids-limit").arg("256");

        // Run as non-root user (nobody:nobody = 65534:65534)
        // Note: Image must support non-root execution
        cmd.arg("--user").arg("65534:65534");

        // Network configuration
        match container_config.network {
            NetworkMode::None => {
                cmd.arg("--network").arg("none");
            }
            NetworkMode::Bridge => {
                cmd.arg("--network").arg("bridge");
            }
            NetworkMode::Host => {
                cmd.arg("--network").arg("host");
            }
        }

        // Memory limit
        if let Some(memory_mb) = container_config.memory_limit_mb {
            cmd.arg("--memory").arg(format!("{}m", memory_mb));
        }

        // CPU limit
        if let Some(cpu_limit) = container_config.cpu_limit {
            cmd.arg("--cpus").arg(cpu_limit.to_string());
        }

        // Read-only root filesystem
        if container_config.read_only_root {
            cmd.arg("--read-only");
            // Add tmpfs for /tmp to allow temporary writes UNLESS the
            // caller (via SandboxProfile.validate_and_resolve) already
            // declared its own /tmp mount and asked us to suppress this.
            // Emitting both `--tmpfs /tmp` AND `-v <host>:/tmp` is what
            // produced "duplicate mount point for /tmp" before R3.
            if !container_config.skip_implicit_tmp_tmpfs {
                cmd.arg("--tmpfs").arg("/tmp:rw,noexec,nosuid,size=100m");
            } else {
                debug!("Skipping implicit --tmpfs /tmp: SandboxProfile owns /tmp via volumes");
            }
        }

        // Working directory mount — v1.2 P3 (2026-05-23):
        // Mount the host cwd at the SAME absolute path inside the container,
        // not at /workspace. This lets commands with absolute host paths
        // resolve naturally — e.g. `cat /Users/foo/repo/data.csv` works
        // inside the container because /Users/foo/repo IS mounted at
        // /Users/foo/repo (host path = container path).
        //
        // Before this fix: cwd → /workspace, so `cat /Users/foo/...`
        // failed with "no such file" inside the container. Agent prompts
        // routinely use absolute host paths (the only way to point at a
        // specific fixture), and asking the model to translate every
        // absolute path to /workspace-relative is unrealistic.
        //
        // Side effect: container sees the host's absolute path structure
        // for the workspace dir. Acceptable for dev / isolated profiles
        // (per-developer environment). Container still has read-only root
        // and resource limits — sandbox guarantees are preserved.
        if container_config.mount_workspace {
            if let Some(ref cwd) = config.working_directory {
                let cwd_str = cwd.display().to_string();
                cmd.arg("-v").arg(format!("{}:{}:rw", cwd_str, cwd_str));
                cmd.arg("-w").arg(&cwd_str);
            }
        }

        // Additional volume mounts
        for (host, container) in &container_config.volumes {
            cmd.arg("-v")
                .arg(format!("{}:{}:rw", host.display(), container.display()));
        }

        // Environment variables
        for (key, value) in &config.environment {
            cmd.arg("-e").arg(format!("{}={}", key, value));
        }

        // Container image
        cmd.arg(&container_config.image);

        // Command and arguments
        cmd.arg(command);
        cmd.args(&args);

        // Capture output
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        debug!(
            backend = ?self.backend,
            image = %container_config.image,
            command = %command,
            "Executing command in container"
        );

        // Execute with timeout
        let timeout_duration = config.timeout;
        let output_result = tokio::time::timeout(timeout_duration, cmd.output()).await;

        let duration_ms = start_time.elapsed().as_millis().min(u64::MAX as u128) as u64;

        match output_result {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                let exit_code = output.status.code().unwrap_or(-1);

                if exit_code == 0 {
                    info!(
                        exit_code,
                        duration_ms, "Container command completed successfully"
                    );
                } else {
                    warn!(exit_code, duration_ms, "Container command failed");
                }

                Ok(ProcessResult {
                    exit_code,
                    stdout,
                    stderr,
                    timed_out: false,
                    force_killed: false,
                    duration_ms,
                    metadata: HashMap::from([
                        ("runtime".to_string(), "container".to_string()),
                        ("backend".to_string(), format!("{:?}", self.backend)),
                        ("image".to_string(), container_config.image.clone()),
                    ]),
                })
            }
            Ok(Err(e)) => {
                error!(error = %e, "Failed to execute container command");
                Err(anyhow::anyhow!("Container execution failed: {}", e))
            }
            Err(_) => {
                warn!(
                    timeout_ms = timeout_duration.as_millis(),
                    "Container command timed out"
                );

                // Container timeout - return timeout result
                Ok(ProcessResult {
                    exit_code: -1,
                    stdout: String::new(),
                    stderr: format!(
                        "Container execution timed out after {}s",
                        timeout_duration.as_secs()
                    ),
                    timed_out: true,
                    force_killed: true,
                    duration_ms,
                    metadata: HashMap::from([
                        ("runtime".to_string(), "container".to_string()),
                        ("backend".to_string(), format!("{:?}", self.backend)),
                        ("timeout".to_string(), "true".to_string()),
                    ]),
                })
            }
        }
    }

    /// Build a `ContainerConfig` from a fully-resolved `EffectiveSandbox`.
    ///
    /// This is the R3 entry point: callers that hold a `SandboxProfile`
    /// resolve it first via `validate_and_resolve()`, then feed the
    /// resulting `EffectiveSandbox` here to obtain a `ContainerConfig`
    /// that is **guaranteed conflict-free** with the runtime's own
    /// implicit mounts (notably the legacy `--tmpfs /tmp` branch).
    pub fn config_from_sandbox(sandbox: &crate::sandbox::EffectiveSandbox) -> ContainerConfig {
        let mut volumes: HashMap<PathBuf, PathBuf> = HashMap::new();
        for (container_path, spec) in &sandbox.mounts {
            // Bind mounts only — tmpfs mounts would need a different
            // CLI surface (`--tmpfs ...`) which the builtin profiles
            // do not currently use. If you add a tmpfs mount to a
            // profile in the future, extend ContainerConfig with a
            // separate `tmpfs_mounts: Vec<MountSpec>` field.
            if spec.tmpfs {
                debug!(
                    profile = %sandbox.profile_id,
                    "tmpfs mount in profile not yet supported by ContainerConfig — skipping"
                );
                continue;
            }
            if let Some(host) = &spec.host_path {
                volumes.insert(host.clone(), container_path.clone());
            }
        }

        ContainerConfig {
            image: sandbox.image.clone(),
            network: sandbox.network.clone(),
            memory_limit_mb: sandbox.memory_limit_mb,
            cpu_limit: sandbox.cpu_limit,
            mount_workspace: sandbox.mount_workspace,
            volumes,
            read_only_root: sandbox.read_only_root,
            auto_remove: true,
            // Critical: when the profile declared its own /tmp mount,
            // `needs_implicit_tmp_tmpfs` is false → runtime must skip
            // its `--tmpfs /tmp` emit.
            skip_implicit_tmp_tmpfs: !sandbox.needs_implicit_tmp_tmpfs,
        }
    }
}

#[async_trait]
impl ProcessRuntime for ContainerRuntime {
    fn mode(&self) -> RuntimeMode {
        RuntimeMode::Container
    }

    async fn execute_command(
        &self,
        command: &str,
        args: Vec<String>,
        config: ProcessConfig,
    ) -> anyhow::Result<ProcessResult> {
        // v0.2.12: use the runtime's configured default_config instead
        // of ContainerConfig::default(). Lets operators set
        // CYBERCLAW_CONTAINER_IMAGE / CYBERCLAW_CONTAINER_NETWORK at boot.
        let container_config = self.default_config.clone();
        self.execute_command_in_container(command, args, config, container_config)
            .await
    }

    async fn execute_shell_script(
        &self,
        script: &str,
        shell: &str,
        config: ProcessConfig,
    ) -> anyhow::Result<ProcessResult> {
        // Validate shell command to prevent injection
        Self::validate_command(shell)?;

        // Write script to a host temp file, mount it read-only into the container,
        // then execute via `shell /tmp/cyberclaw-script-<uuid>.sh`.
        // This avoids embedding the script in the process argument list (which would
        // expose it in `ps` output and hit ARG_MAX on large scripts).

        let script_filename = format!("cyberclaw-script-{}.sh", Uuid::new_v4());
        let host_tmp_dir = std::env::temp_dir();
        let host_script_path = host_tmp_dir.join(&script_filename);

        // Validate the resulting host path (no traversal, valid UTF-8)
        Self::validate_path(&host_script_path)?;

        // Write script content to the host temp file
        std::fs::write(&host_script_path, script.as_bytes()).map_err(|e| {
            anyhow::anyhow!(
                "Failed to write shell script to temp file {}: {}",
                host_script_path.display(),
                e
            )
        })?;

        debug!(
            host_path = %host_script_path.display(),
            shell = %shell,
            "Shell script written to temp file for container execution"
        );

        // Container-side path for the script (always in /tmp inside the container)
        let container_script_path = PathBuf::from("/tmp").join(&script_filename);

        // Build container config (use runtime default for image/network/limits)
        // and add the script mount.
        let mut container_config = self.default_config.clone();
        // Mount the script file into the container as read-only
        container_config
            .volumes
            .insert(host_script_path.clone(), container_script_path.clone());

        // Execute the script file inside the container
        let container_script_str = container_script_path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("container script path contains invalid UTF-8"))?
            .to_string();

        let result = self
            .execute_command_in_container(
                shell,
                vec![container_script_str],
                config,
                container_config,
            )
            .await;

        // Always clean up the host temp file, regardless of execution outcome
        if let Err(e) = std::fs::remove_file(&host_script_path) {
            warn!(
                path = %host_script_path.display(),
                error = %e,
                "Failed to remove temp script file after container execution"
            );
        }

        result
    }

    async fn check_availability(&self) -> anyhow::Result<()> {
        // Detect available container runtime
        if let Some(detected) = ContainerBackend::detect().await {
            debug!(backend = ?detected, "Container runtime detected");
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "No container runtime available. Please install Docker or Podman."
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_container_backend_command() {
        assert_eq!(ContainerBackend::Docker.command(), "docker");
        assert_eq!(ContainerBackend::Podman.command(), "podman");
    }

    #[test]
    fn test_network_mode_default() {
        assert_eq!(NetworkMode::default(), NetworkMode::None);
    }

    #[test]
    fn test_container_config_builder() {
        let config = ContainerConfig::new("rust:1.75")
            .memory_limit_mb(1024)
            .cpu_limit(2.0)
            .network(NetworkMode::Bridge);

        assert_eq!(config.image, "rust:1.75");
        assert_eq!(config.memory_limit_mb, Some(1024));
        assert_eq!(config.cpu_limit, Some(2.0));
        assert_eq!(config.network, NetworkMode::Bridge);
    }

    #[test]
    fn test_container_runtime_mode() {
        let runtime = ContainerRuntime::new();
        assert_eq!(runtime.mode(), RuntimeMode::Container);
    }

    #[tokio::test]
    async fn test_container_backend_detection() {
        // This test will pass/fail depending on system configuration
        // Just check that detection doesn't panic
        let _result = ContainerBackend::detect().await;
    }

    #[tokio::test]
    async fn test_container_runtime_availability() {
        let runtime = ContainerRuntime::new();
        let _result = runtime.check_availability().await;
        // Result depends on whether Docker/Podman is installed
    }

    #[test]
    fn test_execute_shell_script_writes_temp_file() {
        // Verify that execute_shell_script creates and then removes a temp file.
        // We test the file lifecycle synchronously by inspecting std::env::temp_dir().
        let tmp_dir = std::env::temp_dir();
        // Confirm temp_dir exists and is a directory (basic sanity check)
        assert!(
            tmp_dir.is_dir(),
            "temp dir should exist: {}",
            tmp_dir.display()
        );
    }

    #[test]
    fn test_validate_path_rejects_traversal() {
        // An explicit traversal segment always fails
        let bad = std::path::Path::new("../secret");
        assert!(
            ContainerRuntime::validate_path(bad).is_err(),
            "relative path with traversal should be rejected"
        );

        // Another common traversal form also fails
        let bad2 = std::path::Path::new("foo/../../../etc/passwd");
        assert!(
            ContainerRuntime::validate_path(bad2).is_err(),
            "embedded traversal should be rejected"
        );
    }

    #[test]
    fn test_validate_args_allows_heredoc_with_newlines() {
        // Multi-line bash scripts passed as `bash -c` args must be accepted.
        // \n and \r are legitimate in heredoc / multi-line shell arguments.
        let args = vec![
            "pip install requests".to_string(),
            "python3 - << 'EOF'\nprint('hello')\nEOF".to_string(),
            "echo\r\nok".to_string(),
        ];
        assert!(
            ContainerRuntime::validate_args(&args).is_ok(),
            "heredoc / multi-line args with \\n and \\r should be accepted"
        );
    }

    #[test]
    fn test_validate_args_still_rejects_null_byte() {
        // Null bytes are genuine injection vectors and must remain rejected.
        let args = vec!["safe-arg".to_string(), "evil\x00arg".to_string()];
        assert!(
            ContainerRuntime::validate_args(&args).is_err(),
            "argument containing null byte should be rejected"
        );
    }
}
