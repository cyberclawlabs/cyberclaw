//! Episodic memory implementation for CyberClaw.
//!
//! # Overview
//!
//! This module provides the [`EpisodicContextProvider`] which retrieves and filters execution
//! history to build context projections for agent inference. Inspired by the episodic memory
//! model in cognitive science: the system stores a sequence of past execution "episodes" and
//! surfaces relevant ones based on time, actor, capability, or trace correlation.
//!
//! # Architecture
//!
//! ```text
//! ExecutionHistory (in-memory ring buffer)
//!        │
//!        ▼
//! EpisodicContextProvider
//!   ├── query_by_execution_id(...)   → single execution lookup
//!   ├── query_by_actor(...)          → actor-scoped history
//!   ├── query_by_time_range(...)     → temporal window
//!   └── project(EpisodicContextRequest) → relevance-ranked EpisodicProjection
//! ```

use crate::execution::{Execution, ExecutionStatus};
use crate::ids::{AgentId, CaseId, ExecutionId, TraceId};
use crate::memory_context::EpisodicContextRequest;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::{Arc, RwLock};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A snapshot of relevant execution history for a given query context.
///
/// This is the output type of [`EpisodicContextProvider::project`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodicProjection {
    /// Executions that matched the query, sorted by relevance score (descending).
    pub executions: Vec<ScoredExecution>,
    /// Human-readable summary notes generated from the matched executions.
    pub summary_notes: Vec<String>,
    /// Total executions in the store at query time (for context).
    pub total_stored: usize,
    /// Forward-looking predictions (populated by cold-path extraction).
    #[serde(default)]
    pub foresights: Vec<crate::memory_context::ForesightProjection>,
}

/// An execution paired with its relevance score.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoredExecution {
    /// The execution record.
    pub execution: Execution,
    /// Relevance score in [0.0, 1.0].  Higher is more relevant.
    pub score: f32,
}

/// Query parameters for [`EpisodicContextProvider`].
///
/// All fields are optional filters; a `None` field means "match any".
/// `max_results` caps how many scored executions are returned.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodicQuery {
    /// Filter to a specific execution.
    pub execution_id: Option<ExecutionId>,
    /// Filter to a specific actor/agent.
    pub actor_id: Option<AgentId>,
    /// Filter to a specific trace (distributed trace correlation).
    pub trace_id: Option<TraceId>,
    /// Filter to a specific case.
    pub case_id: Option<CaseId>,
    /// Only include executions that finished after this timestamp.
    pub after: Option<chrono::DateTime<chrono::Utc>>,
    /// Only include executions that started before this timestamp.
    pub before: Option<chrono::DateTime<chrono::Utc>>,
    /// Only include executions in these statuses (empty = all statuses).
    pub statuses: Vec<ExecutionStatus>,
    /// Maximum number of results to return.
    pub max_results: usize,
}

impl Default for EpisodicQuery {
    fn default() -> Self {
        Self {
            execution_id: None,
            actor_id: None,
            trace_id: None,
            case_id: None,
            after: None,
            before: None,
            statuses: vec![],
            max_results: 20,
        }
    }
}

// ---------------------------------------------------------------------------
// EpisodicContextProvider
// ---------------------------------------------------------------------------

/// In-memory ring buffer for execution history with query and projection support.
///
/// # Thread safety
///
/// Uses `Arc<RwLock<...>>` internally so it is cheaply cloneable and safe
/// to share across threads.
///
/// # Example
///
/// ```
/// use cyberclaw_core::memory::episodic::{EpisodicContextProvider, EpisodicQuery};
///
/// let provider = EpisodicContextProvider::new(50);
/// // add executions via provider.ingest(exec)
/// let query = EpisodicQuery { max_results: 5, ..Default::default() };
/// let results = provider.query(&query);
/// assert!(results.len() <= 5);
/// ```
#[derive(Clone)]
pub struct EpisodicContextProvider {
    store: Arc<RwLock<VecDeque<Execution>>>,
    /// Maximum number of executions retained.
    capacity: usize,
}

