//! # Shell / REPL Capability Surface — Host-Level (`cmd.*`)
//!
//! This module implements the **host-level** shell execution capabilities for
//! `LocalConnector`.  It is the non-sandboxed counterpart to
//! `crates/cyberclaw-connectors/src/sandbox/mod.rs`.
//!
//! ## Security model — host vs sandbox
//!
//! | Aspect               | `cmd.*` (this module)         | `sandbox.execute_isolated`      |
//! |----------------------|-------------------------------|----------------------------------|
//! | Working directory    | Caller-specified or workspace | Fresh tempdir per request        |
//! | Environment          | Full host env (+ caller adds) | Allowlist-scrubbed env           |
//! | Privilege            | **Full host process user**    | Same uid, smaller env surface    |
//! | Network access       | Unrestricted                  | Unrestricted (no isolation)      |
//! | Filesystem access    | Unrestricted                  | Unrestricted (no chroot)         |
//! | Blast radius         | **Maximum**                   | Reduced                          |
//!
//! `cmd.run` executes arbitrary shell commands with the full privileges of the
//! host process.  Governance layers **MUST** treat it with the same scrutiny
//! as direct `sudo` access.  Use `sandbox.execute_isolated` for internally
//! produced candidate variants where a reduced blast radius is sufficient.
//!
//! ## Capabilities exposed
//!
//! | Capability ID        | Description                           | Risk  |
//! |----------------------|---------------------------------------|-------|
//! | `cmd.exec`           | Legacy whitelist-restricted exec      | High  |
//! | `cmd.run`            | Arbitrary shell exec (full privilege) | High  |
//! | `cmd.run_streaming`  | Streaming output variant of `cmd.run` | High  |
//! | `cmd.run_powershell` | PowerShell script execution (`pwsh`)  | High  |
//!
//! ## §4 Facade export
//!
//! Per EVOLUTION_IDIOMS §4 ("Facades From Owner Crate"), this module exports
//! [`capability_facades`] which returns a [`CmdFacadeDescriptor`] list.
//! Because `cyberclaw-connectors` cannot import `cyberclaw-agent-runtime`
//! (that crate optionally depends on connectors — importing it here would create
//! a dependency cycle), the return type uses a local descriptor struct carrying
//! the same fields as `CapabilityFacade`.  The host binary / server layer converts
//! these descriptors into `CapabilityFacade` instances when composing the agent
//! tool registry, analogous to `BridgedTool::to_capability_facade_fields()` in
//! `crates/cyberclaw-connectors/src/mcp/tool_bridge.rs`.

use super::LocalConnector;
use crate::sandbox::SandboxProfile;
use crate::types::*;
use cyberclaw_core::capability::RiskLevel;
use cyberclaw_core::facade::{CapabilityFacade, FacadeExposure, ToolsetCategory};
use cyberclaw_core::ids::{CapabilityId, ConnectorId};
use cyberclaw_core::manifests::{CapabilityContract, CapabilityTimeouts};
use std::collections::HashSet;
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tracing::{debug, error, info, warn};

// ---------------------------------------------------------------------------
// Allowed-command whitelist — used only by legacy `cmd.exec`
// ---------------------------------------------------------------------------

/// Allowed commands whitelist (safe commands only) — used only by `cmd.exec`.
///
/// SECURITY: Only commands that cannot execute arbitrary code are allowed here.
/// `cmd.run` is the unrestricted alternative for callers granted the necessary
/// governance permission.
const ALLOWED_COMMANDS: &[&str] = &[
    // File inspection (read-only)
    "ls", "cat", "head", "tail", "wc", "file", "stat",
    // Text processing (cannot execute sub-commands)
    "grep", "sort", "uniq", "cut", "tr", // System info (read-only)
    "echo", "date", "pwd", "whoami", "hostname", "uname", // Search tools (read-only)
    "rg", "ripgrep", "fd",
];

/// Default timeout for `cmd.exec` and `cmd.run` (30 s).
const DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// Default timeout for `cmd.run_streaming` (60 s — streaming commands tend to
/// be longer-lived).
// Public API pre-integration: wired by host binary during capability
// composition. See docs/architecture/idioms/EVOLUTION_IDIOMS.md §4.
#[allow(dead_code)]
const DEFAULT_STREAMING_TIMEOUT_MS: u64 = 60_000;

/// Default max output bytes for `cmd.run_streaming` (1 MiB).
// Public API pre-integration: wired by host binary during capability
// composition. See docs/architecture/idioms/EVOLUTION_IDIOMS.md §4.
#[allow(dead_code)]
const DEFAULT_MAX_OUTPUT_BYTES: usize = 1024 * 1024;

// ---------------------------------------------------------------------------
// §4 Facade export — real cyberclaw_core::facade types (Phase 2)
// ---------------------------------------------------------------------------

/// Return the facade list for all `cmd.*` capabilities.
///
/// **§4 compliance**: This is the single authoritative source for cmd facade
/// declarations.  `BuiltinToolRegistry::default_facades` MUST NOT duplicate
/// these entries; the host binary registers them via
/// `BuiltinToolRegistry::register`.
// Public API pre-integration: wired by host binary during capability
// composition. See docs/architecture/idioms/EVOLUTION_IDIOMS.md §4.
#[allow(dead_code)]
pub fn capability_facades() -> Vec<(CapabilityFacade, ToolsetCategory)> {
    let connector_id = ConnectorId::from_string("local".to_string()).unwrap();
    vec![
        (
            CapabilityFacade {
                name: "cmd_run".to_string(),
                description: "Execute an arbitrary shell command on the host with full process \
                    privileges. Commands run via `sh -c`, so pipes, redirects, and compound \
                    expressions are supported. **Risk: High — full host privilege, no sandboxing.** \
                    Prefer `sandbox_execute_isolated` when a reduced blast radius is sufficient.".to_string(),
                connector_id: connector_id.clone(),
                capability_id: CapabilityId::from_string("cmd.run".to_string()).unwrap(),
                risk_level: RiskLevel::High,
                effects: vec!["execute".to_string(), "write".to_string()],
                read_only: false,
                destructive: true,
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "Shell command string (executed via sh -c)"
                        },
                        "workdir": {
                            "type": "string",
                            "description": "Optional working directory (defaults to workspace root)"
                        },
                        "timeout_ms": {
                            "type": "integer",
                            "description": "Timeout in milliseconds (default 30000)"
                        },
                        "env": {
                            "type": "object",
                            "description": "Optional environment variable overrides"
                        },
                        "stdin": {
                            "type": "string",
                            "description": "Optional stdin data"
                        }
                    },
                    "required": ["command"]
                })),
                exposure: FacadeExposure::LlmDefault,
                workspace_root: None,
            },
            ToolsetCategory::Terminal,
        ),
        (
            CapabilityFacade {
                name: "cmd_run_streaming".to_string(),
                description: "Execute a shell command with streaming output. Lines are emitted \
                    incrementally as the process produces them. The final result also contains \
                    the complete captured stdout/stderr. **Risk: High — full host privilege.**".to_string(),
                connector_id: connector_id.clone(),
                capability_id: CapabilityId::from_string("cmd.run_streaming".to_string()).unwrap(),
                risk_level: RiskLevel::High,
                effects: vec!["execute".to_string(), "write".to_string()],
                read_only: false,
                destructive: true,
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "Shell command string (executed via sh -c)"
                        },
                        "workdir": {
                            "type": "string",
                            "description": "Optional working directory"
                        },
                        "timeout_ms": {
                            "type": "integer",
                            "description": "Timeout in milliseconds (default 60000)"
                        },
                        "env": {
                            "type": "object",
                            "description": "Optional environment variable overrides"
                        },
                        "max_output_bytes": {
                            "type": "integer",
                            "description": "Max combined output bytes before truncation (default 1048576)"
                        }
                    },
                    "required": ["command"]
                })),
                exposure: FacadeExposure::LlmAdvanced,
                workspace_root: None,
            },
            ToolsetCategory::Terminal,
        ),
        (
            CapabilityFacade {
                name: "cmd_run_powershell".to_string(),
                description: "Execute a PowerShell script via `pwsh` (PowerShell 7+) on Unix, \
                    or `powershell.exe` as fallback on Windows. Requires PowerShell to be \
                    installed. **Risk: High — full host privilege.**".to_string(),
                connector_id: connector_id.clone(),
                capability_id: CapabilityId::from_string("cmd.run_powershell".to_string()).unwrap(),
                risk_level: RiskLevel::High,
                effects: vec!["execute".to_string(), "write".to_string()],
                read_only: false,
                destructive: true,
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "script": {
                            "type": "string",
                            "description": "PowerShell script or command string"
                        },
                        "workdir": {
                            "type": "string",
                            "description": "Optional working directory"
                        },
                        "timeout_ms": {
                            "type": "integer",
                            "description": "Timeout in milliseconds (default 30000)"
                        },
                        "env": {
                            "type": "object",
                            "description": "Optional environment variable overrides"
                        }
                    },
                    "required": ["script"]
                })),
                exposure: FacadeExposure::LlmAdvanced,
                workspace_root: None,
            },
            ToolsetCategory::Terminal,
        ),
    ]
}

