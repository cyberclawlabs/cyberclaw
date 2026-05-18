use cyberclaw_core::prelude::*;
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone)]
pub struct AutomationJob {
    pub id: String,
    pub task: Task,
    pub cron: String,
}

pub trait AutomationCoordinator: Send + Sync {
    fn register(&self, job: AutomationJob) -> anyhow::Result<()>;
    fn trigger(&self, id: &str) -> anyhow::Result<()>;
    fn list_jobs(&self) -> anyhow::Result<Vec<AutomationJob>>;
    fn unregister(&self, id: &str) -> anyhow::Result<()>;
}

/// InMemory implementation of AutomationCoordinator for development and testing
#[derive(Debug, Clone, Default)]
pub struct InMemoryAutomationCoordinator {
    jobs: Arc<RwLock<BTreeMap<String, AutomationJob>>>,
}

impl InMemoryAutomationCoordinator {
    pub fn new() -> Self {
        Self {
            jobs: Arc::new(RwLock::new(BTreeMap::new())),
        }
    }
}

impl AutomationCoordinator for InMemoryAutomationCoordinator {
    fn register(&self, job: AutomationJob) -> anyhow::Result<()> {
        let mut jobs = self.jobs.blocking_write();
        jobs.insert(job.id.clone(), job);
        Ok(())
    }

    fn trigger(&self, id: &str) -> anyhow::Result<()> {
        let jobs = self.jobs.blocking_read();
        jobs.get(id)
            .ok_or_else(|| anyhow::anyhow!("automation job not found: {}", id))?;
        // In a real implementation, this would trigger the actual execution
        Ok(())
    }

    fn list_jobs(&self) -> anyhow::Result<Vec<AutomationJob>> {
        let jobs = self.jobs.blocking_read();
        Ok(jobs.values().cloned().collect())
    }

    fn unregister(&self, id: &str) -> anyhow::Result<()> {
        let mut jobs = self.jobs.blocking_write();
        jobs.remove(id)
            .ok_or_else(|| anyhow::anyhow!("automation job not found: {}", id))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use cyberclaw_core::identity::ActorType;
    use cyberclaw_core::ids::ActorId;

    fn create_test_actor(id: &str, display_name: &str, actor_type: ActorType) -> ActorRef {
        ActorRef {
            id: ActorId::from_string(id.to_string()).unwrap(),
            actor_type,
            tenant_id: None,
            home_node_id: None,
            display_name: display_name.to_string(),
        }
    }

    #[test]
    fn test_register_and_list_jobs() {
        let coordinator = InMemoryAutomationCoordinator::new();

        let job = AutomationJob {
            id: "daily-security-scan".to_string(),
            task: Task {
                id: TaskId::new(),
                case_id: None,
                title: "Daily Security Scan".to_string(),
                summary: "Automated security scan".to_string(),
                kind: TaskKind::Automation,
                priority: Priority::Medium,
                requested_by: create_test_actor("automation", "Automation", ActorType::System),
                requested_at: Utc::now(),
                trigger: TriggerRef {
                    kind: "cron".to_string(),
                    source: "automation-coordinator".to_string(),
                },
                input: TaskInput::default(),
                desired_outputs: vec![],
                labels: vec!["security".to_string(), "automation".to_string()],
                preferred_agent_id: None,
            },
            cron: "0 0 * * *".to_string(), // Daily at midnight
        };

        coordinator.register(job.clone()).unwrap();
        let jobs = coordinator.list_jobs().unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].id, "daily-security-scan");
    }

    #[test]
    fn test_trigger_job() {
        let coordinator = InMemoryAutomationCoordinator::new();

        let job = AutomationJob {
            id: "test-job".to_string(),
            task: Task {
                id: TaskId::new(),
                case_id: None,
                title: "Test Job".to_string(),
                summary: "Test".to_string(),
                kind: TaskKind::Automation,
                priority: Priority::Low,
                requested_by: create_test_actor("test", "Test", ActorType::System),
                requested_at: Utc::now(),
                trigger: TriggerRef {
                    kind: "manual".to_string(),
                    source: "test".to_string(),
                },
                input: TaskInput::default(),
                desired_outputs: vec![],
                labels: vec![],
                preferred_agent_id: None,
            },
            cron: "*/5 * * * *".to_string(),
        };

        coordinator.register(job).unwrap();
        let result = coordinator.trigger("test-job");
        assert!(result.is_ok());
    }

    #[test]
    fn test_trigger_nonexistent_job_fails() {
        let coordinator = InMemoryAutomationCoordinator::new();
        let result = coordinator.trigger("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_unregister_job() {
        let coordinator = InMemoryAutomationCoordinator::new();

        let job = AutomationJob {
            id: "temp-job".to_string(),
            task: Task {
                id: TaskId::new(),
                case_id: None,
                title: "Temp Job".to_string(),
                summary: "Temporary".to_string(),
                kind: TaskKind::Automation,
                priority: Priority::Low,
                requested_by: create_test_actor("test", "Test", ActorType::System),
                requested_at: Utc::now(),
                trigger: TriggerRef {
                    kind: "manual".to_string(),
                    source: "test".to_string(),
                },
                input: TaskInput::default(),
                desired_outputs: vec![],
                labels: vec![],
                preferred_agent_id: None,
            },
            cron: "0 0 * * *".to_string(),
        };

        coordinator.register(job).unwrap();
        coordinator.unregister("temp-job").unwrap();

        let jobs = coordinator.list_jobs().unwrap();
        assert_eq!(jobs.len(), 0);
    }
}
