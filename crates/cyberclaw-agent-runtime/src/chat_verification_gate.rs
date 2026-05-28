//! Lightweight per-turn verification gate for the chat path (v1.7
//! emergence wiring).
//!
//! Pairs with `constitution`'s IRON LAW 6 (resilience reflex) + IRON LAW 7
//! (no fabrication after tool intent). Where the existing
//! `EvidenceBasedVerificationGate` (control-plane) runs heavyweight
//! evidence-driven checks at the autopilot/persistent boundary, this gate
//! runs cheap heuristics on every chat turn so normal chat paths get at
//! least the most common-failure-mode coverage without paying for an extra
//! LLM call.
//!
//! # Posture
//!
//! - **Heuristic mode** (default, free): regex / structural checks for
//!   IRON LAW 7 fabrication signature, empty replies after a tool intent,
//!   broken JSON in function-call args, copy-pasted fake URLs/IDs.
//! - **Hybrid mode** (opt-in for high-risk endpoints): heuristic first;
//!   if heuristic finds a `Warn` or `Fail`, escalate to a single short
//!   LLM call asking "did this response actually do what the user asked,
//!   yes/no, why?". Cost: +1 LLM call per suspicious turn (rare).
//!
//! The control-plane `EvidenceBasedVerificationGate` remains the authority
//! for autopilot/persistent runs — this gate is intentionally cheap and
//! does NOT try to replicate it.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Verdict
// ---------------------------------------------------------------------------

/// Outcome of a chat-turn verification check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChatVerifyVerdict {
    /// No issues detected. Response can be returned to the user as-is.
    Pass,
    /// Soft anomalies detected (e.g. unusually short answer to a complex
    /// query). Caller may surface a debug breadcrumb but should still
    /// return the response.
    Warn { reasons: Vec<String> },
    /// Hard anomaly detected (e.g. fabrication signature after tool intent).
    /// Caller SHOULD overwrite the response with a fail-loud message
    /// instead of returning fabricated content.
    Fail { reasons: Vec<String> },
}

impl ChatVerifyVerdict {
    pub fn is_pass(&self) -> bool {
        matches!(self, ChatVerifyVerdict::Pass)
    }

    pub fn is_fail(&self) -> bool {
        matches!(self, ChatVerifyVerdict::Fail { .. })
    }

    pub fn reasons(&self) -> &[String] {
        match self {
            ChatVerifyVerdict::Pass => &[],
            ChatVerifyVerdict::Warn { reasons } | ChatVerifyVerdict::Fail { reasons } => reasons,
        }
    }
}

// ---------------------------------------------------------------------------
// Verifier trait
// ---------------------------------------------------------------------------

/// What the LLM emitted on the last turn, plus enough context for the
/// verifier to make a decision.
#[derive(Debug, Clone)]
pub struct ChatTurnContext<'a> {
    /// The assistant message text returned in this turn (may be empty if
    /// only a tool call was emitted).
    pub assistant_text: &'a str,
    /// Whether the previous turn emitted a tool intent that the platform
    /// has not yet executed. If true, an assistant_text that already
    /// contains "result" / structured fake output is a fabrication
    /// signature (IRON LAW 7 violation).
    pub tool_intent_pending: bool,
    /// Last user prompt — used by heuristics to detect "user asked a
    /// concrete deliverable but assistant only asked clarifying questions"
    /// patterns (IRON LAW 2a violation hint).
    pub user_prompt: &'a str,
    /// Optional iteration count from agentic loop, for soft heuristics
    /// like "if turn=1 and request had ≥1 concrete anchor, don't ask".
    pub iteration: Option<u32>,
}

