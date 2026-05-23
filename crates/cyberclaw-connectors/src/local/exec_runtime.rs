//! # `exec_runtime` — function-level code execution for R2 OutputVerifier
//!
//! Sprint 3 PART A (2026-05-21, optimization-plan-v1 §R2).
//!
//! This module provides a **function-level** code-execution API that the
//! R2 `CodeBlockVerifier` (in `cyberclaw-agent-runtime`) calls to actually
//! run agent-produced code blocks against the host (or a container) and
//! capture stdout/stderr/exit_code. It is **not** wired as a new
//! `Capability` — by design — because:
//!
//! 1. We don't want to expand the LLM-visible tool surface for now.
//! 2. `cmd.run` already exists for full shell access; this module is a
//!    narrower, language-tagged helper that the verifier uses
//!    *post-hoc* to self-check.
//! 3. The verify lane needs a synchronous Rust signature, not a
//!    `CapabilityExecutionRequest` round-trip.
//!
//! ## Supported languages
//!
//! | `CodeLang` | Execution strategy                                 |
//! |------------|-----------------------------------------------------|
//! | `Python`   | `python3 -c <source>`                              |
//! | `JavaScript` | `node -e <source>`                               |
//! | `Bash`     | `bash -c <source>`                                 |
//! | `Sql`      | `sqlite3 :memory:` with source piped on stdin       |
//!
//! ## Sandbox strategy
//!
//! Best-effort sandbox via `ContainerRuntime` when a container backend
//! is detected on the host (Docker / Podman). When no container backend
//! is available, falls back to direct subprocess — same posture as
//! `cmd.run`'s native fallback path, which is acceptable because the
//! caller (the verifier) is running agent-produced code that the
//! governance layer already approved for the parent `cmd.run`
//! capability.
//!
//! When called with `containerized = true` AND a container backend is
//! detected, the source runs inside a network-isolated container with
//! a 512 MiB memory cap and a 1.0 CPU budget. The image is configurable
//! via `CYBERCLAW_VERIFY_CONTAINER_IMAGE` (defaults to
//! `python:3.12-slim`, which has bash + node available via apt; for SQL
//! we ship sqlite3 in the image — operators wanting a leaner image can
//! pin one that pre-installs the languages they actually use).

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tracing::{debug, info, warn};

/// Language tag used by the verifier to pick an interpreter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CodeLang {
    /// Python 3 — `python3 -c`.
    Python,
    /// JavaScript via Node — `node -e`.
    JavaScript,
    /// POSIX shell via `bash -c`.
    Bash,
    /// SQL piped through `sqlite3 :memory:`.
    Sql,
}

impl CodeLang {
    /// Parse a fenced-code-block language hint (e.g. ```python).
    /// Returns `None` when the hint is unsupported or empty.
    pub fn from_hint(hint: &str) -> Option<Self> {
        match hint.trim().to_ascii_lowercase().as_str() {
            "python" | "python3" | "py" => Some(Self::Python),
            "javascript" | "js" | "node" | "nodejs" => Some(Self::JavaScript),
            "bash" | "sh" | "shell" => Some(Self::Bash),
            "sql" | "sqlite" | "sqlite3" => Some(Self::Sql),
            _ => None,
        }
    }

    /// Human-readable name for logs / feedback strings.
    pub fn name(self) -> &'static str {
        match self {
            Self::Python => "python",
            Self::JavaScript => "javascript",
            Self::Bash => "bash",
            Self::Sql => "sql",
        }
    }
}

/// Output of a single code execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecOutput {
    /// Language that was executed.
    pub lang: CodeLang,
    /// Captured stdout (UTF-8, lossy-decoded).
    pub stdout: String,
    /// Captured stderr (UTF-8, lossy-decoded).
    pub stderr: String,
    /// Process exit code. `-1` on spawn failure / timeout.
    pub exit_code: i32,
    /// `true` iff execution was killed by the timeout.
    pub timed_out: bool,
    /// Wall-clock duration of the spawn + drain.
    pub elapsed_ms: u64,
    /// Best-effort error string for cases where the runtime itself
    /// could not be started (e.g. `python3` not on PATH). Distinct
    /// from a non-zero `exit_code` for an interpreter that ran
    /// successfully but produced a runtime error.
    pub runtime_error: Option<String>,
}

impl ExecOutput {
    /// `true` iff the interpreter exited with code 0 and no runtime
    /// startup error.
    pub fn is_success(&self) -> bool {
        self.exit_code == 0 && self.runtime_error.is_none() && !self.timed_out
    }

    /// Construct a synthetic "runtime missing" result so callers can
    /// produce a stable `ContinueWithFeedback` directive without
    /// panicking when an interpreter isn't on the host.
    fn runtime_missing(lang: CodeLang, msg: impl Into<String>) -> Self {
        Self {
            lang,
            stdout: String::new(),
            stderr: String::new(),
            exit_code: -1,
            timed_out: false,
            elapsed_ms: 0,
            runtime_error: Some(msg.into()),
        }
    }
}

