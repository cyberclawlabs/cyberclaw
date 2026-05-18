use cyberclaw_core::prelude::*;

#[derive(Debug, Clone)]
pub struct IngressRequest {
    pub actor: ActorRef,
    pub session: Option<SessionRef>,
    pub workspace: Option<WorkspaceRef>,
    pub task: Task,
}

pub trait GatewayRouter: Send + Sync {
    fn normalize(&self, request: IngressRequest) -> anyhow::Result<IngressRequest>;
}

/// InMemory implementation of GatewayRouter for development and testing
///
/// This implementation performs basic normalization:
/// - Validates actor reference
/// - Ensures task has proper metadata
/// - Applies default values where needed
#[derive(Debug, Clone, Default)]
pub struct InMemoryGatewayRouter;

impl InMemoryGatewayRouter {
    pub fn new() -> Self {
        Self
    }

    fn validate_actor(&self, actor: &ActorRef) -> anyhow::Result<()> {
        if actor.id.as_str().is_empty() {
            anyhow::bail!("actor id cannot be empty");
        }
        if actor.display_name.is_empty() {
            anyhow::bail!("actor display_name cannot be empty");
        }
        Ok(())
    }

    fn normalize_task(&self, mut task: Task, actor: &ActorRef) -> Task {
        // Ensure trigger is set
        if task.trigger.kind.is_empty() {
            task.trigger.kind = "ingress".to_string();
        }
        if task.trigger.source.is_empty() {
            task.trigger.source = "gateway".to_string();
        }

        // Ensure requested_by matches the actor if not set
        if task.requested_by.id.as_str().is_empty() {
            task.requested_by = actor.clone();
        }

        task
    }
}

impl GatewayRouter for InMemoryGatewayRouter {
    fn normalize(&self, mut request: IngressRequest) -> anyhow::Result<IngressRequest> {
        // Validate actor
        self.validate_actor(&request.actor)?;

        // Normalize task
        request.task = self.normalize_task(request.task, &request.actor);

        Ok(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use cyberclaw_core::identity::ActorType;
    use cyberclaw_core::ids::ActorId;

    fn create_test_actor(id: &str, display_name: &str) -> ActorRef {
        ActorRef {
            id: ActorId::from_string(id.to_string()).unwrap(),
            actor_type: ActorType::Human,
            tenant_id: None,
            home_node_id: None,
            display_name: display_name.to_string(),
        }
    }

    #[test]
    fn test_normalize_valid_request() {
        let router = InMemoryGatewayRouter::new();

        let actor = create_test_actor("alice", "Alice");
        let request = IngressRequest {
            actor: actor.clone(),
            session: None,
            workspace: None,
            task: Task {
                id: TaskId::new(),
                case_id: None,
                title: "Test Task".to_string(),
                summary: "Test".to_string(),
                kind: TaskKind::Analysis,
                priority: Priority::Medium,
                requested_by: actor,
                requested_at: Utc::now(),
                trigger: TriggerRef {
                    kind: "manual".to_string(),
                    source: "web-ui".to_string(),
                },
                input: TaskInput::default(),
                desired_outputs: vec![],
                labels: vec![],
                preferred_agent_id: None,
            },
        };

        let result = router.normalize(request);
        assert!(result.is_ok());
    }

    // Note: Empty actor ID validation is now handled at the ID type level
    // (ActorId::from_string rejects empty strings), so no need for gateway-level test

    #[test]
    fn test_normalize_sets_default_trigger() {
        let router = InMemoryGatewayRouter::new();

        let actor = create_test_actor("bob", "Bob");
        let request = IngressRequest {
            actor: actor.clone(),
            session: None,
            workspace: None,
            task: Task {
                id: TaskId::new(),
                case_id: None,
                title: "Test Task".to_string(),
                summary: "Test".to_string(),
                kind: TaskKind::Execution,
                priority: Priority::High,
                requested_by: actor,
                requested_at: Utc::now(),
                trigger: TriggerRef {
                    kind: "".to_string(),
                    source: "".to_string(),
                },
                input: TaskInput::default(),
                desired_outputs: vec![],
                labels: vec![],
                preferred_agent_id: None,
            },
        };

        let result = router.normalize(request).unwrap();
        assert_eq!(result.task.trigger.kind, "ingress");
        assert_eq!(result.task.trigger.source, "gateway");
    }

    // Note: The test for empty actor ID replacement has been removed because
    // ActorId validation now prevents empty IDs at construction time.
    // The normalize_task logic for empty requested_by IDs is now defensive code
    // that should never be triggered in practice.
}
