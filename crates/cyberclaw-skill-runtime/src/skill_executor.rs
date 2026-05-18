//! SkillExecutor — translates a Skill text + task input into a sandbox request.
//!
//! This is **NOT** an executor in the sense of "runs the code itself". It is a
//! planner: it reads a `SKILL.md`, decides *what* to run, and returns a
//! [`SkillExecutionPlan`] that the caller dispatches via a sandbox capability.
//! This keeps the invariant that execution goes through `Connector → Capability`.
//!
//! # Responsibilities
//!
//! 1. Parse `SKILL.md` YAML frontmatter and body.
//! 2. Decide an execution entrypoint (a script reference in frontmatter OR the
//!    body itself as an inline bash script).
//! 3. Render the task input into env / stdin so downstream sandbox connectors
//!    can run the plan without knowing anything about skill formats.
//! 4. Parse a sandbox response back into a structured [`SkillExecutionOutcome`]
//!    (including best-effort JSON parsing of stdout).
//!
//! # Non-responsibilities
//!
//! - The module does **not** spawn processes or touch the filesystem.
//! - It does **not** dispatch capabilities.
//! - It does **not** load external script files from disk — v1 uses only what's
//!   inline in the `SKILL.md`.
//!
//! # v1 frontmatter parser
//!
//! v1 frontmatter is parsed with `serde_yaml` as a flat `HashMap<String, Value>`
//! and then projected into [`SkillMetadata`]. Nested YAML structures beyond the
//! documented keys are accepted but ignored — they're not surfaced yet.

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Default hint timeout applied to execution plans when nothing else is set.
pub const DEFAULT_HINT_TIMEOUT: Duration = Duration::from_secs(30);

/// Metadata extracted from the YAML frontmatter of a `SKILL.md`.
///
/// Only the fields we currently act on are projected into strong types; any
/// other frontmatter keys are ignored in v1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillMetadata {
    /// Skill name (required).
    pub name: String,
    /// Freeform description, if provided.
    pub description: Option<String>,
    /// Entrypoint reference. For v1 this is an inline script or shell one-liner
    /// that will be passed to `sh -c`. File paths like `scripts/run.sh` are
    /// accepted at the metadata level but NOT resolved by v1 — the planner will
    /// still wrap them in `sh -c`, which means they only work if the script
    /// they name is already on disk and on `PATH` at dispatch time.
    pub entrypoint: Option<String>,
    /// Declared implementation language (e.g. `"bash"`, `"python"`). v1 only
    /// reacts to `"python"` to (attempt to) pick `InlinePython` strategy.
    pub language: Option<String>,
}

/// Plan for running a Skill in a sandbox.
///
/// This is a pure value — it owns no handles and starts no processes. The
/// caller maps it into whatever request shape its sandbox Connector expects
/// (e.g. `SandboxInput` from `cyberclaw-connectors`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillExecutionPlan {
    /// Program to invoke in the sandbox (e.g. `"sh"`, `"python3"`).
    pub command: String,
    /// Args (typically `["-c", inline_script]` or a single script path).
    pub args: Vec<String>,
    /// Environment variables to inject (including `TASK_INPUT` as JSON).
    pub env: HashMap<String, String>,
    /// Stdin piped into the child (task input as JSON, for stdin-based skills).
    pub stdin: Option<String>,
    /// Expected runtime for the skill — a hint for the sandbox timeout.
    pub hint_timeout: Duration,
}

/// Structured outcome parsed from a sandbox response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillExecutionOutcome {
    /// Raw exit code from the child process. `None` when the sandbox timed out
    /// or the process was killed before producing an exit status.
    pub exit_code: Option<i32>,
    /// Captured stdout (bounded by whatever limit the sandbox enforces).
    pub stdout: String,
    /// Captured stderr (bounded by whatever limit the sandbox enforces).
    pub stderr: String,
    /// Best-effort JSON parse of stdout. `None` when stdout is empty or not
    /// valid JSON; the raw text is still available via `stdout`.
    pub parsed_output: Option<Value>,
    /// Wall-clock time the child took, in milliseconds.
    pub elapsed_ms: u64,
    /// Whether the sandbox reported a timeout.
    pub timed_out: bool,
}

/// Strategy for translating Skill text into an execution plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionStrategy {
    /// Default: inline bash script from frontmatter `entrypoint`, task input
    /// rendered as `TASK_INPUT` env var and as stdin JSON.
    InlineBash,
    /// Python: same but with `python3 -c` wrapper.
    ///
    /// v1 placeholder — falls back to `InlineBash` if the skill's declared
    /// `language` is not `"python"` (case-insensitive). When it IS python,
    /// the entrypoint is treated as Python source.
    InlinePython,
}