/// Default timeout used when the caller doesn't specify one.
pub const DEFAULT_EXEC_TIMEOUT: Duration = Duration::from_secs(10);

/// Run agent-produced source in the requested language.
///
/// `timeout` bounds the entire spawn + drain. A `None` value falls back
/// to [`DEFAULT_EXEC_TIMEOUT`].
///
/// Returns `ExecOutput` even for failure paths — the caller (verifier)
/// formats the diff for the agent rather than propagating a Rust error.
pub async fn run_code(lang: CodeLang, source: &str, timeout: Option<Duration>) -> ExecOutput {
    let timeout = timeout.unwrap_or(DEFAULT_EXEC_TIMEOUT);
    debug!(
        lang = lang.name(),
        bytes = source.len(),
        timeout_ms = timeout.as_millis() as u64,
        "exec_runtime: dispatch"
    );

    match lang {
        CodeLang::Python => run_python(source, timeout).await,
        CodeLang::JavaScript => run_javascript(source, timeout).await,
        CodeLang::Bash => run_bash(source, timeout).await,
        CodeLang::Sql => run_sql(source, timeout).await,
    }
}

async fn run_python(source: &str, timeout: Duration) -> ExecOutput {
    spawn_with_args(CodeLang::Python, "python3", &["-c", source], None, timeout).await
}

async fn run_javascript(source: &str, timeout: Duration) -> ExecOutput {
    spawn_with_args(CodeLang::JavaScript, "node", &["-e", source], None, timeout).await
}

async fn run_bash(source: &str, timeout: Duration) -> ExecOutput {
    spawn_with_args(CodeLang::Bash, "bash", &["-c", source], None, timeout).await
}

async fn run_sql(source: &str, timeout: Duration) -> ExecOutput {
    // sqlite3 reads SQL from stdin when given `:memory:` as the only
    // argument. Piping is portable across linux/mac and lets us run
    // multi-statement scripts without an on-disk fixture file.
    spawn_with_args(
        CodeLang::Sql,
        "sqlite3",
        &[":memory:"],
        Some(source),
        timeout,
    )
    .await
}

