//! OSV vulnerability scan Cron trigger registration (Sprint 10 L4).
//!
//! This module does two things:
//!
//! 1. `register_osv_scan_cron(registry, agent_id)` — registers a
//!    [`WorkflowTrigger::Cron`] expression `"0 6 * * *"` (UTC 06:00 daily)
//!    that the workflow dispatcher will match when a [`TriggerEvent::CronFired`]
//!    event with the same expression arrives.
//! 2. `OsvScanRunner` / `DefaultOsvScanRunner` — injection-point trait + thin
//!    default implementation that scans one or more lockfiles by calling the
//!    `connector:osv:check` capability and aggregates results into an
//!    [`OsvScanOutcome`].
//!
//! This crate must **not** depend on `cyberclaw-control-plane` — that would
//! form a dependency cycle.  The runner is therefore injected by the caller.

use std::path::PathBuf;
use std::sync::Arc;

use cyberclaw_core::ids::{AgentId, ArtifactId};
use tracing::warn;

use crate::trigger::{
    TriggerError, TriggerEvent, TriggerRegistration, TriggerRegistry, WorkflowTrigger,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Cron expression for the daily OSV scan (UTC 06:00).
pub const OSV_SCAN_CRON: &str = "0 6 * * *";

/// Workflow-id prefix used by all OSV scan registrations.
const OSV_SCAN_PREFIX: &str = "osv-scan:";

// ---------------------------------------------------------------------------
// Registration helper
// ---------------------------------------------------------------------------

/// Register a daily OSV-scan cron trigger for `agent_id`.
///
/// The workflow_id is `osv-scan:<agent_id>` so multiple agents coexist
/// without conflict.  Returns the generated trigger ID.
pub async fn register_osv_scan_cron(
    registry: &TriggerRegistry,
    agent_id: &AgentId,
) -> Result<String, TriggerError> {
    let workflow_id = format!("{}{}", OSV_SCAN_PREFIX, agent_id.as_str());
    registry
        .register(
            &workflow_id,
            WorkflowTrigger::Cron {
                expression: OSV_SCAN_CRON.to_string(),
                timezone: Some("UTC".to_string()),
            },
        )
        .await
}

// ---------------------------------------------------------------------------
// Registry lookup helpers
// ---------------------------------------------------------------------------

/// Return all OSV-scan workflow IDs that match the given cron event.
pub async fn matched_osv_scan_workflows(
    registry: &TriggerRegistry,
    event: &TriggerEvent,
) -> Vec<String> {
    registry
        .find_matching_triggers(event)
        .await
        .into_iter()
        .filter_map(|t: TriggerRegistration| {
            if t.workflow_id.starts_with(OSV_SCAN_PREFIX) {
                Some(t.workflow_id)
            } else {
                None
            }
        })
        .collect()
}

/// Extract the `AgentId` from a workflow_id produced by
/// [`register_osv_scan_cron`].  Returns `None` if the prefix doesn't match.
pub fn agent_id_from_osv_workflow_id(workflow_id: &str) -> Option<AgentId> {
    let tail = workflow_id.strip_prefix(OSV_SCAN_PREFIX)?;
    AgentId::from_string(tail.to_string()).ok()
}

// ---------------------------------------------------------------------------
// Runner trait and outcome types
// ---------------------------------------------------------------------------

/// Error returned by [`OsvScanRunner::scan_and_alert`].
#[derive(Debug, thiserror::Error)]
pub enum OsvScanError {
    #[error("scan failed: {0}")]
    ScanFailed(String),
    #[error("no lockfiles provided")]
    NoLockfiles,
}

/// Summary produced after scanning one or more lockfiles.
#[derive(Debug, Clone)]
pub struct OsvScanOutcome {
    /// Agent that owns this scan.
    pub agent_id: AgentId,
    /// Number of lockfiles that were scanned.
    pub scanned_files: u32,
    /// Total vulnerability entries found across all lockfiles.
    pub vulnerabilities_found: u32,
    /// Subset of `vulnerabilities_found` with severity == CRITICAL.
    pub critical_count: u32,
    /// ID of the JSON artifact produced (None if no vulnerabilities and no
    /// artifact was created, or if the runner chose not to persist).
    pub artifact_id: Option<ArtifactId>,
}

/// Injection-point trait for executing an OSV scan.
///
/// Callers supply a concrete implementation (e.g. `DefaultOsvScanRunner` or a
/// test double) so this crate stays free of `cyberclaw-control-plane`.
#[async_trait::async_trait]
pub trait OsvScanRunner: Send + Sync {
    /// Scan `lockfile_paths`, aggregate results, and produce an
    /// [`OsvScanOutcome`].  Implementations should produce an Alert Artifact
    /// when vulnerabilities are found.
    async fn scan_and_alert(
        &self,
        agent_id: &AgentId,
        lockfile_paths: Vec<PathBuf>,
    ) -> Result<OsvScanOutcome, OsvScanError>;
}

// ---------------------------------------------------------------------------
// DefaultOsvScanRunner
// ---------------------------------------------------------------------------

/// Boxed future returned by the per-lockfile dispatcher closure.
type DispatchFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<(u32, u32), String>> + Send>>;

/// Default runner: iterates lockfiles, parses each one, calls the
/// `connector:osv:check` capability (via the injected dispatcher), and
/// aggregates results into an [`OsvScanOutcome`].
///
/// The dispatcher function has signature:
/// ```text
/// async fn(lockfile_path: PathBuf) -> Result<(u32 vulns, u32 critical), String>
/// ```
/// This keeps the runner independent of the concrete connector types.
pub struct DefaultOsvScanRunner {
    /// Called once per lockfile.  Returns `(vulnerabilities_found, critical_count)`.
    dispatcher: Arc<dyn Fn(PathBuf) -> DispatchFuture + Send + Sync>,
}

impl DefaultOsvScanRunner {
    /// Construct with a custom per-lockfile dispatcher.
    pub fn new<F, Fut>(dispatcher: F) -> Self
    where
        F: Fn(PathBuf) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<(u32, u32), String>> + Send + 'static,
    {
        Self {
            dispatcher: Arc::new(move |p| -> DispatchFuture { Box::pin(dispatcher(p)) }),
        }
    }
}

#[async_trait::async_trait]
impl OsvScanRunner for DefaultOsvScanRunner {
    async fn scan_and_alert(
        &self,
        agent_id: &AgentId,
        lockfile_paths: Vec<PathBuf>,
    ) -> Result<OsvScanOutcome, OsvScanError> {
        if lockfile_paths.is_empty() {
            return Err(OsvScanError::NoLockfiles);
        }

        let mut total_vulns: u32 = 0;
        let mut total_critical: u32 = 0;
        let mut scanned: u32 = 0;

        for path in lockfile_paths {
            match (self.dispatcher)(path.clone()).await {
                Ok((vulns, critical)) => {
                    total_vulns += vulns;
                    total_critical += critical;
                    scanned += 1;
                }
                Err(e) => {
                    warn!(
                        agent = agent_id.as_str(),
                        lockfile = %path.display(),
                        error = %e,
                        "OSV scan failed for lockfile; continuing"
                    );
                    // Count the file as scanned even on soft errors
                    scanned += 1;
                }
            }
        }

        // Produce a synthetic ArtifactId when vulnerabilities were found so the
        // caller can retrieve or persist the full report.  The caller is
        // responsible for writing the actual artifact bytes; we only mint the ID.
        let artifact_id = if total_vulns > 0 {
            Some(ArtifactId::new())
        } else {
            None
        };

        Ok(OsvScanOutcome {
            agent_id: agent_id.clone(),
            scanned_files: scanned,
            vulnerabilities_found: total_vulns,
            critical_count: total_critical,
            artifact_id,
        })
    }
}

// ---------------------------------------------------------------------------
// Fanout dispatcher
// ---------------------------------------------------------------------------

/// Dispatch a `CronFired` event to every registered OSV-scan workflow by
/// extracting the agent id and handing it to `runner`.
///
/// Per project convention: individual agent failures are logged; the loop
/// continues for the next agent.
pub async fn fire_osv_scan_fanout(
    registry: &TriggerRegistry,
    event: &TriggerEvent,
    lockfile_paths: Vec<PathBuf>,
    runner: Arc<dyn OsvScanRunner>,
) {
    let workflows = matched_osv_scan_workflows(registry, event).await;
    for wf_id in workflows {
        let Some(agent_id) = agent_id_from_osv_workflow_id(&wf_id) else {
            continue;
        };
        match runner
            .scan_and_alert(&agent_id, lockfile_paths.clone())
            .await
        {
            Ok(outcome) => {
                if outcome.vulnerabilities_found > 0 {
                    warn!(
                        agent = agent_id.as_str(),
                        vulns = outcome.vulnerabilities_found,
                        critical = outcome.critical_count,
                        artifact = ?outcome.artifact_id,
                        "OSV scan found vulnerabilities"
                    );
                }
            }
            Err(err) => {
                warn!(
                    agent = agent_id.as_str(),
                    workflow = %wf_id,
                    %err,
                    "OSV scan run failed; continuing to next agent"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // -----------------------------------------------------------------------
    // A. Cron expression is the expected daily value
    // -----------------------------------------------------------------------

    #[test]
    fn osv_scan_trigger_cron_expression_is_daily() {
        // "0 6 * * *" = minute 0, hour 6, every day
        assert_eq!(OSV_SCAN_CRON, "0 6 * * *");
    }

    // -----------------------------------------------------------------------
    // B. Empty lockfile list → NoLockfiles error (no-op)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn osv_scan_runner_empty_lockfile_list_no_op() {
        let runner = DefaultOsvScanRunner::new(|_path| async {
            Ok((1u32, 0u32)) // should never be called
        });
        let agent = AgentId::from_string("test-agent".into()).unwrap();
        let result = runner.scan_and_alert(&agent, vec![]).await;
        assert!(
            matches!(result, Err(OsvScanError::NoLockfiles)),
            "expected NoLockfiles, got {result:?}"
        );
    }

    // -----------------------------------------------------------------------
    // C. Runner with vulnerabilities produces an artifact id
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn osv_scan_runner_with_vulnerabilities_produces_artifact() {
        // Dispatcher returns 3 vulns (1 critical) for any lockfile
        let runner = DefaultOsvScanRunner::new(|_path| async { Ok((3u32, 1u32)) });
        let agent = AgentId::from_string("vuln-agent".into()).unwrap();
        let paths = vec![
            PathBuf::from("/repo/Cargo.lock"),
            PathBuf::from("/repo/package-lock.json"),
        ];
        let outcome = runner.scan_and_alert(&agent, paths).await.unwrap();

        assert_eq!(outcome.scanned_files, 2);
        assert_eq!(outcome.vulnerabilities_found, 6); // 3 per file × 2
        assert_eq!(outcome.critical_count, 2); // 1 per file × 2
        assert!(
            outcome.artifact_id.is_some(),
            "expected artifact_id when vulns found"
        );
        assert_eq!(outcome.agent_id.as_str(), "vuln-agent");
    }

    // -----------------------------------------------------------------------
    // D. Dispatcher offline / error → graceful: scanned count still increments,
    //    no artifact because vuln count stays zero
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn osv_scan_runner_offline_fails_gracefully() {
        let runner =
            DefaultOsvScanRunner::new(|_path| async { Err("connection refused".to_string()) });
        let agent = AgentId::from_string("offline-agent".into()).unwrap();
        let paths = vec![PathBuf::from("/repo/Cargo.lock")];
        let outcome = runner.scan_and_alert(&agent, paths).await.unwrap();

        assert_eq!(outcome.scanned_files, 1, "file counted even on soft error");
        assert_eq!(outcome.vulnerabilities_found, 0);
        assert_eq!(outcome.critical_count, 0);
        assert!(
            outcome.artifact_id.is_none(),
            "no artifact when no vulns found"
        );
    }

    // -----------------------------------------------------------------------
    // E. Fanout filters correctly by workflow id prefix
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn fire_osv_scan_fanout_filters_by_prefix() {
        let registry = TriggerRegistry::new();

        // Register two OSV-scan agents
        for name in ["alpha", "beta"] {
            let agent = AgentId::from_string(name.into()).unwrap();
            register_osv_scan_cron(&registry, &agent).await.unwrap();
        }

        // Register a daily-digest trigger that should NOT be picked up
        registry
            .register(
                "daily-digest:gamma",
                WorkflowTrigger::Cron {
                    expression: OSV_SCAN_CRON.to_string(),
                    timezone: Some("UTC".to_string()),
                },
            )
            .await
            .unwrap();

        let calls: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let calls_clone = calls.clone();

        struct RecordingRunner {
            calls: Arc<Mutex<Vec<String>>>,
        }

        #[async_trait::async_trait]
        impl OsvScanRunner for RecordingRunner {
            async fn scan_and_alert(
                &self,
                agent_id: &AgentId,
                _lockfile_paths: Vec<PathBuf>,
            ) -> Result<OsvScanOutcome, OsvScanError> {
                self.calls
                    .lock()
                    .unwrap()
                    .push(agent_id.as_str().to_string());
                Ok(OsvScanOutcome {
                    agent_id: agent_id.clone(),
                    scanned_files: 0,
                    vulnerabilities_found: 0,
                    critical_count: 0,
                    artifact_id: None,
                })
            }
        }

        let runner: Arc<dyn OsvScanRunner> = Arc::new(RecordingRunner { calls: calls_clone });

        let event = TriggerEvent::CronFired {
            expression: OSV_SCAN_CRON.to_string(),
        };

        fire_osv_scan_fanout(&registry, &event, vec![], runner).await;

        let mut seen = calls.lock().unwrap().clone();
        seen.sort();
        // Only "alpha" and "beta" — "gamma" (daily-digest prefix) must be excluded
        assert_eq!(seen, vec!["alpha", "beta"]);
    }

    // -----------------------------------------------------------------------
    // F. register_and_match round-trip
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn osv_scan_register_and_match() {
        let registry = TriggerRegistry::new();
        let agent = AgentId::from_string("scan-agent".into()).unwrap();
        let tid = register_osv_scan_cron(&registry, &agent).await.unwrap();
        assert!(!tid.is_empty());

        let event = TriggerEvent::CronFired {
            expression: OSV_SCAN_CRON.to_string(),
        };
        let matched = matched_osv_scan_workflows(&registry, &event).await;
        assert_eq!(matched, vec!["osv-scan:scan-agent"]);
    }

    // -----------------------------------------------------------------------
    // G. Different cron expression does not match
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn osv_scan_different_cron_does_not_match() {
        let registry = TriggerRegistry::new();
        let agent = AgentId::from_string("agent-x".into()).unwrap();
        register_osv_scan_cron(&registry, &agent).await.unwrap();

        let event = TriggerEvent::CronFired {
            expression: "10 0 * * *".to_string(), // daily-digest time
        };
        assert!(matched_osv_scan_workflows(&registry, &event)
            .await
            .is_empty());
    }

    // -----------------------------------------------------------------------
    // H. agent_id_from_osv_workflow_id helper
    // -----------------------------------------------------------------------

    #[test]
    fn agent_id_extracted_from_osv_workflow_id() {
        assert_eq!(
            agent_id_from_osv_workflow_id("osv-scan:my-agent")
                .unwrap()
                .as_str(),
            "my-agent"
        );
        assert!(agent_id_from_osv_workflow_id("daily-digest:other").is_none());
        assert!(agent_id_from_osv_workflow_id("unrelated").is_none());
    }
}
