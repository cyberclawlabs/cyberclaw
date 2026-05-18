// Agent 6: Resolver Autopilot Integration Tests
//
// Tests for TaskKind::Autopilot detection and AutopilotJob creation

use cyberclaw_control_plane::registry::InMemoryRegistry;
use cyberclaw_control_plane::resolver::{InMemoryResolver, Resolver};
use cyberclaw_control_plane::types::ResolutionInput;
use cyberclaw_core::enums::Priority;
use cyberclaw_core::identity::{ActorRef, ActorType};
use cyberclaw_core::ids::{ActorId, AgentId, TaskId};
use cyberclaw_core::task::{Task, TaskInput, TaskKind, TriggerRef};
use std::sync::Arc;

fn create_actor_ref() -> ActorRef {
    ActorRef {
        id: ActorId::from_string("tester".to_string()).unwrap(),
        actor_type: ActorType::Human,
        tenant_id: None,
        home_node_id: None,
        display_name: "Tester".to_string(),
    }
}

#[tokio::test]
async fn test_autopilot_detection_via_task_kind() {
    let task = Task {
        id: TaskId::from_string("autopilot-001".to_string()).unwrap(),
        case_id: None,
        title: "Autopilot Task".to_string(),
        summary: "Detected via TaskKind::Autopilot".to_string(),
        kind: TaskKind::Autopilot,
        priority: Priority::High,
        requested_by: create_actor_ref(),
        requested_at: chrono::Utc::now(),
        trigger: TriggerRef {
            kind: "manual".to_string(),
            source: "test".to_string(),
        },
        input: TaskInput {
            payload: serde_json::json!({
                "goal": "Implement feature X"
            }),
        },
        desired_outputs: vec![],
        labels: vec![],
        preferred_agent_id: None,
    };

    let registry = Arc::new(InMemoryRegistry::new());
    let resolver = InMemoryResolver::new(registry);

    let input = ResolutionInput {
        task,
        case: None,
        actor: create_actor_ref(),
        workspace: None,
        session: None,
        available_agents: vec![AgentId::from_string("test-agent".to_string()).unwrap()],
        available_skills: vec![],
        available_connectors: vec![],
        available_capabilities: vec![],
        available_workflows: vec![],
    };

    let result = resolver.resolve_extended(input).await;
    assert!(
        result.is_ok(),
        "resolve_extended failed: {:?}",
        result.err()
    );

    let extended = result.unwrap();
    assert!(extended.autopilot_job.is_some(), "Expected autopilot_job");

    let job = extended.autopilot_job.unwrap();
    assert_eq!(job.goal, "Implement feature X");
    assert_eq!(job.max_iterations, 50); // default
    assert!(job.job_id.starts_with("autopilot-"));
}

#[tokio::test]
async fn test_autopilot_detection_via_payload_flag() {
    let task = Task {
        id: TaskId::from_string("autopilot-002".to_string()).unwrap(),
        case_id: None,
        title: "Regular Task".to_string(),
        summary: "Detected via payload autopilot:true".to_string(),
        kind: TaskKind::Execution,
        priority: Priority::Medium,
        requested_by: create_actor_ref(),
        requested_at: chrono::Utc::now(),
        trigger: TriggerRef {
            kind: "manual".to_string(),
            source: "test".to_string(),
        },
        input: TaskInput {
            payload: serde_json::json!({
                "autopilot": true,
                "goal": "Refactor module Y",
                "max_iterations": 30
            }),
        },
        desired_outputs: vec![],
        labels: vec![],
        preferred_agent_id: None,
    };

    let registry = Arc::new(InMemoryRegistry::new());
    let resolver = InMemoryResolver::new(registry);

    let input = ResolutionInput {
        task,
        case: None,
        actor: create_actor_ref(),
        workspace: None,
        session: None,
        available_agents: vec![AgentId::from_string("test-agent".to_string()).unwrap()],
        available_skills: vec![],
        available_connectors: vec![],
        available_capabilities: vec![],
        available_workflows: vec![],
    };

    let result = resolver.resolve_extended(input).await;
    assert!(result.is_ok());

    let extended = result.unwrap();
    assert!(extended.autopilot_job.is_some());

    let job = extended.autopilot_job.unwrap();
    assert_eq!(job.goal, "Refactor module Y");
    assert_eq!(job.max_iterations, 30);
}