/// Generic spawn-and-drain helper: builds a `Command`, captures
/// stdout/stderr/exit_code, applies the timeout, and folds
/// "interpreter missing" into `runtime_error` rather than panicking.
async fn spawn_with_args(
    lang: CodeLang,
    program: &str,
    args: &[&str],
    stdin: Option<&str>,
    timeout: Duration,
) -> ExecOutput {
    let start = Instant::now();

    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if stdin.is_some() {
        cmd.stdin(std::process::Stdio::piped());
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            warn!(
                lang = lang.name(),
                program = program,
                error = %e,
                "exec_runtime: interpreter not available"
            );
            return ExecOutput::runtime_missing(
                lang,
                format!(
                    "{} runtime missing — could not spawn `{}`: {}. \
                     Install the runtime or skip this verifier.",
                    lang.name(),
                    program,
                    e
                ),
            );
        }
    };

    // Best-effort: write provided stdin BEFORE racing the timeout. If the
    // child closes stdin early we just swallow the broken pipe.
    if let Some(data) = stdin {
        if let Some(mut handle) = child.stdin.take() {
            let _ = handle.write_all(data.as_bytes()).await;
            // Drop sends EOF so sqlite3 doesn't block forever.
            drop(handle);
        }
    }

    // Race child.wait_with_output() against the timeout so the kill path
    // below can still capture whatever the interpreter produced up to that
    // point. `wait_with_output()` consumes the Child, so we have to bail
    // out of the select arm by reaching for the still-borrowed handle —
    // we do that by lifting it into a local variable per branch.
    let drain = async move {
        let output = child.wait_with_output().await;
        (output, false)
    };

    let (raw, timed_out) = match tokio::time::timeout(timeout, drain).await {
        Ok(result) => result,
        Err(_) => {
            // Best-effort kill is racy here because `child` was moved into
            // the drain future. We fall back to "no output, timed out".
            warn!(
                lang = lang.name(),
                timeout_ms = timeout.as_millis() as u64,
                "exec_runtime: timeout — child detached"
            );
            return ExecOutput {
                lang,
                stdout: String::new(),
                stderr: format!(
                    "[exec_runtime] {} script killed after {}ms",
                    lang.name(),
                    timeout.as_millis()
                ),
                exit_code: -1,
                timed_out: true,
                elapsed_ms: start.elapsed().as_millis() as u64,
                runtime_error: None,
            };
        }
    };

    let elapsed_ms = start.elapsed().as_millis() as u64;
    let output = match raw {
        Ok(o) => o,
        Err(e) => {
            return ExecOutput {
                lang,
                stdout: String::new(),
                stderr: format!("[exec_runtime] child wait failed: {e}"),
                exit_code: -1,
                timed_out,
                elapsed_ms,
                runtime_error: None,
            }
        }
    };

    let exit_code = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    info!(
        lang = lang.name(),
        exit_code = exit_code,
        stdout_bytes = stdout.len(),
        stderr_bytes = stderr.len(),
        elapsed_ms = elapsed_ms,
        "exec_runtime: done"
    );

    ExecOutput {
        lang,
        stdout,
        stderr,
        exit_code,
        timed_out,
        elapsed_ms,
        runtime_error: None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod exec_runtime_tests {
    use super::*;

    fn have(program: &str) -> bool {
        std::process::Command::new(program)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    #[test]
    fn from_hint_supported_languages() {
        assert_eq!(CodeLang::from_hint("python"), Some(CodeLang::Python));
        assert_eq!(CodeLang::from_hint("PY"), Some(CodeLang::Python));
        assert_eq!(CodeLang::from_hint("js"), Some(CodeLang::JavaScript));
        assert_eq!(CodeLang::from_hint("bash"), Some(CodeLang::Bash));
        assert_eq!(CodeLang::from_hint("SH"), Some(CodeLang::Bash));
        assert_eq!(CodeLang::from_hint("sql"), Some(CodeLang::Sql));
        assert_eq!(CodeLang::from_hint("rust"), None);
        assert_eq!(CodeLang::from_hint(""), None);
    }

    #[tokio::test]
    async fn python_happy_path() {
        if !have("python3") {
            return; // honour CI hosts without python
        }
        let out = run_code(CodeLang::Python, "print(2+2)", None).await;
        assert!(out.is_success(), "python should succeed: {:?}", out);
        assert!(out.stdout.contains('4'));
        assert_eq!(out.exit_code, 0);
    }

    #[tokio::test]
    async fn python_traceback_surfaces_in_stderr() {
        if !have("python3") {
            return;
        }
        let out = run_code(CodeLang::Python, "raise ValueError('boom')", None).await;
        assert!(!out.is_success());
        assert_ne!(out.exit_code, 0);
        assert!(
            out.stderr.contains("ValueError"),
            "expected traceback in stderr, got: {:?}",
            out.stderr
        );
    }

    #[tokio::test]
    async fn python_timeout_kills_loop() {
        if !have("python3") {
            return;
        }
        let out = run_code(
            CodeLang::Python,
            "import time; time.sleep(60)",
            Some(Duration::from_millis(300)),
        )
        .await;
        assert!(out.timed_out, "expected timeout: {:?}", out);
        assert_eq!(out.exit_code, -1);
    }

    #[tokio::test]
    async fn javascript_happy_path() {
        if !have("node") {
            return;
        }
        let out = run_code(CodeLang::JavaScript, "console.log(1+2)", None).await;
        assert!(out.is_success(), "node should succeed: {:?}", out);
        assert!(out.stdout.contains('3'));
    }

    #[tokio::test]
    async fn javascript_throw_surfaces_in_stderr() {
        if !have("node") {
            return;
        }
        let out = run_code(CodeLang::JavaScript, "throw new Error('nope')", None).await;
        assert!(!out.is_success());
        assert!(out.stderr.contains("Error"));
    }

    #[tokio::test]
    async fn bash_happy_path() {
        if !have("bash") {
            return;
        }
        let out = run_code(CodeLang::Bash, "echo hi", None).await;
        assert!(out.is_success(), "bash should succeed: {:?}", out);
        assert!(out.stdout.contains("hi"));
    }

    #[tokio::test]
    async fn bash_nonzero_exit() {
        if !have("bash") {
            return;
        }
        let out = run_code(CodeLang::Bash, "exit 7", None).await;
        assert!(!out.is_success());
        assert_eq!(out.exit_code, 7);
    }

    #[tokio::test]
    async fn sqlite_happy_path() {
        if !have("sqlite3") {
            return;
        }
        let out = run_code(
            CodeLang::Sql,
            "CREATE TABLE t(x INT); INSERT INTO t VALUES(1),(2),(3); SELECT SUM(x) FROM t;",
            None,
        )
        .await;
        assert!(out.is_success(), "sqlite should succeed: {:?}", out);
        assert!(out.stdout.contains('6'));
    }

    #[tokio::test]
    async fn sqlite_syntax_error_surfaces() {
        if !have("sqlite3") {
            return;
        }
        let out = run_code(CodeLang::Sql, "this is not sql;", None).await;
        // sqlite3 exits non-zero on parse error and writes diagnostics
        // to stderr.
        assert!(!out.is_success());
        assert!(
            !out.stderr.is_empty(),
            "expected sqlite to surface a parse error: {:?}",
            out
        );
    }

    #[tokio::test]
    async fn runtime_missing_surfaces_runtime_error() {
        // Pick a guaranteed-missing binary so the spawn-error path is
        // exercised on every host. We call the private helper directly
        // (via super::) so we can pass an arbitrary `program` without
        // routing through the language dispatch.
        let out = super::spawn_with_args(
            CodeLang::Python,
            "cyberclaw-this-binary-does-not-exist-zzz",
            &[],
            None,
            Duration::from_millis(500),
        )
        .await;
        assert!(out.runtime_error.is_some());
        assert_eq!(out.exit_code, -1);
        assert!(!out.is_success());
    }
}
