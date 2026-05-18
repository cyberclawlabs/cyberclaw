//! Token economics tracking for CyberClaw execution lifecycle
//!
//! Provides:
//! - [`TokenRecord`] for per-execution token usage data
//! - [`TokenSummary`] for aggregated statistics
//! - [`TokenTracker`] trait for pluggable storage backends
//! - [`InMemoryTokenTracker`] for development and testing
//! - [`TimedExecution`] for convenient record construction with timing

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur during token tracking operations
#[derive(Debug, thiserror::Error)]
pub enum TokenTrackingError {
    /// Underlying storage failure
    #[error("Storage error: {0}")]
    Storage(String),

    /// The provided record contains invalid data
    #[error("Invalid record: {0}")]
    InvalidRecord(String),
}

// ---------------------------------------------------------------------------
// Core data types
// ---------------------------------------------------------------------------

/// A single token usage record for one execution step.
///
/// Captures raw input/output/filtered token counts plus derived savings
/// percentage and wall-clock duration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenRecord {
    /// Unique identifier for the execution that produced this record
    pub execution_id: String,
    /// Wall-clock timestamp at record creation
    pub timestamp: DateTime<Utc>,
    /// Optional agent that drove the execution
    pub agent_id: Option<String>,
    /// Optional connector used during the execution
    pub connector_id: Option<String>,
    /// Optional capability invoked
    pub capability_id: Option<String>,
    /// Number of tokens in the raw input prompt
    pub input_tokens: usize,
    /// Number of tokens in the model output
    pub output_tokens: usize,
    /// Number of tokens remaining after filtering/compression
    pub filtered_tokens: usize,
    /// Percentage of input tokens saved by filtering: `(1 - filtered/input) * 100`
    pub savings_pct: f64,
    /// Wall-clock execution duration in milliseconds
    pub duration_ms: u64,
    /// Optional project path for per-project aggregation
    pub project_path: Option<String>,
}

/// Aggregated token statistics over a set of [`TokenRecord`]s.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TokenSummary {
    /// Sum of all `input_tokens` across records
    pub total_input: usize,
    /// Sum of all `output_tokens` across records
    pub total_output: usize,
    /// Sum of all `filtered_tokens` across records
    pub total_filtered: usize,
    /// Tokens saved by filtering: `total_input - total_filtered`
    pub total_saved: usize,
    /// Mean of `savings_pct` across records (0.0 when `execution_count == 0`)
    pub avg_savings_pct: f64,
    /// Number of records included in this summary
    pub execution_count: usize,
    /// Earliest record timestamp in the window (`None` when empty)
    pub period_start: Option<DateTime<Utc>>,
    /// Latest record timestamp in the window (`None` when empty)
    pub period_end: Option<DateTime<Utc>>,
}

// ---------------------------------------------------------------------------
// Filter and dimension types
// ---------------------------------------------------------------------------

/// Filtering criteria for token tracker queries.
///
/// All fields are optional; only non-`None` fields are applied.
/// Multiple filters are combined with logical AND.
#[derive(Debug, Clone, Default)]
pub struct TokenFilter {
    /// Restrict results to this agent
    pub agent_id: Option<String>,
    /// Restrict results to this connector
    pub connector_id: Option<String>,
    /// Restrict results to this capability
    pub capability_id: Option<String>,
    /// Restrict results to this project path
    pub project_path: Option<String>,
    /// Only include records on or after this timestamp
    pub since: Option<DateTime<Utc>>,
    /// Only include records on or before this timestamp
    pub until: Option<DateTime<Utc>>,
}

/// Grouping dimension for aggregated token queries.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AggregateDimension {
    /// Group by `agent_id` (records with `None` are grouped under `"<none>"`)
    ByAgent,
    /// Group by `connector_id` (records with `None` are grouped under `"<none>"`)
    ByConnector,
    /// Group by `capability_id` (records with `None` are grouped under `"<none>"`)
    ByCapability,
    /// Group by `project_path` (records with `None` are grouped under `"<none>"`)
    ByProject,
    /// Group by calendar day in UTC (`YYYY-MM-DD`)
    ByDay,
}