#[tokio::test]
async fn test_autopilot_detection_via_mode() {
    let task = Task {
        id: TaskId::from_string("autopilot-003".to_string()).unwrap(),
        case_id: None,
        title: "Regular Task".to_string(),
        summary: "Detected via mode:autopilot".to_string(),
        kind: TaskKind::Execution,
        priority: Priority::Medium,
        requested_by: create_actor_ref(),
        requested_at: chrono::Utc::now(),
        trigger: TriggerRef {
            kind: "manual".to_string(),
            source: "test".to_string(),
        },
        input: TaskInput {
            payload: serde_json::json!({
                "mode": "autopilot",
                "goal": "Debug issue Z"
            }),
        },
        desired_outputs: vec![],
        labels: vec![],
        preferred_agent_id: None,
    };

    let registry = Arc::new(InMemoryRegistry::new());
    let resolver = InMemoryResolver::new(registry);

    let input = ResolutionInput {
        task,
        case: None,
        actor: create_actor_ref(),
        workspace: None,
        session: None,
        available_agents: vec![AgentId::from_string("test-agent".to_string()).unwrap()],
        available_skills: vec![],
        available_connectors: vec![],
        available_capabilities: vec![],
        available_workflows: vec![],
    };

    let result = resolver.resolve_extended(input).await;
    assert!(result.is_ok());

    let extended = result.unwrap();
    assert!(extended.autopilot_job.is_some());
}

#[tokio::test]
async fn test_non_autopilot_task() {
    let task = Task {
        id: TaskId::from_string("regular-001".to_string()).unwrap(),
        case_id: None,
        title: "Regular Task".to_string(),
        summary: "Not an autopilot task".to_string(),
        kind: TaskKind::Analysis,
        priority: Priority::Low,
        requested_by: create_actor_ref(),
        requested_at: chrono::Utc::now(),
        trigger: TriggerRef {
            kind: "manual".to_string(),
            source: "test".to_string(),
        },
        input: TaskInput::default(),
        desired_outputs: vec![],
        labels: vec![],
        preferred_agent_id: None,
    };

    let registry = Arc::new(InMemoryRegistry::new());
    let resolver = InMemoryResolver::new(registry);

    let input = ResolutionInput {
        task,
        case: None,
        actor: create_actor_ref(),
        workspace: None,
        session: None,
        available_agents: vec![AgentId::from_string("test-agent".to_string()).unwrap()],
        available_skills: vec![],
        available_connectors: vec![],
        available_capabilities: vec![],
        available_workflows: vec![],
    };

    let result = resolver.resolve_extended(input).await;
    assert!(result.is_ok());

    let extended = result.unwrap();
    assert!(
        extended.autopilot_job.is_none(),
        "Should not have autopilot_job"
    );
}

