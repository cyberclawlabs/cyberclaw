//! Daily Digest — per-agent self-reflection loop (Sprint 8 L6).
//!
//! Drives the "every day the agent writes a summary of what it did, learns
//! from it, and persists the learnings" pattern borrowed from
//! `hermes-agent-self-evolution`. The Skill-level specification lives in
//! `ecosystem/skills/daily-digest/SKILL.md`; this module is the runtime
//! skeleton.
//!
//! # Architectural placement
//!
//! Per EVOLUTION_IDIOMS §1 this module is a **plan-in + events-out**
//! orchestrator: it takes a declarative [`DailyDigestConfig`], dispatches
//! through three injected traits ([`DigestCollector`] + [`DigestSummarizer`]
//! + [`DigestPersister`]), and emits a [`DailyDigestOutcome`]. It does not
//! know about Cron, LLMs, or `cyberclaw-store` — those live behind the
//! three traits so the coordinator remains testable in isolation.
//!
//! # Scope (what this file **does not** do yet)
//!
//! - No Cron trigger wiring. The `WorkflowTrigger::Cron` registration will
//!   land in Sprint 9; for now a caller invokes [`DefaultDailyDigestCoordinator::run`]
//!   on their own schedule.
//! - No LLM reflection. The default summarizer path is a mechanical
//!   aggregator that produces the three-section markdown body specified in
//!   `ecosystem/skills/daily-digest/SKILL.md`; true "reflection → rule
//!   extraction" is the next iteration's job.
//! - No governance-rule mutation. Per SKILL.md "HARD-AVOID", the persist
//!   step only writes `SemanticMemory`; it never touches
//!   `DangerousCapabilityFilter` or `ToolPermissionMatcher`.

use std::collections::BTreeMap;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use cyberclaw_core::ids::{AgentId, ArtifactId, ExecutionId, TraceId};

// ============================================================================
// Data model (plan-in per EVOLUTION_IDIOMS §3)
// ============================================================================

/// Declarative config for one digest run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyDigestConfig {
    /// Agent this digest is scoped to. One digest per agent per day.
    pub agent_id: AgentId,
    /// UTC window covered by this digest. Typically the previous UTC day.
    pub window_start: DateTime<Utc>,
    pub window_end: DateTime<Utc>,
    /// Max rules to retain (SKILL.md caps at 10).
    #[serde(default = "default_max_rules")]
    pub max_rules: usize,
}

fn default_max_rules() -> usize {
    10
}

/// Raw facts gathered in Stage 1.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DigestInputs {
    pub executions: Vec<ExecutionFact>,
    pub artifacts: Vec<ArtifactFact>,
    pub traces: Vec<TraceFact>,
    /// Optional journal iterations if the agent ran under a persistent loop.
    pub journal_iterations: Vec<JournalFact>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionFact {
    pub execution_id: ExecutionId,
    pub status: String,
    pub execution_mode: String,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactFact {
    pub artifact_id: ArtifactId,
    pub kind: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceFact {
    pub trace_id: TraceId,
    pub event_type: String,
    pub severity: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalFact {
    pub iteration: u32,
    pub verdict: String,
}

/// Three-section summary produced in Stage 2.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DigestSummary {
    pub facts_md: String,
    pub problems_md: String,
    pub learnings_md: String,
}

/// A procedural-memory rule candidate produced in Stage 4.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuleCandidate {
    /// Rule text (≤ 100 chars per SKILL.md).
    pub rule: String,
    /// Which executions supported this rule, for provenance.
    pub source_executions: Vec<ExecutionId>,
}

/// Final record persisted in Stage 5.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyDigestEntry {
    pub agent_id: AgentId,
    pub window_start: DateTime<Utc>,
    pub window_end: DateTime<Utc>,
    pub summary: DigestSummary,
    pub rules: Vec<RuleCandidate>,
    pub source_executions: Vec<ExecutionId>,
    pub source_artifacts: Vec<ArtifactId>,
    pub reflection_trace_id: Option<TraceId>,
    pub created_at: DateTime<Utc>,
}

