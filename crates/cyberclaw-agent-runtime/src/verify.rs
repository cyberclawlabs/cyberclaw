//! # OutputVerifier — R2 sprint 3 PART A (2026-05-21)
//!
//! Baseline F-class (codegen) prompts scored cb 7%–93% vs hermes 57%–87%.
//! Root cause: the cb agentic loop produces source code and immediately
//! `stop()`s — never running the code and never reading the traceback.
//! Hermes uses a `code_execution` toolset to run the code, look at the
//! traceback, and self-correct. This module gives cb the same lever
//! **without** patching the F-class prompts.
//!
//! ## Design (optimization-plan-v1 §R2)
//!
//! - [`OutputVerifier`] is a `Send + Sync` trait — one trait, many strategies.
//! - [`VerifierChain`] runs verifiers in declared order with a defined merge
//!   strategy (see `run()` doc-comment).
//! - Three built-in verifiers cover the bulk of the baseline failure modes:
//!     - [`CodeBlockVerifier`] — extracts fenced code blocks, runs them,
//!       compares stdout to an expected pattern. Closes F-class.
//!     - [`JsonStructureVerifier`] — checks JSON-shaped output for required
//!       keys / item count. Closes E-class "5-step plan" prompts.
//!     - [`RegexAssertVerifier`] — runs prompt-declared expectation regexes
//!       against the response. Closes pricing / numeric self-checks.
//!
//! Sprint 3b will wire these into [`crate::agentic_loop`] — see
//! "Wiring hooks for sprint 3b" at the bottom of this file.
//!
//! ## What this module does NOT do
//!
//! - Does NOT modify `agentic_loop.rs` (sprint 3b territory).
//! - Does NOT expand the LLM-visible tool surface — `CodeBlockVerifier`
//!   calls a function-level helper in
//!   `cyberclaw_connectors::local::exec_runtime`, not a `Capability`.
//! - Does NOT patch F-class prompts. The verifier chain is a post-hoc
//!   gate, run after the agent says it is done.

use std::sync::Arc;

use async_trait::async_trait;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, warn};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Context handed to every verifier — agentic-loop carries this from the
/// prompt that triggered the iteration. Sprint 3b will populate it from
/// `AgenticLoop::ctx`; for now it is a free-standing struct so unit tests
/// can fabricate it without dragging the full loop state.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VerifyCtx {
    /// Optional language hint copied from prompt metadata
    /// (e.g. `lang: python`). Lets `CodeBlockVerifier` skip the hint-
    /// detection step when the prompt is unambiguous.
    pub language_hint: Option<String>,
    /// Optional substring that the response must contain after the
    /// code is executed. When set, `CodeBlockVerifier` fails the
    /// verification if the captured stdout does not contain it.
    pub expected_stdout_contains: Option<String>,
    /// Optional regex (one per entry) that the response is required
    /// to match. Used by `RegexAssertVerifier` directly when no
    /// regexes are configured on the verifier itself.
    pub expected_regexes: Vec<String>,
    /// Optional JSON-structure expectation. When `expected_json_keys`
    /// is non-empty, `JsonStructureVerifier` checks that the response
    /// parses as a JSON object containing every key. When
    /// `expected_json_array_len` is set, it checks the top-level
    /// value is an array of exactly that length.
    pub expected_json_keys: Vec<String>,
    /// Optional expected length of the top-level JSON array.
    pub expected_json_array_len: Option<usize>,
}

impl VerifyCtx {
    /// Convenience builder.
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_language_hint(mut self, hint: impl Into<String>) -> Self {
        self.language_hint = Some(hint.into());
        self
    }

    pub fn with_expected_stdout(mut self, needle: impl Into<String>) -> Self {
        self.expected_stdout_contains = Some(needle.into());
        self
    }

    pub fn with_expected_regex(mut self, pattern: impl Into<String>) -> Self {
        self.expected_regexes.push(pattern.into());
        self
    }

    pub fn with_expected_json_keys(mut self, keys: impl IntoIterator<Item = String>) -> Self {
        self.expected_json_keys = keys.into_iter().collect();
        self
    }

    pub fn with_expected_json_array_len(mut self, n: usize) -> Self {
        self.expected_json_array_len = Some(n);
        self
    }
}

/// Continuation directive returned by every verifier.
///
/// Semantics match optimization-plan-v1 §R2:
///   - `Accept` — verifier is happy, loop can finalize.
///   - `ContinueWithFeedback(msg)` — verifier wants another iteration
///     but the existing response is salvageable; `msg` is prepended to
///     the next-iteration user turn.
///   - `RejectAndRetry(msg)` — verifier considers the response wrong;
///     `msg` explains why and the loop re-runs the same prompt with the
///     feedback as additional user instruction. Burns an iteration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "feedback")]
pub enum VerificationDirective {
    Accept,
    ContinueWithFeedback(String),
    RejectAndRetry(String),
}