#[tokio::test]
async fn test_autopilot_job_creation_with_review_gates() {
    let task = Task {
        id: TaskId::from_string("autopilot-004".to_string()).unwrap(),
        case_id: None,
        title: "Autopilot with Review Gates".to_string(),
        summary: "Test review_gates extraction".to_string(),
        kind: TaskKind::Autopilot,
        priority: Priority::High,
        requested_by: create_actor_ref(),
        requested_at: chrono::Utc::now(),
        trigger: TriggerRef {
            kind: "manual".to_string(),
            source: "test".to_string(),
        },
        input: TaskInput {
            payload: serde_json::json!({
                "goal": "Build secure API",
                "max_iterations": 40,
                "review_gates": ["security", "architecture", "testing"]
            }),
        },
        desired_outputs: vec![],
        labels: vec![],
        preferred_agent_id: None,
    };

    let registry = Arc::new(InMemoryRegistry::new());
    let resolver = InMemoryResolver::new(registry);

    let input = ResolutionInput {
        task,
        case: None,
        actor: create_actor_ref(),
        workspace: None,
        session: None,
        available_agents: vec![AgentId::from_string("test-agent".to_string()).unwrap()],
        available_skills: vec![],
        available_connectors: vec![],
        available_capabilities: vec![],
        available_workflows: vec![],
    };

    let result = resolver.resolve_extended(input).await;
    assert!(result.is_ok());

    let extended = result.unwrap();
    let job = extended.autopilot_job.unwrap();

    assert_eq!(
        job.review_gates,
        vec!["security", "architecture", "testing"]
    );
}

#[tokio::test]
async fn test_autopilot_job_missing_goal_error() {
    let task = Task {
        id: TaskId::from_string("autopilot-005".to_string()).unwrap(),
        case_id: None,
        title: "Autopilot No Goal".to_string(),
        summary: "Should fail without goal".to_string(),
        kind: TaskKind::Autopilot,
        priority: Priority::High,
        requested_by: create_actor_ref(),
        requested_at: chrono::Utc::now(),
        trigger: TriggerRef {
            kind: "manual".to_string(),
            source: "test".to_string(),
        },
        input: TaskInput {
            payload: serde_json::json!({
                "max_iterations": 20
                // Missing "goal"
            }),
        },
        desired_outputs: vec![],
        labels: vec![],
        preferred_agent_id: None,
    };

    let registry = Arc::new(InMemoryRegistry::new());
    let resolver = InMemoryResolver::new(registry);

    let input = ResolutionInput {
        task,
        case: None,
        actor: create_actor_ref(),
        workspace: None,
        session: None,
        available_agents: vec![AgentId::from_string("test-agent".to_string()).unwrap()],
        available_skills: vec![],
        available_connectors: vec![],
        available_capabilities: vec![],
        available_workflows: vec![],
    };

    let result = resolver.resolve_extended(input).await;
    assert!(result.is_err(), "Should fail with missing goal");
    assert!(result.unwrap_err().to_string().contains("missing 'goal'"));
}

#[tokio::test]
async fn test_autopilot_job_defaults() {
    let task = Task {
        id: TaskId::from_string("autopilot-006".to_string()).unwrap(),
        case_id: None,
        title: "Autopilot Minimal".to_string(),
        summary: "Only goal provided".to_string(),
        kind: TaskKind::Autopilot,
        priority: Priority::Medium,
        requested_by: create_actor_ref(),
        requested_at: chrono::Utc::now(),
        trigger: TriggerRef {
            kind: "manual".to_string(),
            source: "test".to_string(),
        },
        input: TaskInput {
            payload: serde_json::json!({
                "goal": "Minimal task"
            }),
        },
        desired_outputs: vec![],
        labels: vec![],
        preferred_agent_id: None,
    };

    let registry = Arc::new(InMemoryRegistry::new());
    let resolver = InMemoryResolver::new(registry);

    let input = ResolutionInput {
        task,
        case: None,
        actor: create_actor_ref(),
        workspace: None,
        session: None,
        available_agents: vec![AgentId::from_string("test-agent".to_string()).unwrap()],
        available_skills: vec![],
        available_connectors: vec![],
        available_capabilities: vec![],
        available_workflows: vec![],
    };

    let result = resolver.resolve_extended(input).await;
    assert!(result.is_ok());

    let extended = result.unwrap();
    let job = extended.autopilot_job.unwrap();

    assert_eq!(job.max_iterations, 50); // default
    assert!(job.review_gates.is_empty()); // default
}
