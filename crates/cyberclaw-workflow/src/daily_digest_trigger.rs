//! Daily-digest Cron trigger registration (Sprint 9 L9).
//!
//! This module does **two** things and nothing else:
//!
//! 1. `register_daily_digest_cron(registry, agent_id)` — registers a
//!    [`WorkflowTrigger::Cron`] expression `"10 0 * * *"` (UTC) that the
//!    workflow dispatcher will later match when a [`TriggerEvent::CronFired`]
//!    event with the same expression arrives.
//! 2. `DailyDigestFanout` — a thin convenience that, given a
//!    `TriggerEvent::CronFired` matching the canonical expression, calls a
//!    user-supplied async closure per registered agent so the caller can
//!    invoke the `DefaultDailyDigestCoordinator` without this crate taking
//!    a dependency on `cyberclaw-control-plane`.
//!
//! This crate must **not** depend on `cyberclaw-control-plane` — that would
//! form a cycle (`control-plane → workflow`). Hence the closure-based fanout.

use std::sync::Arc;

use cyberclaw_core::ids::AgentId;
use tracing::warn;

use crate::trigger::WorkflowTrigger;
use crate::trigger::{TriggerError, TriggerEvent, TriggerRegistration, TriggerRegistry};

/// Canonical daily-digest cron expression (UTC 00:10 every day).
pub const DAILY_DIGEST_CRON: &str = "10 0 * * *";

/// Register a daily-digest cron trigger for `agent_id`.
///
/// Returns the generated trigger ID so callers can later unregister or disable
/// it (e.g. when an agent is removed). The workflow_id is
/// `daily-digest:<agent_id>` so multiple agents coexist without conflict.
///
/// Failure to register (e.g. lock contention inside `TriggerRegistry`) surfaces
/// as `TriggerError` and the caller decides whether to retry. Per SKILL.md
/// failure mode "Collect 阶段查询超时", we never block other agents' digests.
pub async fn register_daily_digest_cron(
    registry: &TriggerRegistry,
    agent_id: &AgentId,
) -> Result<String, TriggerError> {
    let workflow_id = format!("daily-digest:{}", agent_id.as_str());
    registry
        .register(
            &workflow_id,
            WorkflowTrigger::Cron {
                expression: DAILY_DIGEST_CRON.to_string(),
                timezone: Some("UTC".to_string()),
            },
        )
        .await
}

/// Given a cron event and the full trigger registry, yield the set of
/// daily-digest workflow IDs that matched.
///
/// The caller is expected to have already constructed the
/// `TriggerEvent::CronFired` (e.g. from a scheduler tick). This function is
/// infallible: it catches internal lookup errors and logs them, returning an
/// empty slice so the scheduler proceeds with the next agent.
pub async fn matched_daily_digest_workflows(
    registry: &TriggerRegistry,
    event: &TriggerEvent,
) -> Vec<String> {
    let matches = registry.find_matching_triggers(event).await;
    matches
        .into_iter()
        .filter_map(|t: TriggerRegistration| {
            if t.workflow_id.starts_with("daily-digest:") {
                Some(t.workflow_id)
            } else {
                None
            }
        })
        .collect()
}

/// Extract the agent-id tail from a workflow id produced by
/// [`register_daily_digest_cron`]. Returns `None` if the prefix doesn't
/// match (so the fanout below silently skips unrelated workflows).
pub fn agent_id_from_workflow_id(workflow_id: &str) -> Option<AgentId> {
    let tail = workflow_id.strip_prefix("daily-digest:")?;
    AgentId::from_string(tail.to_string()).ok()
}

/// Trait that the caller (e.g. server boot) implements to actually run a
/// digest for one agent. Fail-open: one agent's failure is logged and the
/// next agent still runs.
#[async_trait::async_trait]
pub trait DailyDigestRunner: Send + Sync {
    async fn run_for_agent(&self, agent_id: AgentId) -> Result<(), String>;
}

/// Dispatch a `CronFired` event to every registered daily-digest workflow
/// by extracting the agent id and handing it to `runner`.
///
/// # Error handling
///
/// Per SKILL.md: "失败时记 trace warning，不阻塞下一次调度". Individual agent
/// failures are logged and the loop continues.
pub async fn fire_daily_digest(
    registry: &TriggerRegistry,
    event: &TriggerEvent,
    runner: Arc<dyn DailyDigestRunner>,
) {
    let workflows = matched_daily_digest_workflows(registry, event).await;
    for wf_id in workflows {
        let Some(agent_id) = agent_id_from_workflow_id(&wf_id) else {
            continue;
        };
        if let Err(err) = runner.run_for_agent(agent_id.clone()).await {
            warn!(
                agent = agent_id.as_str(),
                workflow = %wf_id,
                %err,
                "daily digest run failed; continuing to next agent"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[tokio::test]
    async fn register_and_match_cron() {
        let registry = TriggerRegistry::new();
        let agent = AgentId::from_string("zeta".into()).unwrap();
        let tid = register_daily_digest_cron(&registry, &agent).await.unwrap();
        assert!(!tid.is_empty());

        let event = TriggerEvent::CronFired {
            expression: DAILY_DIGEST_CRON.to_string(),
        };
        let matched = matched_daily_digest_workflows(&registry, &event).await;
        assert_eq!(matched, vec!["daily-digest:zeta".to_string()]);
    }

    #[tokio::test]
    async fn different_cron_does_not_match() {
        let registry = TriggerRegistry::new();
        let agent = AgentId::from_string("zeta".into()).unwrap();
        register_daily_digest_cron(&registry, &agent).await.unwrap();
        let event = TriggerEvent::CronFired {
            expression: "0 12 * * *".to_string(),
        };
        assert!(matched_daily_digest_workflows(&registry, &event)
            .await
            .is_empty());
    }

    #[test]
    fn agent_id_extracted_from_workflow_id() {
        assert_eq!(
            agent_id_from_workflow_id("daily-digest:abc")
                .unwrap()
                .as_str(),
            "abc"
        );
        assert!(agent_id_from_workflow_id("other:abc").is_none());
    }

    struct RecordingRunner {
        calls: Arc<Mutex<Vec<String>>>,
        fail_for: Option<String>,
    }

    #[async_trait::async_trait]
    impl DailyDigestRunner for RecordingRunner {
        async fn run_for_agent(&self, agent_id: AgentId) -> Result<(), String> {
            self.calls
                .lock()
                .unwrap()
                .push(agent_id.as_str().to_string());
            if self.fail_for.as_deref() == Some(agent_id.as_str()) {
                return Err("boom".into());
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn fanout_calls_runner_per_agent_and_survives_failures() {
        let registry = TriggerRegistry::new();
        for name in ["alpha", "beta", "gamma"] {
            let agent = AgentId::from_string(name.into()).unwrap();
            register_daily_digest_cron(&registry, &agent).await.unwrap();
        }

        let calls = Arc::new(Mutex::new(Vec::new()));
        let runner: Arc<dyn DailyDigestRunner> = Arc::new(RecordingRunner {
            calls: calls.clone(),
            fail_for: Some("beta".into()),
        });
        let event = TriggerEvent::CronFired {
            expression: DAILY_DIGEST_CRON.to_string(),
        };

        fire_daily_digest(&registry, &event, runner).await;
        let mut seen = calls.lock().unwrap().clone();
        seen.sort();
        assert_eq!(seen, vec!["alpha", "beta", "gamma"]);
    }
}