impl VerificationDirective {
    /// `true` iff the response can be finalized.
    pub fn is_accept(&self) -> bool {
        matches!(self, Self::Accept)
    }

    /// Borrow the feedback string for the non-`Accept` variants.
    pub fn feedback(&self) -> Option<&str> {
        match self {
            Self::Accept => None,
            Self::ContinueWithFeedback(s) | Self::RejectAndRetry(s) => Some(s.as_str()),
        }
    }
}

/// Trait implemented by every verifier.
///
/// The trait is `Send + Sync` so a `Vec<Box<dyn OutputVerifier>>` can
/// be dispatched across the async agentic loop without `Mutex`.
#[async_trait]
pub trait OutputVerifier: Send + Sync {
    /// Inspect a final assistant response.
    ///
    /// `prompt` — the user prompt that triggered the iteration. Verifiers
    /// may inspect it (e.g. to find expected substrings); they should NOT
    /// rewrite or echo it.
    ///
    /// `response` — the assistant message about to be finalized.
    ///
    /// `ctx` — verifier-shared context (see [`VerifyCtx`]).
    async fn verify(
        &self,
        prompt: &str,
        response: &str,
        ctx: &VerifyCtx,
    ) -> VerificationDirective;

    /// Stable identifier — used in log output and chain merging
    /// (later verifiers running with the same name are dropped to
    /// keep the chain idempotent).
    fn name(&self) -> &'static str;
}

// ---------------------------------------------------------------------------
// Chain
// ---------------------------------------------------------------------------

/// Ordered list of [`OutputVerifier`]s with a defined merge rule.
///
/// ## Merge rule (`run()`)
///
/// Verifiers run in declared order. The chain short-circuits on the
/// first `RejectAndRetry` (hard fail — return immediately). Otherwise
/// it merges the directives:
///
///  - If any verifier returned `ContinueWithFeedback`, the chain
///    returns `ContinueWithFeedback(<merged feedback>)`.
///  - Otherwise it returns `Accept`.
///
/// Rationale:
///  - `RejectAndRetry` is the strongest signal — one failed code
///    execution invalidates the whole response, regardless of what
///    the regex/JSON verifier thought. Short-circuit avoids running
///    cheap regex checks that would be misleading.
///  - `ContinueWithFeedback` is additive — multiple verifiers can
///    each have something to say, so we concatenate their feedback
///    bullets into one message for the next iteration.
pub struct VerifierChain {
    verifiers: Vec<Box<dyn OutputVerifier>>,
}

impl Default for VerifierChain {
    fn default() -> Self {
        Self::new()
    }
}

impl VerifierChain {
    /// Empty chain. Add verifiers with [`Self::add`].
    pub fn new() -> Self {
        Self {
            verifiers: Vec::new(),
        }
    }

    /// Builder-style add. Verifiers with duplicate `name()` are
    /// silently dropped — the first registration wins to keep
    /// chains idempotent when the loop attaches default verifiers
    /// before the caller adds their own.
    #[allow(clippy::should_implement_trait)] // builder-style; not std::ops::Add semantics
    pub fn add(mut self, v: Box<dyn OutputVerifier>) -> Self {
        if self.verifiers.iter().any(|existing| existing.name() == v.name()) {
            debug!(name = v.name(), "verifier_chain: dropped duplicate");
        } else {
            self.verifiers.push(v);
        }
        self
    }

    /// Number of verifiers in the chain.
    pub fn len(&self) -> usize {
        self.verifiers.len()
    }

    /// `true` iff the chain has no verifiers.
    pub fn is_empty(&self) -> bool {
        self.verifiers.is_empty()
    }

    /// Run every verifier in order. See struct doc-comment for the
    /// merge rule.
    pub async fn run(
        &self,
        prompt: &str,
        response: &str,
        ctx: &VerifyCtx,
    ) -> VerificationDirective {
        let mut feedback: Vec<String> = Vec::new();

        for v in &self.verifiers {
            let directive = v.verify(prompt, response, ctx).await;
            debug!(
                verifier = v.name(),
                directive = ?directive,
                "verifier_chain: step"
            );
            match directive {
                VerificationDirective::Accept => {}
                VerificationDirective::ContinueWithFeedback(msg) => {
                    feedback.push(format!("[{}] {}", v.name(), msg));
                }
                VerificationDirective::RejectAndRetry(msg) => {
                    // Hard fail — short-circuit. Prepend any previously
                    // accumulated continuation feedback so the agent
                    // sees the full picture.
                    let mut combined = feedback;
                    combined.push(format!("[{}] {}", v.name(), msg));
                    return VerificationDirective::RejectAndRetry(combined.join("\n"));
                }
            }
        }

        if feedback.is_empty() {
            VerificationDirective::Accept
        } else {
            VerificationDirective::ContinueWithFeedback(feedback.join("\n"))
        }
    }
}

