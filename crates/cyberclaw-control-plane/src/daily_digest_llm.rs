//! LLM-powered Daily Digest summarizer (Sprint 9 Wave 3 L4).
//!
//! Implements the Stage 2 + 3 + 4 HARD-GATE reflection loop specified in
//! `ecosystem/skills/daily-digest/SKILL.md`. This is the non-mechanical
//! counterpart to [`crate::daily_digest`]'s built-in summarizer: it asks an
//! [`LlmClient`] two targeted questions — "top 3 failure categories" and
//! "which learnings survive the three-question HARD-GATE" — and returns a
//! structured summary + rule list.
//!
//! # Architectural placement
//!
//! - Implements [`DigestSummarizer`] (Stage 2 + 3 + 4). It does **not**
//!   collect facts (Stage 1 lives in `daily_digest_runtime::StoreDigestCollector`)
//!   and does **not** persist (Stage 5 is `RepositoryPersister`).
//! - Per SKILL.md HARD-AVOIDs: if the LLM surfaces a policy suggestion, this
//!   module still writes only `SemanticMemory` — the `DangerousCapabilityFilter`
//!   / `ToolPermissionMatcher` rules are never mutated here. Any governance
//!   mutation must flow through `EvolutionOrchestrator` separately.
//! - Cheap-attribution defence: the prompt forbids the phrases "运气好",
//!   "环境问题", "网络抖动", and every returned learning is re-checked in
//!   Rust against a deny-list before it reaches the coordinator. The Rust
//!   gate is the authoritative filter; the prompt is a best-effort hint.
//! - Failure containment: LLM errors never abort a digest run. When the LLM
//!   fails we fall back to the mechanical [`build_facts_md`] path with an
//!   empty rule set — matching the SKILL.md "不编造事实" contract.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use tracing::{debug, warn};

use cyberclaw_llm::client::LlmClient;
use cyberclaw_llm::types::{ChatRequest, Message};

use crate::daily_digest::{
    build_facts_md, DailyDigestConfig, DigestError, DigestInputs, DigestSummarizer, DigestSummary,
    RuleCandidate,
};

// ============================================================================
// Config
// ============================================================================

/// Knobs for [`LlmReflectionSummarizer`].
///
/// `reflection_strictness` is a 0..=1 hint that scales the HARD-GATE
/// aggression: higher values tell the model to drop more borderline
/// learnings. It does **not** short-circuit the Rust-side deny-list, which
/// always runs regardless of strictness.
#[derive(Debug, Clone)]
pub struct ReflectionConfig {
    /// Model name forwarded verbatim to the LLM client. Defaults to
    /// `claude-sonnet` per the task brief; the actual routing is the LLM
    /// client's job.
    pub model: String,
    /// Hard cap on the number of rules returned. Mirrors the SKILL.md
    /// "Top 10" bound and is also enforced by the coordinator
    /// (`DailyDigestConfig::max_rules`); whichever is tighter wins.
    pub max_rules: u32,
    /// 0.0 = lenient HARD-GATE, 1.0 = strict. The value is surfaced in the
    /// system prompt so the model can self-calibrate.
    pub reflection_strictness: f32,
}

impl Default for ReflectionConfig {
    fn default() -> Self {
        Self {
            model: "claude-sonnet".to_string(),
            max_rules: 10,
            reflection_strictness: 0.7,
        }
    }
}

// ============================================================================
// Cheap-attribution deny-list (HARD-GATE)
// ============================================================================

/// Phrases that SKILL.md §反模式 explicitly rejects. Any learning whose text
/// contains any of these (case-insensitive substring match) is dropped.
const CHEAP_ATTRIBUTION_PHRASES: &[&str] = &[
    "运气好",
    "环境问题",
    "网络抖动",
    "运气",
    "bad luck",
    "good luck",
    "flaky network",
    "environment issue",
    "it just worked",
];