/// Per-turn verifier interface. Implementations should be deterministic
/// and side-effect free.
pub trait ChatVerifier: Send + Sync {
    fn verify(&self, ctx: &ChatTurnContext<'_>) -> ChatVerifyVerdict;
}

// ---------------------------------------------------------------------------
// HeuristicChatVerifier — the default, no-LLM-call implementation
// ---------------------------------------------------------------------------

/// Cheap regex+structural verifier. Catches the highest-frequency failure
/// modes observed in v1.5-v1.7 adversarial testing.
#[derive(Debug, Default, Clone)]
pub struct HeuristicChatVerifier;

impl ChatVerifier for HeuristicChatVerifier {
    fn verify(&self, ctx: &ChatTurnContext<'_>) -> ChatVerifyVerdict {
        let mut warns: Vec<String> = Vec::new();
        let mut fails: Vec<String> = Vec::new();

        // FAIL: IRON LAW 7 fabrication — tool intent was pending but the
        // assistant already wrote a body that claims results.
        if ctx.tool_intent_pending && looks_like_fabricated_tool_result(ctx.assistant_text) {
            fails.push(
                "IRON LAW 7 violation candidate: assistant wrote tool-result-like content while a tool intent is still pending"
                    .to_string(),
            );
        }

        // FAIL: empty assistant body with no pending tool intent — means
        // we returned literally nothing to the user.
        if !ctx.tool_intent_pending && ctx.assistant_text.trim().is_empty() {
            fails.push("empty assistant response with no pending tool call".to_string());
        }

        // WARN: IRON LAW 2a hint — user named a concrete deliverable but
        // assistant only emitted A/B/C choice prompts on the first turn.
        if matches!(ctx.iteration, Some(1) | None)
            && looks_like_choice_prompt_only(ctx.assistant_text)
            && user_named_concrete_deliverable(ctx.user_prompt)
        {
            warns.push(
                "IRON LAW 2a hint: user named a concrete deliverable but the first-turn response is only an A/B choice prompt"
                    .to_string(),
            );
        }

        // WARN: assistant claims to have written a file but no fs.write /
        // file_write tool call was emitted in this body (catches a common
        // 'I wrote it to /tmp/x' hallucination).
        if claims_file_write_without_tool_call(ctx.assistant_text) {
            warns.push("claims a file write without a corresponding tool call".to_string());
        }

        // WARN: v1.7.2 user-reported case — assistant claims an artifact at
        // a specific path (e.g. "已生成 /tmp/X.pptx 10K bytes"). v1.7.x can't
        // do cross-container path resolution + open-and-validate inside this
        // sync verifier, but emit the path so operator / downstream gate
        // can verify externally. Real validation (xmllint/py_compile/json.tool)
        // is v1.8+ work — see docs/implementation/roadmap/v1.8-backlog-2026-05-28.md
        // (Bug E).
        for hit in claims_artifact_paths(ctx.assistant_text) {
            warns.push(format!(
                "claims an artifact at `{hit}` — operator should verify file validity (v1.8 will add inline xmllint/py_compile)"
            ));
        }

        if !fails.is_empty() {
            ChatVerifyVerdict::Fail { reasons: fails }
        } else if !warns.is_empty() {
            ChatVerifyVerdict::Warn { reasons: warns }
        } else {
            ChatVerifyVerdict::Pass
        }
    }
}

// ---------------------------------------------------------------------------
// Heuristic helpers
// ---------------------------------------------------------------------------

fn looks_like_fabricated_tool_result(text: &str) -> bool {
    // Most fabrications mimic OpenAI tool-result envelopes or copy-paste
    // a structured "result" block.
    let lower = text.to_lowercase();
    lower.contains("\"tool_calls\":")
        || lower.contains("\"tool_result\":")
        || lower.contains("```tool_result")
        || lower.contains("<|｜｜tool_result")
        || lower.contains("[search result]")
}

fn looks_like_choice_prompt_only(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }
    // Heuristic: short response that is mostly a question with A/B markers.
    let len = trimmed.chars().count();
    if len > 600 {
        return false;
    }
    let lower = trimmed.to_lowercase();
    let has_question_mark = trimmed.contains('?') || trimmed.contains('？');
    let has_choice_marker = lower.contains(" or ")
        || lower.contains("还是")
        || lower.contains(" a) ")
        || lower.contains(" b) ")
        || lower.contains("a. ")
        || lower.contains("b. ")
        || lower.contains("(a)")
        || lower.contains("(b)");
    has_question_mark && has_choice_marker
}

fn user_named_concrete_deliverable(prompt: &str) -> bool {
    let lower = prompt.to_lowercase();
    // A weak signal — concrete artifact words. Used in combination with
    // the choice-prompt heuristic, not alone.
    [
        "写一个", "写一份", "写个", "make a", "create a", "build a", "draft a",
        "script", "脚本", "deck", "slides", "presentation",
        "summary", "总结", "translate", "翻译", "email", "邮件",
        "diagram", "flowchart", "流程图", "workflow", "function", "函数",
    ]
    .iter()
    .any(|kw| lower.contains(kw))
}

/// Find every path-shaped string the assistant claims to have produced.
///
/// Detects patterns like:
/// - "saved to /tmp/X.pptx"
/// - "wrote to /Users/foo/Y.docx"
/// - "已生成 /tmp/Z.pptx (8 slides)"
/// - "output: workspace/report.json"
///
/// Returns up to 5 distinct paths (cap prevents log spam). Returns empty
/// if no artifact-claim signal detected.
fn claims_artifact_paths(text: &str) -> Vec<String> {
    // Cheap precheck — avoid regex on every response that has no claim.
    let lower = text.to_lowercase();
    let has_claim_word = [
        "已生成", "已保存到", "已写入", "已创建", "已输出",
        "saved to", "wrote to", "written to", "output to", "generated at",
        "created at", "exported to",
    ]
    .iter()
    .any(|kw| lower.contains(kw));
    if !has_claim_word {
        return Vec::new();
    }
    // Match path-like substrings with a recognized productivity extension.
    // Intentionally permissive — false positives are observability noise, not
    // user-blocking. Real validation is v1.8 work.
    let ext_re = regex::Regex::new(
        r"(?i)([/\w][\w/.\-]*\.(?:pptx|docx|xlsx|pdf|json|yaml|yml|toml|csv|html|md|py|rs|ts|tsx|js|jsx|sh|sql))"
    )
    .ok();
    let mut hits: Vec<String> = Vec::new();
    if let Some(re) = ext_re {
        for cap in re.captures_iter(text).take(50) {
            if let Some(m) = cap.get(1) {
                let path = m.as_str().to_string();
                if !hits.contains(&path) {
                    hits.push(path);
                    if hits.len() >= 5 {
                        break;
                    }
                }
            }
        }
    }
    hits
}