// ---------------------------------------------------------------------------
// TokenTracker trait
// ---------------------------------------------------------------------------

/// Pluggable storage backend for per-execution token usage records.
///
/// Implementations must be `Send + Sync` so they can be shared across async
/// tasks and stored behind `Arc<dyn TokenTracker>`.
#[async_trait]
pub trait TokenTracker: Send + Sync {
    /// Persist a single token usage record.
    ///
    /// Returns [`TokenTrackingError::InvalidRecord`] if the record contains
    /// logically impossible values (e.g. `filtered_tokens > input_tokens`).
    async fn record(&self, record: TokenRecord) -> Result<(), TokenTrackingError>;

    /// Return aggregated statistics for all records matching `filter`.
    async fn get_summary(&self, filter: &TokenFilter) -> Result<TokenSummary, TokenTrackingError>;

    /// Return per-group summaries for `dimension`, restricted to `filter`.
    ///
    /// Each element is `(group_key, summary)` where `group_key` is derived
    /// from the chosen dimension (e.g. agent id, connector id, date string).
    async fn get_aggregated(
        &self,
        filter: &TokenFilter,
        dimension: AggregateDimension,
    ) -> Result<Vec<(String, TokenSummary)>, TokenTrackingError>;

    /// Return the `limit` most recent records (by timestamp, descending).
    async fn get_recent(&self, limit: usize) -> Result<Vec<TokenRecord>, TokenTrackingError>;

    /// Delete all records whose timestamp is strictly before `before`.
    ///
    /// Returns the number of records removed.
    async fn cleanup_before(&self, before: DateTime<Utc>) -> Result<usize, TokenTrackingError>;
}

// ---------------------------------------------------------------------------
// InMemoryTokenTracker
// ---------------------------------------------------------------------------

/// Thread-safe in-memory implementation of [`TokenTracker`].
///
/// Stores records in a `Vec` behind a `RwLock`.  Suitable for unit tests,
/// integration tests, and development; not intended for production persistence.
#[derive(Clone)]
pub struct InMemoryTokenTracker {
    inner: Arc<RwLock<Vec<TokenRecord>>>,
}

impl Default for InMemoryTokenTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryTokenTracker {
    /// Create a new, empty tracker.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(Vec::new())),
        }
    }
}

#[async_trait]
impl TokenTracker for InMemoryTokenTracker {
    async fn record(&self, record: TokenRecord) -> Result<(), TokenTrackingError> {
        if record.filtered_tokens > record.input_tokens {
            return Err(TokenTrackingError::InvalidRecord(format!(
                "filtered_tokens ({}) cannot exceed input_tokens ({})",
                record.filtered_tokens, record.input_tokens
            )));
        }
        let mut records = self.inner.write().await;
        records.push(record);
        Ok(())
    }

    async fn get_summary(&self, filter: &TokenFilter) -> Result<TokenSummary, TokenTrackingError> {
        let records = self.inner.read().await;
        let matched: Vec<&TokenRecord> = records
            .iter()
            .filter(|r| matches_filter(r, filter))
            .collect();
        Ok(build_summary(matched.into_iter()))
    }

    async fn get_aggregated(
        &self,
        filter: &TokenFilter,
        dimension: AggregateDimension,
    ) -> Result<Vec<(String, TokenSummary)>, TokenTrackingError> {
        let records = self.inner.read().await;
        let mut groups: HashMap<String, Vec<&TokenRecord>> = HashMap::new();

        for record in records.iter().filter(|r| matches_filter(r, filter)) {
            let key = dimension_key(record, &dimension);
            groups.entry(key).or_default().push(record);
        }

        let mut result: Vec<(String, TokenSummary)> = groups
            .into_iter()
            .map(|(key, recs)| (key, build_summary(recs.into_iter())))
            .collect();

        // Stable sort by key for deterministic output
        result.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(result)
    }