// ---------------------------------------------------------------------------
// CodeBlockVerifier
// ---------------------------------------------------------------------------

/// Function signature for the executor that `CodeBlockVerifier` uses
/// to actually run an extracted code block. Injected so the verifier
/// is testable without spawning subprocesses and so we don't need to
/// hard-code a dep on a specific runtime.
///
/// Returns `(stdout, stderr, exit_code, runtime_error)`. `exit_code`
/// is `-1` on timeout or spawn failure. `runtime_error` is `Some` when
/// the interpreter could not be started (e.g. python3 missing) — the
/// verifier folds that into `ContinueWithFeedback` rather than rejecting.
pub type CodeExecResult = (String, String, i32, Option<String>);

pub type CodeExecutor =
    Arc<dyn Fn(String, String) -> futures_or_pin::PinFut<CodeExecResult> + Send + Sync>;

// Lightweight local re-export so we don't pull a futures crate dep just
// for boxing a future. Mirrors the pattern in `tool_result_pipeline.rs`.
mod futures_or_pin {
    use std::future::Future;
    use std::pin::Pin;
    pub type PinFut<T> = Pin<Box<dyn Future<Output = T> + Send>>;
}

/// Extracts fenced ``` code blocks from the response and runs them via
/// the injected executor. Compares stdout against the expectation in
/// [`VerifyCtx::expected_stdout_contains`]; if the run fails (non-zero
/// exit / traceback / timeout), returns `ContinueWithFeedback` with
/// the stderr appended so the agent can self-correct on the next
/// iteration.
///
/// Decision tree:
///  - No fenced block found → `Accept` (this verifier has nothing to
///    say; the prompt didn't ask for code).
///  - Block found, no interpreter (runtime_error) → `ContinueWithFeedback`
///    explaining the missing runtime. Does NOT reject — the agent should
///    not be punished for a missing host tool.
///  - Block ran, exit_code == 0, expected_stdout_contains matched → `Accept`.
///  - Block ran, exit_code != 0 → `ContinueWithFeedback(stderr)`.
///  - Block ran, exit_code == 0, expected_stdout_contains missed →
///    `RejectAndRetry("expected … not found in stdout")`.
pub struct CodeBlockVerifier {
    executor: CodeExecutor,
    /// When `true`, only the first code block is executed. Default `true`
    /// — running every block in a long answer can blow the iteration
    /// budget for diminishing returns.
    first_block_only: bool,
}

impl CodeBlockVerifier {
    /// Build a verifier with a custom executor (typical for tests).
    pub fn with_executor(executor: CodeExecutor) -> Self {
        Self {
            executor,
            first_block_only: true,
        }
    }

    /// Override the first-block-only behaviour.
    pub fn run_all_blocks(mut self) -> Self {
        self.first_block_only = false;
        self
    }

    /// Default executor that calls into
    /// [`cyberclaw_connectors::local::exec_runtime::run_code`]. Only
    /// available when the `tool-calling` feature is enabled — that
    /// feature pulls the connectors crate in.
    #[cfg(feature = "tool-calling")]
    pub fn with_default_executor() -> Self {
        use std::time::Duration;
        let executor: CodeExecutor = Arc::new(|lang_hint: String, source: String| {
            Box::pin(async move {
                use cyberclaw_connectors::local::exec_runtime::{run_code, CodeLang};
                let lang = CodeLang::from_hint(&lang_hint).unwrap_or(CodeLang::Bash);
                let out = run_code(lang, &source, Some(Duration::from_secs(10))).await;
                (
                    out.stdout,
                    out.stderr,
                    out.exit_code,
                    out.runtime_error,
                )
            })
        });
        Self::with_executor(executor)
    }