/// Terminal result of one run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyDigestOutcome {
    /// `true` when the day had zero activity; the coordinator skipped all
    /// downstream stages and wrote nothing per the SKILL.md "skip empty days"
    /// rule.
    pub skipped_empty_day: bool,
    /// The entry actually persisted (None when skipped).
    pub entry: Option<DailyDigestEntry>,
}

// ============================================================================
// Traits (injected per EVOLUTION_IDIOMS §1)
// ============================================================================

/// Stage 1: pull raw facts from the platform's structured sources
/// (Execution / Artifact / Trace / ProgressJournal). Implementations live in
/// `cyberclaw-store` / `cyberclaw-observability`; this module doesn't know
/// about them.
#[async_trait]
pub trait DigestCollector: Send + Sync {
    async fn collect(&self, config: &DailyDigestConfig) -> Result<DigestInputs, DigestError>;
}

/// Stage 2 + 3 + 4: reduce facts to a three-section summary and extract
/// `RuleCandidate`s. The "reflection" HARD-GATE (why did it succeed/fail,
/// will it generalise, is it worth recording) lives inside the summarizer;
/// this module only asserts the output shape.
#[async_trait]
pub trait DigestSummarizer: Send + Sync {
    async fn summarize(
        &self,
        config: &DailyDigestConfig,
        inputs: &DigestInputs,
    ) -> Result<(DigestSummary, Vec<RuleCandidate>), DigestError>;
}

/// Stage 5: persist the final entry to Semantic Memory (and only to Semantic
/// Memory — governance rules are out of scope).
#[async_trait]
pub trait DigestPersister: Send + Sync {
    async fn persist(&self, entry: &DailyDigestEntry) -> Result<(), DigestError>;
}

/// Errors that can stop a digest run. Collector errors should be logged and
/// turned into `skipped_empty_day = true` rather than aborting the schedule
/// — but that policy lives in the coordinator, not the error type.
#[derive(Debug, Error)]
pub enum DigestError {
    #[error("collector failed: {0}")]
    Collect(String),
    #[error("summarizer failed: {0}")]
    Summarize(String),
    #[error("persister failed: {0}")]
    Persist(String),
}

// ============================================================================
// Coordinator (events-out)
// ============================================================================

/// Default orchestrator. Composes a [`DigestCollector`], [`DigestSummarizer`],
/// and [`DigestPersister`] into the 5-stage loop documented in
/// `ecosystem/skills/daily-digest/SKILL.md`.
pub struct DefaultDailyDigestCoordinator {
    collector: Box<dyn DigestCollector>,
    summarizer: Box<dyn DigestSummarizer>,
    persister: Box<dyn DigestPersister>,
}

impl DefaultDailyDigestCoordinator {
    pub fn new(
        collector: Box<dyn DigestCollector>,
        summarizer: Box<dyn DigestSummarizer>,
        persister: Box<dyn DigestPersister>,
    ) -> Self {
        Self {
            collector,
            summarizer,
            persister,
        }
    }

    /// Execute the full 5-stage loop for one config. Returns a terminal
    /// outcome; callers (workflow Cron, CLI) decide how to surface it.
    pub async fn run(&self, config: DailyDigestConfig) -> Result<DailyDigestOutcome, DigestError> {
        // Stage 1
        let inputs = self.collector.collect(&config).await?;
        if is_empty_day(&inputs) {
            return Ok(DailyDigestOutcome {
                skipped_empty_day: true,
                entry: None,
            });
        }

        // Stage 2 + 3 + 4
        let (summary, mut rules) = self.summarizer.summarize(&config, &inputs).await?;
        if rules.len() > config.max_rules {
            rules.truncate(config.max_rules);
        }

        // Stage 5
        let entry = DailyDigestEntry {
            agent_id: config.agent_id.clone(),
            window_start: config.window_start,
            window_end: config.window_end,
            summary,
            rules,
            source_executions: inputs
                .executions
                .iter()
                .map(|e| e.execution_id.clone())
                .collect(),
            source_artifacts: inputs
                .artifacts
                .iter()
                .map(|a| a.artifact_id.clone())
                .collect(),
            reflection_trace_id: None,
            created_at: Utc::now(),
        };

        self.persister.persist(&entry).await?;

        Ok(DailyDigestOutcome {
            skipped_empty_day: false,
            entry: Some(entry),
        })
    }
}