    async fn get_recent(&self, limit: usize) -> Result<Vec<TokenRecord>, TokenTrackingError> {
        let records = self.inner.read().await;
        let mut sorted: Vec<TokenRecord> = records.iter().cloned().collect();
        sorted.sort_by_key(|b| std::cmp::Reverse(b.timestamp));
        sorted.truncate(limit);
        Ok(sorted)
    }

    async fn cleanup_before(&self, before: DateTime<Utc>) -> Result<usize, TokenTrackingError> {
        let mut records = self.inner.write().await;
        let initial_len = records.len();
        records.retain(|r| r.timestamp >= before);
        Ok(initial_len - records.len())
    }
}

// ---------------------------------------------------------------------------
// TimedExecution helper
// ---------------------------------------------------------------------------

/// A lightweight timer that captures execution start time and builds a
/// [`TokenRecord`] from raw text strings using whitespace tokenisation.
///
/// # Example
///
/// ```
/// use cyberclaw_observability::token_economics::TimedExecution;
///
/// let timer = TimedExecution::start();
/// // … perform work …
/// let record = timer.into_record("exec-001", "hello world", "hi", "hi");
/// assert_eq!(record.input_tokens, 2);
/// ```
pub struct TimedExecution {
    start: std::time::Instant,
}

impl TimedExecution {
    /// Begin timing an execution.
    pub fn start() -> Self {
        Self {
            start: std::time::Instant::now(),
        }
    }