// ---------------------------------------------------------------------------
// R-1 (2026-05-05) — host-level command-content safety gate
// ---------------------------------------------------------------------------
//
// `cmd.run` is dispatched by the LLM-visible `bash` facade and is therefore
// the highest-blast-radius capability the agent can reach. Even though the
// PolicyEngine governs the *capability id*, it does not see the *command
// string*; this gate inspects the literal command and rejects classes of
// instruction that have no legitimate business use in CyberClaw's expected
// workloads (system destruction, raw network exfil to link-local metadata,
// privilege escalation primitives, etc.).
//
// The list is intentionally short — a too-eager filter would break python3,
// pytest, cargo, npm and similar harmless tooling. Operators wanting more
// restrictive runtime sandboxing should configure
// `LocalConnector::with_container_runtime` so the entire shell call runs
// inside an alpine container.
//
// Returning `Err` here surfaces as a normal capability error, which is
// captured both by the audit chain (R-3) and by the agent's tool-result
// pipeline.

/// Substrings that must never appear in a `cmd.run` shell string. Matched
/// case-insensitively against the literal command — no shell parsing — so a
/// payload like `r''m -rf /` won't bypass it.
const CMD_RUN_BLOCKED_PATTERNS: &[&str] = &[
    // Filesystem destruction
    "rm -rf /",
    "rm -fr /",
    "rm -rf --no-preserve-root",
    "mkfs",
    "dd if=/dev/zero",
    "dd if=/dev/random",
    "dd of=/dev/sd",
    "dd of=/dev/nvme",
    "shred /dev/",
    // Fork bomb
    ":(){:|:&};:",
    ":() { :|:& };:",
    // Privilege primitives that have no place in agent self-driven workloads
    "sudo passwd",
    "chmod 777 /",
    "chown -r root /",
    // Network exfil to cloud / hypervisor metadata
    "169.254.169.254",
    "metadata.google.internal",
    // Sensitive system credential / user / key files (v1.0-rc12 — adversarial
    // smoke AT6 exposed that /etc/passwd was readable via cmd.run cat).
    // Even though /etc/passwd is technically world-readable on most systems,
    // exposing it via agent enables user enumeration → further attacks.
    "/etc/shadow",
    "/etc/sudoers",
    "/etc/passwd",
    "/etc/ssh/",
    "/etc/security/",
    "/root/",
    "/root/.ssh",
    ".ssh/id_rsa",
    ".ssh/id_ed25519",
    ".ssh/id_ecdsa",
    "/var/log/auth.log",
    "/var/log/secure",
    "/proc/self/environ",
    "/proc/self/cmdline",
    // Raw block device & kernel pokes
    "/dev/sda",
    "/dev/nvme",
    "/proc/sys/kernel",
    "/proc/sysrq-trigger",
];

/// Build the volume map shared between `cmd.exec` and `cmd.run` container
/// invocations. Maps a host directory (default `/tmp/cyberclaw-shared`) to
/// `/tmp` inside the container so files written there survive the
/// `auto_remove=true` container teardown — closes v1.2 backlog #20 where
/// LLM-written files in /tmp were lost between turns.
///
/// Override via `CYBERCLAW_CONTAINER_SHARED_TMP=<absolute host path>`. The
/// host dir is created on first use (best-effort; failure is logged via
/// the runtime's mount error path rather than blocking the call).
///
/// R3 (2026-05-21) — back-compat shim. The single source of truth for the
/// `/tmp` mount now lives in [`SandboxProfile::dev`]; this helper extracts
/// the `/tmp` mount from a resolved profile so existing callers and tests
/// keep working with no behavior change. Marked `#[allow(dead_code)]`
/// because in-tree call sites have been migrated to the profile API; the
/// function survives only to keep external/test callers compiling.
#[allow(dead_code)]
pub(crate) fn build_shared_container_volumes(
) -> std::collections::HashMap<std::path::PathBuf, std::path::PathBuf> {
    use std::collections::HashMap;
    let profile = SandboxProfile::dev();
    let mut volumes = HashMap::new();
    for spec in &profile.mounts {
        if let Some(host) = &spec.host_path {
            volumes.insert(host.clone(), spec.container_path.clone());
        }
    }
    volumes
}

/// Build a [`CapabilityContract`] shim describing the timing constraint
/// for a `cmd.*` capability so [`SandboxProfile::validate_and_resolve`]
/// can negotiate the time budget. We only fill `id` and `timeouts` — the
/// rest of the contract isn't consulted by the resolver.
fn cmd_capability_shim(cap_id: &str, request_ms: u64) -> CapabilityContract {
    CapabilityContract {
        id: cap_id.to_string(),
        title: cap_id.to_string(),
        description: None,
        input_schema: "".to_string(),
        output_schema: "".to_string(),
        risk: RiskLevel::High,
        effects: vec![],
        placement: None,
        timeouts: CapabilityTimeouts {
            request_ms: Some(request_ms),
        },
    }
}

