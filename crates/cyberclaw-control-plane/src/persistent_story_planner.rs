//! Sprint D3: LLM-driven Story DAG planner for persistent execution.
//!
//! Builds a [`crate::persistent_execution::PersistentExecutionPlan`] from a
//! free-form goal by:
//!
//! 1. Prompting an [`cyberclaw_llm::LlmClient`] with the user goal **and** the
//!    list of available `(connector_id, capability_id)` pairs from
//!    [`crate::capability_discovery::CapabilityDiscovery::discover_local`].
//! 2. Parsing the JSON Story DAG response.
//! 3. Validating: every story has ≥1 acceptance criterion, every
//!    `capability_id` exists in the local discovery hit, and the dependency
//!    graph is acyclic ([`crate::persistent_execution::topological_order`]).
//! 4. On parse / validation failure, retry **once** with a corrective prompt;
//!    after the second failure, fall back to a single-story placeholder plan
//!    with a `ScriptExitZero{script:"echo placeholder"}` verifier so the
//!    persistent runtime always has a runnable plan.
//!
//! # LLM JSON contract
//!
//! ```json
//! {
//!   "stories": [
//!     {
//!       "id": "S1",
//!       "description": "...",
//!       "capability_id": "voice.transcribe",
//!       "depends_on": [],
//!       "criteria": [
//!         {"description": "transcript non-empty",
//!          "verifier": {"type": "text_non_empty", "path": "/tmp/out.txt"}}
//!       ]
//!     }
//!   ]
//! }
//! ```
//!
//! # Defence-in-depth
//!
//! The pipeline mirrors the Sprint S10 hardening (commit 46b4e81): the LLM
//! produces a draft, then Rust post-processing rejects malformed inputs
//! (cycle / missing criteria / unknown capability). LLM JSON accuracy is not
//! relied on as the only line of defence.

use std::collections::HashSet;
use std::sync::Arc;

use serde::Deserialize;

use cyberclaw_core::ids::{CapabilityId, ConnectorId};
use cyberclaw_llm::types::{ChatRequest, Message};
use cyberclaw_llm::LlmClient;

use crate::persistent_execution::{
    topological_order, AcceptanceCriterion, CapabilitySource, PersistentExecutionError,
    PersistentExecutionPlan, Story, VerifierKind,
};

/// Maximum number of retry attempts after the initial LLM call. The spec asks
/// for "失败 retry 1 次"; we bake it in as a constant so callers don't have to
/// re-discover it.
const MAX_LLM_RETRIES: u32 = 1;

const SYSTEM_PROMPT: &str = r#"You are a persistent-execution Story DAG planner. Given a goal, output ONLY a JSON object describing one or more stories with acceptance criteria.

Top-level schema:

{
  "stories": [
    {
      "id": "<unique short id>",
      "description": "<what this story accomplishes>",
      "capability_id": "<id from AVAILABLE CAPABILITIES below, or null>",
      "capability_input": { ...JSON object matching the capability's input schema, or omit for null... },
      "depends_on": ["<other story id>", ...],
      "criteria": [
        { "description": "<testable criterion>", "verifier": { ...see below... } }
      ]
    }
  ]
}

CAPABILITY INPUT — when `capability_id` is set, you SHOULD also set `capability_input` to a JSON object the dispatcher can pass to that capability. Examples:
  capability_id "fs.write"      → capability_input { "path": "/tmp/out.txt", "content": "hello" }
  capability_id "cmd.run"       → capability_input { "command": "awk -F',' '{sum+=$3} END {print sum}' /tmp/data.csv" }
  capability_id "slides.render" → capability_input { "markdown": "<deck markdown body>", "output_path": "/tmp/deck.pptx" }
Omit `capability_input` only when the capability genuinely takes no parameters; otherwise the dispatcher receives `null` and the call no-ops.

VERIFIER schema (pick one `type` per criterion; ALL listed fields are REQUIRED):

  { "type": "file_exists", "path": "/abs/path" }
  { "type": "file_mime_matches", "path": "/abs/path", "expected": "image/png" }
  { "type": "file_non_empty", "path": "/abs/path" }
  { "type": "audio_duration_gt", "path": "/abs/out.wav", "min_secs": 2.0 }
  { "type": "text_non_empty", "path": "/abs/note.txt" }
  { "type": "numeric_matches_csv", "csv_path": "/abs/data.csv", "formula": "sum(revenue)", "tolerance": 0.01 }
  { "type": "script_exit_zero", "script": "pytest", "args": ["-q","/abs/repo"], "cwd": "/abs/repo" }
  { "type": "llm_semantic_match", "reference": "<expected meaning>", "actual_path": "/abs/output.txt", "threshold": 0.7 }

Rules:
- Output ONLY the JSON object — no `<think>` block, no commentary, no markdown fences.
- Every story MUST have at least one criterion.
- Every criterion MUST include a verifier; pick the verifier type that best fits the goal AND include EVERY required field for that type (e.g. script_exit_zero always needs `script`, `args`, `cwd`).
- depends_on must reference existing story ids in the same plan; cycles are forbidden.
- capability_id, when set, must come from AVAILABLE CAPABILITIES below; use null when no capability fits.

VERIFIER SELECTION HEURISTIC — pick a verifier that *meaningfully* tests the goal, not the most convenient one:
- Goal mentions "sum / aggregate / total / count / average" over a CSV → use `numeric_matches_csv` (csv_path + formula + tolerance). This forces a second-pass arithmetic check that catches the LLM-arithmetic-hallucination class (e.g. claiming total=23,000 when the CSV sums to 19,500).
- Goal asks for a runnable test / script (pytest, cargo test, npm test, python script.py) → use `script_exit_zero` (script + args + cwd). This is the only verifier that actually runs code.
- Goal asks for a deck/document/report file in a specific format (pptx, pdf, png, mp4) → use `file_mime_matches` (path + expected MIME).
- Goal asks for a plain text/markdown/json output file → use `text_non_empty` (path) — the weakest assurance, only acceptable when no stronger fit exists.
- Goal asks for an audio/video file with duration → use `audio_duration_gt` (path + min_secs).
- Goal requires semantic equivalence between reference and actual (translation, summary, refactor) → use `llm_semantic_match` (reference + actual_path + threshold).
- Bare existence checks → use `file_exists` only when no other verifier fits — prefer stronger checks above.

REFUSAL RULE — refuse the goal at planning time when ALL of the following hold:
- The goal would cause irreversible data loss (drop database, delete production files, format disks).
- The goal would bypass governance/audit (run as root, use --no-verify, force-push to main).
- The goal would exfiltrate secrets, credentials, or PII to an unauthorized destination.
- The goal would cause physical harm or violate a clearly criminal law.

When you decide to refuse, output:
{
  "refusal": "<one-sentence reason rooted in the rule above>",
  "stories": []
}

Refusal is **structural** — emitting a `refusal` field bypasses Story DAG construction. Do NOT refuse merely-risky goals (e.g. running tests, deploying to staging, sending notifications) — those go through the normal Story DAG with appropriate verifiers and the dispatcher's PolicyEngine handles per-capability approval.

AVAILABLE CAPABILITIES:
"#;

const RETRY_CORRECTION: &str = "Your previous response was rejected. Re-emit ONLY a valid JSON object matching the Story DAG schema. The previous error was: ";

/// Sprint D3: thin wire-format for the LLM-emitted Story DAG. We keep it
/// permissive (Strings instead of typed IDs) so a single bad entry does not
/// fail the whole plan; per-field validation runs after deserialisation.
///
/// `refusal` is a planner-level governance signal added 2026-05-06: when
/// the LLM decides the goal hits the REFUSAL RULE in the system prompt
/// (irreversible data loss / governance bypass / exfiltration / physical
/// harm), it emits `{"refusal": "<reason>", "stories": []}` instead of a
/// real DAG. The planner short-circuits to `refusal_plan(goal, reason)`.
#[derive(Debug, Deserialize)]
struct WireStoryPlan {
    #[serde(default)]
    stories: Vec<WireStory>,
    #[serde(default)]
    refusal: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WireStory {
    id: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    capability_id: Option<String>,
    /// 2026-05-06 — JSON input payload for the capability dispatch.
    /// Mirrors `Story::capability_input` so the planner can carry real
    /// parameters (e.g. `{ "command": "awk -F',' …" }` for cmd.run,
    /// `{ "markdown": "...", "output_path": "..." }` for slides.render).
    /// Without this every capability got `Value::Null` as input and
    /// non-trivial connectors silently no-oped.
    #[serde(default)]
    capability_input: Option<serde_json::Value>,
    #[serde(default)]
    depends_on: Vec<String>,
    #[serde(default)]
    criteria: Vec<WireCriterion>,
}

#[derive(Debug, Deserialize)]
struct WireCriterion {
    #[serde(default)]
    description: String,
    #[serde(default)]
    verifier: Option<VerifierKind>,
}

/// Sprint D3: planner that asks the LLM for a Story DAG, validates it, and
/// falls back to a placeholder plan on failure.
///
/// The planner is intentionally **decoupled** from the existing
/// [`crate::llm_planner::LlmExecutionPlanner`]: that planner targets the
/// `expected_outcomes` / `actions` shape used by Autopilot, while this one
/// targets the `stories` shape used by the persistent runtime. Sharing one
/// planner would require either two prompt branches or a discriminated
/// response, both of which cost more than this small parallel module.
pub struct PersistentStoryPlanner {
    llm_client: Arc<dyn LlmClient>,
    model: String,
    /// Local capability hits — used to validate `capability_id` references
    /// emitted by the LLM. Stories that reference a capability outside this
    /// list are rejected (treated as a parse failure, drives retry).
    available_capabilities: Vec<(ConnectorId, CapabilityId)>,
}

impl PersistentStoryPlanner {
    /// Build a new planner with the given LLM client + model + capability
    /// allowlist. Pass `available_capabilities = vec![]` to disable
    /// capability validation (every emitted capability_id will be rejected).
    pub fn new(
        llm_client: Arc<dyn LlmClient>,
        model: impl Into<String>,
        available_capabilities: Vec<(ConnectorId, CapabilityId)>,
    ) -> Self {
        Self {
            llm_client,
            model: model.into(),
            available_capabilities,
        }
    }

    fn system_prompt(&self) -> String {
        let mut s = String::from(SYSTEM_PROMPT);
        if self.available_capabilities.is_empty() {
            s.push_str("- (no capabilities available; set capability_id to null in every story)\n");
        } else {
            for (cid, cap) in &self.available_capabilities {
                s.push_str(&format!(
                    "- connector_id: {}, capability_id: {}\n",
                    cid, cap
                ));
            }
        }
        s
    }

    /// Plan a Story DAG for the given `goal`. Returns a validated
    /// [`PersistentExecutionPlan`] or — on persistent LLM/parse failure — a
    /// single-story placeholder plan that the persistent runtime can still
    /// run.
    pub async fn plan(&self, goal: &str) -> PersistentExecutionPlan {
        let mut messages = vec![
            Message::system(self.system_prompt()),
            Message::user(format!("Goal: {}", goal)),
        ];

        for attempt in 0..=MAX_LLM_RETRIES {
            let request = ChatRequest {
                model: self.model.clone(),
                messages: messages.clone(),
                temperature: Some(0.1),
                ..Default::default()
            };

            let response = match self.llm_client.chat_completion(request).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(error = %e, "PersistentStoryPlanner: LLM call failed; falling back");
                    return placeholder_plan(goal);
                }
            };

            let content = response
                .choices
                .first()
                .map(|c| c.message.content.clone())
                .unwrap_or_default();
            let json_str = strip_optional_fences(&content);

            // Governance: planner-level refusal short-circuit. If the LLM
            // decided the goal hits the REFUSAL RULE in the system prompt,
            // it emits `{"refusal": "<reason>", "stories": []}`. We honor
            // it without going through Story DAG validation — the
            // resulting plan has a single sentinel story whose acceptance
            // criterion records the refusal text and resolves to a
            // FileExists("/dev/null") verifier (which always passes on
            // Unix), so the loop reports the refusal as a cleanly
            // completed plan rather than crashing on validation.
            if let Ok(wire) = serde_json::from_str::<WireStoryPlan>(&json_str) {
                if let Some(reason) = wire.refusal.as_deref() {
                    if !reason.trim().is_empty() {
                        tracing::info!(
                            goal = %goal,
                            reason = %reason,
                            "PersistentStoryPlanner: planner refused at planning time"
                        );
                        return refusal_plan(goal, reason);
                    }
                }
            }

            match self.parse_and_validate(&json_str, goal) {
                Ok(plan) => return plan,
                Err(reason) => {
                    let preview: String = content.chars().take(200).collect();
                    if attempt < MAX_LLM_RETRIES {
                        tracing::warn!(
                            attempt = attempt + 1,
                            reason = %reason,
                            content_preview = %preview,
                            "PersistentStoryPlanner: validation failed; retrying with corrective prompt"
                        );
                        messages.push(Message::assistant(content));
                        messages.push(Message::user(format!("{}{}", RETRY_CORRECTION, reason)));
                    } else {
                        tracing::warn!(
                            reason = %reason,
                            content_preview = %preview,
                            "PersistentStoryPlanner: max retries exhausted; falling back"
                        );
                    }
                }
            }
        }

        placeholder_plan(goal)
    }

    /// Parse the wire JSON, then run the defence-in-depth validators:
    /// 1. Every story has ≥1 criterion (verification completeness).
    /// 2. Every criterion has a verifier (mandatory by schema).
    /// 3. Every `capability_id`, when present, is in
    ///    `available_capabilities`.
    /// 4. The dependency graph is a DAG (Kahn cycle detection).
    /// 5. depends_on entries reference real ids.
    fn parse_and_validate(
        &self,
        json: &str,
        goal: &str,
    ) -> Result<PersistentExecutionPlan, String> {
        let wire: WireStoryPlan =
            serde_json::from_str(json).map_err(|e| format!("invalid JSON: {}", e))?;

        if wire.stories.is_empty() {
            return Err("plan must contain at least one story".to_string());
        }

        let allowed_caps: HashSet<String> = self
            .available_capabilities
            .iter()
            .map(|(_, c)| c.as_str().to_string())
            .collect();
        let connector_for_cap: std::collections::HashMap<String, ConnectorId> = self
            .available_capabilities
            .iter()
            .map(|(c, k)| (k.as_str().to_string(), c.clone()))
            .collect();

        let mut plan = PersistentExecutionPlan::new(goal);

        for ws in &wire.stories {
            // 1. verification completeness
            if ws.criteria.is_empty() {
                return Err(format!("story '{}' has no acceptance criteria", ws.id));
            }

            // 2. every criterion must have a verifier
            let mut criteria = Vec::with_capacity(ws.criteria.len());
            for wc in &ws.criteria {
                let verifier = wc.verifier.clone().ok_or_else(|| {
                    format!(
                        "story '{}' criterion '{}' is missing a verifier",
                        ws.id, wc.description
                    )
                })?;
                criteria
                    .push(AcceptanceCriterion::new(wc.description.clone()).with_verifier(verifier));
            }

            let mut story = Story::new(&ws.id, &ws.description, criteria);
            for dep in &ws.depends_on {
                story = story.with_dependency(dep.clone());
            }

            // 3. capability_id whitelist check
            if let Some(cap_str) = &ws.capability_id {
                if !allowed_caps.contains(cap_str) {
                    return Err(format!(
                        "story '{}' references unknown capability '{}'",
                        ws.id, cap_str
                    ));
                }
                let cap_id = CapabilityId::from_string(cap_str.clone())
                    .map_err(|e| format!("story '{}' capability_id parse error: {}", ws.id, e))?;
                let connector = connector_for_cap
                    .get(cap_str)
                    .cloned()
                    .ok_or_else(|| format!("story '{}' missing connector mapping", ws.id))?;
                story = story
                    .with_capability_id(cap_id)
                    .with_source(CapabilitySource::Native { connector });
            }

            // 2026-05-06 — carry capability_input through to Story so
            // PersistentLoop dispatches with real data instead of Null.
            if let Some(input) = ws.capability_input.clone() {
                story = story.with_capability_input(input);
            }

            plan.add_story(story);
        }

        // 4 + 5. Cycle / unknown-dependency check.
        topological_order(&plan).map_err(|e| match e {
            PersistentExecutionError::DependencyCycle { stories } => {
                format!("dependency cycle among stories: {:?}", stories)
            }
            PersistentExecutionError::UnknownDependency { story, missing } => {
                format!("story '{}' depends on unknown story '{}'", story, missing)
            }
            other => format!("unexpected validation error: {}", other),
        })?;

        Ok(plan)
    }
}