    /// Return elapsed time since [`TimedExecution::start`] was called.
    pub fn elapsed_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    /// Consume the timer and build a [`TokenRecord`] from raw text strings.
    ///
    /// Token counts are approximated by splitting on whitespace.
    /// `savings_pct` is computed as `(1 - filtered/input) * 100`; when
    /// `input` is empty it defaults to `0.0`.
    pub fn into_record(
        self,
        execution_id: &str,
        input: &str,
        output: &str,
        filtered: &str,
    ) -> TokenRecord {
        let input_tokens = input.split_whitespace().count();
        let output_tokens = output.split_whitespace().count();
        let filtered_tokens = filtered.split_whitespace().count();
        let savings_pct = if input_tokens > 0 {
            (1.0 - filtered_tokens as f64 / input_tokens as f64) * 100.0
        } else {
            0.0
        };
        TokenRecord {
            execution_id: execution_id.to_string(),
            timestamp: Utc::now(),
            agent_id: None,
            connector_id: None,
            capability_id: None,
            input_tokens,
            output_tokens,
            filtered_tokens,
            savings_pct,
            duration_ms: self.elapsed_ms(),
            project_path: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Returns `true` when `record` satisfies every non-`None` field in `filter`.
fn matches_filter(record: &TokenRecord, filter: &TokenFilter) -> bool {
    if let Some(ref agent_id) = filter.agent_id {
        if record.agent_id.as_deref() != Some(agent_id.as_str()) {
            return false;
        }
    }
    if let Some(ref connector_id) = filter.connector_id {
        if record.connector_id.as_deref() != Some(connector_id.as_str()) {
            return false;
        }
    }
    if let Some(ref capability_id) = filter.capability_id {
        if record.capability_id.as_deref() != Some(capability_id.as_str()) {
            return false;
        }
    }
    if let Some(ref project_path) = filter.project_path {
        if record.project_path.as_deref() != Some(project_path.as_str()) {
            return false;
        }
    }
    if let Some(since) = filter.since {
        if record.timestamp < since {
            return false;
        }
    }
    if let Some(until) = filter.until {
        if record.timestamp > until {
            return false;
        }
    }
    true
}

/// Extract the grouping key for a record under the given dimension.
fn dimension_key(record: &TokenRecord, dimension: &AggregateDimension) -> String {
    match dimension {
        AggregateDimension::ByAgent => record
            .agent_id
            .clone()
            .unwrap_or_else(|| "<none>".to_string()),
        AggregateDimension::ByConnector => record
            .connector_id
            .clone()
            .unwrap_or_else(|| "<none>".to_string()),
        AggregateDimension::ByCapability => record
            .capability_id
            .clone()
            .unwrap_or_else(|| "<none>".to_string()),
        AggregateDimension::ByProject => record
            .project_path
            .clone()
            .unwrap_or_else(|| "<none>".to_string()),
        AggregateDimension::ByDay => record.timestamp.format("%Y-%m-%d").to_string(),
    }
}

/// Build a [`TokenSummary`] from an iterator of record references.
fn build_summary<'a>(records: impl Iterator<Item = &'a TokenRecord>) -> TokenSummary {
    let mut summary = TokenSummary::default();
    let mut savings_sum: f64 = 0.0;

    for record in records {
        summary.total_input += record.input_tokens;
        summary.total_output += record.output_tokens;
        summary.total_filtered += record.filtered_tokens;
        savings_sum += record.savings_pct;
        summary.execution_count += 1;

        let ts = record.timestamp;
        summary.period_start = Some(match summary.period_start {
            None => ts,
            Some(prev) => prev.min(ts),
        });
        summary.period_end = Some(match summary.period_end {
            None => ts,
            Some(prev) => prev.max(ts),
        });
    }

    summary.total_saved = summary.total_input.saturating_sub(summary.total_filtered);
    summary.avg_savings_pct = if summary.execution_count > 0 {
        savings_sum / summary.execution_count as f64
    } else {
        0.0
    };

    summary
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn make_record(
        execution_id: &str,
        input: usize,
        output: usize,
        filtered: usize,
    ) -> TokenRecord {
        let savings_pct = if input > 0 {
            (1.0 - filtered as f64 / input as f64) * 100.0
        } else {
            0.0
        };
        TokenRecord {
            execution_id: execution_id.to_string(),
            timestamp: Utc::now(),
            agent_id: None,
            connector_id: None,
            capability_id: None,
            input_tokens: input,
            output_tokens: output,
            filtered_tokens: filtered,
            savings_pct,
            duration_ms: 10,
            project_path: None,
        }
    }

    // -----------------------------------------------------------------------
    // test_record_and_retrieve
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_record_and_retrieve() {
        let tracker = InMemoryTokenTracker::new();
        tracker
            .record(make_record("exec-001", 100, 50, 80))
            .await
            .unwrap();
        tracker
            .record(make_record("exec-002", 200, 100, 150))
            .await
            .unwrap();

        let recent = tracker.get_recent(10).await.unwrap();
        assert_eq!(recent.len(), 2);
    }

    // -----------------------------------------------------------------------
    // test_summary_calculation
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_summary_calculation() {
        let tracker = InMemoryTokenTracker::new();
        tracker
            .record(make_record("exec-001", 100, 50, 80))
            .await
            .unwrap();
        tracker
            .record(make_record("exec-002", 200, 100, 150))
            .await
            .unwrap();

        let summary = tracker.get_summary(&TokenFilter::default()).await.unwrap();
        assert_eq!(summary.total_input, 300);
        assert_eq!(summary.total_output, 150);
        assert_eq!(summary.total_filtered, 230);
        assert_eq!(summary.total_saved, 70); // 300 - 230
        assert_eq!(summary.execution_count, 2);
        assert!(summary.avg_savings_pct > 0.0);
        assert!(summary.period_start.is_some());
        assert!(summary.period_end.is_some());
    }

    // -----------------------------------------------------------------------
    // test_aggregate_by_agent
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_aggregate_by_agent() {
        let tracker = InMemoryTokenTracker::new();

        let mut r1 = make_record("exec-001", 100, 50, 80);
        r1.agent_id = Some("agent-alpha".to_string());
        let mut r2 = make_record("exec-002", 200, 100, 150);
        r2.agent_id = Some("agent-beta".to_string());
        let mut r3 = make_record("exec-003", 50, 25, 40);
        r3.agent_id = Some("agent-alpha".to_string());

        tracker.record(r1).await.unwrap();
        tracker.record(r2).await.unwrap();
        tracker.record(r3).await.unwrap();

        let groups = tracker
            .get_aggregated(&TokenFilter::default(), AggregateDimension::ByAgent)
            .await
            .unwrap();

        assert_eq!(groups.len(), 2);
        let alpha = groups.iter().find(|(k, _)| k == "agent-alpha").unwrap();
        assert_eq!(alpha.1.execution_count, 2);
        assert_eq!(alpha.1.total_input, 150);

        let beta = groups.iter().find(|(k, _)| k == "agent-beta").unwrap();
        assert_eq!(beta.1.execution_count, 1);
        assert_eq!(beta.1.total_input, 200);
    }

    // -----------------------------------------------------------------------
    // test_aggregate_by_connector
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_aggregate_by_connector() {
        let tracker = InMemoryTokenTracker::new();

        let mut r1 = make_record("exec-001", 100, 50, 80);
        r1.connector_id = Some("connector-http".to_string());
        let mut r2 = make_record("exec-002", 200, 100, 180);
        r2.connector_id = Some("connector-grpc".to_string());

        tracker.record(r1).await.unwrap();
        tracker.record(r2).await.unwrap();

        let groups = tracker
            .get_aggregated(&TokenFilter::default(), AggregateDimension::ByConnector)
            .await
            .unwrap();

        assert_eq!(groups.len(), 2);
        let http_group = groups.iter().find(|(k, _)| k == "connector-http").unwrap();
        assert_eq!(http_group.1.total_input, 100);
    }

    // -----------------------------------------------------------------------
    // test_filter_by_time_range
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_filter_by_time_range() {
        let tracker = InMemoryTokenTracker::new();
        let now = Utc::now();

        let mut old = make_record("exec-old", 100, 50, 80);
        old.timestamp = now - Duration::hours(2);
        let mut recent = make_record("exec-recent", 200, 100, 150);
        recent.timestamp = now - Duration::minutes(5);

        tracker.record(old).await.unwrap();
        tracker.record(recent).await.unwrap();

        let filter = TokenFilter {
            since: Some(now - Duration::hours(1)),
            ..Default::default()
        };
        let summary = tracker.get_summary(&filter).await.unwrap();
        assert_eq!(summary.execution_count, 1);
        assert_eq!(summary.total_input, 200);
    }

    // -----------------------------------------------------------------------
    // test_filter_by_project
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_filter_by_project() {
        let tracker = InMemoryTokenTracker::new();

        let mut r1 = make_record("exec-001", 100, 50, 80);
        r1.project_path = Some("/projects/alpha".to_string());
        let mut r2 = make_record("exec-002", 200, 100, 150);
        r2.project_path = Some("/projects/beta".to_string());

        tracker.record(r1).await.unwrap();
        tracker.record(r2).await.unwrap();

        let filter = TokenFilter {
            project_path: Some("/projects/alpha".to_string()),
            ..Default::default()
        };
        let summary = tracker.get_summary(&filter).await.unwrap();
        assert_eq!(summary.execution_count, 1);
        assert_eq!(summary.total_input, 100);
    }

    // -----------------------------------------------------------------------
    // test_cleanup_old_records
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_cleanup_old_records() {
        let tracker = InMemoryTokenTracker::new();
        let now = Utc::now();

        let mut old1 = make_record("exec-old-1", 100, 50, 80);
        old1.timestamp = now - Duration::days(10);
        let mut old2 = make_record("exec-old-2", 200, 100, 150);
        old2.timestamp = now - Duration::days(5);
        let fresh = make_record("exec-fresh", 50, 25, 40);

        tracker.record(old1).await.unwrap();
        tracker.record(old2).await.unwrap();
        tracker.record(fresh).await.unwrap();

        let cutoff = now - Duration::days(3);
        let removed = tracker.cleanup_before(cutoff).await.unwrap();
        assert_eq!(removed, 2);

        let summary = tracker.get_summary(&TokenFilter::default()).await.unwrap();
        assert_eq!(summary.execution_count, 1);
    }

    // -----------------------------------------------------------------------
    // test_timed_execution
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_timed_execution() {
        let timer = TimedExecution::start();
        // Simulate minimal work
        tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;

        let record = timer.into_record("exec-timed", "hello world foo", "hello world", "hello");

        assert_eq!(record.execution_id, "exec-timed");
        assert_eq!(record.input_tokens, 3);
        assert_eq!(record.output_tokens, 2);
        assert_eq!(record.filtered_tokens, 1);
        assert!(record.duration_ms >= 1, "timer should record elapsed ms");
        assert!(record.savings_pct > 0.0);
    }

    // -----------------------------------------------------------------------
    // test_savings_calculation
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_savings_calculation() {
        // 100 input, 25 filtered => 75% saved
        let record = make_record("exec-savings", 100, 50, 25);
        assert!((record.savings_pct - 75.0).abs() < 1e-9);

        let tracker = InMemoryTokenTracker::new();
        tracker.record(record).await.unwrap();

        let summary = tracker.get_summary(&TokenFilter::default()).await.unwrap();
        assert_eq!(summary.total_saved, 75);
        assert!((summary.avg_savings_pct - 75.0).abs() < 1e-9);
    }

    // -----------------------------------------------------------------------
    // test_empty_input_savings
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_empty_input_savings() {
        // Zero input tokens must not panic; savings_pct should be 0.0
        let record = make_record("exec-empty", 0, 0, 0);
        assert_eq!(record.savings_pct, 0.0);

        let tracker = InMemoryTokenTracker::new();
        tracker.record(record).await.unwrap();

        let summary = tracker.get_summary(&TokenFilter::default()).await.unwrap();
        assert_eq!(summary.avg_savings_pct, 0.0);
        assert_eq!(summary.total_saved, 0);
    }

    // -----------------------------------------------------------------------
    // test_invalid_record_rejected
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_invalid_record_rejected() {
        let tracker = InMemoryTokenTracker::new();
        // filtered > input should be rejected
        let bad = make_record("exec-bad", 10, 5, 50); // filtered=50 > input=10 would be caught
                                                      // make_record computes savings_pct from the values, so we build one manually
        let bad_record = TokenRecord {
            execution_id: "exec-bad".to_string(),
            timestamp: Utc::now(),
            agent_id: None,
            connector_id: None,
            capability_id: None,
            input_tokens: 10,
            output_tokens: 5,
            filtered_tokens: 50, // invalid
            savings_pct: 0.0,
            duration_ms: 1,
            project_path: None,
        };
        let _ = bad; // suppress unused warning
        let result = tracker.record(bad_record).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TokenTrackingError::InvalidRecord(_)
        ));
    }

    // -----------------------------------------------------------------------
    // test_get_recent_ordering
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_get_recent_ordering() {
        let tracker = InMemoryTokenTracker::new();
        let now = Utc::now();

        for i in 0..5u64 {
            let mut r = make_record(&format!("exec-{:03}", i), 10, 5, 8);
            r.timestamp = now + Duration::seconds(i as i64);
            tracker.record(r).await.unwrap();
        }

        // get_recent(3) should return newest 3
        let recent = tracker.get_recent(3).await.unwrap();
        assert_eq!(recent.len(), 3);
        // First element is newest
        assert_eq!(recent[0].execution_id, "exec-004");
        assert_eq!(recent[1].execution_id, "exec-003");
        assert_eq!(recent[2].execution_id, "exec-002");
    }
}