/// View of a sandbox response the executor needs, decoupled from the concrete
/// `SandboxResponse` type in `cyberclaw-connectors`.
///
/// This avoids a hard dependency on `cyberclaw-connectors` from this crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxResponseView {
    /// Raw exit code. `None` if the process didn't exit normally.
    pub exit_code: Option<i32>,
    /// Captured stdout bytes, lossy-UTF-8-decoded.
    pub stdout: String,
    /// Captured stderr bytes, lossy-UTF-8-decoded.
    pub stderr: String,
    /// Wall-clock elapsed time, in milliseconds.
    pub elapsed_ms: u64,
    /// Whether the sandbox killed the child due to timeout.
    pub timed_out: bool,
}

/// Env variable name used to pass the task input JSON into the sandbox child.
pub const TASK_INPUT_ENV: &str = "TASK_INPUT";

/// Plans Skill executions. See module docs for responsibilities.
#[derive(Debug, Clone)]
pub struct SkillExecutor {
    default_strategy: ExecutionStrategy,
    default_timeout: Duration,
}

impl Default for SkillExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl SkillExecutor {
    /// Build an executor with `InlineBash` strategy and [`DEFAULT_HINT_TIMEOUT`].
    pub fn new() -> Self {
        Self {
            default_strategy: ExecutionStrategy::InlineBash,
            default_timeout: DEFAULT_HINT_TIMEOUT,
        }
    }

    /// Override the default strategy. Individual plans can still override
    /// based on skill metadata (e.g. `language: python` + `InlinePython`).
    pub fn with_strategy(mut self, strategy: ExecutionStrategy) -> Self {
        self.default_strategy = strategy;
        self
    }