/// Planner-level governance refusal plan (added 2026-05-06).
///
/// Returns a plan whose single story records the refusal `reason` and
/// uses a `ScriptExitZero { script: "true" }` verifier so the persistent
/// runtime reports the refusal as a *cleanly* completed plan (`true`
/// always exits 0 on POSIX) rather than crashing on a missing verifier.
/// Clients can detect a refusal by checking `plan.goal.starts_with("[refused]")`.
pub fn refusal_plan(goal: &str, reason: &str) -> PersistentExecutionPlan {
    let crit = AcceptanceCriterion::new(format!("Refused at planning: {}", reason)).with_verifier(
        VerifierKind::ScriptExitZero {
            script: "true".to_string(),
            args: Vec::new(),
            cwd: ".".to_string(),
        },
    );
    let mut plan = PersistentExecutionPlan::new(format!("[refused] {}", goal));
    plan.add_story(Story::new(
        "S_refused",
        format!("Goal refused at planning time: {}", reason),
        vec![crit],
    ));
    plan
}

/// Sprint D3: fallback plan when the LLM cannot produce a valid Story DAG.
///
/// Single story with one criterion: `ScriptExitZero { script: "echo
/// placeholder" }`. The persistent runtime can dispatch this to confirm the
/// pipeline plumbing without wedging the whole execution.
pub fn placeholder_plan(goal: &str) -> PersistentExecutionPlan {
    let crit = AcceptanceCriterion::new("placeholder verifier").with_verifier(
        VerifierKind::ScriptExitZero {
            script: "echo".to_string(),
            args: vec!["placeholder".to_string()],
            cwd: ".".to_string(),
        },
    );
    let mut plan = PersistentExecutionPlan::new(format!("[placeholder] {}", goal));
    plan.add_story(Story::new(
        "S1",
        "PersistentStoryPlanner placeholder — LLM Story DAG unavailable",
        vec![crit],
    ));
    plan
}