fn is_empty_day(inputs: &DigestInputs) -> bool {
    inputs.executions.is_empty()
        && inputs.artifacts.is_empty()
        && inputs.traces.is_empty()
        && inputs.journal_iterations.is_empty()
}

// ============================================================================
// Helpers — mechanical summary (default, no LLM)
// ============================================================================

/// Build the "facts layer" markdown section from raw inputs. Pure function,
/// exposed so implementers can compose it into their own summarizer.
pub fn build_facts_md(inputs: &DigestInputs) -> String {
    let mut status_counts: BTreeMap<String, usize> = BTreeMap::new();
    for e in &inputs.executions {
        *status_counts.entry(e.status.clone()).or_default() += 1;
    }
    let mut mode_counts: BTreeMap<String, usize> = BTreeMap::new();
    for e in &inputs.executions {
        *mode_counts.entry(e.execution_mode.clone()).or_default() += 1;
    }
    let mut kind_counts: BTreeMap<String, usize> = BTreeMap::new();
    for a in &inputs.artifacts {
        *kind_counts.entry(a.kind.clone()).or_default() += 1;
    }
    format!(
        "## 今日发生了什么（事实层）\n\
         - 完成执行 {n_exec} 条（状态分布：{status}）\n\
         - 新增 Artifact {n_art} 个（kind 分布：{kinds}）\n\
         - 执行模式分布：{modes}\n",
        n_exec = inputs.executions.len(),
        status = fmt_counts(&status_counts),
        n_art = inputs.artifacts.len(),
        kinds = fmt_counts(&kind_counts),
        modes = fmt_counts(&mode_counts),
    )
}