impl std::fmt::Debug for EpisodicContextProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let len = self.store.read().map(|g| g.len()).unwrap_or(0);
        f.debug_struct("EpisodicContextProvider")
            .field("capacity", &self.capacity)
            .field("stored", &len)
            .finish()
    }
}

impl EpisodicContextProvider {
    /// Create a new provider with the given ring-buffer capacity.
    ///
    /// When `capacity` executions are stored, ingesting a new one silently
    /// drops the oldest.
    pub fn new(capacity: usize) -> Self {
        Self {
            store: Arc::new(RwLock::new(VecDeque::with_capacity(capacity))),
            capacity,
        }
    }

    /// Ingest an execution into the ring buffer.
    ///
    /// If the buffer is full, the oldest entry is evicted to make room.
    pub fn ingest(&self, execution: Execution) {
        let mut store = self.store.write().unwrap();
        if store.len() >= self.capacity {
            store.pop_front();
        }
        store.push_back(execution);
    }

    /// Return how many executions are currently stored.
    pub fn len(&self) -> usize {
        self.store.read().unwrap().len()
    }

    /// Return `true` when no executions are stored.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Remove all stored executions.
    pub fn clear(&self) {
        self.store.write().unwrap().clear();
    }

    /// Look up a single execution by its ID.
    ///
    /// Returns `None` if the execution is not in the ring buffer (either never
    /// ingested or already evicted).
    pub fn query_by_execution_id(&self, id: &ExecutionId) -> Option<Execution> {
        let store = self.store.read().unwrap();
        store.iter().find(|e| &e.id == id).cloned()
    }

    /// Return all executions whose agent matches `actor_id`, newest first,
    /// capped at `max_results`.
    pub fn query_by_actor(&self, actor_id: &AgentId, max_results: usize) -> Vec<Execution> {
        let store = self.store.read().unwrap();
        store
            .iter()
            .rev()
            .filter(|e| &e.agent.id == actor_id)
            .take(max_results)
            .cloned()
            .collect()
    }

    /// Return all executions whose `started_at` falls within `[after, before]`
    /// (both bounds are inclusive and optional), newest first, capped at
    /// `max_results`.
    pub fn query_by_time_range(
        &self,
        after: Option<chrono::DateTime<chrono::Utc>>,
        before: Option<chrono::DateTime<chrono::Utc>>,
        max_results: usize,
    ) -> Vec<Execution> {
        let store = self.store.read().unwrap();
        store
            .iter()
            .rev()
            .filter(|e| {
                let ts = e.started_at.or(e.finished_at);
                match ts {
                    None => false,
                    Some(t) => after.is_none_or(|a| t >= a) && before.is_none_or(|b| t <= b),
                }
            })
            .take(max_results)
            .cloned()
            .collect()
    }

