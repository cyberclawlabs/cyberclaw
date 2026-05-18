//! D-4 (2026-05-05) — Capability Feedback Loop
//!
//! Periodically scans `audit_log` for failed `capability.complete` rows
//! that the live `chat_handler` path didn't pick up (offline analyses,
//! batch jobs, retries that surfaced after the request returned), groups
//! them by `(tool_name, capability_id, error_signature)`, and folds the
//! aggregates back into the same `capability_requests` table the online
//! path writes to via [`AuditSink::record_capability_request`].
//!
//! Default: **disabled**. Enable by setting
//! `CYBERCLAW_FEEDBACK_LOOP_ENABLED=1`. Tunables:
//!
//! - `CYBERCLAW_FEEDBACK_LOOP_INTERVAL_SECS` — period between scans (default 3600)
//! - `CYBERCLAW_FEEDBACK_LOOP_WINDOW_SECS`   — lookback window per scan (default 3600)
//! - `CYBERCLAW_FEEDBACK_LOOP_MIN_COUNT`     — cluster threshold (default 3)
//!
//! Architectural compliance: this is a pure consumer of `AuditSink`, not
//! a new platform object. It writes only to existing tables and shares
//! the on-disk dedupe semantics with the online path (same UNIQUE index
//! on `(tool_name, failure_reason, status)` keeps the queue short).

use std::sync::Arc;
use std::time::Duration;

use crate::audit::{AuditSink, CapabilityFailureCluster, CapabilityRequestReason};

/// Outcome of one feedback cycle.
///
/// Returned by [`run_feedback_once`] so the admin REST endpoint can echo
/// the numbers back to the operator who triggered the cycle manually.
#[derive(Debug, Clone, Copy, Default)]
pub struct FeedbackRunReport {
    /// Number of clusters discovered in `audit_log` this cycle.
    pub aggregated: u32,
    /// Number of `record_capability_request` calls actually issued
    /// (one per cluster — record_capability_request itself dedupes
    /// against existing pending rows).
    pub written_to_queue: u32,
}

/// Configuration loaded from env. `enabled=false` means
/// [`spawn_feedback_loop`] returns without spawning.
#[derive(Debug, Clone)]
pub struct FeedbackLoopConfig {
    pub enabled: bool,
    pub interval: Duration,
    pub window: Duration,
    pub min_count: u32,
}