    /// Extract fenced code blocks. Each entry is `(language_hint, source)`.
    /// Empty language hint stays empty (caller picks default).
    pub(crate) fn extract_blocks(response: &str) -> Vec<(String, String)> {
        let mut out = Vec::new();
        // We accept triple-backtick fences only — the LLM can emit `~~~`
        // but cb prompts have never observed it; keep regex simple.
        // The regex is non-greedy and uses (?s) for dotall.
        let re = Regex::new(r"(?s)```([^\n`]*)\n(.*?)```").expect("code-block regex compiles");
        for cap in re.captures_iter(response) {
            let lang = cap
                .get(1)
                .map(|m| m.as_str().trim().to_string())
                .unwrap_or_default();
            let body = cap
                .get(2)
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();
            if !body.trim().is_empty() {
                out.push((lang, body));
            }
        }
        out
    }
}

#[async_trait]
impl OutputVerifier for CodeBlockVerifier {
    async fn verify(
        &self,
        _prompt: &str,
        response: &str,
        ctx: &VerifyCtx,
    ) -> VerificationDirective {
        let blocks = Self::extract_blocks(response);
        if blocks.is_empty() {
            debug!("code_block_verifier: no fenced blocks — accept");
            return VerificationDirective::Accept;
        }

        let mut accumulated_feedback: Vec<String> = Vec::new();

        for (i, (raw_lang, source)) in blocks.iter().enumerate() {
            let lang = if raw_lang.is_empty() {
                ctx.language_hint.clone().unwrap_or_else(|| "bash".to_string())
            } else {
                raw_lang.clone()
            };

            debug!(
                idx = i,
                lang = lang.as_str(),
                bytes = source.len(),
                "code_block_verifier: executing block"
            );

            let (stdout, stderr, exit_code, runtime_error) =
                (self.executor)(lang.clone(), source.clone()).await;

            if let Some(err) = runtime_error {
                warn!(lang = lang.as_str(), error = %err, "code_block_verifier: runtime missing");
                accumulated_feedback.push(format!(
                    "code block {} ({}) could not be executed: {}",
                    i + 1,
                    lang,
                    err
                ));
                if self.first_block_only {
                    break;
                }
                continue;
            }

            if exit_code != 0 {
                let msg = format!(
                    "code block {} ({}) exited with code {}. stderr:\n{}\nstdout:\n{}",
                    i + 1,
                    lang,
                    exit_code,
                    truncate(&stderr, 1024),
                    truncate(&stdout, 256),
                );
                return VerificationDirective::ContinueWithFeedback(msg);
            }

            if let Some(needle) = &ctx.expected_stdout_contains {
                if !stdout.contains(needle) {
                    let msg = format!(
                        "code block {} ran successfully but expected stdout to contain `{}`. \
                         Actual stdout:\n{}",
                        i + 1,
                        needle,
                        truncate(&stdout, 1024),
                    );
                    return VerificationDirective::RejectAndRetry(msg);
                }
            }

            if self.first_block_only {
                break;
            }
        }

        if accumulated_feedback.is_empty() {
            VerificationDirective::Accept
        } else {
            VerificationDirective::ContinueWithFeedback(accumulated_feedback.join("\n"))
        }
    }

    fn name(&self) -> &'static str {
        "code_block"
    }
}

// ---------------------------------------------------------------------------
// JsonStructureVerifier
// ---------------------------------------------------------------------------

/// Verifies that the response contains a JSON payload matching the
/// shape declared in [`VerifyCtx::expected_json_keys`] or
/// [`VerifyCtx::expected_json_array_len`].
///
/// Strategy:
///  1. Try to parse the entire response as JSON.
///  2. If that fails, try to extract the first fenced ```json block.
///  3. If still no JSON, return `RejectAndRetry`.
///  4. Otherwise compare against the expectations.
pub struct JsonStructureVerifier;

impl JsonStructureVerifier {
    pub fn new() -> Self {
        Self
    }

    fn extract_json(response: &str) -> Option<Value> {
        if let Ok(v) = serde_json::from_str::<Value>(response.trim()) {
            return Some(v);
        }
        // Look for a fenced json block.
        let re = Regex::new(r"(?s)```(?:json)?\s*\n(.*?)```").ok()?;
        for cap in re.captures_iter(response) {
            if let Some(body) = cap.get(1) {
                if let Ok(v) = serde_json::from_str::<Value>(body.as_str().trim()) {
                    return Some(v);
                }
            }
        }
        None
    }
}