/// Reject a `cmd.run` invocation if the literal command string matches any
/// of [`CMD_RUN_BLOCKED_PATTERNS`]. Case-insensitive substring match.
pub(crate) fn check_cmd_run_safety(command: &str) -> anyhow::Result<()> {
    let lower = command.to_ascii_lowercase();
    for pattern in CMD_RUN_BLOCKED_PATTERNS {
        if lower.contains(&pattern.to_ascii_lowercase()) {
            error!(
                pattern = pattern,
                "cmd.run: blocked dangerous command pattern (host-level safety gate)"
            );
            return Err(anyhow::anyhow!(
                "cmd.run blocked: command contains forbidden pattern `{}`. \
                 Use a narrower capability or request governance review for this workflow.",
                pattern
            ));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Helper: parse command string (legacy whitelist path only)
// ---------------------------------------------------------------------------

/// Parse a command string into `(cmd_name, args)` by splitting on whitespace.
/// Does NOT invoke a shell — used only by the whitelist-restricted `cmd.exec`.
fn parse_command(raw: &str) -> anyhow::Result<(String, Vec<String>)> {
    let parts: Vec<&str> = raw.split_whitespace().collect();
    if parts.is_empty() {
        return Err(anyhow::anyhow!("Empty command"));
    }
    let cmd = parts[0].to_string();
    let args = parts[1..].iter().map(|s| s.to_string()).collect();
    Ok((cmd, args))
}

/// Return `true` if any argument contains a shell metacharacter.
/// Used only by `cmd.exec` to prevent injection via the non-shell code path.
fn contains_shell_metacharacters(args: &[String]) -> bool {
    for arg in args {
        if arg.contains('|')
            || arg.contains(';')
            || arg.contains('&')
            || arg.contains('`')
            || arg.contains("$(")
            || arg.contains('>')
            || arg.contains('<')
            || arg.contains('*')
            || arg.contains('?')
            || arg.contains('\n')
            || arg.contains('\r')
        {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Helper: detect PowerShell executable
// ---------------------------------------------------------------------------

/// Detect the PowerShell executable available on this host.
///
/// Preference: `pwsh` (PowerShell 7+) → `powershell` (Windows 5.x fallback).
/// Returns `None` if neither is on `PATH`.
fn detect_powershell() -> Option<String> {
    for candidate in &["pwsh", "powershell"] {
        if which_on_path(candidate) {
            return Some((*candidate).to_string());
        }
    }
    None
}

/// Return `true` if `name` resolves to an executable via `PATH`.
fn which_on_path(name: &str) -> bool {
    std::process::Command::new("which")
        .arg(name)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
        || std::process::Command::new("where")
            .arg(name)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// cmd.exec — legacy whitelist-restricted execution
// ---------------------------------------------------------------------------

/// Execute a whitelisted shell command (legacy, restricted capability).
///
/// Security model:
/// - Only commands in `ALLOWED_COMMANDS` are permitted.
/// - Executed directly (no `sh -c`) to prevent shell injection.
/// - Arguments are checked for metacharacters.
/// - Hard timeout with process kill on expiry.
///
/// For unrestricted host-level execution use [`run`] (`cmd.run`).
pub async fn exec(
    connector: &LocalConnector,
    request: CapabilityExecutionRequest,
) -> anyhow::Result<serde_json::Value> {
    // Sprint 20 — ADR-0002 Phase 1/2. Snapshot the actor before
    // moving `request.input` into the deserialiser so the default
    // workdir derivation below can scope to per-agent subdir, and
    // so validate_path_for_actor has the actor for strict mode.
    let input: CmdExecInput = serde_json::from_value(request.input)?;
    let actor = request.actor.clone();
    debug!("cmd.exec: command='{}'", input.command);

    let (cmd_name, args) = parse_command(&input.command)?;

    let allowed: HashSet<&str> = ALLOWED_COMMANDS.iter().copied().collect();
    if !allowed.contains(cmd_name.as_str()) {
        error!("cmd.exec: blocked disallowed command '{}'", cmd_name);
        return Err(anyhow::anyhow!(
            "Command '{}' is not in the allowed list. Allowed commands: {:?}",
            cmd_name,
            ALLOWED_COMMANDS
        ));
    }

    if contains_shell_metacharacters(&args) {
        error!(
            "cmd.exec: blocked metacharacters in args for '{}'",
            cmd_name
        );
        return Err(anyhow::anyhow!(
            "Command arguments contain shell metacharacters which are not allowed"
        ));
    }

    // Sprint 20 W1 — ADR-0002 Phase 1: when the caller does not
    // pin a workdir, default to the agent-scoped subdir
    // (`<workspace_root>/agents/<actor_id>`) instead of the shared
    // workspace root. Caller-supplied `workdir` still validates
    // against the shared root — Phase 2 tightens that.
    let workdir = if let Some(dir) = &input.workdir {
        connector.validate_path_for_actor(dir, &actor)?
    } else {
        connector.resolve_agent_workspace(&actor)
    };

    let timeout_duration = Duration::from_millis(input.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS));

    // Sprint 19 W4 — when a ContainerRuntime is wired (operator opted
    // into Container isolation via `LocalConnector::with_container_runtime`),
    // run the whitelisted command inside a container governed by the
    // R3 `isolated` SandboxProfile (no network, shared /tmp, ripgrep/jq/
    // python3 preinstalled). This is the production-correct path for
    // High-risk capabilities. When no ContainerRuntime is configured,
    // falls back to the legacy bare subprocess path below.
    if let Some(rt) = connector.container_runtime.as_ref() {
        use crate::runtime::ProcessConfig;

        // R3 — single source of truth for mounts/network/image lives in
        // the sandbox profile, not inline here. `cmd.exec` is whitelisted
        // and historically had NetworkMode::None → use `isolated`.
        let profile = SandboxProfile::isolated();
        let cap_shim =
            cmd_capability_shim("cmd.exec", input.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS));
        let effective = profile
            .validate_and_resolve(&cap_shim)
            .map_err(|e| anyhow::anyhow!("sandbox profile resolve failed: {}", e))?;
        let container_cfg = crate::runtime::ContainerRuntime::config_from_sandbox(&effective);

        let process_cfg = ProcessConfig {
            timeout: timeout_duration,
            kill_grace_period: Duration::from_secs(5),
            working_directory: Some(workdir.clone()),
            environment: std::collections::HashMap::new(),
            inherit_env: false,
            max_memory_bytes: Some(512 * 1024 * 1024),
            max_file_descriptors: Some(256),
            capture_stdout: true,
            capture_stderr: true,
        };

        info!(
            "cmd.exec: dispatching '{}' ({} args) inside container image={} profile={}",
            cmd_name,
            args.len(),
            container_cfg.image,
            effective.profile_id,
        );

        let result = rt
            .execute_command_in_container(&cmd_name, args.clone(), process_cfg, container_cfg)
            .await
            .map_err(|e| anyhow::anyhow!("container dispatch failed: {}", e))?;

        return Ok(serde_json::to_value(CmdExecOutput {
            stdout: result.stdout,
            stderr: result.stderr,
            exit_code: result.exit_code,
            timed_out: result.timed_out,
        })?);
    }

    let mut cmd = Command::new(&cmd_name);
    cmd.args(&args)
        .current_dir(&workdir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    info!(
        "cmd.exec: executing '{}' ({} args) in {:?}",
        cmd_name,
        args.len(),
        workdir
    );

    let mut child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| anyhow::anyhow!("Failed to spawn command: {}", e))?;

    let child_pid = child.id();
    let stdout_handle = child.stdout.take();
    let stderr_handle = child.stderr.take();

    let (stdout, stderr, exit_code, timed_out) = tokio::select! {
        wait_result = child.wait() => {
            match wait_result {
                Ok(status) => {
                    let exit_code = status.code().unwrap_or(-1);
                    let mut stdout = String::new();
                    let mut stderr = String::new();
                    if let Some(mut h) = stdout_handle {
                        let mut buf = Vec::new();
                        let _ = h.read_to_end(&mut buf).await;
                        stdout = String::from_utf8_lossy(&buf).to_string();
                    }
                    if let Some(mut h) = stderr_handle {
                        let mut buf = Vec::new();
                        let _ = h.read_to_end(&mut buf).await;
                        stderr = String::from_utf8_lossy(&buf).to_string();
                    }
                    if status.success() {
                        info!("cmd.exec: '{}' succeeded (exit {})", cmd_name, exit_code);
                    } else {
                        warn!("cmd.exec: '{}' failed (exit {})", cmd_name, exit_code);
                    }
                    (stdout, stderr, exit_code, false)
                }
                Err(e) => return Err(anyhow::anyhow!("Failed to wait for command: {}", e)),
            }
        }
        _ = tokio::time::sleep(timeout_duration) => {
            warn!("cmd.exec: '{}' (pid {:?}) timed out", cmd_name, child_pid);
            if let Err(e) = child.kill().await {
                error!("cmd.exec: failed to kill pid {:?}: {}", child_pid, e);
            }
            let _ = child.wait().await;
            (
                String::new(),
                "Command timed out and was terminated".to_string(),
                -1,
                true,
            )
        }
    };

    Ok(serde_json::to_value(CmdExecOutput {
        stdout,
        stderr,
        exit_code,
        timed_out,
    })?)
}

// ---------------------------------------------------------------------------
// cmd.run — unrestricted host-level execution
// ---------------------------------------------------------------------------

/// Execute an arbitrary shell command with full host privileges.
///
/// Unlike [`exec`], this function places **no whitelist restriction** on the
/// command.  It invokes `sh -c <command>` so pipes, redirects, and compound
/// expressions work as expected.  Governance layers are responsible for
/// ensuring callers hold appropriate permissions before this capability is
/// dispatched.
///
/// **Distinct from `sandbox.execute_isolated`**: the sandbox provides a fresh
/// tempdir with a scrubbed environment for a reduced blast radius.  `cmd.run`
/// inherits the full host environment and uses the caller-specified (or workspace)
/// working directory — maximum privilege, no isolation.
pub async fn run(
    connector: &LocalConnector,
    request: CapabilityExecutionRequest,
) -> anyhow::Result<serde_json::Value> {
    let input: CmdRunInput = serde_json::from_value(request.input)?;
    let actor = request.actor.clone();
    debug!("cmd.run: command='{}'", input.command);

    // R-1 (2026-05-05) — host-level safety gate. The PolicyEngine guards the
    // capability id; this guards the literal command string.
    check_cmd_run_safety(&input.command)?;

    let workdir = if let Some(dir) = &input.workdir {
        connector.validate_path_for_actor(dir, &actor)?
    } else {
        connector.workspace.clone()
    };

    let timeout_ms = input.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS);
    let timeout_duration = Duration::from_millis(timeout_ms);

    // v1.0-rc13 SECURITY: route cmd.run through ContainerRuntime when wired.
    // Before this fix, cmd::exec (whitelisted) had a container path
    // (line 459) but cmd::run (unrestricted bash) did NOT — meaning the
    // sandbox was bypassed for the very capability that needs it most.
    // Red-team smoke AT6 (smoke-agi-adversarial.sh) exposed: LLM used
    // cmd.run cat /etc/passwd → succeeded reading host file, because cmd.run
    // ran native bash regardless of ContainerRuntime being configured.
    //
    // Fix: when connector.container_runtime is Some, dispatch the bash
    // invocation inside an alpine container with workspace mounted +
    // network=none + read-only root. Native fallback path only kicks in
    // when no container runtime is configured (legacy / test scenarios).
    if let Some(rt) = connector.container_runtime.as_ref() {
        use crate::runtime::ProcessConfig;

        // R3 (2026-05-21) — `cmd.run` is the agent `bash` facade; it
        // needs the richest tooling (ripgrep / jq / python3) and bridge
        // network so business tasks like web_search → python compute
        // work. The `dev` SandboxProfile is the single source of truth.
        // Image, /tmp mount, workspace mount, network, memory, and CPU
        // all come from the profile — no inline ContainerConfig assembly
        // that could collide with the runtime's implicit `--tmpfs /tmp`.
        let profile = SandboxProfile::dev();
        let cap_shim =
            cmd_capability_shim("cmd.run", input.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS));
        let effective = profile
            .validate_and_resolve(&cap_shim)
            .map_err(|e| anyhow::anyhow!("sandbox profile resolve failed: {}", e))?;
        let container_cfg = crate::runtime::ContainerRuntime::config_from_sandbox(&effective);

        let process_cfg = ProcessConfig {
            timeout: timeout_duration,
            kill_grace_period: Duration::from_secs(5),
            working_directory: Some(workdir.clone()),
            environment: input.env.clone(),
            inherit_env: false,
            max_memory_bytes: Some(512 * 1024 * 1024),
            max_file_descriptors: Some(256),
            capture_stdout: true,
            capture_stderr: true,
        };

        info!(
            "cmd.run: dispatching inside container image={} profile={} (cmd=bash -c)",
            container_cfg.image, effective.profile_id,
        );

        let result = rt
            .execute_command_in_container(
                "bash",
                vec!["-c".to_string(), input.command.clone()],
                process_cfg,
                container_cfg,
            )
            .await
            .map_err(|e| anyhow::anyhow!("container dispatch failed: {}", e))?;

        return Ok(serde_json::to_value(CmdRunOutput {
            stdout: result.stdout,
            stderr: result.stderr,
            exit_code: result.exit_code,
            timed_out: result.timed_out,
            elapsed_ms: result.duration_ms,
        })?);
    }

    // Fallback: native bash (no container) — for tests + hosts without
    // docker/podman. Denylist patterns still apply via check_cmd_run_safety
    // above, but this is NOT a hard sandbox.
    let mut cmd = Command::new("bash");
    cmd.arg("-c")
        .arg(&input.command)
        .current_dir(&workdir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    for (k, v) in &input.env {
        cmd.env(k, v);
    }

    if input.stdin.is_some() {
        cmd.stdin(std::process::Stdio::piped());
    }

    info!(
        "cmd.run: '{}' in {:?} (timeout {}ms) [NATIVE — no container]",
        input.command, workdir, timeout_ms
    );

    let start = Instant::now();
    let mut child = cmd
        .spawn()
        .map_err(|e| anyhow::anyhow!("Failed to spawn command: {}", e))?;

    let child_pid = child.id();

    if let Some(stdin_data) = &input.stdin {
        use tokio::io::AsyncWriteExt;
        if let Some(mut stdin_handle) = child.stdin.take() {
            let _ = stdin_handle.write_all(stdin_data.as_bytes()).await;
        }
    }

    let stdout_handle = child.stdout.take();
    let stderr_handle = child.stderr.take();

    let (stdout, stderr, exit_code, timed_out) = tokio::select! {
        wait_result = child.wait() => {
            match wait_result {
                Ok(status) => {
                    let exit_code = status.code().unwrap_or(-1);
                    let mut stdout = String::new();
                    let mut stderr = String::new();
                    if let Some(mut h) = stdout_handle {
                        let mut buf = Vec::new();
                        let _ = h.read_to_end(&mut buf).await;
                        stdout = String::from_utf8_lossy(&buf).to_string();
                    }
                    if let Some(mut h) = stderr_handle {
                        let mut buf = Vec::new();
                        let _ = h.read_to_end(&mut buf).await;
                        stderr = String::from_utf8_lossy(&buf).to_string();
                    }
                    if status.success() {
                        info!("cmd.run: succeeded (exit {})", exit_code);
                    } else {
                        warn!("cmd.run: failed (exit {})", exit_code);
                    }
                    (stdout, stderr, exit_code, false)
                }
                Err(e) => return Err(anyhow::anyhow!("Failed to wait for command: {}", e)),
            }
        }
        _ = tokio::time::sleep(timeout_duration) => {
            warn!("cmd.run: pid {:?} timed out after {}ms", child_pid, timeout_ms);
            if let Err(e) = child.kill().await {
                error!("cmd.run: failed to kill pid {:?}: {}", child_pid, e);
            }
            let _ = child.wait().await;
            (
                String::new(),
                "Command timed out and was terminated".to_string(),
                -1,
                true,
            )
        }
    };

    let elapsed_ms = start.elapsed().as_millis() as u64;
    Ok(serde_json::to_value(CmdRunOutput {
        stdout,
        stderr,
        exit_code,
        timed_out,
        elapsed_ms,
    })?)
}

// ---------------------------------------------------------------------------
// cmd.run_streaming — streaming output variant
// ---------------------------------------------------------------------------

/// Execute a shell command with streaming line-by-line output capture.
///
/// Output lines are accumulated in arrival order in the `lines` field.  The
/// flat `stdout`/`stderr` fields contain the complete text for callers that
/// only need the final result.  Output is truncated at `max_output_bytes` to
/// protect against runaway processes.
pub async fn run_streaming(
    connector: &LocalConnector,
    request: CapabilityExecutionRequest,
) -> anyhow::Result<serde_json::Value> {
    let input: CmdRunStreamingInput = serde_json::from_value(request.input)?;
    let actor = request.actor.clone();
    debug!("cmd.run_streaming: command='{}'", input.command);

    // R-1 (2026-05-05) — same host-level safety gate as cmd.run.
    check_cmd_run_safety(&input.command)?;

    let workdir = if let Some(dir) = &input.workdir {
        connector.validate_path_for_actor(dir, &actor)?
    } else {
        connector.workspace.clone()
    };

    let timeout_ms = input.timeout_ms.unwrap_or(DEFAULT_STREAMING_TIMEOUT_MS);
    let timeout_duration = Duration::from_millis(timeout_ms);
    let max_bytes = input.max_output_bytes.unwrap_or(DEFAULT_MAX_OUTPUT_BYTES);

    // R3 (2026-05-21) — container dispatch via SandboxProfile when a
    // ContainerRuntime is wired. The container path buffers output (no
    // live streaming inside a one-shot `docker run`) but synthesises the
    // `lines` array by splitting on `\n` so callers still see the
    // per-line breakdown they expect from this capability.
    if let Some(rt) = connector.container_runtime.as_ref() {
        use crate::runtime::ProcessConfig;

        let profile = SandboxProfile::dev();
        let cap_shim = cmd_capability_shim("cmd.run_streaming", timeout_ms);
        let effective = profile
            .validate_and_resolve(&cap_shim)
            .map_err(|e| anyhow::anyhow!("sandbox profile resolve failed: {}", e))?;
        let container_cfg = crate::runtime::ContainerRuntime::config_from_sandbox(&effective);

        let process_cfg = ProcessConfig {
            timeout: timeout_duration,
            kill_grace_period: Duration::from_secs(5),
            working_directory: Some(workdir.clone()),
            environment: input.env.clone(),
            inherit_env: false,
            max_memory_bytes: Some(512 * 1024 * 1024),
            max_file_descriptors: Some(256),
            capture_stdout: true,
            capture_stderr: true,
        };

        info!(
            "cmd.run_streaming: dispatching inside container image={} profile={} (cmd=bash -c)",
            container_cfg.image, effective.profile_id,
        );

        let start = Instant::now();
        let result = rt
            .execute_command_in_container(
                "bash",
                vec!["-c".to_string(), input.command.clone()],
                process_cfg,
                container_cfg,
            )
            .await
            .map_err(|e| anyhow::anyhow!("container dispatch failed: {}", e))?;

        // Synthesise per-line StreamLine entries from the buffered
        // container output. Truncate at max_bytes to honour the
        // capability's contract; mark truncated=true when applicable.
        let mut lines: Vec<StreamLine> = Vec::new();
        let mut total_bytes: usize = 0;
        let mut truncated = false;
        for line in result.stdout.lines() {
            total_bytes += line.len() + 1;
            if total_bytes > max_bytes {
                truncated = true;
                break;
            }
            lines.push(StreamLine {
                stream: "stdout".to_string(),
                line: line.to_string(),
            });
        }
        if !truncated {
            for line in result.stderr.lines() {
                total_bytes += line.len() + 1;
                if total_bytes > max_bytes {
                    truncated = true;
                    break;
                }
                lines.push(StreamLine {
                    stream: "stderr".to_string(),
                    line: line.to_string(),
                });
            }
        }

        return Ok(serde_json::to_value(CmdRunStreamingOutput {
            lines,
            stdout: result.stdout,
            stderr: result.stderr,
            exit_code: result.exit_code,
            timed_out: result.timed_out,
            truncated,
            elapsed_ms: start.elapsed().as_millis() as u64,
        })?);
    }

    // v1.0-rc9: switched from sh to bash — see comment in run_command above
    // for the echo -n / SHA256 silent corruption rationale.
    let mut cmd = Command::new("bash");
    cmd.arg("-c")
        .arg(&input.command)
        .current_dir(&workdir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    for (k, v) in &input.env {
        cmd.env(k, v);
    }

    info!(
        "cmd.run_streaming: '{}' in {:?} (timeout {}ms, max_bytes {})",
        input.command, workdir, timeout_ms, max_bytes
    );

    let start = Instant::now();
    let mut child = cmd
        .spawn()
        .map_err(|e| anyhow::anyhow!("Failed to spawn command: {}", e))?;

    let child_pid = child.id();

    use tokio::io::{AsyncBufReadExt, BufReader};

    // Drain stdout + stderr and wait for process exit, all under a single
    // deadline.  We wrap the entire drain+wait block in `tokio::time::timeout`
    // so that blocking reads on the BufReader are cancelled when the deadline
    // fires — a plain `tokio::select!` after the drain loops would not fire
    // until the sequential reads completed.
    let drain_result = tokio::time::timeout(timeout_duration, async {
        let mut lines: Vec<StreamLine> = Vec::new();
        let mut stdout_buf = String::new();
        let mut stderr_buf = String::new();
        let mut total_bytes: usize = 0;
        let mut truncated = false;

        if let Some(handle) = child.stdout.take() {
            let mut reader = BufReader::new(handle).lines();
            loop {
                match reader.next_line().await {
                    Ok(Some(line)) => {
                        total_bytes += line.len() + 1;
                        if total_bytes > max_bytes {
                            truncated = true;
                            break;
                        }
                        stdout_buf.push_str(&line);
                        stdout_buf.push('\n');
                        lines.push(StreamLine {
                            stream: "stdout".to_string(),
                            line,
                        });
                    }
                    Ok(None) => break,
                    Err(e) => {
                        warn!("cmd.run_streaming: stdout read error: {}", e);
                        break;
                    }
                }
            }
        }

        if let Some(handle) = child.stderr.take() {
            let mut reader = BufReader::new(handle).lines();
            loop {
                match reader.next_line().await {
                    Ok(Some(line)) => {
                        total_bytes += line.len() + 1;
                        if total_bytes > max_bytes {
                            truncated = true;
                            break;
                        }
                        stderr_buf.push_str(&line);
                        stderr_buf.push('\n');
                        lines.push(StreamLine {
                            stream: "stderr".to_string(),
                            line,
                        });
                    }
                    Ok(None) => break,
                    Err(e) => {
                        warn!("cmd.run_streaming: stderr read error: {}", e);
                        break;
                    }
                }
            }
        }

        let exit_code = match child.wait().await {
            Ok(status) => {
                let code = status.code().unwrap_or(-1);
                if status.success() {
                    info!("cmd.run_streaming: succeeded (exit {})", code);
                } else {
                    warn!("cmd.run_streaming: failed (exit {})", code);
                }
                code
            }
            Err(e) => return Err(anyhow::anyhow!("Failed to wait for command: {}", e)),
        };

        Ok::<_, anyhow::Error>((lines, stdout_buf, stderr_buf, exit_code, truncated))
    })
    .await;

    let (lines, stdout_buf, stderr_buf, exit_code, truncated, timed_out) = match drain_result {
        Ok(Ok((lines, stdout, stderr, code, truncated))) => {
            (lines, stdout, stderr, code, truncated, false)
        }
        Ok(Err(e)) => return Err(e),
        Err(_elapsed) => {
            // Timeout fired — kill the child and reap
            warn!(
                "cmd.run_streaming: pid {:?} timed out after {}ms",
                child_pid, timeout_ms
            );
            if let Err(e) = child.kill().await {
                error!(
                    "cmd.run_streaming: failed to kill pid {:?}: {}",
                    child_pid, e
                );
            }
            let _ = child.wait().await;
            (Vec::new(), String::new(), String::new(), -1, false, true)
        }
    };

    let elapsed_ms = start.elapsed().as_millis() as u64;
    Ok(serde_json::to_value(CmdRunStreamingOutput {
        lines,
        stdout: stdout_buf,
        stderr: stderr_buf,
        exit_code,
        timed_out,
        truncated,
        elapsed_ms,
    })?)
}

// ---------------------------------------------------------------------------
// cmd.run_powershell — PowerShell variant
// ---------------------------------------------------------------------------

/// Execute a PowerShell script on the host.
///
/// Requires `pwsh` (PowerShell 7+) on `PATH`.  Falls back to `powershell`
/// (Windows PowerShell 5.x) when `pwsh` is not found.  Returns a clear error
/// if neither is installed.
///
/// The script is invoked as: `pwsh -NoProfile -NonInteractive -Command <script>`.
pub async fn run_powershell(
    connector: &LocalConnector,
    request: CapabilityExecutionRequest,
) -> anyhow::Result<serde_json::Value> {
    let input: CmdRunPowerShellInput = serde_json::from_value(request.input)?;
    let actor = request.actor.clone();
    debug!("cmd.run_powershell: script len={}", input.script.len());

    let ps_exe = detect_powershell().ok_or_else(|| {
        anyhow::anyhow!(
            "PowerShell not found. Install PowerShell 7+ (pwsh) from \
             https://github.com/PowerShell/PowerShell/releases"
        )
    })?;

    let workdir = if let Some(dir) = &input.workdir {
        connector.validate_path_for_actor(dir, &actor)?
    } else {
        connector.workspace.clone()
    };

    let timeout_ms = input.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS);

    // R3 (2026-05-21) — pwsh is not bundled in the default `python:3.12-slim`
    // container image, so this capability cannot dispatch into the
    // SandboxProfile container path. We still run the profile resolver
    // to negotiate the time budget (profile vs capability) so the budget
    // contract stays consistent with cmd.run / cmd.run_streaming /
    // cmd.exec. Future work: an operator-configurable
    // `desired_profile = "powershell"` once a pwsh-bearing base image
    // is published; until then the profile observation is informational.
    let ps_profile = SandboxProfile::minimal();
    let ps_cap_shim = cmd_capability_shim("cmd.run_powershell", timeout_ms);
    let effective_ps_time = match ps_profile.validate_and_resolve(&ps_cap_shim) {
        Ok(eff) => eff.time_budget,
        Err(e) => {
            warn!(
                "cmd.run_powershell: sandbox profile resolve failed ({}), using raw timeout",
                e
            );
            Duration::from_millis(timeout_ms)
        }
    };
    let timeout_duration = effective_ps_time;

    let mut cmd = Command::new(&ps_exe);
    cmd.arg("-NoProfile")
        .arg("-NonInteractive")
        .arg("-Command")
        .arg(&input.script)
        .current_dir(&workdir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    for (k, v) in &input.env {
        cmd.env(k, v);
    }

    info!(
        "cmd.run_powershell: '{}' via '{}' in {:?} (timeout {}ms)",
        &input.script[..input.script.len().min(80)],
        ps_exe,
        workdir,
        timeout_ms
    );

    let start = Instant::now();
    let mut child = cmd
        .spawn()
        .map_err(|e| anyhow::anyhow!("Failed to spawn PowerShell: {}", e))?;

    let child_pid = child.id();
    let stdout_handle = child.stdout.take();
    let stderr_handle = child.stderr.take();

    let (stdout, stderr, exit_code, timed_out) = tokio::select! {
        wait_result = child.wait() => {
            match wait_result {
                Ok(status) => {
                    let exit_code = status.code().unwrap_or(-1);
                    let mut stdout = String::new();
                    let mut stderr = String::new();
                    if let Some(mut h) = stdout_handle {
                        let mut buf = Vec::new();
                        let _ = h.read_to_end(&mut buf).await;
                        stdout = String::from_utf8_lossy(&buf).to_string();
                    }
                    if let Some(mut h) = stderr_handle {
                        let mut buf = Vec::new();
                        let _ = h.read_to_end(&mut buf).await;
                        stderr = String::from_utf8_lossy(&buf).to_string();
                    }
                    if status.success() {
                        info!("cmd.run_powershell: succeeded (exit {})", exit_code);
                    } else {
                        warn!("cmd.run_powershell: failed (exit {})", exit_code);
                    }
                    (stdout, stderr, exit_code, false)
                }
                Err(e) => return Err(anyhow::anyhow!("Failed to wait for PowerShell: {}", e)),
            }
        }
        _ = tokio::time::sleep(timeout_duration) => {
            warn!(
                "cmd.run_powershell: pid {:?} timed out after {}ms",
                child_pid, timeout_ms
            );
            if let Err(e) = child.kill().await {
                error!("cmd.run_powershell: failed to kill pid {:?}: {}", child_pid, e);
            }
            let _ = child.wait().await;
            (
                String::new(),
                "PowerShell timed out and was terminated".to_string(),
                -1,
                true,
            )
        }
    };

    let elapsed_ms = start.elapsed().as_millis() as u64;
    Ok(serde_json::to_value(CmdRunPowerShellOutput {
        stdout,
        stderr,
        exit_code,
        timed_out,
        elapsed_ms,
        powershell_path: ps_exe,
    })?)
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::LocalConnector;
    use tempfile::TempDir;

    fn make_connector(tmp: &TempDir) -> LocalConnector {
        LocalConnector::new(tmp.path().to_path_buf())
    }

    fn make_request(
        connector: &LocalConnector,
        capability: &str,
        input: serde_json::Value,
    ) -> CapabilityExecutionRequest {
        CapabilityExecutionRequest {
            execution_id: cyberclaw_core::ids::ExecutionId::new(),
            trace_id: "test-trace".to_string(),
            actor: cyberclaw_core::identity::ActorRef {
                id: cyberclaw_core::ids::ActorId::new(),
                actor_type: cyberclaw_core::identity::ActorType::System,
                tenant_id: None,
                home_node_id: None,
                display_name: "test".to_string(),
            },
            workspace: cyberclaw_core::workspace::WorkspaceRef {
                id: cyberclaw_core::ids::WorkspaceId::new(),
                mode: cyberclaw_core::workspace::WorkspaceMode::Isolated,
                materialization_mode: None,
                home_node_id: None,
                backing_store: None,
                root: connector.workspace.to_string_lossy().to_string(),
                writable_roots: vec![connector.workspace.to_string_lossy().to_string()],
            },
            connector_id: cyberclaw_core::ids::ConnectorId::from_string("local".to_string())
                .unwrap(),
            capability_id: cyberclaw_core::ids::CapabilityId::from_string(capability.to_string())
                .unwrap(),
            input,
        }
    }

    // --- capability_facades() ---

    #[test]
    fn facades_returns_three_entries() {
        let facades = capability_facades();
        assert_eq!(
            facades.len(),
            3,
            "expected cmd.run, cmd.run_streaming, cmd.run_powershell"
        );
    }

    #[test]
    fn facades_all_high_risk_with_execute_effect() {
        for (desc, _cat) in capability_facades() {
            assert_eq!(
                desc.risk_level,
                RiskLevel::High,
                "{} must be RiskLevel::High",
                desc.capability_id.as_str()
            );
            assert!(
                desc.effects.iter().any(|e| e == "execute"),
                "{} must declare execute effect",
                desc.capability_id.as_str()
            );
            assert_eq!(desc.connector_id.as_str(), "local");
        }
    }

    #[test]
    fn facades_capability_ids_are_distinct() {
        let ids: Vec<String> = capability_facades()
            .into_iter()
            .map(|(d, _)| d.capability_id.as_str().to_string())
            .collect();
        let unique: HashSet<&str> = ids.iter().map(|s| s.as_str()).collect();
        assert_eq!(ids.len(), unique.len(), "capability IDs must be distinct");
    }

    #[test]
    fn facades_input_schemas_are_valid_json_objects() {
        for (desc, _cat) in capability_facades() {
            let schema = desc
                .input_schema
                .as_ref()
                .expect("input_schema must be Some");
            assert!(
                schema.is_object(),
                "{}: input_schema must be a JSON object",
                desc.capability_id.as_str()
            );
            assert_eq!(
                schema.get("type").and_then(|v| v.as_str()),
                Some("object"),
                "{}: input_schema must have type=object",
                desc.capability_id.as_str()
            );
            assert!(
                schema.get("properties").is_some(),
                "{}: input_schema must have properties",
                desc.capability_id.as_str()
            );
        }
    }

    // --- cmd.run ---

    #[tokio::test]
    #[cfg(unix)]
    async fn run_happy_path_echo() {
        let tmp = TempDir::new().unwrap();
        let connector = make_connector(&tmp);
        let req = make_request(
            &connector,
            "cmd.run",
            serde_json::json!({ "command": "echo hello" }),
        );
        let result = run(&connector, req).await.unwrap();
        let output: CmdRunOutput = serde_json::from_value(result).unwrap();
        assert_eq!(output.exit_code, 0);
        assert_eq!(output.stdout.trim(), "hello");
        assert!(!output.timed_out);
        assert!(output.elapsed_ms < 5_000);
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn run_nonzero_exit_code() {
        let tmp = TempDir::new().unwrap();
        let connector = make_connector(&tmp);
        let req = make_request(
            &connector,
            "cmd.run",
            serde_json::json!({ "command": "exit 42" }),
        );
        let result = run(&connector, req).await.unwrap();
        let output: CmdRunOutput = serde_json::from_value(result).unwrap();
        assert_eq!(output.exit_code, 42);
        assert!(!output.timed_out);
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn run_timeout_kills_process() {
        let tmp = TempDir::new().unwrap();
        let connector = make_connector(&tmp);
        let req = make_request(
            &connector,
            "cmd.run",
            serde_json::json!({ "command": "sleep 60", "timeout_ms": 200 }),
        );
        let result = run(&connector, req).await.unwrap();
        let output: CmdRunOutput = serde_json::from_value(result).unwrap();
        assert!(
            output.timed_out,
            "process should have been killed by timeout"
        );
        assert_eq!(output.exit_code, -1);
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn run_captures_stderr() {
        let tmp = TempDir::new().unwrap();
        let connector = make_connector(&tmp);
        let req = make_request(
            &connector,
            "cmd.run",
            serde_json::json!({ "command": "echo error_msg >&2" }),
        );
        let result = run(&connector, req).await.unwrap();
        let output: CmdRunOutput = serde_json::from_value(result).unwrap();
        assert!(
            output.stderr.contains("error_msg"),
            "stderr should contain 'error_msg', got: {:?}",
            output.stderr
        );
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn run_env_override() {
        let tmp = TempDir::new().unwrap();
        let connector = make_connector(&tmp);
        let req = make_request(
            &connector,
            "cmd.run",
            serde_json::json!({
                "command": "echo $CYBERCLAW_TEST_VAR",
                "env": { "CYBERCLAW_TEST_VAR": "injected_value" }
            }),
        );
        let result = run(&connector, req).await.unwrap();
        let output: CmdRunOutput = serde_json::from_value(result).unwrap();
        assert_eq!(output.exit_code, 0);
        assert!(
            output.stdout.contains("injected_value"),
            "stdout should contain env var value, got: {:?}",
            output.stdout
        );
    }

    // --- cmd.run_streaming ---

    #[tokio::test]
    #[cfg(unix)]
    async fn run_streaming_happy_path() {
        let tmp = TempDir::new().unwrap();
        let connector = make_connector(&tmp);
        let req = make_request(
            &connector,
            "cmd.run_streaming",
            serde_json::json!({ "command": "printf 'line1\\nline2\\nline3\\n'" }),
        );
        let result = run_streaming(&connector, req).await.unwrap();
        let output: CmdRunStreamingOutput = serde_json::from_value(result).unwrap();
        assert_eq!(output.exit_code, 0);
        assert!(!output.timed_out);
        assert!(!output.truncated);
        assert!(output.stdout.contains("line1"));
        assert!(output.stdout.contains("line2"));
        assert!(output.stdout.contains("line3"));
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn run_streaming_timeout() {
        let tmp = TempDir::new().unwrap();
        let connector = make_connector(&tmp);
        let req = make_request(
            &connector,
            "cmd.run_streaming",
            serde_json::json!({ "command": "sleep 60", "timeout_ms": 200 }),
        );
        let result = run_streaming(&connector, req).await.unwrap();
        let output: CmdRunStreamingOutput = serde_json::from_value(result).unwrap();
        assert!(output.timed_out);
        assert_eq!(output.exit_code, -1);
    }

    // --- cmd.exec (legacy) ---

    #[tokio::test]
    #[cfg(unix)]
    async fn exec_legacy_allowed_command_succeeds() {
        let tmp = TempDir::new().unwrap();
        let connector = make_connector(&tmp);
        let req = make_request(
            &connector,
            "cmd.exec",
            serde_json::json!({ "command": "echo legacy" }),
        );
        let result = exec(&connector, req).await.unwrap();
        let output: CmdExecOutput = serde_json::from_value(result).unwrap();
        assert_eq!(output.exit_code, 0);
        assert!(output.stdout.contains("legacy"));
    }

    // --- R-1 host-level cmd.run safety gate ---

    #[test]
    fn cmd_run_safety_blocks_destructive_patterns() {
        for c in [
            "rm -rf /",
            "rm -rf --no-preserve-root /home",
            "echo a; rm -rf /",
            "RM -RF /",
            "mkfs.ext4 /dev/sda",
            "dd if=/dev/zero of=/dev/sda bs=1M",
            ":(){:|:&};:",
            "curl -s 169.254.169.254/latest",
            "cat /etc/shadow",
        ] {
            let err = check_cmd_run_safety(c).expect_err(&format!("expected block for `{}`", c));
            assert!(
                err.to_string().contains("forbidden pattern"),
                "expected forbidden-pattern error for `{}`, got `{}`",
                c,
                err
            );
        }
    }

    #[test]
    fn cmd_run_safety_allows_business_workloads() {
        for c in [
            "python3 -c 'print(1+1)'",
            "pytest tests/",
            "node -e \"console.log(1)\"",
            "cargo test --workspace",
            "ls -la",
            "git status",
            "rg -n hello src/",
            // benign rm in a workspace subdir
            "rm -rf ./build",
            "rm /tmp/foo.txt",
        ] {
            check_cmd_run_safety(c)
                .unwrap_or_else(|e| panic!("expected allow for `{}`, got error `{}`", c, e));
        }
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn run_safety_gate_blocks_dangerous_input() {
        let tmp = TempDir::new().unwrap();
        let connector = make_connector(&tmp);
        let req = make_request(
            &connector,
            "cmd.run",
            serde_json::json!({ "command": "rm -rf /" }),
        );
        let err = run(&connector, req).await.unwrap_err();
        assert!(
            err.to_string().contains("forbidden pattern"),
            "expected safety-gate error, got: {}",
            err
        );
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn run_safety_gate_allows_python3() {
        // R-1 regression — agents must be able to run python3 to self-verify.
        if std::process::Command::new("python3")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            let tmp = TempDir::new().unwrap();
            let connector = make_connector(&tmp);
            let req = make_request(
                &connector,
                "cmd.run",
                serde_json::json!({ "command": "python3 -c \"print(2+2)\"" }),
            );
            let result = run(&connector, req).await.unwrap();
            let output: CmdRunOutput = serde_json::from_value(result).unwrap();
            assert_eq!(output.exit_code, 0);
            assert!(output.stdout.contains('4'));
        }
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn exec_legacy_blocked_command_returns_error() {
        let tmp = TempDir::new().unwrap();
        let connector = make_connector(&tmp);
        let req = make_request(
            &connector,
            "cmd.exec",
            serde_json::json!({ "command": "rm -rf /tmp/test" }),
        );
        let err = exec(&connector, req).await.unwrap_err();
        assert!(
            err.to_string().contains("not in the allowed list"),
            "unexpected error: {}",
            err
        );
    }

    // -----------------------------------------------------------------------
    // v1.2 #20 — shared /tmp container mount
    // -----------------------------------------------------------------------

    #[test]
    fn build_shared_container_volumes_default_path() {
        // Clear the override so we hit the default branch.
        std::env::remove_var("CYBERCLAW_CONTAINER_SHARED_TMP");
        let map = build_shared_container_volumes();
        assert_eq!(map.len(), 1, "exactly one shared mount entry expected");
        let host = std::path::PathBuf::from("/tmp/cyberclaw-shared");
        assert_eq!(
            map.get(&host),
            Some(&std::path::PathBuf::from("/tmp")),
            "default mount: /tmp/cyberclaw-shared (host) → /tmp (container)",
        );
    }

    // -----------------------------------------------------------------------
    // R13-BUG-01 extension — credential gate covers cmd.run / cmd.run_streaming
    // -----------------------------------------------------------------------

    /// Verify that a cmd.run request containing an AWS key in a shell redirect
    /// is rejected by the R13-BUG-01 credential gate (via LocalConnector::execute)
    /// before the command reaches bash.
    #[tokio::test]
    #[cfg(unix)]
    async fn test_cmd_run_aws_key_in_redirect_rejected_by_runtime() {
        use crate::types::{Connector, ExecutionStatus as ConnectorExecutionStatus};

        let tmp = TempDir::new().unwrap();
        let connector = make_connector(&tmp);

        // Build a full CapabilityExecutionRequest so it routes through
        // LocalConnector::execute() and hits the write_caps gate in mod.rs.
        let req = crate::types::CapabilityExecutionRequest {
            execution_id: cyberclaw_core::ids::ExecutionId::new(),
            trace_id: "test-trace-r13".to_string(),
            actor: cyberclaw_core::identity::ActorRef {
                id: cyberclaw_core::ids::ActorId::new(),
                actor_type: cyberclaw_core::identity::ActorType::System,
                tenant_id: None,
                home_node_id: None,
                display_name: "test".to_string(),
            },
            workspace: cyberclaw_core::workspace::WorkspaceRef {
                id: cyberclaw_core::ids::WorkspaceId::new(),
                mode: cyberclaw_core::workspace::WorkspaceMode::Isolated,
                materialization_mode: None,
                home_node_id: None,
                backing_store: None,
                root: connector.workspace().to_string_lossy().to_string(),
                writable_roots: vec![connector.workspace().to_string_lossy().to_string()],
            },
            connector_id: cyberclaw_core::ids::ConnectorId::from_string("local".to_string())
                .unwrap(),
            capability_id: cyberclaw_core::ids::CapabilityId::from_string("cmd.run".to_string())
                .unwrap(),
            input: serde_json::json!({
                "command": "echo \"aws_access_key_id = AKIAIOSFODNN7EXAMPLE\" > /tmp/test.txt"
            }),
        };

        let result = connector.execute(req).await.unwrap();
        assert_eq!(
            result.status,
            ConnectorExecutionStatus::Failed,
            "cmd.run with credential in redirect must be rejected by the R13 gate"
        );
        let err_str = result.error.unwrap_or_default();
        assert!(
            err_str.contains("governance") || err_str.contains("D010") || err_str.contains("credential"),
            "error should reference governance/credential block, got: {err_str}"
        );
    }

    /// Regression: a safe cmd.run command (ls -la) must still pass through the
    /// R13-BUG-01 credential gate without being blocked.
    #[tokio::test]
    #[cfg(unix)]
    async fn test_cmd_run_safe_command_passes() {
        use crate::types::{Connector, ExecutionStatus as ConnectorExecutionStatus};

        let tmp = TempDir::new().unwrap();
        let connector = make_connector(&tmp);

        let req = crate::types::CapabilityExecutionRequest {
            execution_id: cyberclaw_core::ids::ExecutionId::new(),
            trace_id: "test-trace-r13-safe".to_string(),
            actor: cyberclaw_core::identity::ActorRef {
                id: cyberclaw_core::ids::ActorId::new(),
                actor_type: cyberclaw_core::identity::ActorType::System,
                tenant_id: None,
                home_node_id: None,
                display_name: "test".to_string(),
            },
            workspace: cyberclaw_core::workspace::WorkspaceRef {
                id: cyberclaw_core::ids::WorkspaceId::new(),
                mode: cyberclaw_core::workspace::WorkspaceMode::Isolated,
                materialization_mode: None,
                home_node_id: None,
                backing_store: None,
                root: connector.workspace().to_string_lossy().to_string(),
                writable_roots: vec![connector.workspace().to_string_lossy().to_string()],
            },
            connector_id: cyberclaw_core::ids::ConnectorId::from_string("local".to_string())
                .unwrap(),
            capability_id: cyberclaw_core::ids::CapabilityId::from_string("cmd.run".to_string())
                .unwrap(),
            input: serde_json::json!({ "command": "ls -la" }),
        };

        let result = connector.execute(req).await.unwrap();
        assert_eq!(
            result.status,
            ConnectorExecutionStatus::Success,
            "safe cmd.run (ls -la) must not be blocked by the credential gate"
        );
    }

    #[test]
    fn build_shared_container_volumes_env_override() {
        // tempdir-based override path to keep the test self-contained.
        let tmp = tempfile::TempDir::new().expect("create tempdir");
        let host_dir = tmp.path().join("shared-cache");
        // SAFETY: tests don't run env-var changes concurrently here because
        // this module's tests serialize on cargo's per-module thread group.
        std::env::set_var("CYBERCLAW_CONTAINER_SHARED_TMP", &host_dir);
        let map = build_shared_container_volumes();
        std::env::remove_var("CYBERCLAW_CONTAINER_SHARED_TMP");
        assert_eq!(map.len(), 1);
        assert_eq!(
            map.get(&host_dir),
            Some(&std::path::PathBuf::from("/tmp")),
            "override host path should map to container /tmp",
        );
        // create_dir_all should have produced the directory.
        assert!(
            host_dir.exists(),
            "helper must create the host dir if missing"
        );
    }
}
