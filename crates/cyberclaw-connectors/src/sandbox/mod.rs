//! # Process-Isolation Sandbox Connector
//!
//! A [`Connector`] implementation that runs arbitrary commands in a lightly
//! isolated child process. The primary use case is CyberClaw's self-evolution
//! pipeline, which mutates candidate Skills and must execute untested variants
//! with a reduced blast radius.
//!
//! ## Single capability
//!
//! The connector exposes exactly one capability: `sandbox.execute_isolated`.
//! It runs in a fresh temporary working directory, with a scrubbed environment
//! (allowlist only), a hard timeout, and size-bounded stdout/stderr capture.
//!
//! ## Security caveat (READ BEFORE USE)
//!
//! **This module is a "reduced blast radius" boundary, NOT a hard security
//! boundary.** It is explicitly NOT suitable for running adversarial or
//! untrusted code. The following isolations are NOT applied:
//!
//! - No container / namespace isolation (no Docker, no LXC, no user namespace)
//! - No filesystem namespace isolation (no chroot, no mount namespace)
//! - No network isolation (child has full host network access)
//! - No seccomp / SELinux / AppArmor profiles
//! - No CPU / memory cgroup enforcement
//!
//! Memory limit enforcement (`memory_limit_mb`) is enforced on **Linux** via
//! `RLIMIT_AS` set in a `pre_exec` hook (see `process_sandbox.rs`). On macOS
//! and other non-Linux platforms the field remains advisory because
//! `RLIMIT_AS` there caps the entire virtual address space including
//! memory-mapped dylibs, making it impractical for normal process execution.
//!
//! For production-grade isolation of untrusted code, use Docker, Firecracker,
//! or gVisor. This module is the pragmatic minimum needed to run internally
//! produced Skill variants during evolution experiments.
//!
//! ## Risk classification
//!
//! The `sandbox.execute_isolated` capability is [`RiskLevel::High`] and has
//! [`CapabilityEffect::Execute`] plus [`CapabilityEffect::Write`] (it creates
//! a tempdir and may spawn arbitrary processes). Governance layers MUST treat
//! it as if it were `cmd.exec`.

mod process_sandbox;

use crate::types::{
    CapabilityExecutionRequest, CapabilityExecutionResult, Connector,
    ExecutionStatus as ConnectorExecutionStatus,
};
use cyberclaw_core::manifests::{CapabilityContract, ConnectorRuntime};
use cyberclaw_core::prelude::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use tracing::{debug, error, info};

pub use process_sandbox::{run_sandboxed, SandboxRequest, SandboxResponse, MAX_OUTPUT_BYTES};

/// Default env keys that may pass through to the child process.
///
/// Anything outside this list is scrubbed before the child starts, even if
/// explicitly set in the request. The request-level `env` map is merged on top
/// of the filtered inherited env, but its keys are also filtered against the
/// allowlist.
pub const DEFAULT_ENV_ALLOWLIST: &[&str] = &["PATH", "LANG", "LC_ALL", "HOME"];

/// Default timeout when request doesn't specify one.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Default (advisory) memory limit.
pub const DEFAULT_MEMORY_LIMIT_MB: u64 = 512;

/// Process-isolation sandbox connector.
///
/// Exposes the single capability `sandbox.execute_isolated`.
///
/// See module-level docs for the security model and limitations.
#[derive(Debug, Clone)]
pub struct SandboxConnector {
    id: ConnectorId,
    /// Default timeout applied when the request does not override.
    default_timeout: Duration,
    /// Default memory limit. **Currently advisory — not enforced.** See TODO.
    default_memory_limit_mb: u64,
    /// Parent directory under which per-request tempdirs are created.
    /// `None` = use system default (`std::env::temp_dir()`).
    tempdir_root: Option<PathBuf>,
    /// Environment variable keys that may be passed through to the child.
    env_allowlist: Vec<String>,
}

impl Default for SandboxConnector {
    fn default() -> Self {
        Self::new()
    }
}