fn is_cheap_attribution(text: &str) -> bool {
    let lower = text.to_lowercase();
    CHEAP_ATTRIBUTION_PHRASES
        .iter()
        .any(|p| lower.contains(&p.to_lowercase()))
}

// ============================================================================
// Prompt templates
// ============================================================================

/// System prompt for the **problems layer** LLM call (Stage 2 bottom third).
///
/// SKILL.md §Stage 2 demands "最频繁的失败信号：<top-3 error categories>",
/// so the prompt pins the shape hard and forbids creative elaboration.
pub const PROBLEMS_SYSTEM_PROMPT: &str = "\
你是 CyberClaw Agent 的 Daily Digest 反思助手，负责 Stage 2「问题层」。\n\
\n\
硬约束（任一违反直接 fail）：\n\
1. 只输出 markdown，以 `## 今日卡在哪（问题层）` 开头。\n\
2. 只列 **Top 3** failure categories，不多不少；如果证据不足 3 条，缺的位置写 `_（证据不足）_`。\n\
3. 每条 ≤ 80 字符，基于输入的 execution/trace 事实；**禁止** 编造不在输入里的错误。\n\
4. 禁用廉价归因：不得出现「运气好」「环境问题」「网络抖动」「网络波动」。\n\
5. 不要加总结段、不要加安慰语、不要加下一步建议 —— 只列 3 条。";

/// System prompt for the **learnings layer** LLM call (Stage 3 + 4).
///
/// This is the HARD-GATE. The three questions are verbatim from
/// `ecosystem/skills/daily-digest/SKILL.md §Stage 3`.
pub const LEARNINGS_SYSTEM_PROMPT: &str = "\
你是 CyberClaw Agent 的 Daily Digest 反思助手，负责 Stage 3 + 4「经验层 + 规则提炼」。\n\
\n\
对每一条候选经验，严格按三问筛选，三问全过才保留：\n\
1. 为什么这件事成了 / 没成？—— 必须有因果链，不得用「运气好」「环境问题」「网络抖动」。\n\
2. 换一个上下文会不会复现？—— 单点事件直接丢；只保留可泛化的模式。\n\
3. 如果没有这条经验，下次会重犯吗？—— 不够「值得沉淀」的阈值就丢。\n\
\n\
输出约束：\n\
- 只输出 **JSON 数组**（不要任何 markdown、解释、前言、后言、代码块围栏）。\n\
- 数组每项 shape：`{\"rule\": \"...\", \"source_executions\": [\"<exec_id>\", ...]}`\n\
- `rule` ≤ 100 字符，祈使句或条件句（例：`Autopilot 连续失败 3 次后切 Persistent`）。\n\
- `source_executions` 必须是从输入的 execution_id 列表里挑出的子集；不得编造 ID。\n\
- 最多 {max_rules} 条；宁可少、不要凑。\n\
- 如果没有任何经验能通过三问，输出 `[]`。\n\
- reflection_strictness = {strictness}（0=宽松 1=严苛）。越高越该丢。";

/// Build a compact facts payload for the user-message side of the LLM call.
/// Deterministic so unit tests can assert against it.
fn build_facts_payload(inputs: &DigestInputs) -> String {
    let mut out = String::new();
    out.push_str("executions:\n");
    for e in &inputs.executions {
        out.push_str(&format!(
            "  - id={} status={} mode={} started={} finished={}\n",
            e.execution_id.as_str(),
            e.status,
            e.execution_mode,
            e.started_at.to_rfc3339(),
            e.completed_at
                .map(|t| t.to_rfc3339())
                .unwrap_or_else(|| "-".to_string()),
        ));
    }
    out.push_str("traces:\n");
    for t in &inputs.traces {
        out.push_str(&format!(
            "  - id={} type={} severity={}\n",
            t.trace_id.as_str(),
            t.event_type,
            t.severity
        ));
    }
    out.push_str("artifacts:\n");
    for a in &inputs.artifacts {
        out.push_str(&format!(
            "  - id={} kind={} size={}\n",
            a.artifact_id.as_str(),
            a.kind,
            a.size_bytes
        ));
    }
    out.push_str("journal:\n");
    for j in &inputs.journal_iterations {
        out.push_str(&format!("  - iter={} verdict={}\n", j.iteration, j.verdict));
    }
    out
}