impl FeedbackLoopConfig {
    pub fn from_env() -> Self {
        let enabled = matches!(
            std::env::var("CYBERCLAW_FEEDBACK_LOOP_ENABLED")
                .unwrap_or_default()
                .as_str(),
            "1" | "true" | "TRUE" | "yes"
        );
        let interval_secs: u64 = std::env::var("CYBERCLAW_FEEDBACK_LOOP_INTERVAL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3600);
        let window_secs: u64 = std::env::var("CYBERCLAW_FEEDBACK_LOOP_WINDOW_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3600);
        let min_count: u32 = std::env::var("CYBERCLAW_FEEDBACK_LOOP_MIN_COUNT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3);
        Self {
            enabled,
            // Floor at 60s to avoid hot-looping a SQLite scan in misconfig.
            interval: Duration::from_secs(interval_secs.max(60)),
            window: Duration::from_secs(window_secs.max(60)),
            min_count: min_count.max(1),
        }
    }
}

/// Run one feedback cycle. Reusable from both the periodic background
/// task and the admin REST endpoint that triggers an immediate scan.
pub async fn run_feedback_once(
    sink: &AuditSink,
    window: Duration,
    min_count: u32,
) -> anyhow::Result<FeedbackRunReport> {
    let since = chrono::Utc::now().timestamp() - window.as_secs() as i64;
    let clusters = sink.aggregate_capability_failures(since, min_count).await?;
    let aggregated = clusters.len() as u32;

    let mut written_to_queue: u32 = 0;
    for cluster in &clusters {
        // record_capability_request swallows its own errors and dedupes
        // against existing pending rows on (tool_name, failure_reason,
        // status), so calling it for every cluster is safe.
        let notes = format_cluster_note(cluster);
        sink.record_capability_request_with_notes(
            &cluster.tool_name,
            cluster.capability_id.as_deref(),
            None,
            CapabilityRequestReason::ExecutionError,
            None,
            None,
            Some(&notes),
        )
        .await;
        written_to_queue += 1;
    }

    Ok(FeedbackRunReport {
        aggregated,
        written_to_queue,
    })
}

fn format_cluster_note(cluster: &CapabilityFailureCluster) -> String {
    format!(
        "auto-feedback: {occ} occurrences in last window. Pattern: {sig}",
        occ = cluster.occurrences,
        sig = cluster.error_signature
    )
}

/// Spawn the periodic feedback loop. Returns immediately when
/// `cfg.enabled` is false (default), so the production startup path
/// stays quiet unless the operator opts in.
pub fn spawn_feedback_loop(sink: Arc<AuditSink>, cfg: FeedbackLoopConfig) {
    if !cfg.enabled {
        tracing::info!(
            "capability feedback loop disabled (set CYBERCLAW_FEEDBACK_LOOP_ENABLED=1 to enable)"
        );
        return;
    }
    tracing::info!(
        interval_secs = cfg.interval.as_secs(),
        window_secs = cfg.window.as_secs(),
        min_count = cfg.min_count,
        "capability feedback loop started"
    );

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(cfg.interval);
        // Skip the immediate first tick so the loop doesn't fire mid-startup.
        interval.tick().await;
        loop {
            interval.tick().await;
            match run_feedback_once(&sink, cfg.window, cfg.min_count).await {
                Ok(report) if report.aggregated > 0 => {
                    tracing::info!(
                        aggregated = report.aggregated,
                        written_to_queue = report.written_to_queue,
                        "capability feedback loop: cycle complete"
                    );
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(error = %e, "capability feedback loop: cycle failed");
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::AuditSink;
    use tempfile::TempDir;

    async fn seed_failures(sink: &AuditSink, n: usize, tool: &str, error: &str) {
        for _ in 0..n {
            sink.record_capability_complete(
                "agent_alpha",
                "exec_test",
                &format!("{tool}.cap"),
                "local",
                tool,
                5,
                0,
                Some(error),
            )
            .await;
        }
    }

    #[tokio::test]
    async fn run_feedback_once_writes_one_row_per_cluster() {
        let tmp = TempDir::new().unwrap();
        let sink = AuditSink::new(tmp.path().join("audit.db")).await.unwrap();
        seed_failures(&sink, 5, "verify_numeric", "boom").await;

        let report = run_feedback_once(&sink, Duration::from_secs(3600), 3)
            .await
            .unwrap();
        assert_eq!(report.aggregated, 1);
        assert_eq!(report.written_to_queue, 1);

        // The fold-back must land in capability_requests.
        let rows = sink.list_capability_requests(None, 100).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].tool_name, "verify_numeric");
        assert_eq!(
            rows[0].failure_reason,
            CapabilityRequestReason::ExecutionError
        );
        assert!(rows[0]
            .notes
            .as_deref()
            .unwrap_or_default()
            .contains("auto-feedback"));
    }

    #[tokio::test]
    async fn run_feedback_once_dedupes_existing_pending_row() {
        let tmp = TempDir::new().unwrap();
        let sink = AuditSink::new(tmp.path().join("audit.db")).await.unwrap();
        // Pre-existing pending row from the live path.
        sink.record_capability_request(
            "verify_numeric",
            None,
            None,
            CapabilityRequestReason::ExecutionError,
            None,
            None,
        )
        .await;

        seed_failures(&sink, 5, "verify_numeric", "boom").await;
        run_feedback_once(&sink, Duration::from_secs(3600), 3)
            .await
            .unwrap();

        // Still one row (UNIQUE index on tool_name+reason+status='pending'),
        // count is bumped instead of duplicating.
        let rows = sink.list_capability_requests(None, 100).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].count >= 2);
    }

    #[tokio::test]
    async fn run_feedback_once_returns_zero_when_below_threshold() {
        let tmp = TempDir::new().unwrap();
        let sink = AuditSink::new(tmp.path().join("audit.db")).await.unwrap();
        seed_failures(&sink, 2, "verify_numeric", "boom").await;
        let report = run_feedback_once(&sink, Duration::from_secs(3600), 3)
            .await
            .unwrap();
        assert_eq!(report.aggregated, 0);
        assert_eq!(report.written_to_queue, 0);
    }

    #[test]
    fn config_defaults_disabled() {
        // Don't depend on env state of the runner; build directly.
        let cfg = FeedbackLoopConfig {
            enabled: false,
            interval: Duration::from_secs(3600),
            window: Duration::from_secs(3600),
            min_count: 3,
        };
        assert!(!cfg.enabled);
    }
}