    /// Override the default hint timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    /// Parse a `SKILL.md`'s YAML frontmatter. Returns `(metadata, body)`.
    ///
    /// Expects frontmatter delimited by `---` lines:
    ///
    /// ```text
    /// ---
    /// name: my-skill
    /// description: greets the user
    /// ---
    /// body markdown here
    /// ```
    ///
    /// # Errors
    ///
    /// - Frontmatter block missing or not closed.
    /// - Frontmatter is not valid YAML.
    /// - Required `name` field is missing.
    pub fn parse_skill(&self, skill_text: &str) -> anyhow::Result<(SkillMetadata, String)> {
        let (yaml_block, body) = split_frontmatter(skill_text)?;
        let map: HashMap<String, serde_yaml::Value> = serde_yaml::from_str(yaml_block)
            .map_err(|e| anyhow::anyhow!("invalid YAML frontmatter: {e}"))?;

        let name = map
            .get("name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("frontmatter missing required `name` field"))?;

        let description = map
            .get("description")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let entrypoint = map
            .get("entrypoint")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let language = map
            .get("language")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        Ok((
            SkillMetadata {
                name,
                description,
                entrypoint,
                language,
            },
            body,
        ))
    }

    /// Build an execution plan from `skill_text` + `task_input`.
    pub fn plan_execution(
        &self,
        skill_text: &str,
        task_input: &Value,
    ) -> anyhow::Result<SkillExecutionPlan> {
        let (meta, body) = self.parse_skill(skill_text)?;

        // Choose the strategy — v1 only auto-switches to InlinePython when the
        // skill explicitly declares `language: python` (case-insensitive).
        let strategy = match meta.language.as_deref() {
            Some(lang) if lang.eq_ignore_ascii_case("python") => ExecutionStrategy::InlinePython,
            _ => self.default_strategy,
        };

        // Script source: prefer explicit entrypoint, else fall back to body.
        let script = match meta.entrypoint.as_deref() {
            Some(ep) if !ep.trim().is_empty() => ep.to_string(),
            _ => body.trim().to_string(),
        };

        if script.is_empty() {
            return Err(anyhow::anyhow!(
                "skill has neither `entrypoint` nor non-empty body"
            ));
        }

        let task_input_json = serde_json::to_string(task_input)
            .map_err(|e| anyhow::anyhow!("failed to serialize task input: {e}"))?;

        let mut env = HashMap::new();
        env.insert(TASK_INPUT_ENV.to_string(), task_input_json.clone());

        let (command, args) = match strategy {
            ExecutionStrategy::InlineBash => {
                ("sh".to_string(), vec!["-c".to_string(), script.clone()])
            }
            ExecutionStrategy::InlinePython => (
                "python3".to_string(),
                vec!["-c".to_string(), script.clone()],
            ),
        };

        Ok(SkillExecutionPlan {
            command,
            args,
            env,
            stdin: Some(task_input_json),
            hint_timeout: self.default_timeout,
        })
    }

    /// Parse a sandbox response into a structured outcome.
    ///
    /// Attempts a best-effort JSON parse of stdout. If stdout is empty or not
    /// valid JSON, `parsed_output` is `None` but `stdout` is still captured.
    pub fn parse_outcome(&self, response: &SandboxResponseView) -> SkillExecutionOutcome {
        let parsed_output = if response.stdout.trim().is_empty() {
            None
        } else {
            serde_json::from_str::<Value>(response.stdout.trim()).ok()
        };

        SkillExecutionOutcome {
            exit_code: response.exit_code,
            stdout: response.stdout.clone(),
            stderr: response.stderr.clone(),
            parsed_output,
            elapsed_ms: response.elapsed_ms,
            timed_out: response.timed_out,
        }
    }
}

/// Split `SKILL.md` text into (frontmatter_yaml, body).
///
/// Expects the very first non-whitespace line to be `---`, followed by YAML,
/// followed by a closing `---` line. Everything after is returned as body.
fn split_frontmatter(skill_text: &str) -> anyhow::Result<(&str, String)> {
    // Skip leading blank lines/whitespace but keep byte-index tracking.
    let trimmed_start = skill_text.trim_start_matches(|c: char| c.is_whitespace() && c != '\n');
    let after_leading_ws = trimmed_start.trim_start_matches('\n');

    // First line must be a `---` marker.
    let first_line_end = after_leading_ws
        .find('\n')
        .ok_or_else(|| anyhow::anyhow!("SKILL.md has no frontmatter (no newline found)"))?;
    let first_line = after_leading_ws[..first_line_end].trim_end();
    if first_line != "---" {
        return Err(anyhow::anyhow!(
            "SKILL.md must begin with `---` frontmatter marker"
        ));
    }

    let rest = &after_leading_ws[first_line_end + 1..];

    // Find the closing `---` on its own line.
    let mut cursor = 0usize;
    let closing_offset = loop {
        let line_end = match rest[cursor..].find('\n') {
            Some(n) => cursor + n,
            None => {
                // No newline — check if the remaining slice is itself the closer.
                if rest[cursor..].trim_end() == "---" {
                    // Closer with no trailing body.
                    let yaml = &rest[..cursor];
                    return Ok((yaml, String::new()));
                }
                return Err(anyhow::anyhow!("frontmatter not closed with `---`"));
            }
        };
        let line = rest[cursor..line_end].trim_end();
        if line == "---" {
            break cursor;
        }
        cursor = line_end + 1;
    };

    let yaml_block = &rest[..closing_offset];
    // Skip the closing `---` line.
    let after_closing = rest[closing_offset..]
        .find('\n')
        .map(|n| &rest[closing_offset + n + 1..])
        .unwrap_or("");

    Ok((yaml_block, after_closing.to_string()))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const HAPPY_SKILL: &str = "---\n\
name: greeter\n\
description: greets the user\n\
entrypoint: echo hello\n\
language: bash\n\
---\n\
# Body\n\
\n\
This is the body.\n";

    #[test]
    fn parse_skill_happy_path() {
        let ex = SkillExecutor::new();
        let (meta, body) = ex.parse_skill(HAPPY_SKILL).expect("parse ok");
        assert_eq!(meta.name, "greeter");
        assert_eq!(meta.description.as_deref(), Some("greets the user"));
        assert_eq!(meta.entrypoint.as_deref(), Some("echo hello"));
        assert_eq!(meta.language.as_deref(), Some("bash"));
        assert!(body.contains("# Body"));
        assert!(body.contains("This is the body."));
    }

    #[test]
    fn parse_skill_missing_frontmatter_errors() {
        let ex = SkillExecutor::new();
        let err = ex
            .parse_skill("# Just a header, no frontmatter\n\nbody")
            .unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("---") || msg.contains("frontmatter"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn parse_skill_with_entrypoint() {
        let ex = SkillExecutor::new();
        let text = "---\nname: s\nentrypoint: scripts/run.sh\n---\nbody\n";
        let (meta, _body) = ex.parse_skill(text).expect("parse ok");
        assert_eq!(meta.entrypoint.as_deref(), Some("scripts/run.sh"));
    }

    #[test]
    fn parse_skill_with_description_and_language() {
        let ex = SkillExecutor::new();
        let text = "---\nname: s\ndescription: d\nlanguage: python\n---\nbody\n";
        let (meta, _body) = ex.parse_skill(text).expect("parse ok");
        assert_eq!(meta.description.as_deref(), Some("d"));
        assert_eq!(meta.language.as_deref(), Some("python"));
    }

    #[test]
    fn parse_skill_requires_name() {
        let ex = SkillExecutor::new();
        let text = "---\ndescription: nope\n---\nbody\n";
        let err = ex.parse_skill(text).unwrap_err();
        assert!(format!("{err}").contains("name"));
    }

    #[test]
    fn plan_execution_injects_task_input_env() {
        let ex = SkillExecutor::new();
        let plan = ex
            .plan_execution(HAPPY_SKILL, &json!({"x": 1}))
            .expect("plan ok");
        assert_eq!(
            plan.env.get(TASK_INPUT_ENV).map(String::as_str),
            Some("{\"x\":1}")
        );
    }

    #[test]
    fn plan_execution_injects_task_input_stdin() {
        let ex = SkillExecutor::new();
        let plan = ex
            .plan_execution(HAPPY_SKILL, &json!({"x": 1}))
            .expect("plan ok");
        assert_eq!(plan.stdin.as_deref(), Some("{\"x\":1}"));
    }

    #[test]
    fn plan_execution_uses_entrypoint_when_set() {
        let ex = SkillExecutor::new();
        let plan = ex
            .plan_execution(HAPPY_SKILL, &json!(null))
            .expect("plan ok");
        assert_eq!(plan.command, "sh");
        assert_eq!(plan.args, vec!["-c".to_string(), "echo hello".to_string()]);
    }

    #[test]
    fn plan_execution_falls_back_to_body_when_no_entrypoint() {
        let ex = SkillExecutor::new();
        let skill = "---\nname: fallback\n---\necho from-body\n";
        let plan = ex.plan_execution(skill, &json!(null)).expect("plan ok");
        assert_eq!(plan.command, "sh");
        assert_eq!(plan.args[0], "-c");
        assert!(
            plan.args[1].contains("echo from-body"),
            "expected body to be used as script, got {:?}",
            plan.args
        );
    }

    #[test]
    fn plan_execution_respects_timeout_override() {
        let ex = SkillExecutor::new().with_timeout(Duration::from_millis(777));
        let plan = ex
            .plan_execution(HAPPY_SKILL, &json!(null))
            .expect("plan ok");
        assert_eq!(plan.hint_timeout, Duration::from_millis(777));
    }

    #[test]
    fn plan_execution_python_language_switches_strategy() {
        let ex = SkillExecutor::new();
        let skill = "---\nname: p\nlanguage: python\nentrypoint: print('hi')\n---\nignored body\n";
        let plan = ex.plan_execution(skill, &json!(null)).expect("plan ok");
        assert_eq!(plan.command, "python3");
        assert_eq!(plan.args, vec!["-c".to_string(), "print('hi')".to_string()]);
    }

    #[test]
    fn plan_execution_errors_on_empty_script() {
        let ex = SkillExecutor::new();
        let skill = "---\nname: empty\n---\n";
        let err = ex.plan_execution(skill, &json!(null)).unwrap_err();
        assert!(format!("{err}").contains("entrypoint") || format!("{err}").contains("body"));
    }

    #[test]
    fn parse_outcome_with_valid_json_stdout() {
        let ex = SkillExecutor::new();
        let resp = SandboxResponseView {
            exit_code: Some(0),
            stdout: "{\"ok\":true}".to_string(),
            stderr: String::new(),
            elapsed_ms: 12,
            timed_out: false,
        };
        let out = ex.parse_outcome(&resp);
        assert_eq!(out.exit_code, Some(0));
        assert_eq!(out.parsed_output, Some(json!({"ok": true})));
        assert_eq!(out.stdout, "{\"ok\":true}");
    }

    #[test]
    fn parse_outcome_with_non_json_stdout() {
        let ex = SkillExecutor::new();
        let resp = SandboxResponseView {
            exit_code: Some(0),
            stdout: "hello world".to_string(),
            stderr: String::new(),
            elapsed_ms: 3,
            timed_out: false,
        };
        let out = ex.parse_outcome(&resp);
        assert_eq!(out.parsed_output, None);
        assert_eq!(out.stdout, "hello world");
    }

    #[test]
    fn parse_outcome_captures_exit_code() {
        let ex = SkillExecutor::new();
        let resp = SandboxResponseView {
            exit_code: Some(7),
            stdout: "".to_string(),
            stderr: "boom".to_string(),
            elapsed_ms: 1,
            timed_out: false,
        };
        let out = ex.parse_outcome(&resp);
        assert_eq!(out.exit_code, Some(7));
        assert_eq!(out.stderr, "boom");
        assert_eq!(out.parsed_output, None);
    }

    #[test]
    fn parse_outcome_handles_timeout() {
        let ex = SkillExecutor::new();
        let resp = SandboxResponseView {
            exit_code: None,
            stdout: "partial".to_string(),
            stderr: String::new(),
            elapsed_ms: 30_000,
            timed_out: true,
        };
        let out = ex.parse_outcome(&resp);
        assert!(out.timed_out);
        assert_eq!(out.exit_code, None);
        assert_eq!(out.stdout, "partial");
    }
}
