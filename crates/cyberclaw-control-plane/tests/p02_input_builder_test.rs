// P0-2 Input Builder Integration Test
// This test verifies that PlannedAction.input contains real parameters, not empty objects

use cyberclaw_control_plane::registry::InMemoryRegistry;
use cyberclaw_control_plane::resolver::{InMemoryResolver, Resolver};
use cyberclaw_control_plane::types::ResolutionInput;
use cyberclaw_control_plane::{PackageRecord, PackageSource, Registry, RegistryState};
use cyberclaw_core::capability::{CapabilityEffect, RiskLevel};
use cyberclaw_core::cluster::CapabilityPlacement;
use cyberclaw_core::enums::Priority;
use cyberclaw_core::identity::{ActorRef, ActorType};
use cyberclaw_core::ids::{ActorId, AgentId, CapabilityId, ConnectorId, TaskId};
use cyberclaw_core::manifests::{
    Artifacts, CapabilityContract, Compatibility, ConnectorSpec, Dependencies, PackageKind,
    PackageManifest, PackageSpec,
};
use cyberclaw_core::task::{Task, TaskInput, TaskKind, TriggerRef};
use serde_json::json;
use std::sync::Arc;

/// Build a test local connector manifest with all local capabilities registered.
/// Uses apiVersion "cyberclaw.io/v2" (required by ManifestLoader) and Low risk for
/// test capabilities to bypass review approval workflow.
fn build_test_local_connector_manifest() -> PackageManifest {
    PackageManifest {
        api_version: "cyberclaw.io/v2".to_string(),
        kind: PackageKind::Connector,
        id: "local".to_string(),
        version: "0.1.0".to_string(),
        name: "LocalConnector".to_string(),
        display_name: Some("Local File System and Command Connector".to_string()),
        summary: "Provides file system, search, and command execution capabilities".to_string(),
        owner: Some("cyberclaw".to_string()),
        license: None,
        homepage: None,
        repository: None,
        tags: vec![
            "filesystem".to_string(),
            "search".to_string(),
            "command".to_string(),
        ],
        compatibility: Compatibility {
            platform: "cyberclaw".to_string(),
            runtime: vec!["native".to_string()],
            os: vec!["linux".to_string(), "macos".to_string()],
            arch: vec!["x86_64".to_string()],
        },
        dependencies: Dependencies::default(),
        artifacts: Artifacts::default(),
        config_schema: None,
        spec: PackageSpec::Connector(ConnectorSpec {
            subtype: "filesystem".to_string(),
            runtime: cyberclaw_core::manifests::ConnectorRuntime::Native,
            auth: None,
            capabilities: vec![
                CapabilityContract {
                    id: "fs.read".to_string(),
                    title: "Read File".to_string(),
                    description: Some("Read contents of a file".to_string()),
                    input_schema: "FsReadInput".to_string(),
                    output_schema: "FsReadOutput".to_string(),
                    risk: RiskLevel::Low,
                    effects: vec![CapabilityEffect::Read],
                    placement: Some(CapabilityPlacement {
                        allowed_node_labels: vec!["worker".to_string()],
                        required_runtime: vec!["native".to_string()],
                        requires_local_secret: false,
                        network_zone: None,
                    }),
                    timeouts: Default::default(),
                },
                CapabilityContract {
                    id: "fs.write".to_string(),
                    title: "Write File".to_string(),
                    description: Some("Write contents to a file".to_string()),
                    input_schema: "FsWriteInput".to_string(),
                    output_schema: "FsWriteOutput".to_string(),
                    risk: RiskLevel::Low,
                    effects: vec![CapabilityEffect::Write],
                    placement: Some(CapabilityPlacement {
                        allowed_node_labels: vec!["worker".to_string()],
                        required_runtime: vec!["native".to_string()],
                        requires_local_secret: false,
                        network_zone: None,
                    }),
                    timeouts: Default::default(),
                },
                CapabilityContract {
                    id: "search.grep".to_string(),
                    title: "Search with Grep".to_string(),
                    description: Some("Search for pattern in files".to_string()),
                    input_schema: "SearchGrepInput".to_string(),
                    output_schema: "SearchGrepOutput".to_string(),
                    risk: RiskLevel::Low,
                    effects: vec![CapabilityEffect::Read],
                    placement: Some(CapabilityPlacement {
                        allowed_node_labels: vec!["worker".to_string()],
                        required_runtime: vec!["native".to_string()],
                        requires_local_secret: false,
                        network_zone: None,
                    }),
                    timeouts: Default::default(),
                },
                CapabilityContract {
                    id: "cmd.exec".to_string(),
                    title: "Execute Command".to_string(),
                    description: Some("Execute a shell command".to_string()),
                    input_schema: "CmdExecInput".to_string(),
                    output_schema: "CmdExecOutput".to_string(),
                    // Use Low risk in tests to bypass review approval workflow
                    risk: RiskLevel::Low,
                    effects: vec![CapabilityEffect::Execute],
                    placement: Some(CapabilityPlacement {
                        allowed_node_labels: vec!["worker".to_string()],
                        required_runtime: vec!["native".to_string()],
                        requires_local_secret: false,
                        network_zone: None,
                    }),
                    timeouts: Default::default(),
                },
            ],
        }),
    }
}