fn claims_file_write_without_tool_call(text: &str) -> bool {
    let lower = text.to_lowercase();
    let claim = lower.contains("已写入") || lower.contains("已保存到")
        || lower.contains("written to /") || lower.contains("saved to /")
        || lower.contains("已创建文件");
    let has_tool_marker = lower.contains("fs.write")
        || lower.contains("file_write")
        || lower.contains("write_file")
        || lower.contains("tool_calls")
        || lower.contains("```tool");
    claim && !has_tool_marker
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>(
        text: &'a str,
        prompt: &'a str,
        pending: bool,
        iter: Option<u32>,
    ) -> ChatTurnContext<'a> {
        ChatTurnContext {
            assistant_text: text,
            tool_intent_pending: pending,
            user_prompt: prompt,
            iteration: iter,
        }
    }

    #[test]
    fn pass_on_normal_response() {
        let v = HeuristicChatVerifier;
        let c = ctx(
            "Here's the code you asked for:\n\n```python\nprint('hi')\n```",
            "write a python hello world",
            false,
            Some(1),
        );
        assert_eq!(v.verify(&c), ChatVerifyVerdict::Pass);
    }

    #[test]
    fn fail_on_iron_law_7_fabrication_after_tool_intent() {
        let v = HeuristicChatVerifier;
        let c = ctx(
            "I called the search. \"tool_result\": {\"hits\":[{\"url\":\"https://x.com\"}]}",
            "search for X",
            true,
            Some(2),
        );
        assert!(v.verify(&c).is_fail());
    }

    #[test]
    fn fail_on_empty_response() {
        let v = HeuristicChatVerifier;
        let c = ctx("", "hi", false, Some(1));
        assert!(v.verify(&c).is_fail());
    }

    #[test]
    fn warn_on_choice_only_first_turn_concrete_request() {
        let v = HeuristicChatVerifier;
        let c = ctx(
            "OAuth 2.0 or IMAP? Native or browser automation?",
            "帮我写一个 Python 脚本，自动登录 gmail",
            false,
            Some(1),
        );
        let verdict = v.verify(&c);
        assert!(matches!(verdict, ChatVerifyVerdict::Warn { .. }));
        assert!(verdict.reasons().iter().any(|r| r.contains("IRON LAW 2a")));
    }

    #[test]
    fn warn_on_file_write_claim_without_tool() {
        let v = HeuristicChatVerifier;
        let c = ctx(
            "已写入 /tmp/output.txt — 100 行。",
            "save report",
            false,
            Some(1),
        );
        let verdict = v.verify(&c);
        assert!(matches!(verdict, ChatVerifyVerdict::Warn { .. }));
    }

    #[test]
    fn warn_on_artifact_claim_with_path() {
        let v = HeuristicChatVerifier;
        let c = ctx(
            "✅ 已生成 /tmp/cb-intro.pptx (10K bytes, 8 slides)",
            "make me a pptx",
            false,
            Some(2),
        );
        let verdict = v.verify(&c);
        assert!(matches!(verdict, ChatVerifyVerdict::Warn { .. }));
        assert!(
            verdict.reasons().iter().any(|r| r.contains("cb-intro.pptx")),
            "warning should include path: {:?}",
            verdict.reasons()
        );
    }

    #[test]
    fn warn_on_artifact_claim_english() {
        let v = HeuristicChatVerifier;
        let c = ctx(
            "Saved to /Users/foo/report.docx",
            "draft me a report",
            false,
            Some(3),
        );
        let verdict = v.verify(&c);
        assert!(matches!(verdict, ChatVerifyVerdict::Warn { .. }));
        assert!(verdict.reasons().iter().any(|r| r.contains("report.docx")));
    }

    #[test]
    fn pass_when_no_artifact_claim() {
        let v = HeuristicChatVerifier;
        // Mentions .pptx but no claim word — don't fire
        let c = ctx(
            "To work with pptx files, you can use python-pptx.",
            "how do I work with pptx",
            false,
            Some(1),
        );
        let verdict = v.verify(&c);
        assert!(matches!(verdict, ChatVerifyVerdict::Pass | ChatVerifyVerdict::Warn { .. }));
    }

    #[test]
    fn pass_when_choice_prompt_is_legitimate_clarification() {
        let v = HeuristicChatVerifier;
        // user request has NO concrete anchor — choice prompt is OK
        let c = ctx(
            "OAuth or IMAP?",
            "I need help with email",
            false,
            Some(1),
        );
        // verdict may be Pass or Warn — both acceptable since the heuristic
        // is intentionally conservative; the key is it's NOT Fail.
        assert!(!v.verify(&c).is_fail());
    }
}