/// Extract the JSON payload from a raw LLM response.
///
/// Reasoning-style models (MiniMax-M2.x, DeepSeek-R1, QwQ, o1-mini, etc.)
/// emit a `<think>...</think>` block before the answer, and chat-tuned
/// models often wrap the JSON in markdown fences or surround it with
/// prose. The historical implementation only stripped ` ```json ` fences,
/// so the planner failed JSON parsing the first time it ran against any
/// thinking model and fell back to `placeholder_plan` — which is exactly
/// what masked the entire D5 pipeline during the 2026-05-05 GA eval.
///
/// Strategy:
///   1. Drop a leading `<think>...</think>` block if present.
///   2. Strip a leading ` ```json ` / ` ``` ` fence if the result still
///      starts with one.
///   3. Slice from the first `{` to the last matching `}` so any
///      surrounding prose is ignored. This makes the parser robust to
///      both bare JSON (`{...}`) and chatty wrappers
///      (`Here is the plan: {...} — let me know if anything is unclear.`).
fn strip_optional_fences(content: &str) -> String {
    let mut s = content.trim().to_string();

    // 1. <think>...</think> reasoning prefix.
    if let Some(rest) = s.strip_prefix("<think>") {
        if let Some(end_idx) = rest.find("</think>") {
            s = rest[end_idx + "</think>".len()..].trim().to_string();
        }
    }

    // 2. ```json / ``` markdown fence.
    for fence in ["```json", "```"] {
        if let Some(stripped) = s.strip_prefix(fence) {
            if let Some(end) = stripped.rfind("```") {
                s = stripped[..end].trim().to_string();
                break;
            }
        }
    }

    // 3. Slice to the outermost { ... } so any pre/post prose is ignored.
    if let (Some(first), Some(last)) = (s.find('{'), s.rfind('}')) {
        if last > first {
            return s[first..=last].to_string();
        }
    }

    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use cyberclaw_llm::error::LlmResult;
    use cyberclaw_llm::types::{ChatChunk, ChatResponse, Choice};
    use futures::stream::Stream;
    use std::sync::Mutex;

    /// Mock client returning a fixed sequence of contents (per call).
    struct MockLlm {
        responses: Mutex<Vec<String>>,
        idx: std::sync::atomic::AtomicUsize,
    }

    impl MockLlm {
        fn new(responses: Vec<&str>) -> Arc<Self> {
            Arc::new(Self {
                responses: Mutex::new(responses.into_iter().map(|s| s.to_string()).collect()),
                idx: std::sync::atomic::AtomicUsize::new(0),
            })
        }
    }

    #[async_trait]
    impl LlmClient for MockLlm {
        async fn chat_completion(&self, _req: ChatRequest) -> LlmResult<ChatResponse> {
            let i = self.idx.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let body = {
                let v = self.responses.lock().unwrap();
                let idx = i.min(v.len() - 1);
                v[idx].clone()
            };
            Ok(ChatResponse {
                id: "mock".to_string(),
                object: "chat.completion".to_string(),
                created: 0,
                model: "mock".to_string(),
                choices: vec![Choice {
                    index: 0,
                    message: Message::assistant(body),
                    finish_reason: Some("stop".to_string()),
                }],
                usage: None,
            })
        }
        async fn chat_completion_stream(
            &self,
            _req: ChatRequest,
        ) -> LlmResult<Box<dyn Stream<Item = LlmResult<ChatChunk>> + Send + Unpin>> {
            unreachable!("streaming not used in tests")
        }
        fn provider(&self) -> &str {
            "mock"
        }
        async fn validate_connection(&self) -> LlmResult<()> {
            Ok(())
        }
    }

    fn caps() -> Vec<(ConnectorId, CapabilityId)> {
        vec![(
            ConnectorId::from_string("voice".to_string()).unwrap(),
            CapabilityId::from_string("voice.transcribe".to_string()).unwrap(),
        )]
    }

    #[tokio::test]
    async fn parses_well_formed_dag_response() {
        // Sprint D3: a clean LLM response yields a validated plan.
        let body = r#"{
            "stories": [
                {
                    "id": "S1",
                    "description": "transcribe",
                    "capability_id": "voice.transcribe",
                    "depends_on": [],
                    "criteria": [
                        {"description": "out", "verifier": {"type":"text_non_empty","path":"/tmp/o.txt"}}
                    ]
                }
            ]
        }"#;
        let llm = MockLlm::new(vec![body]);
        let planner = PersistentStoryPlanner::new(llm, "mock-model", caps());
        let plan = planner.plan("transcribe a voice memo").await;
        assert_eq!(plan.stories.len(), 1);
        assert_eq!(plan.stories[0].id, "S1");
        assert_eq!(plan.stories[0].criteria.len(), 1);
        assert!(plan.stories[0].capability_id.is_some());
    }

    #[tokio::test]
    async fn rejects_dag_cycle_and_falls_back_after_retry() {
        // Sprint D3: A→B→A cycle on first response; second response is also
        // bad; final result is the placeholder plan.
        let cycle = r#"{"stories":[
            {"id":"A","description":"a","capability_id":null,"depends_on":["B"],
             "criteria":[{"description":"x","verifier":{"type":"file_exists","path":"a"}}]},
            {"id":"B","description":"b","capability_id":null,"depends_on":["A"],
             "criteria":[{"description":"y","verifier":{"type":"file_exists","path":"b"}}]}
        ]}"#;
        let llm = MockLlm::new(vec![cycle, cycle]);
        let planner = PersistentStoryPlanner::new(llm, "mock-model", caps());
        let plan = planner.plan("cycle goal").await;
        // Falls back to placeholder.
        assert!(
            plan.goal.contains("[placeholder]"),
            "expected placeholder plan, got goal: {}",
            plan.goal
        );
        assert_eq!(plan.stories.len(), 1);
    }

    #[tokio::test]
    async fn rejects_empty_criteria_list() {
        // Sprint D3: every story must have ≥1 criterion. First response
        // violates this; second is the well-formed retry.
        let bad = r#"{"stories":[
            {"id":"S1","description":"d","capability_id":null,"depends_on":[],"criteria":[]}
        ]}"#;
        let good = r#"{"stories":[
            {"id":"S1","description":"d","capability_id":null,"depends_on":[],
             "criteria":[{"description":"c","verifier":{"type":"file_exists","path":"x"}}]}
        ]}"#;
        let llm = MockLlm::new(vec![bad, good]);
        let planner = PersistentStoryPlanner::new(llm, "mock-model", caps());
        let plan = planner.plan("empty crit goal").await;
        // Retry succeeded.
        assert!(!plan.goal.contains("[placeholder]"));
        assert_eq!(plan.stories.len(), 1);
        assert_eq!(plan.stories[0].criteria.len(), 1);
    }

    #[tokio::test]
    async fn rejects_unknown_capability_id() {
        // Sprint D3: a capability_id not in the allowlist is rejected.
        let bad = r#"{"stories":[
            {"id":"S1","description":"d","capability_id":"ghost.cap","depends_on":[],
             "criteria":[{"description":"c","verifier":{"type":"file_exists","path":"x"}}]}
        ]}"#;
        let llm = MockLlm::new(vec![bad, bad]);
        let planner = PersistentStoryPlanner::new(llm, "mock-model", caps());
        let plan = planner.plan("unknown cap").await;
        assert!(plan.goal.contains("[placeholder]"));
    }

    #[tokio::test]
    async fn falls_back_to_placeholder_on_llm_error() {
        // Sprint D3: hard LLM failure short-circuits to the placeholder.
        struct ErrLlm;
        #[async_trait]
        impl LlmClient for ErrLlm {
            async fn chat_completion(&self, _req: ChatRequest) -> LlmResult<ChatResponse> {
                Err(cyberclaw_llm::error::LlmError::Internal(
                    "network down".to_string(),
                ))
            }
            async fn chat_completion_stream(
                &self,
                _req: ChatRequest,
            ) -> LlmResult<Box<dyn Stream<Item = LlmResult<ChatChunk>> + Send + Unpin>>
            {
                unreachable!()
            }
            fn provider(&self) -> &str {
                "err"
            }
            async fn validate_connection(&self) -> LlmResult<()> {
                Ok(())
            }
        }
        let llm: Arc<dyn LlmClient> = Arc::new(ErrLlm);
        let planner = PersistentStoryPlanner::new(llm, "mock-model", caps());
        let plan = planner.plan("anything").await;
        assert!(plan.goal.contains("[placeholder]"));
        assert_eq!(plan.stories.len(), 1);
        // Placeholder verifier must be ScriptExitZero.
        match &plan.stories[0].criteria[0].verifier {
            Some(VerifierKind::ScriptExitZero { script, .. }) => {
                assert_eq!(script, "echo");
            }
            other => panic!("expected ScriptExitZero placeholder, got {:?}", other),
        }
    }

    #[test]
    fn placeholder_plan_has_one_story_one_criterion_one_script_verifier() {
        let p = placeholder_plan("test goal");
        assert_eq!(p.stories.len(), 1);
        assert_eq!(p.stories[0].criteria.len(), 1);
        assert!(p.goal.contains("[placeholder]"));
    }

    #[test]
    fn refusal_plan_marks_goal_with_prefix_and_records_reason() {
        let p = refusal_plan("drop production database", "irreversible data loss");
        assert!(p.goal.starts_with("[refused]"), "goal: {}", p.goal);
        assert_eq!(p.stories.len(), 1);
        assert_eq!(p.stories[0].id, "S_refused");
        assert!(p.stories[0]
            .description
            .contains("Goal refused at planning time"));
        assert_eq!(p.stories[0].criteria.len(), 1);
        // ScriptExitZero('true') verifier so the runtime cleanly reports
        // the refusal as a completed plan rather than crashing.
        match &p.stories[0].criteria[0].verifier {
            Some(VerifierKind::ScriptExitZero { script, .. }) => {
                assert_eq!(script, "true");
            }
            other => panic!("expected ScriptExitZero('true'), got {:?}", other),
        }
    }

    #[test]
    fn strip_fences_handles_bare_json() {
        assert_eq!(strip_optional_fences(r#"{"goal":"x"}"#), r#"{"goal":"x"}"#);
    }

    #[test]
    fn strip_fences_handles_markdown_fence() {
        let raw = "```json\n{\"goal\":\"x\"}\n```";
        assert_eq!(strip_optional_fences(raw), r#"{"goal":"x"}"#);
    }

    #[test]
    fn strip_fences_drops_think_block_prefix() {
        // MiniMax-M2.x / DeepSeek-R1 / QwQ / o1-mini emit <think>...</think>
        // before the answer. This is the regression that masked the entire
        // D5 PersistentLoop pipeline during the 2026-05-05 GA eval — the
        // planner saw `<think>...` as JSON column 0, failed parsing twice,
        // and fell back to placeholder_plan.
        let raw = "<think>\nLet me plan...\n</think>\n\n{\"goal\":\"real\",\"stories\":[]}";
        assert_eq!(
            strip_optional_fences(raw),
            r#"{"goal":"real","stories":[]}"#
        );
    }

    #[test]
    fn strip_fences_slices_outermost_braces_through_prose() {
        let raw = "Here is your plan: {\"goal\":\"x\"} — let me know!";
        assert_eq!(strip_optional_fences(raw), r#"{"goal":"x"}"#);
    }

    #[test]
    fn strip_fences_handles_think_then_fence() {
        // Some models emit both: think block AND markdown fence.
        let raw = "<think>reasoning</think>\n```json\n{\"goal\":\"x\"}\n```";
        assert_eq!(strip_optional_fences(raw), r#"{"goal":"x"}"#);
    }
}