/// Build an InMemoryRegistry pre-populated with the local connector manifest.
/// This allows resolver.plan() to validate capabilities against registry records.
async fn build_registry_with_local_connector() -> Arc<InMemoryRegistry> {
    let registry = Arc::new(InMemoryRegistry::new());

    let manifest = build_test_local_connector_manifest();
    let record = PackageRecord {
        id: "local".to_string(),
        kind: PackageKind::Connector,
        latest_version: "0.1.0".to_string(),
        installed_versions: vec!["0.1.0".to_string()],
        active_version: Some("0.1.0".to_string()),
        source: PackageSource::LocalPath("/test/local-connector".to_string()),
        state: RegistryState::Active,
        available_nodes: vec![],
        runtime_requirements: vec!["native".to_string()],
        manifest,
    };

    registry.upsert(record).await.unwrap();
    registry
}

#[tokio::test]
async fn test_p02_action_input_has_required_fields() {
    // Create registry with local connector so capability validation succeeds
    let registry = build_registry_with_local_connector().await;
    let resolver = InMemoryResolver::new(registry);

    // Test 1: fs.read with explicit params
    let task = Task {
        id: TaskId::from_string("test-fs-read".to_string()).unwrap(),
        case_id: None,
        title: "Read config file".to_string(),
        summary: "Read the configuration file".to_string(),
        kind: TaskKind::Execution,
        priority: Priority::Medium,
        requested_by: ActorRef {
            id: ActorId::from_string("tester".to_string()).unwrap(),
            actor_type: ActorType::Human,
            tenant_id: None,
            home_node_id: None,
            display_name: "Tester".to_string(),
        },
        requested_at: chrono::Utc::now(),
        trigger: TriggerRef {
            kind: "manual".to_string(),
            source: "test".to_string(),
        },
        input: TaskInput {
            payload: json!({
                "capability": "fs.read",
                "params": {
                    "path": "config.yaml"
                }
            }),
        },
        desired_outputs: vec![],
        labels: vec![],
        preferred_agent_id: None,
    };

    let resolution_input = ResolutionInput {
        task,
        case: None,
        actor: ActorRef {
            id: ActorId::from_string("tester".to_string()).unwrap(),
            actor_type: ActorType::Human,
            tenant_id: None,
            home_node_id: None,
            display_name: "Tester".to_string(),
        },
        workspace: None,
        session: None,
        available_agents: vec![AgentId::from_string("test-agent".to_string()).unwrap()],
        available_skills: vec![],
        available_connectors: vec![ConnectorId::from_string("local".to_string()).unwrap()],
        available_capabilities: vec![CapabilityId::from_string("fs.read".to_string()).unwrap()],
        available_workflows: vec![],
    };

    let plan = resolver.plan(resolution_input).await.unwrap();
    assert_eq!(plan.actions.len(), 1, "fs.read should generate one action");
    assert_eq!(plan.actions[0].capability.as_str(), "fs.read");
    assert_eq!(
        plan.actions[0].input["path"], "config.yaml",
        "fs.read input should contain the path parameter"
    );
    assert!(
        !plan.actions[0].input.as_object().unwrap().is_empty(),
        "fs.read input should not be empty"
    );

    // Test 2: cmd.exec with command and timeout
    let task = Task {
        id: TaskId::from_string("test-cmd-exec".to_string()).unwrap(),
        case_id: None,
        title: "Run build".to_string(),
        summary: "Build the project".to_string(),
        kind: TaskKind::Execution,
        priority: Priority::Medium,
        requested_by: ActorRef {
            id: ActorId::from_string("tester".to_string()).unwrap(),
            actor_type: ActorType::Human,
            tenant_id: None,
            home_node_id: None,
            display_name: "Tester".to_string(),
        },
        requested_at: chrono::Utc::now(),
        trigger: TriggerRef {
            kind: "manual".to_string(),
            source: "test".to_string(),
        },
        input: TaskInput {
            payload: json!({
                "capability": "cmd.exec",
                "params": {
                    "command": "cargo build",
                    "timeout_ms": 30000
                }
            }),
        },
        desired_outputs: vec![],
        labels: vec![],
        preferred_agent_id: None,
    };

    let resolution_input = ResolutionInput {
        task,
        case: None,
        actor: ActorRef {
            id: ActorId::from_string("tester".to_string()).unwrap(),
            actor_type: ActorType::Human,
            tenant_id: None,
            home_node_id: None,
            display_name: "Tester".to_string(),
        },
        workspace: None,
        session: None,
        available_agents: vec![AgentId::from_string("test-agent".to_string()).unwrap()],
        available_skills: vec![],
        available_connectors: vec![ConnectorId::from_string("local".to_string()).unwrap()],
        available_capabilities: vec![CapabilityId::from_string("cmd.exec".to_string()).unwrap()],
        available_workflows: vec![],
    };

    let plan = resolver.plan(resolution_input).await.unwrap();
    assert_eq!(plan.actions.len(), 1, "cmd.exec should generate one action");
    assert_eq!(plan.actions[0].capability.as_str(), "cmd.exec");
    assert_eq!(
        plan.actions[0].input["command"], "cargo build",
        "cmd.exec input should contain the command parameter"
    );
    assert_eq!(
        plan.actions[0].input["timeout_ms"], 30000,
        "cmd.exec input should contain timeout_ms parameter"
    );

    // Test 3: search.grep with pattern
    let task = Task {
        id: TaskId::from_string("test-search-grep".to_string()).unwrap(),
        case_id: None,
        title: "Search code".to_string(),
        summary: "Search for patterns".to_string(),
        kind: TaskKind::Execution,
        priority: Priority::Medium,
        requested_by: ActorRef {
            id: ActorId::from_string("tester".to_string()).unwrap(),
            actor_type: ActorType::Human,
            tenant_id: None,
            home_node_id: None,
            display_name: "Tester".to_string(),
        },
        requested_at: chrono::Utc::now(),
        trigger: TriggerRef {
            kind: "manual".to_string(),
            source: "test".to_string(),
        },
        input: TaskInput {
            payload: json!({
                "capability": "search.grep",
                "params": {
                    "pattern": "TODO",
                    "path": "src"
                }
            }),
        },
        desired_outputs: vec![],
        labels: vec![],
        preferred_agent_id: None,
    };

    let resolution_input = ResolutionInput {
        task,
        case: None,
        actor: ActorRef {
            id: ActorId::from_string("tester".to_string()).unwrap(),
            actor_type: ActorType::Human,
            tenant_id: None,
            home_node_id: None,
            display_name: "Tester".to_string(),
        },
        workspace: None,
        session: None,
        available_agents: vec![AgentId::from_string("test-agent".to_string()).unwrap()],
        available_skills: vec![],
        available_connectors: vec![ConnectorId::from_string("local".to_string()).unwrap()],
        available_capabilities: vec![CapabilityId::from_string("search.grep".to_string()).unwrap()],
        available_workflows: vec![],
    };

    let plan = resolver.plan(resolution_input).await.unwrap();
    assert_eq!(
        plan.actions.len(),
        1,
        "search.grep should generate one action"
    );
    assert_eq!(plan.actions[0].capability.as_str(), "search.grep");
    assert_eq!(
        plan.actions[0].input["pattern"], "TODO",
        "search.grep input should contain the pattern parameter"
    );
    assert_eq!(
        plan.actions[0].input["path"], "src",
        "search.grep input should contain the path parameter"
    );

    println!("P0-2: All action inputs contain real parameters - VERIFIED");
}
