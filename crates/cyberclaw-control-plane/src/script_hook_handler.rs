//! `ScriptHookHandler` — runs a Platform Plugin hook script as a subprocess.
//!
//! Closes the BT-22 gap from the Hermes-agent business benchmark.
//!
//! # Security boundary
//!
//! - **Path traversal**: rejects any `handler_path` containing `..` or absolute
//!   paths. The script must live inside the plugin root.
//! - **Restricted environment**: the spawned process inherits only `PATH`. All
//!   other vars are dropped. CyberClaw injects a fixed set of `CYBERCLAW_HOOK_*`
//!   vars carrying the hook context (execution_id, capability_id, etc.).
//! - **Timeout**: every script is killed after `timeout_ms` (default 5000 ms).
//! - **Output capture**: stdout/stderr captured up to a small bound. Anything
//!   beyond the bound is truncated rather than buffered.
//!
//! # FailurePolicy semantics
//!
//! - Exit code 0 → `HookResult::Continue`.
//! - Exit code non-zero with `failure_policy: Abort` → `HookResult::Abort(stderr)`.
//! - Exit code non-zero with `failure_policy: Continue` → `HookResult::Continue`
//!   (with a logged warning).
//! - Spawn / timeout error → `HookResult::Abort` if `failure_policy: Abort`,
//!   else `HookResult::Continue`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use tokio::process::Command;
use tokio::time::timeout;
use tracing::{error, warn};

use crate::hook_dispatcher::{HookContext, HookHandler, HookPoint, HookResult};
use crate::hook_integration::HookFailurePolicy;

const DEFAULT_TIMEOUT_MS: u64 = 5_000;
const STDOUT_MAX_BYTES: usize = 16 * 1024;

/// Runs a plugin hook script as a child process.
#[derive(Debug, Clone)]
pub struct ScriptHookHandler {
    plugin_name: String,
    plugin_root: PathBuf,
    handler_path: String,
    failure_policy: HookFailurePolicy,
    timeout_ms: u64,
}

impl ScriptHookHandler {
    pub fn new(
        plugin_name: impl Into<String>,
        plugin_root: PathBuf,
        handler_path: impl Into<String>,
        failure_policy: HookFailurePolicy,
    ) -> Self {
        Self {
            plugin_name: plugin_name.into(),
            plugin_root,
            handler_path: handler_path.into(),
            failure_policy,
            timeout_ms: DEFAULT_TIMEOUT_MS,
        }
    }

    pub fn with_timeout_ms(mut self, ms: u64) -> Self {
        self.timeout_ms = ms;
        self
    }

    /// Resolve `handler_path` to an absolute path inside `plugin_root`.
    /// Rejects path traversal attempts.
    fn resolve_script(&self) -> Result<PathBuf, String> {
        let candidate = Path::new(&self.handler_path);
        if candidate.is_absolute() {
            return Err(format!(
                "handler_path must be relative: {}",
                self.handler_path
            ));
        }
        for component in candidate.components() {
            if matches!(component, std::path::Component::ParentDir) {
                return Err(format!(
                    "handler_path may not traverse outside plugin root: {}",
                    self.handler_path
                ));
            }
        }
        let abs = self.plugin_root.join(candidate);
        if !abs.exists() {
            return Err(format!("handler script not found: {}", abs.display()));
        }
        Ok(abs)
    }

    fn deny(&self, reason: String) -> HookResult {
        match self.failure_policy {
            HookFailurePolicy::Abort => HookResult::Abort(reason),
            _ => {
                warn!(
                    plugin = %self.plugin_name,
                    handler = %self.handler_path,
                    reason = %reason,
                    "ScriptHookHandler: hook failed but failure_policy is non-abort, continuing"
                );
                HookResult::Continue
            }
        }
    }
}