impl Default for JsonStructureVerifier {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl OutputVerifier for JsonStructureVerifier {
    async fn verify(
        &self,
        _prompt: &str,
        response: &str,
        ctx: &VerifyCtx,
    ) -> VerificationDirective {
        if ctx.expected_json_keys.is_empty() && ctx.expected_json_array_len.is_none() {
            // Nothing to check — this verifier sleeps.
            return VerificationDirective::Accept;
        }

        let Some(value) = Self::extract_json(response) else {
            return VerificationDirective::RejectAndRetry(
                "response was expected to contain a JSON payload (either the whole reply or a \
                 fenced ```json block) but none could be parsed."
                    .to_string(),
            );
        };

        let mut missing_keys: Vec<String> = Vec::new();

        if !ctx.expected_json_keys.is_empty() {
            // We accept either {…} or [{…}, …] — for arrays, every item
            // must carry the keys.
            match &value {
                Value::Object(obj) => {
                    for key in &ctx.expected_json_keys {
                        if !obj.contains_key(key) {
                            missing_keys.push(key.clone());
                        }
                    }
                }
                Value::Array(items) => {
                    for (i, item) in items.iter().enumerate() {
                        if let Some(obj) = item.as_object() {
                            for key in &ctx.expected_json_keys {
                                if !obj.contains_key(key) {
                                    missing_keys.push(format!("[{i}].{key}"));
                                }
                            }
                        } else {
                            missing_keys.push(format!("[{i}] (not an object)"));
                        }
                    }
                }
                _ => {
                    return VerificationDirective::RejectAndRetry(
                        "expected JSON object or array but found a scalar.".to_string(),
                    );
                }
            }
        }

        if let Some(expected_len) = ctx.expected_json_array_len {
            match &value {
                Value::Array(items) if items.len() == expected_len => {}
                Value::Array(items) => {
                    return VerificationDirective::RejectAndRetry(format!(
                        "expected JSON array of length {} but got {}",
                        expected_len,
                        items.len()
                    ));
                }
                _ => {
                    return VerificationDirective::RejectAndRetry(format!(
                        "expected JSON array of length {} but root value was {}",
                        expected_len,
                        value_kind(&value)
                    ));
                }
            }
        }

        if !missing_keys.is_empty() {
            return VerificationDirective::RejectAndRetry(format!(
                "JSON payload missing required keys: {}",
                missing_keys.join(", ")
            ));
        }

        VerificationDirective::Accept
    }

    fn name(&self) -> &'static str {
        "json_structure"
    }
}

fn value_kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

// ---------------------------------------------------------------------------
// RegexAssertVerifier
// ---------------------------------------------------------------------------

/// Verifies that the response matches every regex declared either on
/// the verifier itself or in [`VerifyCtx::expected_regexes`].
///
/// Invalid regexes are reported via `ContinueWithFeedback` rather than
/// crashing the chain — we don't want a malformed expectation to break
/// the agent's ability to finish.
pub struct RegexAssertVerifier {
    patterns: Vec<String>,
}

impl RegexAssertVerifier {
    pub fn new() -> Self {
        Self {
            patterns: Vec::new(),
        }
    }

    pub fn with_patterns(mut self, patterns: impl IntoIterator<Item = String>) -> Self {
        self.patterns = patterns.into_iter().collect();
        self
    }
}

impl Default for RegexAssertVerifier {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl OutputVerifier for RegexAssertVerifier {
    async fn verify(
        &self,
        _prompt: &str,
        response: &str,
        ctx: &VerifyCtx,
    ) -> VerificationDirective {
        let mut patterns: Vec<&str> = self.patterns.iter().map(|p| p.as_str()).collect();
        for ctx_pattern in &ctx.expected_regexes {
            patterns.push(ctx_pattern.as_str());
        }

        if patterns.is_empty() {
            return VerificationDirective::Accept;
        }

        let mut missed: Vec<String> = Vec::new();
        let mut bad: Vec<String> = Vec::new();

        for pattern in patterns {
            match Regex::new(pattern) {
                Ok(re) => {
                    if !re.is_match(response) {
                        missed.push(pattern.to_string());
                    }
                }
                Err(e) => {
                    bad.push(format!("`{pattern}` ({e})"));
                }
            }
        }

        if !missed.is_empty() {
            return VerificationDirective::RejectAndRetry(format!(
                "response failed to match required regex(es): {}",
                missed.join(", ")
            ));
        }
        if !bad.is_empty() {
            return VerificationDirective::ContinueWithFeedback(format!(
                "regex verifier could not compile pattern(s) (skipped): {}",
                bad.join(", ")
            ));
        }

        VerificationDirective::Accept
    }

    fn name(&self) -> &'static str {
        "regex_assert"
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push_str("…[truncated]");
        out
    }
}