fn fmt_counts(map: &BTreeMap<String, usize>) -> String {
    if map.is_empty() {
        return "(空)".to_string();
    }
    map.iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(", ")
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    // ------- Mock trait impls -------

    struct MockCollector(DigestInputs);
    #[async_trait]
    impl DigestCollector for MockCollector {
        async fn collect(&self, _c: &DailyDigestConfig) -> Result<DigestInputs, DigestError> {
            Ok(self.0.clone())
        }
    }

    struct MockSummarizer {
        rules: Vec<RuleCandidate>,
    }
    #[async_trait]
    impl DigestSummarizer for MockSummarizer {
        async fn summarize(
            &self,
            _c: &DailyDigestConfig,
            inputs: &DigestInputs,
        ) -> Result<(DigestSummary, Vec<RuleCandidate>), DigestError> {
            Ok((
                DigestSummary {
                    facts_md: build_facts_md(inputs),
                    problems_md: "## 今日卡在哪（问题层）\n_none_".into(),
                    learnings_md: "## 今日学到什么（经验层）\n_none_".into(),
                },
                self.rules.clone(),
            ))
        }
    }

    #[derive(Default)]
    struct MockPersister(Arc<Mutex<Vec<DailyDigestEntry>>>);
    #[async_trait]
    impl DigestPersister for MockPersister {
        async fn persist(&self, entry: &DailyDigestEntry) -> Result<(), DigestError> {
            self.0.lock().unwrap().push(entry.clone());
            Ok(())
        }
    }

    fn sample_config() -> DailyDigestConfig {
        DailyDigestConfig {
            agent_id: AgentId::new(),
            window_start: "2026-04-18T00:00:00Z".parse().unwrap(),
            window_end: "2026-04-19T00:00:00Z".parse().unwrap(),
            max_rules: 3,
        }
    }

    fn sample_inputs() -> DigestInputs {
        DigestInputs {
            executions: vec![ExecutionFact {
                execution_id: ExecutionId::new(),
                status: "completed".into(),
                execution_mode: "normal".into(),
                started_at: "2026-04-18T01:00:00Z".parse().unwrap(),
                completed_at: Some("2026-04-18T01:05:00Z".parse().unwrap()),
            }],
            artifacts: vec![],
            traces: vec![],
            journal_iterations: vec![],
        }
    }

    #[tokio::test]
    async fn empty_day_skips_summarize_and_persist() {
        let log = Arc::new(Mutex::new(Vec::<DailyDigestEntry>::new()));
        let coord = DefaultDailyDigestCoordinator::new(
            Box::new(MockCollector(DigestInputs::default())),
            Box::new(MockSummarizer { rules: vec![] }),
            Box::new(MockPersister(log.clone())),
        );
        let out = coord.run(sample_config()).await.unwrap();
        assert!(out.skipped_empty_day);
        assert!(out.entry.is_none());
        assert!(log.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn non_empty_day_persists_entry() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let coord = DefaultDailyDigestCoordinator::new(
            Box::new(MockCollector(sample_inputs())),
            Box::new(MockSummarizer {
                rules: vec![RuleCandidate {
                    rule: "RiskLevel Critical 前先跑 ToolPermissionMatcher 预演".into(),
                    source_executions: vec![],
                }],
            }),
            Box::new(MockPersister(log.clone())),
        );
        let out = coord.run(sample_config()).await.unwrap();
        assert!(!out.skipped_empty_day);
        let entry = out.entry.expect("entry must be present");
        assert_eq!(entry.rules.len(), 1);
        assert_eq!(log.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn rules_truncated_to_max() {
        let many = (0..20)
            .map(|i| RuleCandidate {
                rule: format!("rule-{}", i),
                source_executions: vec![],
            })
            .collect::<Vec<_>>();
        let log = Arc::new(Mutex::new(Vec::new()));
        let coord = DefaultDailyDigestCoordinator::new(
            Box::new(MockCollector(sample_inputs())),
            Box::new(MockSummarizer { rules: many }),
            Box::new(MockPersister(log.clone())),
        );
        let mut cfg = sample_config();
        cfg.max_rules = 5;
        let out = coord.run(cfg).await.unwrap();
        let entry = out.entry.unwrap();
        assert_eq!(entry.rules.len(), 5);
    }

    #[tokio::test]
    async fn persister_failure_surfaces() {
        struct FailingPersister;
        #[async_trait]
        impl DigestPersister for FailingPersister {
            async fn persist(&self, _e: &DailyDigestEntry) -> Result<(), DigestError> {
                Err(DigestError::Persist("semantic memory down".into()))
            }
        }
        let coord = DefaultDailyDigestCoordinator::new(
            Box::new(MockCollector(sample_inputs())),
            Box::new(MockSummarizer { rules: vec![] }),
            Box::new(FailingPersister),
        );
        let err = coord.run(sample_config()).await.unwrap_err();
        assert!(matches!(err, DigestError::Persist(_)));
    }

    #[test]
    fn facts_md_includes_status_and_kind_breakdown() {
        let inputs = DigestInputs {
            executions: vec![
                ExecutionFact {
                    execution_id: ExecutionId::new(),
                    status: "completed".into(),
                    execution_mode: "normal".into(),
                    started_at: Utc::now(),
                    completed_at: None,
                },
                ExecutionFact {
                    execution_id: ExecutionId::new(),
                    status: "failed".into(),
                    execution_mode: "autopilot".into(),
                    started_at: Utc::now(),
                    completed_at: None,
                },
            ],
            artifacts: vec![ArtifactFact {
                artifact_id: ArtifactId::new(),
                kind: "code".into(),
                size_bytes: 1024,
            }],
            traces: vec![],
            journal_iterations: vec![],
        };
        let md = build_facts_md(&inputs);
        assert!(md.contains("completed=1"));
        assert!(md.contains("failed=1"));
        assert!(md.contains("code=1"));
        assert!(md.contains("normal=1"));
        assert!(md.contains("autopilot=1"));
    }
}