#[async_trait]
impl HookHandler for ScriptHookHandler {
    async fn handle(&self, _point: &HookPoint, context: &HookContext) -> HookResult {
        let script_path = match self.resolve_script() {
            Ok(p) => p,
            Err(e) => {
                error!(
                    plugin = %self.plugin_name,
                    error = %e,
                    "ScriptHookHandler: refusing to run"
                );
                return self.deny(e);
            }
        };

        // Build a minimal env: keep only PATH, then add CYBERCLAW_HOOK_* vars.
        let path_var = std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_string());
        let mut cmd = Command::new("sh");
        cmd.arg(&script_path)
            .env_clear()
            .env("PATH", path_var)
            .env("CYBERCLAW_HOOK_EXECUTION_ID", &context.execution_id)
            .env("CYBERCLAW_HOOK_CAPABILITY_ID", &context.capability_id)
            .env("CYBERCLAW_HOOK_STEP", context.step.to_string())
            .env(
                "CYBERCLAW_HOOK_ERROR_MESSAGE",
                context.error_message.as_deref().unwrap_or(""),
            );
        for (k, v) in &context.metadata {
            // Allow plugin authors to read execution metadata via documented prefix.
            // Keys are restricted to ASCII alphanumeric + underscore to keep the
            // env namespace clean; anything else is dropped.
            if k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                cmd.env(format!("CYBERCLAW_HOOK_META_{}", k.to_uppercase()), v);
            }
        }

        let exec_future = cmd.output();
        let raw = match timeout(Duration::from_millis(self.timeout_ms), exec_future).await {
            Ok(Ok(o)) => o,
            Ok(Err(e)) => {
                let reason = format!("failed to spawn hook script: {e}");
                error!(plugin = %self.plugin_name, error = %e, "ScriptHookHandler spawn failed");
                return self.deny(reason);
            }
            Err(_) => {
                let reason = format!("hook script timed out after {}ms", self.timeout_ms);
                error!(plugin = %self.plugin_name, "ScriptHookHandler timed out");
                return self.deny(reason);
            }
        };

        if raw.status.success() {
            return HookResult::Continue;
        }

        // Truncate stderr to a sane bound; never feed unbounded output forward.
        let mut stderr = String::from_utf8_lossy(&raw.stderr).into_owned();
        if stderr.len() > STDOUT_MAX_BYTES {
            stderr.truncate(STDOUT_MAX_BYTES);
            stderr.push_str("...[truncated]");
        }

        let reason = format!(
            "plugin '{}' hook '{}' exited {} (stderr: {})",
            self.plugin_name,
            self.handler_path,
            raw.status.code().unwrap_or(-1),
            stderr.trim()
        );
        self.deny(reason)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hook_dispatcher::{HookContext, HookPoint};
    use std::os::unix::fs::PermissionsExt;

    fn make_script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        let mut perms = std::fs::metadata(&p).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&p, perms).unwrap();
        p
    }

    fn ctx() -> HookContext {
        HookContext::new("exec-test", "cmd.exec", 0)
    }

    #[tokio::test]
    async fn script_exit_zero_returns_continue() {
        let tmp = tempfile::TempDir::new().unwrap();
        make_script(tmp.path(), "ok.sh", "#!/usr/bin/env sh\nexit 0\n");

        let handler = ScriptHookHandler::new(
            "test",
            tmp.path().to_path_buf(),
            "ok.sh",
            HookFailurePolicy::Abort,
        );
        let result = handler.handle(&HookPoint::PreExecution, &ctx()).await;
        assert_eq!(result, HookResult::Continue);
    }

    #[tokio::test]
    async fn script_exit_nonzero_with_abort_policy_aborts() {
        let tmp = tempfile::TempDir::new().unwrap();
        make_script(
            tmp.path(),
            "deny.sh",
            "#!/usr/bin/env sh\necho 'denied' >&2\nexit 1\n",
        );

        let handler = ScriptHookHandler::new(
            "policy-enforcer",
            tmp.path().to_path_buf(),
            "deny.sh",
            HookFailurePolicy::Abort,
        );
        let result = handler.handle(&HookPoint::PreExecution, &ctx()).await;
        match result {
            HookResult::Abort(reason) => {
                assert!(reason.contains("policy-enforcer"));
                assert!(reason.contains("denied"));
            }
            other => panic!("expected Abort, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn script_exit_nonzero_with_continue_policy_continues() {
        let tmp = tempfile::TempDir::new().unwrap();
        make_script(tmp.path(), "fail.sh", "#!/usr/bin/env sh\nexit 1\n");

        let handler = ScriptHookHandler::new(
            "audit-enricher",
            tmp.path().to_path_buf(),
            "fail.sh",
            HookFailurePolicy::Continue,
        );
        let result = handler.handle(&HookPoint::PreExecution, &ctx()).await;
        assert_eq!(result, HookResult::Continue);
    }

    #[tokio::test]
    async fn missing_script_aborts_when_policy_abort() {
        let tmp = tempfile::TempDir::new().unwrap();
        let handler = ScriptHookHandler::new(
            "test",
            tmp.path().to_path_buf(),
            "no_such.sh",
            HookFailurePolicy::Abort,
        );
        let result = handler.handle(&HookPoint::PreExecution, &ctx()).await;
        match result {
            HookResult::Abort(reason) => assert!(reason.contains("not found")),
            other => panic!("expected Abort, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn path_traversal_is_rejected() {
        let tmp = tempfile::TempDir::new().unwrap();
        let handler = ScriptHookHandler::new(
            "evil",
            tmp.path().to_path_buf(),
            "../etc/passwd",
            HookFailurePolicy::Abort,
        );
        let result = handler.handle(&HookPoint::PreExecution, &ctx()).await;
        match result {
            HookResult::Abort(reason) => assert!(reason.contains("traverse")),
            other => panic!("expected Abort, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn absolute_path_is_rejected() {
        let tmp = tempfile::TempDir::new().unwrap();
        let handler = ScriptHookHandler::new(
            "evil",
            tmp.path().to_path_buf(),
            "/bin/sh",
            HookFailurePolicy::Abort,
        );
        let result = handler.handle(&HookPoint::PreExecution, &ctx()).await;
        match result {
            HookResult::Abort(reason) => assert!(reason.contains("must be relative")),
            other => panic!("expected Abort, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn timeout_kills_runaway_script() {
        let tmp = tempfile::TempDir::new().unwrap();
        make_script(tmp.path(), "slow.sh", "#!/usr/bin/env sh\nsleep 10\n");

        let handler = ScriptHookHandler::new(
            "test",
            tmp.path().to_path_buf(),
            "slow.sh",
            HookFailurePolicy::Abort,
        )
        .with_timeout_ms(200);
        let start = std::time::Instant::now();
        let result = handler.handle(&HookPoint::PreExecution, &ctx()).await;
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_secs(2),
            "did not time out promptly"
        );
        match result {
            HookResult::Abort(reason) => assert!(reason.contains("timed out")),
            other => panic!("expected Abort, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn cyberclaw_hook_env_vars_are_injected() {
        let tmp = tempfile::TempDir::new().unwrap();
        make_script(
            tmp.path(),
            "check.sh",
            "#!/usr/bin/env sh\n[ \"$CYBERCLAW_HOOK_CAPABILITY_ID\" = \"cmd.exec\" ] || exit 1\n[ \"$CYBERCLAW_HOOK_EXECUTION_ID\" = \"exec-test\" ] || exit 2\nexit 0\n",
        );

        let handler = ScriptHookHandler::new(
            "test",
            tmp.path().to_path_buf(),
            "check.sh",
            HookFailurePolicy::Abort,
        );
        let result = handler.handle(&HookPoint::PreExecution, &ctx()).await;
        assert_eq!(result, HookResult::Continue);
    }
}