// ============================================================================
// Summarizer
// ============================================================================

/// [`DigestSummarizer`] that drives Stage 2-4 through an [`LlmClient`].
///
/// The mechanical Stage 2 "facts layer" is kept (reusing
/// [`build_facts_md`]) — no LLM call is needed to count executions. Only the
/// **problems layer** and **learnings layer** ask the model.
pub struct LlmReflectionSummarizer {
    llm_client: Arc<dyn LlmClient>,
    reflection_config: ReflectionConfig,
}

impl LlmReflectionSummarizer {
    pub fn new(llm_client: Arc<dyn LlmClient>, reflection_config: ReflectionConfig) -> Self {
        Self {
            llm_client,
            reflection_config,
        }
    }

    /// Convenience constructor with default [`ReflectionConfig`].
    pub fn with_default_config(llm_client: Arc<dyn LlmClient>) -> Self {
        Self::new(llm_client, ReflectionConfig::default())
    }

    fn learnings_system_prompt(&self) -> String {
        LEARNINGS_SYSTEM_PROMPT
            .replace("{max_rules}", &self.reflection_config.max_rules.to_string())
            .replace(
                "{strictness}",
                &format!("{:.2}", self.reflection_config.reflection_strictness),
            )
    }

    /// Single-shot chat helper. Returns the first choice's content or a
    /// digest-level error; **does not** fall back — callers decide policy.
    async fn chat(
        &self,
        system: impl Into<String>,
        user: impl Into<String>,
    ) -> Result<String, DigestError> {
        let request = ChatRequest {
            model: self.reflection_config.model.clone(),
            messages: vec![Message::system(system), Message::user(user)],
            temperature: Some(0.2),
            ..Default::default()
        };
        let resp = self
            .llm_client
            .chat_completion(request)
            .await
            .map_err(|e| DigestError::Summarize(format!("llm call failed: {}", e)))?;
        let content = resp
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .ok_or_else(|| DigestError::Summarize("llm returned no choices".into()))?;
        Ok(content)
    }

    async fn build_problems_md(&self, inputs: &DigestInputs) -> Result<String, DigestError> {
        let content = self
            .chat(PROBLEMS_SYSTEM_PROMPT, build_facts_payload(inputs))
            .await?;
        let trimmed = content.trim();
        if trimmed.is_empty() {
            return Ok("## 今日卡在哪（问题层）\n_（无数据）_".to_string());
        }
        // Defensive: if the model forgot the header, prepend it.
        if trimmed.starts_with("## 今日卡在哪") {
            Ok(trimmed.to_string())
        } else {
            Ok(format!("## 今日卡在哪（问题层）\n{}", trimmed))
        }
    }

    async fn extract_rules(
        &self,
        inputs: &DigestInputs,
    ) -> Result<(Vec<RuleCandidate>, String), DigestError> {
        let system = self.learnings_system_prompt();
        let content = self.chat(system, build_facts_payload(inputs)).await?;
        let rules = parse_rules_json(&content, self.reflection_config.max_rules as usize, inputs);
        let learnings_md = render_learnings_md(&rules);
        Ok((rules, learnings_md))
    }
}