impl SandboxConnector {
    /// Create a new sandbox connector with sensible defaults.
    pub fn new() -> Self {
        let id = ConnectorId::from_string("sandbox".to_string())
            .expect("connector id 'sandbox' must be valid");
        Self {
            id,
            default_timeout: DEFAULT_TIMEOUT,
            default_memory_limit_mb: DEFAULT_MEMORY_LIMIT_MB,
            tempdir_root: None,
            env_allowlist: DEFAULT_ENV_ALLOWLIST
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }

    /// Override the default timeout for sandboxed commands.
    pub fn with_default_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    /// Override the advisory memory limit (in MB).
    pub fn with_default_memory_limit_mb(mut self, mb: u64) -> Self {
        self.default_memory_limit_mb = mb;
        self
    }

    /// Pin tempdirs to a specific parent directory (else system temp dir).
    pub fn with_tempdir_root(mut self, root: PathBuf) -> Self {
        self.tempdir_root = Some(root);
        self
    }

    /// Replace the env allowlist entirely.
    pub fn with_env_allowlist<I, S>(mut self, keys: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.env_allowlist = keys.into_iter().map(Into::into).collect();
        self
    }

    fn build_capabilities() -> Vec<CapabilityContract> {
        vec![CapabilityContract {
            id: "sandbox.execute_isolated".to_string(),
            title: "Execute Command in Process Sandbox".to_string(),
            description: Some(
                "Run a command in a child process with a tempdir workdir, \
                 scrubbed environment, and timeout. NOT a hard security \
                 boundary — see module docs."
                    .to_string(),
            ),
            input_schema: "SandboxRequest".to_string(),
            output_schema: "SandboxResponse".to_string(),
            risk: RiskLevel::High,
            effects: vec![CapabilityEffect::Execute, CapabilityEffect::Write],
            placement: None,
            timeouts: CapabilityTimeouts {
                request_ms: Some(30_000),
            },
        }]
    }
}

/// JSON input schema for `sandbox.execute_isolated`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SandboxInput {
    /// Program to invoke (looked up via `PATH` in the filtered env).
    pub command: String,
    /// CLI arguments passed verbatim (no shell).
    #[serde(default)]
    pub args: Vec<String>,
    /// Optional stdin data to pipe into the child.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdin: Option<String>,
    /// Override the connector's default timeout.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// Override the connector's default (advisory) memory limit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_limit_mb: Option<u64>,
    /// Extra env to add (keys still filtered through allowlist).
    #[serde(default)]
    pub env: HashMap<String, String>,
}

#[async_trait::async_trait]
impl Connector for SandboxConnector {
    fn id(&self) -> &ConnectorId {
        &self.id
    }

    fn runtime(&self) -> ConnectorRuntime {
        ConnectorRuntime::Process
    }

    fn capabilities(&self) -> Vec<CapabilityContract> {
        Self::build_capabilities()
    }