    /// Execute a structured [`EpisodicQuery`] and return scored executions
    /// sorted by relevance (highest score first).
    ///
    /// Relevance scoring applies the following additive components:
    ///
    /// | Signal                          | Weight |
    /// |----------------------------------|--------|
    /// | exact execution_id match         |  0.40  |
    /// | same actor                       |  0.25  |
    /// | same trace_id                    |  0.20  |
    /// | same case_id                     |  0.15  |
    /// | recency (exponential decay)      |  0.10  |
    ///
    /// Each component contributes independently; the total may exceed 1.0 for
    /// highly correlated executions but is clamped at 1.0 before return.
    pub fn query(&self, q: &EpisodicQuery) -> Vec<ScoredExecution> {
        let now = chrono::Utc::now();

        let mut results: Vec<ScoredExecution> = {
            let store = self.store.read().unwrap();
            store
                .iter()
                .filter(|e| self.passes_hard_filters(e, q))
                .map(|e| ScoredExecution {
                    score: self.score(e, q, now).min(1.0),
                    execution: e.clone(),
                })
                .collect()
        };

        // Sort: highest score first, then newest started_at as tiebreaker
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    let ta = a
                        .execution
                        .started_at
                        .unwrap_or(chrono::DateTime::UNIX_EPOCH);
                    let tb = b
                        .execution
                        .started_at
                        .unwrap_or(chrono::DateTime::UNIX_EPOCH);
                    tb.cmp(&ta)
                })
        });

        results.truncate(q.max_results);
        results
    }

    /// Build an [`EpisodicProjection`] from an [`EpisodicContextRequest`].
    ///
    /// This adapts the higher-level `MemoryContextProvider` request type into
    /// the [`EpisodicQuery`] format and enriches the output with summary notes.
    pub fn project(&self, request: &EpisodicContextRequest) -> EpisodicProjection {
        let q = EpisodicQuery {
            execution_id: request.execution_id.clone(),
            trace_id: request.trace_id.clone(),
            case_id: request.case_id.clone(),
            max_results: request.max_events,
            ..Default::default()
        };

        let total_stored = self.len();
        let scored = self.query(&q);
        let summary_notes = Self::generate_summary_notes(&scored);

        EpisodicProjection {
            executions: scored,
            summary_notes,
            total_stored,
            foresights: vec![],
        }
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    /// Hard (boolean) filter – returns `false` when the execution should be
    /// excluded regardless of relevance score.
    fn passes_hard_filters(&self, e: &Execution, q: &EpisodicQuery) -> bool {
        // Status filter
        if !q.statuses.is_empty() && !q.statuses.contains(&e.status) {
            return false;
        }

        // Temporal filters
        let ts = e.started_at.or(e.finished_at);
        if let Some(after) = q.after {
            match ts {
                None => return false,
                Some(t) if t < after => return false,
                _ => {}
            }
        }
        if let Some(before) = q.before {
            match ts {
                None => return false,
                Some(t) if t > before => return false,
                _ => {}
            }
        }

        true
    }

    /// Soft relevance score for a single execution given the query.
    fn score(&self, e: &Execution, q: &EpisodicQuery, now: chrono::DateTime<chrono::Utc>) -> f32 {
        let mut s: f32 = 0.0;

        if let Some(ref qid) = q.execution_id {
            if &e.id == qid {
                s += 0.40;
            }
        }

        if let Some(ref actor) = q.actor_id {
            if &e.agent.id == actor {
                s += 0.25;
            }
        }

        if let Some(ref trace) = q.trace_id {
            if &e.trace_id == trace {
                s += 0.20;
            }
        }

        if let Some(ref case) = q.case_id {
            if e.case_id.as_ref() == Some(case) {
                s += 0.15;
            }
        }

        // Recency bonus: exponential decay with half-life of 1 hour
        let ts = e.started_at.or(e.finished_at);
        if let Some(t) = ts {
            // i64 → f32 精度损失是可接受的 (用于时间衰减计算)
            #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
            let age_secs = (now - t).num_seconds().max(0) as f32;
            let half_life = 3600.0_f32; // 1 hour
            let decay = 0.10 * (-age_secs * std::f32::consts::LN_2 / half_life).exp();
            s += decay;
        }

        // If no query signals match at all, assign a small base score so
        // unfiltered queries still return results.
        if q.execution_id.is_none()
            && q.actor_id.is_none()
            && q.trace_id.is_none()
            && q.case_id.is_none()
        {
            // Recency-only mode: give a baseline so results sort by recency
            s = s.max(0.01);
        }

        s
    }

    /// Generate human-readable summary notes from a list of scored executions.
    fn generate_summary_notes(executions: &[ScoredExecution]) -> Vec<String> {
        if executions.is_empty() {
            return vec!["No matching executions found in episodic history.".to_string()];
        }

        let mut notes = Vec::new();

        // Overall statistics
        let total = executions.len();
        let completed = executions
            .iter()
            .filter(|se| se.execution.status == ExecutionStatus::Completed)
            .count();
        let failed = executions
            .iter()
            .filter(|se| se.execution.status == ExecutionStatus::Failed)
            .count();

        notes.push(format!(
            "Retrieved {total} execution(s) from episodic history: \
             {completed} completed, {failed} failed."
        ));

        // Most recent execution
        if let Some(most_recent) = executions.iter().max_by_key(|se| {
            se.execution
                .started_at
                .or(se.execution.finished_at)
                .unwrap_or(chrono::DateTime::UNIX_EPOCH)
        }) {
            let ts_str = most_recent
                .execution
                .started_at
                .map(|t| t.to_rfc3339())
                .unwrap_or_else(|| "unknown time".to_string());
            notes.push(format!(
                "Most recent execution: {} (agent: {}, started: {}).",
                most_recent.execution.id, most_recent.execution.agent.id, ts_str,
            ));
        }

        // Highest scored execution
        if let Some(top) = executions.first() {
            if executions.len() > 1 {
                notes.push(format!(
                    "Highest relevance: {} (score: {:.2}, status: {:?}).",
                    top.execution.id, top.score, top.execution.status
                ));
            }
        }

        notes
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::RiskLevel;
    use crate::execution::{AgentRef, ExecutionBudget};

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

    fn make_exec_id(s: &str) -> ExecutionId {
        ExecutionId::from_string(s.to_string()).unwrap()
    }

    fn make_agent_id(s: &str) -> AgentId {
        AgentId::from_string(s.to_string()).unwrap()
    }

    fn make_trace_id(s: &str) -> TraceId {
        TraceId::from_string(s.to_string()).unwrap()
    }

    fn make_case_id(s: &str) -> CaseId {
        CaseId::from_string(s.to_string()).unwrap()
    }

    fn make_execution(id: &str, agent: &str) -> Execution {
        let exec_id = make_exec_id(id);
        Execution {
            id: exec_id.clone(),
            root_execution_id: exec_id.clone(),
            parent_execution_id: None,
            owner_node_id: None,
            scheduled_node_id: None,
            placement_group: None,
            lease_id: None,
            handoff_count: 0,
            case_id: None,
            task_id: None,
            agent: AgentRef {
                id: make_agent_id(agent),
                role: "test".to_string(),
            },
            status: ExecutionStatus::Completed,
            join_strategy: None,
            budget: ExecutionBudget::default(),
            workspace: None,
            trace_id: make_trace_id("trace-default"),
            started_at: Some(chrono::Utc::now()),
            finished_at: Some(chrono::Utc::now()),
            risk_level: RiskLevel::Low,
            execution_mode: crate::execution::ExecutionMode::Normal,
        }
    }

    fn make_execution_with_time(
        id: &str,
        agent: &str,
        started: chrono::DateTime<chrono::Utc>,
    ) -> Execution {
        let mut exec = make_execution(id, agent);
        exec.started_at = Some(started);
        exec.finished_at = Some(started + chrono::Duration::seconds(10));
        exec
    }

    // -----------------------------------------------------------------------
    // Construction and basic operations
    // -----------------------------------------------------------------------

    #[test]
    fn test_new_provider_is_empty() {
        let p = EpisodicContextProvider::new(10);
        assert!(p.is_empty());
        assert_eq!(p.len(), 0);
    }

    #[test]
    fn test_ingest_single_execution() {
        let p = EpisodicContextProvider::new(10);
        p.ingest(make_execution("exec-001", "agent-a"));
        assert_eq!(p.len(), 1);
        assert!(!p.is_empty());
    }

    #[test]
    fn test_ring_buffer_evicts_oldest_when_full() {
        let p = EpisodicContextProvider::new(3);

        p.ingest(make_execution("exec-001", "agent-a"));
        p.ingest(make_execution("exec-002", "agent-a"));
        p.ingest(make_execution("exec-003", "agent-a"));
        // capacity reached; next ingest should evict exec-001
        p.ingest(make_execution("exec-004", "agent-a"));

        assert_eq!(p.len(), 3);
        assert!(p.query_by_execution_id(&make_exec_id("exec-001")).is_none());
        assert!(p.query_by_execution_id(&make_exec_id("exec-004")).is_some());
    }

    // -----------------------------------------------------------------------
    // query_by_execution_id
    // -----------------------------------------------------------------------

    #[test]
    fn test_query_by_execution_id_found() {
        let p = EpisodicContextProvider::new(10);
        p.ingest(make_execution("exec-abc", "agent-a"));
        let result = p.query_by_execution_id(&make_exec_id("exec-abc"));
        assert!(result.is_some());
        assert_eq!(result.unwrap().id, make_exec_id("exec-abc"));
    }

    #[test]
    fn test_query_by_execution_id_not_found() {
        let p = EpisodicContextProvider::new(10);
        p.ingest(make_execution("exec-abc", "agent-a"));
        let result = p.query_by_execution_id(&make_exec_id("exec-xyz"));
        assert!(result.is_none());
    }

    // -----------------------------------------------------------------------
    // query_by_actor
    // -----------------------------------------------------------------------

    #[test]
    fn test_query_by_actor_returns_matching_executions() {
        let p = EpisodicContextProvider::new(20);
        p.ingest(make_execution("exec-001", "agent-alice"));
        p.ingest(make_execution("exec-002", "agent-bob"));
        p.ingest(make_execution("exec-003", "agent-alice"));

        let results = p.query_by_actor(&make_agent_id("agent-alice"), 10);
        assert_eq!(results.len(), 2);
        for e in &results {
            assert_eq!(e.agent.id, make_agent_id("agent-alice"));
        }
    }

    #[test]
    fn test_query_by_actor_respects_max_results() {
        let p = EpisodicContextProvider::new(20);
        for i in 0..10 {
            p.ingest(make_execution(&format!("exec-{:03}", i), "agent-alice"));
        }
        let results = p.query_by_actor(&make_agent_id("agent-alice"), 3);
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn test_query_by_actor_empty_when_no_match() {
        let p = EpisodicContextProvider::new(10);
        p.ingest(make_execution("exec-001", "agent-alice"));
        let results = p.query_by_actor(&make_agent_id("agent-bob"), 10);
        assert!(results.is_empty());
    }

    // -----------------------------------------------------------------------
    // query_by_time_range
    // -----------------------------------------------------------------------

    #[test]
    fn test_query_by_time_range_filters_correctly() {
        let p = EpisodicContextProvider::new(20);
        let base = chrono::Utc::now() - chrono::Duration::hours(5);

        p.ingest(make_execution_with_time(
            "exec-old",
            "agent-a",
            base - chrono::Duration::hours(2),
        ));
        p.ingest(make_execution_with_time("exec-mid", "agent-a", base));
        p.ingest(make_execution_with_time(
            "exec-new",
            "agent-a",
            base + chrono::Duration::hours(2),
        ));

        let after = base - chrono::Duration::minutes(30);
        let before = base + chrono::Duration::hours(3);
        let results = p.query_by_time_range(Some(after), Some(before), 10);

        // exec-old is before `after`, so only exec-mid and exec-new should be returned
        assert_eq!(results.len(), 2);
        let ids: Vec<_> = results.iter().map(|e| e.id.as_str()).collect();
        assert!(ids.contains(&"exec-mid"));
        assert!(ids.contains(&"exec-new"));
    }

    #[test]
    fn test_query_by_time_range_no_bounds_returns_nothing_without_timestamps() {
        let p = EpisodicContextProvider::new(10);
        let mut exec = make_execution("exec-no-ts", "agent-a");
        exec.started_at = None;
        exec.finished_at = None;
        p.ingest(exec);

        // Executions without timestamps are excluded when bounds are specified
        let results = p.query_by_time_range(
            Some(chrono::Utc::now() - chrono::Duration::hours(1)),
            None,
            10,
        );
        assert!(results.is_empty());
    }

    #[test]
    fn test_query_by_time_range_unbounded_includes_all_timestamped() {
        let p = EpisodicContextProvider::new(10);
        p.ingest(make_execution("exec-001", "agent-a"));
        p.ingest(make_execution("exec-002", "agent-a"));

        let results = p.query_by_time_range(None, None, 10);
        assert_eq!(results.len(), 2);
    }

    // -----------------------------------------------------------------------
    // query (structured EpisodicQuery)
    // -----------------------------------------------------------------------

    #[test]
    fn test_query_returns_all_when_no_filters() {
        let p = EpisodicContextProvider::new(20);
        for i in 0..5 {
            p.ingest(make_execution(&format!("exec-{}", i), "agent-a"));
        }
        let q = EpisodicQuery {
            max_results: 20,
            ..Default::default()
        };
        let results = p.query(&q);
        assert_eq!(results.len(), 5);
    }

    #[test]
    fn test_query_respects_max_results() {
        let p = EpisodicContextProvider::new(20);
        for i in 0..10 {
            p.ingest(make_execution(&format!("exec-{:02}", i), "agent-a"));
        }
        let q = EpisodicQuery {
            max_results: 3,
            ..Default::default()
        };
        let results = p.query(&q);
        assert!(results.len() <= 3);
    }

    #[test]
    fn test_query_by_execution_id_scores_highest() {
        let p = EpisodicContextProvider::new(20);
        p.ingest(make_execution("exec-target", "agent-a"));
        p.ingest(make_execution("exec-other", "agent-a"));
        p.ingest(make_execution("exec-more", "agent-b"));

        let q = EpisodicQuery {
            execution_id: Some(make_exec_id("exec-target")),
            max_results: 10,
            ..Default::default()
        };
        let results = p.query(&q);
        // The target execution should have the highest score
        assert!(!results.is_empty());
        assert_eq!(results[0].execution.id, make_exec_id("exec-target"));
        assert!(results[0].score > 0.39); // at least the 0.40 bonus
    }

    #[test]
    fn test_query_by_actor_scores_higher_for_matching_actor() {
        let p = EpisodicContextProvider::new(20);
        p.ingest(make_execution("exec-alice-1", "agent-alice"));
        p.ingest(make_execution("exec-bob-1", "agent-bob"));

        let q = EpisodicQuery {
            actor_id: Some(make_agent_id("agent-alice")),
            max_results: 10,
            ..Default::default()
        };
        let results = p.query(&q);
        // alice's execution must come first
        assert!(!results.is_empty());
        assert_eq!(results[0].execution.agent.id, make_agent_id("agent-alice"));
    }

    #[test]
    fn test_query_by_trace_id_scores_matching_trace() {
        let p = EpisodicContextProvider::new(20);
        let mut exec_matching = make_execution("exec-trace-match", "agent-a");
        exec_matching.trace_id = make_trace_id("trace-special");
        p.ingest(exec_matching);
        p.ingest(make_execution("exec-other", "agent-a")); // trace-default

        let q = EpisodicQuery {
            trace_id: Some(make_trace_id("trace-special")),
            max_results: 10,
            ..Default::default()
        };
        let results = p.query(&q);
        assert!(!results.is_empty());
        assert_eq!(results[0].execution.id, make_exec_id("exec-trace-match"));
    }

    #[test]
    fn test_query_by_case_id_scores_matching_case() {
        let p = EpisodicContextProvider::new(20);
        let mut exec_with_case = make_execution("exec-with-case", "agent-a");
        exec_with_case.case_id = Some(make_case_id("case-special"));
        p.ingest(exec_with_case);
        p.ingest(make_execution("exec-no-case", "agent-a"));

        let q = EpisodicQuery {
            case_id: Some(make_case_id("case-special")),
            max_results: 10,
            ..Default::default()
        };
        let results = p.query(&q);
        assert!(!results.is_empty());
        assert_eq!(results[0].execution.id, make_exec_id("exec-with-case"));
        assert!(results[0].score >= 0.15);
    }

    #[test]
    fn test_query_status_filter_only_returns_matching_statuses() {
        let p = EpisodicContextProvider::new(20);

        let mut exec_failed = make_execution("exec-failed", "agent-a");
        exec_failed.status = ExecutionStatus::Failed;
        p.ingest(exec_failed);
        p.ingest(make_execution("exec-completed", "agent-a")); // Completed by default

        let q = EpisodicQuery {
            statuses: vec![ExecutionStatus::Failed],
            max_results: 10,
            ..Default::default()
        };
        let results = p.query(&q);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].execution.status, ExecutionStatus::Failed);
    }

    #[test]
    fn test_query_time_filter_excludes_old_executions() {
        let p = EpisodicContextProvider::new(20);
        let cutoff = chrono::Utc::now() - chrono::Duration::hours(1);
        let old_ts = cutoff - chrono::Duration::hours(1);
        let new_ts = cutoff + chrono::Duration::minutes(30);

        p.ingest(make_execution_with_time("exec-old", "agent-a", old_ts));
        p.ingest(make_execution_with_time("exec-new", "agent-a", new_ts));

        let q = EpisodicQuery {
            after: Some(cutoff),
            max_results: 10,
            ..Default::default()
        };
        let results = p.query(&q);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].execution.id, make_exec_id("exec-new"));
    }

    #[test]
    fn test_query_scores_clamped_at_one() {
        let p = EpisodicContextProvider::new(10);
        let mut exec = make_execution("exec-max", "agent-a");
        exec.trace_id = make_trace_id("trace-t");
        exec.case_id = Some(make_case_id("case-c"));
        p.ingest(exec.clone());

        let q = EpisodicQuery {
            execution_id: Some(exec.id.clone()),
            actor_id: Some(exec.agent.id.clone()),
            trace_id: Some(exec.trace_id.clone()),
            case_id: exec.case_id.clone(),
            max_results: 10,
            ..Default::default()
        };
        let results = p.query(&q);
        assert!(!results.is_empty());
        assert!(results[0].score <= 1.0, "score must be clamped to 1.0");
    }

    // -----------------------------------------------------------------------
    // project (EpisodicContextRequest)
    // -----------------------------------------------------------------------

    #[test]
    fn test_project_empty_store_returns_empty_projection() {
        let p = EpisodicContextProvider::new(50);
        let req = EpisodicContextRequest {
            case_id: None,
            execution_id: None,
            trace_id: None,
            max_events: 10,
        };
        let proj = p.project(&req);
        assert!(proj.executions.is_empty());
        assert_eq!(proj.total_stored, 0);
        assert!(!proj.summary_notes.is_empty()); // at least one note about empty results
    }

    #[test]
    fn test_project_returns_up_to_max_events() {
        let p = EpisodicContextProvider::new(50);
        for i in 0..15 {
            p.ingest(make_execution(&format!("exec-{:02}", i), "agent-a"));
        }
        let req = EpisodicContextRequest {
            case_id: None,
            execution_id: None,
            trace_id: None,
            max_events: 5,
        };
        let proj = p.project(&req);
        assert!(proj.executions.len() <= 5);
        assert_eq!(proj.total_stored, 15);
    }

    #[test]
    fn test_project_generates_summary_notes() {
        let p = EpisodicContextProvider::new(10);
        p.ingest(make_execution("exec-001", "agent-a"));
        let mut exec_failed = make_execution("exec-002", "agent-b");
        exec_failed.status = ExecutionStatus::Failed;
        p.ingest(exec_failed);

        let req = EpisodicContextRequest {
            case_id: None,
            execution_id: None,
            trace_id: None,
            max_events: 10,
        };
        let proj = p.project(&req);
        assert!(!proj.summary_notes.is_empty());
        // The first note should mention the count and status breakdown
        let first_note = &proj.summary_notes[0];
        assert!(first_note.contains("2"), "should mention 2 executions");
    }

    #[test]
    fn test_project_filtered_by_execution_id() {
        let p = EpisodicContextProvider::new(20);
        p.ingest(make_execution("exec-target", "agent-a"));
        p.ingest(make_execution("exec-other", "agent-b"));

        let req = EpisodicContextRequest {
            case_id: None,
            execution_id: Some(make_exec_id("exec-target")),
            trace_id: None,
            max_events: 10,
        };
        let proj = p.project(&req);
        // exec-target should be top result
        assert!(!proj.executions.is_empty());
        assert_eq!(proj.executions[0].execution.id, make_exec_id("exec-target"));
    }

    // -----------------------------------------------------------------------
    // Summary note generation
    // -----------------------------------------------------------------------

    #[test]
    fn test_summary_notes_empty_input() {
        let notes = EpisodicContextProvider::generate_summary_notes(&[]);
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("No matching"));
    }

    #[test]
    fn test_summary_notes_single_completed() {
        let exec = make_execution("exec-001", "agent-a");
        let scored = vec![ScoredExecution {
            execution: exec,
            score: 0.8,
        }];
        let notes = EpisodicContextProvider::generate_summary_notes(&scored);
        assert!(!notes.is_empty());
        assert!(notes[0].contains("1"));
        assert!(notes[0].contains("1 completed"));
    }

    #[test]
    fn test_summary_notes_multiple_with_failures() {
        let exec1 = make_execution("exec-001", "agent-a");
        let mut exec2 = make_execution("exec-002", "agent-b");
        exec2.status = ExecutionStatus::Failed;
        let scored = vec![
            ScoredExecution {
                execution: exec1,
                score: 0.9,
            },
            ScoredExecution {
                execution: exec2,
                score: 0.4,
            },
        ];
        let notes = EpisodicContextProvider::generate_summary_notes(&scored);
        assert!(notes[0].contains("1 failed"));
    }

    // -----------------------------------------------------------------------
    // Serialization (EpisodicProjection)
    // -----------------------------------------------------------------------

    #[test]
    fn test_episodic_projection_serde_roundtrip() {
        let p = EpisodicContextProvider::new(10);
        p.ingest(make_execution("exec-serde", "agent-a"));

        let req = EpisodicContextRequest {
            case_id: None,
            execution_id: None,
            trace_id: None,
            max_events: 10,
        };
        let proj = p.project(&req);

        let json = serde_json::to_string(&proj).expect("serialization must succeed");
        assert!(!json.is_empty());

        let deserialized: EpisodicProjection =
            serde_json::from_str(&json).expect("deserialization must succeed");
        assert_eq!(deserialized.executions.len(), proj.executions.len());
        assert_eq!(deserialized.total_stored, proj.total_stored);
    }

    // -----------------------------------------------------------------------
    // EpisodicQuery defaults
    // -----------------------------------------------------------------------

    #[test]
    fn test_episodic_query_default() {
        let q = EpisodicQuery::default();
        assert!(q.execution_id.is_none());
        assert!(q.actor_id.is_none());
        assert!(q.trace_id.is_none());
        assert!(q.case_id.is_none());
        assert!(q.after.is_none());
        assert!(q.before.is_none());
        assert!(q.statuses.is_empty());
        assert_eq!(q.max_results, 20);
    }

    // -----------------------------------------------------------------------
    // Debug impl
    // -----------------------------------------------------------------------

    #[test]
    fn test_debug_impl() {
        let p = EpisodicContextProvider::new(5);
        let debug_str = format!("{:?}", p);
        assert!(debug_str.contains("EpisodicContextProvider"));
        assert!(debug_str.contains("capacity"));
    }
}