#[async_trait]
impl DigestSummarizer for LlmReflectionSummarizer {
    async fn summarize(
        &self,
        _config: &DailyDigestConfig,
        inputs: &DigestInputs,
    ) -> Result<(DigestSummary, Vec<RuleCandidate>), DigestError> {
        // SKILL.md: 若当天无任何 Execution，跳过，不生成空 digest。
        // 防御性兜底：coordinator 已经拦 empty day，这里再挡一次编造风险。
        if inputs.executions.is_empty()
            && inputs.artifacts.is_empty()
            && inputs.traces.is_empty()
            && inputs.journal_iterations.is_empty()
        {
            return Ok((
                DigestSummary {
                    facts_md: build_facts_md(inputs),
                    problems_md: "## 今日卡在哪（问题层）\n_（无数据）_".to_string(),
                    learnings_md: "## 今日学到什么（经验层）\n_（无数据）_".to_string(),
                },
                Vec::new(),
            ));
        }

        let facts_md = build_facts_md(inputs);

        // Stage 2 — problems layer. LLM failure falls back to a static
        // placeholder so the digest still records facts for the day.
        let problems_md = match self.build_problems_md(inputs).await {
            Ok(md) => md,
            Err(e) => {
                warn!(error = %e, "problems-layer LLM call failed; using fallback");
                "## 今日卡在哪（问题层）\n_（LLM 不可用，跳过本层）_".to_string()
            }
        };

        // Stage 3 + 4 — learnings + rules. LLM failure yields an empty rule
        // list; we never fabricate rules.
        let (rules, learnings_md) = match self.extract_rules(inputs).await {
            Ok(pair) => pair,
            Err(e) => {
                warn!(error = %e, "learnings-layer LLM call failed; falling back to mechanical");
                (
                    Vec::new(),
                    "## 今日学到什么（经验层）\n_（LLM 不可用，跳过本层）_".to_string(),
                )
            }
        };

        Ok((
            DigestSummary {
                facts_md,
                problems_md,
                learnings_md,
            },
            rules,
        ))
    }
}

// ============================================================================
// Rule parsing + filtering
// ============================================================================

#[derive(Debug, Deserialize)]
struct LlmRule {
    rule: String,
    #[serde(default)]
    source_executions: Vec<String>,
}

/// Parse a JSON array of `{rule, source_executions}` into filtered
/// [`RuleCandidate`]s.
///
/// Filters applied, in order:
/// 1. JSON must parse; otherwise `[]`.
/// 2. `rule` trimmed, non-empty, ≤ 100 chars.
/// 3. Cheap-attribution deny-list (hard gate).
/// 4. `source_executions` intersected with the real input execution IDs
///    (fabricated IDs silently dropped).
/// 5. Truncated to `max_rules`.
fn parse_rules_json(raw: &str, max_rules: usize, inputs: &DigestInputs) -> Vec<RuleCandidate> {
    let trimmed = strip_code_fences(raw.trim());
    let parsed: Vec<LlmRule> = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(e) => {
            debug!(error = %e, raw = %trimmed, "failed to parse LLM rules JSON");
            return Vec::new();
        }
    };

    let valid_ids: std::collections::HashSet<String> = inputs
        .executions
        .iter()
        .map(|e| e.execution_id.as_str().to_string())
        .collect();

    let mut out = Vec::new();
    for r in parsed {
        let rule = r.rule.trim().to_string();
        if rule.is_empty() || rule.chars().count() > 100 {
            continue;
        }
        if is_cheap_attribution(&rule) {
            debug!(rule = %rule, "dropped rule: cheap attribution");
            continue;
        }
        let mut source_executions = Vec::new();
        for id in r.source_executions {
            if !valid_ids.contains(&id) {
                continue;
            }
            // Rebuild into the typed ExecutionId via the originating fact
            // (cheap linear scan; rule sets are small).
            if let Some(fact) = inputs
                .executions
                .iter()
                .find(|e| e.execution_id.as_str() == id)
            {
                source_executions.push(fact.execution_id.clone());
            }
        }
        out.push(RuleCandidate {
            rule,
            source_executions,
        });
        if out.len() >= max_rules {
            break;
        }
    }
    out
}