// ---------------------------------------------------------------------------
// Wiring hooks for sprint 3b
// ---------------------------------------------------------------------------
//
// Sprint 3b will call into this module from `agentic_loop.rs` at the
// point where the loop currently decides whether to finalize. Concrete
// hook surface (no code change in this sprint):
//
//   1. `AgenticLoop::ctx` gains an optional `VerifierChain` + optional
//      `VerifyCtx` (defaults: empty chain + empty ctx — old behaviour).
//   2. Just before `finalize()`, the loop calls
//      `chain.run(prompt, response, ctx).await` and acts on the
//      directive:
//        - `Accept`               → finalize as before.
//        - `ContinueWithFeedback` → push feedback as a user-role
//                                   message and `continue` the loop.
//        - `RejectAndRetry`       → push feedback, do NOT finalize, and
//                                   burn one iteration.
//   3. Loop-budget enforcement stays in agentic_loop (verifier doesn't
//      know about iteration counts).
//
// This file deliberately exposes everything the loop needs but does NOT
// modify the loop itself — that's sprint 3b's stitching task.

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod verify_tests {
    use super::*;

    fn ok_executor() -> CodeExecutor {
        Arc::new(|_lang: String, _source: String| {
            Box::pin(async move { ("4\n".to_string(), String::new(), 0, None) })
        })
    }

    fn fail_executor() -> CodeExecutor {
        Arc::new(|_lang: String, _source: String| {
            Box::pin(async move {
                (
                    String::new(),
                    "Traceback (most recent call last):\n  File '<string>', line 1, in <module>\nValueError: boom".to_string(),
                    1,
                    None,
                )
            })
        })
    }

    fn missing_runtime_executor() -> CodeExecutor {
        Arc::new(|_lang: String, _source: String| {
            Box::pin(async move {
                (
                    String::new(),
                    String::new(),
                    -1,
                    Some("python3 runtime missing".to_string()),
                )
            })
        })
    }

    // ----------------------------- VerifyCtx -----------------------------

    #[test]
    fn verify_ctx_builder() {
        let ctx = VerifyCtx::new()
            .with_language_hint("python")
            .with_expected_stdout("hi")
            .with_expected_regex(r"\d+")
            .with_expected_json_keys(["role".to_string(), "artifact".to_string()])
            .with_expected_json_array_len(5);
        assert_eq!(ctx.language_hint.as_deref(), Some("python"));
        assert_eq!(ctx.expected_stdout_contains.as_deref(), Some("hi"));
        assert_eq!(ctx.expected_regexes, vec![r"\d+".to_string()]);
        assert_eq!(ctx.expected_json_keys.len(), 2);
        assert_eq!(ctx.expected_json_array_len, Some(5));
    }

    // ----------------------------- Directive -----------------------------

    #[test]
    fn directive_helpers() {
        assert!(VerificationDirective::Accept.is_accept());
        assert!(VerificationDirective::Accept.feedback().is_none());
        assert_eq!(
            VerificationDirective::ContinueWithFeedback("hi".to_string()).feedback(),
            Some("hi")
        );
        assert_eq!(
            VerificationDirective::RejectAndRetry("nope".to_string()).feedback(),
            Some("nope")
        );
    }

    // --------------------------- CodeBlockVerifier ----------------------

    #[tokio::test]
    async fn code_block_no_block_accepts() {
        let v = CodeBlockVerifier::with_executor(fail_executor());
        let out = v
            .verify("prompt", "no code here, just prose", &VerifyCtx::new())
            .await;
        assert_eq!(out, VerificationDirective::Accept);
    }

    #[tokio::test]
    async fn code_block_happy_path() {
        let v = CodeBlockVerifier::with_executor(ok_executor());
        let resp = "Here it is:\n```python\nprint(2+2)\n```";
        let ctx = VerifyCtx::new().with_expected_stdout("4");
        let out = v.verify("prompt", resp, &ctx).await;
        assert_eq!(out, VerificationDirective::Accept);
    }

    #[tokio::test]
    async fn code_block_nonzero_exit_continues_with_feedback() {
        let v = CodeBlockVerifier::with_executor(fail_executor());
        let resp = "```python\nraise ValueError('boom')\n```";
        let out = v.verify("prompt", resp, &VerifyCtx::new()).await;
        match out {
            VerificationDirective::ContinueWithFeedback(msg) => {
                assert!(msg.contains("ValueError") || msg.contains("exited with code"));
            }
            other => panic!("expected ContinueWithFeedback, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn code_block_runtime_missing_continues() {
        let v = CodeBlockVerifier::with_executor(missing_runtime_executor());
        let resp = "```python\nprint(1)\n```";
        let out = v.verify("prompt", resp, &VerifyCtx::new()).await;
        match out {
            VerificationDirective::ContinueWithFeedback(msg) => {
                assert!(msg.contains("runtime missing"));
            }
            other => panic!("expected ContinueWithFeedback, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn code_block_expected_stdout_missing_rejects() {
        let v = CodeBlockVerifier::with_executor(ok_executor());
        let resp = "```python\nprint(2+2)\n```";
        let ctx = VerifyCtx::new().with_expected_stdout("99");
        let out = v.verify("prompt", resp, &ctx).await;
        match out {
            VerificationDirective::RejectAndRetry(msg) => {
                assert!(msg.contains("expected stdout"));
            }
            other => panic!("expected RejectAndRetry, got {other:?}"),
        }
    }

    #[test]
    fn extract_blocks_handles_multiple_fences() {
        let resp = "intro\n```python\nprint(1)\n```\nmiddle\n```bash\nls\n```\nend";
        let blocks = CodeBlockVerifier::extract_blocks(resp);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].0, "python");
        assert_eq!(blocks[0].1.trim(), "print(1)");
        assert_eq!(blocks[1].0, "bash");
        assert_eq!(blocks[1].1.trim(), "ls");
    }

    #[test]
    fn extract_blocks_strips_empty_blocks() {
        // Empty fenced block should not show up.
        let resp = "```python\n\n```";
        let blocks = CodeBlockVerifier::extract_blocks(resp);
        assert!(blocks.is_empty());
    }

    // -------------------------- JsonStructureVerifier --------------------

    #[tokio::test]
    async fn json_no_expectation_accepts() {
        let v = JsonStructureVerifier::new();
        let out = v.verify("p", "not json at all", &VerifyCtx::new()).await;
        assert_eq!(out, VerificationDirective::Accept);
    }

    #[tokio::test]
    async fn json_inline_object_with_required_keys() {
        let v = JsonStructureVerifier::new();
        let resp = r#"{"role": "writer", "artifact": "draft"}"#;
        let ctx = VerifyCtx::new()
            .with_expected_json_keys(["role".to_string(), "artifact".to_string()]);
        let out = v.verify("p", resp, &ctx).await;
        assert_eq!(out, VerificationDirective::Accept);
    }

    #[tokio::test]
    async fn json_array_length_check_passes() {
        let v = JsonStructureVerifier::new();
        let resp = r#"[{"role":"a","artifact":"x"},{"role":"b","artifact":"y"},
                       {"role":"c","artifact":"z"},{"role":"d","artifact":"w"},
                       {"role":"e","artifact":"v"}]"#;
        let ctx = VerifyCtx::new()
            .with_expected_json_keys(["role".to_string(), "artifact".to_string()])
            .with_expected_json_array_len(5);
        let out = v.verify("p", resp, &ctx).await;
        assert_eq!(out, VerificationDirective::Accept);
    }

    #[tokio::test]
    async fn json_array_length_check_rejects_wrong_len() {
        let v = JsonStructureVerifier::new();
        let resp = r#"[{"role":"a","artifact":"x"}]"#;
        let ctx = VerifyCtx::new().with_expected_json_array_len(5);
        let out = v.verify("p", resp, &ctx).await;
        match out {
            VerificationDirective::RejectAndRetry(msg) => {
                assert!(msg.contains("length 5"));
            }
            other => panic!("expected RejectAndRetry, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn json_missing_keys_rejects() {
        let v = JsonStructureVerifier::new();
        let resp = r#"{"role": "writer"}"#;
        let ctx = VerifyCtx::new()
            .with_expected_json_keys(["role".to_string(), "artifact".to_string()]);
        let out = v.verify("p", resp, &ctx).await;
        match out {
            VerificationDirective::RejectAndRetry(msg) => {
                assert!(msg.contains("artifact"));
            }
            other => panic!("expected RejectAndRetry, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn json_extracted_from_fenced_block() {
        let v = JsonStructureVerifier::new();
        let resp = "Here you go:\n```json\n{\"role\":\"r\",\"artifact\":\"a\"}\n```\nDone.";
        let ctx = VerifyCtx::new()
            .with_expected_json_keys(["role".to_string(), "artifact".to_string()]);
        let out = v.verify("p", resp, &ctx).await;
        assert_eq!(out, VerificationDirective::Accept);
    }

    #[tokio::test]
    async fn json_response_not_parsable_rejects() {
        let v = JsonStructureVerifier::new();
        let resp = "I don't feel like emitting JSON today.";
        let ctx = VerifyCtx::new()
            .with_expected_json_keys(["role".to_string()]);
        let out = v.verify("p", resp, &ctx).await;
        assert!(matches!(out, VerificationDirective::RejectAndRetry(_)));
    }

    // -------------------------- RegexAssertVerifier ----------------------

    #[tokio::test]
    async fn regex_no_patterns_accepts() {
        let v = RegexAssertVerifier::new();
        let out = v.verify("p", "anything", &VerifyCtx::new()).await;
        assert_eq!(out, VerificationDirective::Accept);
    }

    #[tokio::test]
    async fn regex_pattern_matches() {
        let v = RegexAssertVerifier::new().with_patterns([r"\d+".to_string()]);
        let out = v.verify("p", "answer is 42", &VerifyCtx::new()).await;
        assert_eq!(out, VerificationDirective::Accept);
    }

    #[tokio::test]
    async fn regex_pattern_misses_rejects() {
        let v = RegexAssertVerifier::new().with_patterns([r"price=\$\d+".to_string()]);
        let out = v.verify("p", "no price here", &VerifyCtx::new()).await;
        match out {
            VerificationDirective::RejectAndRetry(msg) => {
                assert!(msg.contains("price="));
            }
            other => panic!("expected RejectAndRetry, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn regex_ctx_pattern_merges() {
        let v = RegexAssertVerifier::new();
        let ctx = VerifyCtx::new().with_expected_regex(r"\bfoo\b");
        let out = v.verify("p", "say foo loudly", &ctx).await;
        assert_eq!(out, VerificationDirective::Accept);
        let out2 = v.verify("p", "say bar loudly", &ctx).await;
        assert!(matches!(out2, VerificationDirective::RejectAndRetry(_)));
    }

    #[tokio::test]
    async fn regex_bad_pattern_continues_with_feedback() {
        let v = RegexAssertVerifier::new().with_patterns(["(unclosed".to_string()]);
        let out = v.verify("p", "ok", &VerifyCtx::new()).await;
        match out {
            VerificationDirective::ContinueWithFeedback(msg) => {
                assert!(msg.contains("could not compile"));
            }
            other => panic!("expected ContinueWithFeedback, got {other:?}"),
        }
    }

    // ----------------------------- VerifierChain -------------------------

    #[tokio::test]
    async fn chain_accept_with_no_verifiers() {
        let chain = VerifierChain::new();
        let out = chain.run("p", "r", &VerifyCtx::new()).await;
        assert_eq!(out, VerificationDirective::Accept);
    }

    #[tokio::test]
    async fn chain_dedupe_drops_duplicate_names() {
        let chain = VerifierChain::new()
            .add(Box::new(RegexAssertVerifier::new()))
            .add(Box::new(RegexAssertVerifier::new()));
        assert_eq!(chain.len(), 1);
    }

    #[tokio::test]
    async fn chain_short_circuits_on_reject() {
        let chain = VerifierChain::new()
            .add(Box::new(JsonStructureVerifier::new()))
            .add(Box::new(RegexAssertVerifier::new()));
        let ctx = VerifyCtx::new()
            .with_expected_json_array_len(5)
            .with_expected_regex(r"\d+");
        // JSON rejects (not an array) → chain returns RejectAndRetry
        // even though the regex would have accepted "42".
        let out = chain.run("p", "answer is 42", &ctx).await;
        assert!(matches!(out, VerificationDirective::RejectAndRetry(_)));
    }

    #[tokio::test]
    async fn chain_merges_continue_with_feedback() {
        // CodeBlockVerifier with a missing-runtime executor returns
        // ContinueWithFeedback. RegexAssertVerifier passes. Chain
        // should be ContinueWithFeedback with the code-block message.
        let chain = VerifierChain::new()
            .add(Box::new(CodeBlockVerifier::with_executor(
                missing_runtime_executor(),
            )))
            .add(Box::new(RegexAssertVerifier::new().with_patterns(["ok".to_string()])));
        let out = chain
            .run("p", "ok\n```python\nprint(1)\n```", &VerifyCtx::new())
            .await;
        match out {
            VerificationDirective::ContinueWithFeedback(msg) => {
                assert!(msg.contains("code_block"));
            }
            other => panic!("expected ContinueWithFeedback, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn chain_all_accept() {
        let chain = VerifierChain::new()
            .add(Box::new(CodeBlockVerifier::with_executor(ok_executor())))
            .add(Box::new(JsonStructureVerifier::new()))
            .add(Box::new(RegexAssertVerifier::new()));
        let resp = r#"```json
{"role":"r","artifact":"a"}
```
```python
print(2+2)
```"#;
        let ctx = VerifyCtx::new()
            .with_expected_stdout("4")
            .with_expected_json_keys(["role".to_string()]);
        let out = chain.run("p", resp, &ctx).await;
        assert_eq!(out, VerificationDirective::Accept);
    }
}