    async fn execute(
        &self,
        request: CapabilityExecutionRequest,
    ) -> anyhow::Result<CapabilityExecutionResult> {
        debug!(
            "SandboxConnector executing capability {} for execution {}",
            request.capability_id, request.execution_id
        );

        if request.capability_id.as_str() != "sandbox.execute_isolated" {
            let msg = format!(
                "Unknown sandbox capability: {}",
                request.capability_id.as_str()
            );
            error!("{}", msg);
            return Ok(CapabilityExecutionResult {
                execution_id: request.execution_id,
                trace_id: request.trace_id,
                connector_id: request.connector_id,
                capability_id: request.capability_id,
                output: serde_json::json!({ "error": msg }),
                status: ConnectorExecutionStatus::Failed,
                error: Some(msg),
                actual_runtime: None,
            });
        }

        let input: SandboxInput = match serde_json::from_value(request.input.clone()) {
            Ok(v) => v,
            Err(e) => {
                let msg = format!("Invalid SandboxInput: {}", e);
                error!("{}", msg);
                return Ok(CapabilityExecutionResult {
                    execution_id: request.execution_id,
                    trace_id: request.trace_id,
                    connector_id: request.connector_id,
                    capability_id: request.capability_id,
                    output: serde_json::json!({ "error": msg }),
                    status: ConnectorExecutionStatus::Failed,
                    error: Some(msg),
                    actual_runtime: None,
                });
            }
        };

        let sandbox_request = SandboxRequest {
            command: input.command,
            args: input.args,
            stdin: input.stdin,
            timeout: input
                .timeout_ms
                .map(Duration::from_millis)
                .unwrap_or(self.default_timeout),
            memory_limit_mb: input
                .memory_limit_mb
                .unwrap_or(self.default_memory_limit_mb),
            env: input.env,
            env_allowlist: self.env_allowlist.clone(),
            tempdir_root: self.tempdir_root.clone(),
        };

        // The `run_sandboxed` function is synchronous; move it to a blocking
        // worker so we don't block the tokio runtime with `Command::wait`.
        let response = tokio::task::spawn_blocking(move || run_sandboxed(sandbox_request))
            .await
            .map_err(|e| anyhow::anyhow!("sandbox join failed: {}", e))?;

        match response {
            Ok(resp) => {
                info!(
                    "Sandbox capability succeeded: exit={:?} timed_out={} elapsed_ms={}",
                    resp.exit_code, resp.timed_out, resp.elapsed_ms
                );
                let output = serde_json::to_value(&resp)?;
                Ok(CapabilityExecutionResult {
                    execution_id: request.execution_id,
                    trace_id: request.trace_id,
                    connector_id: request.connector_id,
                    capability_id: request.capability_id,
                    output,
                    status: ConnectorExecutionStatus::Success,
                    error: None,
                    actual_runtime: None,
                })
            }
            Err(e) => {
                error!("Sandbox capability failed: {:?}", e);
                Ok(CapabilityExecutionResult {
                    execution_id: request.execution_id,
                    trace_id: request.trace_id,
                    connector_id: request.connector_id,
                    capability_id: request.capability_id,
                    output: serde_json::json!({ "error": e.to_string() }),
                    status: ConnectorExecutionStatus::Failed,
                    error: Some(e.to_string()),
                    actual_runtime: None,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyberclaw_core::identity::Identity;

    fn mk_request(input: serde_json::Value) -> CapabilityExecutionRequest {
        use cyberclaw_core::workspace::{WorkspaceMode, WorkspaceRef};
        CapabilityExecutionRequest {
            execution_id: ExecutionId::new(),
            trace_id: "trace-sandbox-test".to_string(),
            actor: Identity::System.to_actor_ref(None).unwrap(),
            workspace: WorkspaceRef {
                id: WorkspaceId::from_string("sandbox-test".to_string()).unwrap(),
                mode: WorkspaceMode::Ephemeral,
                materialization_mode: None,
                home_node_id: None,
                backing_store: None,
                root: "/tmp/sandbox-test".to_string(),
                writable_roots: vec![],
            },
            connector_id: ConnectorId::from_string("sandbox".to_string()).unwrap(),
            capability_id: CapabilityId::from_string("sandbox.execute_isolated".to_string())
                .unwrap(),
            input,
        }
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn connector_exposes_one_capability() {
        let c = SandboxConnector::new();
        let caps = c.capabilities();
        assert_eq!(caps.len(), 1);
        assert_eq!(caps[0].id, "sandbox.execute_isolated");
        assert_eq!(caps[0].risk, RiskLevel::High);
        assert_eq!(c.runtime(), ConnectorRuntime::Process);
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn rejects_unknown_capability() {
        let c = SandboxConnector::new();
        let mut req = mk_request(serde_json::json!({"command": "true"}));
        req.capability_id = CapabilityId::from_string("sandbox.bogus".to_string()).unwrap();
        let res = c.execute(req).await.unwrap();
        assert_eq!(res.status, ConnectorExecutionStatus::Failed);
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn echo_through_connector() {
        let c = SandboxConnector::new();
        let req = mk_request(serde_json::json!({
            "command": "echo",
            "args": ["hello"]
        }));
        let res = c.execute(req).await.unwrap();
        assert_eq!(res.status, ConnectorExecutionStatus::Success);
        let resp: SandboxResponse = serde_json::from_value(res.output).unwrap();
        assert_eq!(resp.exit_code, Some(0));
        assert!(resp.stdout.trim() == "hello");
        assert!(!resp.timed_out);
    }
}