fn strip_code_fences(s: &str) -> &str {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix("```json") {
        return rest.trim_start().trim_end_matches("```").trim();
    }
    if let Some(rest) = s.strip_prefix("```") {
        return rest.trim_start().trim_end_matches("```").trim();
    }
    s
}

fn render_learnings_md(rules: &[RuleCandidate]) -> String {
    if rules.is_empty() {
        return "## 今日学到什么（经验层）\n_（无）_".to_string();
    }
    let mut out = String::from("## 今日学到什么（经验层）\n");
    for r in rules {
        out.push_str(&format!("- {}\n", r.rule));
    }
    out
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use chrono::Utc;
    use cyberclaw_core::ids::{AgentId, ExecutionId};
    use cyberclaw_llm::error::LlmResult;
    use cyberclaw_llm::prelude::Stream;
    use cyberclaw_llm::types::{ChatChunk, ChatResponse, Choice};
    use std::sync::Mutex;

    use crate::daily_digest::{DigestSummarizer, ExecutionFact};

    // ----------------- Mock LLM -----------------

    /// LLM that returns a scripted sequence of responses — one per
    /// `chat_completion` call. Calls beyond the script reuse the last entry.
    struct ScriptedLlm {
        responses: Mutex<Vec<String>>,
        calls: Mutex<Vec<ChatRequest>>,
    }

    impl ScriptedLlm {
        fn new(script: Vec<&str>) -> Self {
            Self {
                responses: Mutex::new(script.into_iter().map(String::from).collect()),
                calls: Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<ChatRequest> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl LlmClient for ScriptedLlm {
        async fn chat_completion(&self, request: ChatRequest) -> LlmResult<ChatResponse> {
            self.calls.lock().unwrap().push(request.clone());
            let mut guard = self.responses.lock().unwrap();
            let content = if guard.len() > 1 {
                guard.remove(0)
            } else {
                guard.first().cloned().unwrap_or_default()
            };
            Ok(ChatResponse {
                id: "test".into(),
                object: "chat.completion".into(),
                created: 0,
                model: request.model,
                choices: vec![Choice {
                    index: 0,
                    message: Message::assistant(content),
                    finish_reason: Some("stop".into()),
                }],
                usage: None,
            })
        }

        async fn chat_completion_stream(
            &self,
            _request: ChatRequest,
        ) -> LlmResult<Box<dyn Stream<Item = LlmResult<ChatChunk>> + Send + Unpin>> {
            unimplemented!("streaming not used by digest summarizer")
        }

        fn provider(&self) -> &str {
            "scripted-test"
        }

        async fn validate_connection(&self) -> LlmResult<()> {
            Ok(())
        }
    }

    /// LLM that always fails; exercises the fallback path.
    struct FailingLlm;

    #[async_trait]
    impl LlmClient for FailingLlm {
        async fn chat_completion(&self, _request: ChatRequest) -> LlmResult<ChatResponse> {
            Err(cyberclaw_llm::error::LlmError::Internal("boom".to_string()))
        }
        async fn chat_completion_stream(
            &self,
            _request: ChatRequest,
        ) -> LlmResult<Box<dyn Stream<Item = LlmResult<ChatChunk>> + Send + Unpin>> {
            unimplemented!()
        }
        fn provider(&self) -> &str {
            "failing-test"
        }
        async fn validate_connection(&self) -> LlmResult<()> {
            Ok(())
        }
    }

    // ----------------- Fixtures -----------------

    fn sample_config() -> DailyDigestConfig {
        DailyDigestConfig {
            agent_id: AgentId::new(),
            window_start: "2026-04-18T00:00:00Z".parse().unwrap(),
            window_end: "2026-04-19T00:00:00Z".parse().unwrap(),
            max_rules: 10,
        }
    }

    fn exec_fact() -> ExecutionFact {
        ExecutionFact {
            execution_id: ExecutionId::new(),
            status: "completed".into(),
            execution_mode: "normal".into(),
            started_at: Utc::now(),
            completed_at: Some(Utc::now()),
        }
    }

    fn inputs_with_one_exec() -> (DigestInputs, String) {
        let fact = exec_fact();
        let id = fact.execution_id.as_str().to_string();
        (
            DigestInputs {
                executions: vec![fact],
                artifacts: vec![],
                traces: vec![],
                journal_iterations: vec![],
            },
            id,
        )
    }

    // ----------------- Tests -----------------

    #[tokio::test]
    async fn test_empty_inputs_returns_empty_rules() {
        let llm = Arc::new(ScriptedLlm::new(vec!["should-not-be-called"]));
        let summ = LlmReflectionSummarizer::new(llm.clone(), ReflectionConfig::default());
        let cfg = sample_config();
        let inputs = DigestInputs::default();
        let (summary, rules) = summ.summarize(&cfg, &inputs).await.unwrap();
        assert!(rules.is_empty(), "empty day must yield zero rules");
        assert!(
            summary.learnings_md.contains("无数据"),
            "learnings_md should flag no data"
        );
        assert!(
            llm.calls().is_empty(),
            "LLM must not be called for empty days"
        );
    }

    #[tokio::test]
    async fn test_reflection_filters_cheap_attribution() {
        // Problems-layer response + learnings JSON mixing cheap + valid rules.
        let learnings_json = r#"[
            {"rule":"今天成功是因为运气好","source_executions":[]},
            {"rule":"Autopilot 连续失败 3 次后切 Persistent","source_executions":[]},
            {"rule":"网络抖动导致 retry，无需处理","source_executions":[]}
        ]"#;
        let llm = Arc::new(ScriptedLlm::new(vec![
            "## 今日卡在哪（问题层）\n- retry 激增",
            learnings_json,
        ]));
        let summ = LlmReflectionSummarizer::new(llm, ReflectionConfig::default());
        let (inputs, _id) = inputs_with_one_exec();
        let (_summary, rules) = summ
            .summarize(&sample_config(), &inputs)
            .await
            .expect("summarize should succeed");
        assert_eq!(rules.len(), 1, "cheap-attribution rules must be dropped");
        assert_eq!(rules[0].rule, "Autopilot 连续失败 3 次后切 Persistent");
    }

    #[tokio::test]
    async fn test_max_rules_truncation() {
        // Build a JSON array with 20 valid rules.
        let rule_items: Vec<String> = (0..20)
            .map(|i| format!(r#"{{"rule":"valid-rule-{}","source_executions":[]}}"#, i))
            .collect();
        let learnings_json = format!("[{}]", rule_items.join(","));
        let llm = Arc::new(ScriptedLlm::new(vec![
            "## 今日卡在哪（问题层）\n- noop",
            &learnings_json,
        ]));
        let cfg = ReflectionConfig {
            max_rules: 5,
            ..Default::default()
        };
        let summ = LlmReflectionSummarizer::new(llm, cfg);
        let (inputs, _id) = inputs_with_one_exec();
        let (_summary, rules) = summ.summarize(&sample_config(), &inputs).await.unwrap();
        assert_eq!(
            rules.len(),
            5,
            "parser must truncate to reflection_config.max_rules"
        );
    }

    #[tokio::test]
    async fn test_llm_failure_falls_back_to_mechanical() {
        let llm = Arc::new(FailingLlm);
        let summ = LlmReflectionSummarizer::new(llm, ReflectionConfig::default());
        let (inputs, _id) = inputs_with_one_exec();
        let (summary, rules) = summ
            .summarize(&sample_config(), &inputs)
            .await
            .expect("LLM failure must not abort the digest");
        assert!(rules.is_empty(), "LLM failure must yield zero rules");
        // facts_md always uses the mechanical builder — it must still contain
        // the execution count.
        assert!(
            summary.facts_md.contains("完成执行 1 条"),
            "facts_md fallback missing execution count: {}",
            summary.facts_md
        );
        assert!(
            summary.problems_md.contains("LLM 不可用"),
            "problems_md should signal LLM fallback"
        );
        assert!(
            summary.learnings_md.contains("LLM 不可用"),
            "learnings_md should signal LLM fallback"
        );
    }

    #[tokio::test]
    async fn test_fabricated_execution_ids_are_stripped() {
        let (inputs, real_id) = inputs_with_one_exec();
        let learnings_json = format!(
            r#"[{{"rule":"good rule","source_executions":["{}","fake-id-123"]}}]"#,
            real_id
        );
        let llm = Arc::new(ScriptedLlm::new(vec![
            "## 今日卡在哪（问题层）\n- noop",
            &learnings_json,
        ]));
        let summ = LlmReflectionSummarizer::new(llm, ReflectionConfig::default());
        let (_summary, rules) = summ.summarize(&sample_config(), &inputs).await.unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(
            rules[0].source_executions.len(),
            1,
            "fabricated execution id must be dropped"
        );
        assert_eq!(rules[0].source_executions[0].as_str(), real_id);
    }

    #[tokio::test]
    async fn test_oversized_rule_is_dropped() {
        let long_rule = "x".repeat(150);
        let learnings_json = format!(
            r#"[{{"rule":"{}","source_executions":[]}},{{"rule":"short","source_executions":[]}}]"#,
            long_rule
        );
        let llm = Arc::new(ScriptedLlm::new(vec![
            "## 今日卡在哪（问题层）\n- noop",
            &learnings_json,
        ]));
        let summ = LlmReflectionSummarizer::new(llm, ReflectionConfig::default());
        let (inputs, _id) = inputs_with_one_exec();
        let (_summary, rules) = summ.summarize(&sample_config(), &inputs).await.unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].rule, "short");
    }

    #[tokio::test]
    async fn test_malformed_json_yields_empty_rules() {
        let llm = Arc::new(ScriptedLlm::new(vec![
            "## 今日卡在哪（问题层）\n- noop",
            "not json at all",
        ]));
        let summ = LlmReflectionSummarizer::new(llm, ReflectionConfig::default());
        let (inputs, _id) = inputs_with_one_exec();
        let (_summary, rules) = summ.summarize(&sample_config(), &inputs).await.unwrap();
        assert!(rules.is_empty(), "unparseable JSON must yield zero rules");
    }

    #[tokio::test]
    async fn test_code_fenced_json_is_parsed() {
        let learnings_json = "```json\n[{\"rule\":\"fenced rule\",\"source_executions\":[]}]\n```";
        let llm = Arc::new(ScriptedLlm::new(vec![
            "## 今日卡在哪（问题层）\n- noop",
            learnings_json,
        ]));
        let summ = LlmReflectionSummarizer::new(llm, ReflectionConfig::default());
        let (inputs, _id) = inputs_with_one_exec();
        let (_summary, rules) = summ.summarize(&sample_config(), &inputs).await.unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].rule, "fenced rule");
    }

    #[test]
    fn test_cheap_attribution_detector() {
        assert!(is_cheap_attribution("今天运气好"));
        assert!(is_cheap_attribution("Flaky network tripped the test"));
        assert!(!is_cheap_attribution("Autopilot 连续失败切 Persistent"));
    }

    #[test]
    fn test_reflection_config_default() {
        let cfg = ReflectionConfig::default();
        assert_eq!(cfg.model, "claude-sonnet");
        assert_eq!(cfg.max_rules, 10);
        assert!((cfg.reflection_strictness - 0.7).abs() < f32::EPSILON);
    }

    #[test]
    fn test_strip_code_fences() {
        assert_eq!(strip_code_fences("```json\n[]\n```"), "[]");
        assert_eq!(strip_code_fences("```\n[]\n```"), "[]");
        assert_eq!(strip_code_fences("[]"), "[]");
    }
}
