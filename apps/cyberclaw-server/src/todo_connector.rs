//! Todo Connector — wraps the pre-existing `execute_todo_read` /
//! `execute_todo_write` helpers in `cyberclaw_agent_runtime::
//! builtin_tools_todo` with a real `Connector` registration so the LLM
//! `todo_read` / `todo_write` tool calls actually dispatch.
//!
//! Sprint 19 W3 — same pattern as `MemoryConnector`: the facade
//! declared `connector_id = "todo"` (was "internal" before this sprint
//! re-pointed it) and `capability_id = "agent:todo:{read,write}"`,
//! but no connector with that id had ever been registered.
//!
//! # Capabilities exposed
//!
//! | Capability ID        | Risk    | Effects | Description |
//! |----------------------|---------|---------|-------------|
//! | `agent:todo:read`    | Low     | Read    | Return all todos for `agent_id`. |
//! | `agent:todo:write`   | Medium  | Write   | Add / update / remove a todo by `action`. |
//!
//! # Architecture
//!
//! Storage is delegated entirely to
//! `cyberclaw_agent_runtime::builtin_tools_todo::execute_todo_*` which
//! reads/writes a JSON file under `~/.cyberclaw/agents/<agent_id>/
//! todos.json` (the path resolution lives in agent-runtime). The
//! connector is a thin shim — no extra state, no extra threading.

use async_trait::async_trait;
use cyberclaw_agent_runtime::builtin_tools_todo::{execute_todo_read, execute_todo_write};
use cyberclaw_connectors::types::{
    CapabilityExecutionRequest, CapabilityExecutionResult, Connector, ExecutionStatus,
};
use cyberclaw_core::capability::{CapabilityEffect, RiskLevel};
use cyberclaw_core::facade::{CapabilityFacade, FacadeExposure, ToolsetCategory};
use cyberclaw_core::ids::{CapabilityId, ConnectorId};
use cyberclaw_core::manifests::{CapabilityContract, CapabilityTimeouts, ConnectorRuntime};

/// Connector that exposes `agent:todo:read` / `agent:todo:write`
/// capabilities backed by the agent-runtime todo helpers.
#[derive(Clone)]
pub struct TodoConnector {
    id: ConnectorId,
}

impl std::fmt::Debug for TodoConnector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TodoConnector")
            .field("id", &self.id)
            .finish()
    }
}

impl Default for TodoConnector {
    fn default() -> Self {
        Self::new()
    }
}

impl TodoConnector {
    pub fn new() -> Self {
        Self {
            id: ConnectorId::from_string("todo".to_string())
                .expect("'todo' is a valid connector id"),
        }
    }
}

#[async_trait]
impl Connector for TodoConnector {
    fn id(&self) -> &ConnectorId {
        &self.id
    }

    fn runtime(&self) -> ConnectorRuntime {
        ConnectorRuntime::Native
    }

    fn capabilities(&self) -> Vec<CapabilityContract> {
        vec![
            CapabilityContract {
                id: "agent:todo:read".to_string(),
                title: "Read agent todo list".to_string(),
                description: Some(
                    "Return all todo items for the given agent_id.".to_string(),
                ),
                input_schema: r#"{"agent_id":"string"}"#.to_string(),
                output_schema: r#"{"todos":[{"id":"string","title":"string","status":"string","created_at":"rfc3339","updated_at":"rfc3339"}]}"#.to_string(),
                risk: RiskLevel::Low,
                effects: vec![CapabilityEffect::Read],
                placement: None,
                timeouts: CapabilityTimeouts::default(),
            },
            CapabilityContract {
                id: "agent:todo:write".to_string(),
                title: "Mutate agent todo list".to_string(),
                description: Some(
                    "Add, update, or remove a todo item by `action` (add|update|remove)."
                        .to_string(),
                ),
                input_schema: r#"{"agent_id":"string","action":"add|update|remove","id":"string?","title":"string?","status":"pending|in_progress|completed?"}"#.to_string(),
                output_schema: r#"{"id":"string","action":"string","todos":[...]}"#.to_string(),
                risk: RiskLevel::Medium,
                effects: vec![CapabilityEffect::Write],
                placement: None,
                timeouts: CapabilityTimeouts::default(),
            },
        ]
    }

    async fn execute(
        &self,
        request: CapabilityExecutionRequest,
    ) -> anyhow::Result<CapabilityExecutionResult> {
        let cap_id = request.capability_id.as_ref().to_string();
        let output = match cap_id.as_str() {
            "agent:todo:read" => execute_todo_read(&request.input),
            "agent:todo:write" => execute_todo_write(&request.input),
            other => {
                return Ok(CapabilityExecutionResult {
                    execution_id: request.execution_id,
                    trace_id: request.trace_id,
                    connector_id: request.connector_id,
                    capability_id: request.capability_id,
                    output: serde_json::json!({
                        "error": format!("todo connector: unknown capability '{}'", other),
                    }),
                    status: ExecutionStatus::Failed,
                    error: Some(format!("unknown capability '{}'", other)),
                    actual_runtime: None,
                });
            }
        };

        // The inner helpers report errors as `{"error": "..."}` JSON
        // rather than Result::Err, so reflect that into status.
        let (status, error) = if let Some(err) = output.get("error").and_then(|v| v.as_str()) {
            (ExecutionStatus::Failed, Some(err.to_string()))
        } else {
            (ExecutionStatus::Success, None)
        };

        Ok(CapabilityExecutionResult {
            execution_id: request.execution_id,
            trace_id: request.trace_id,
            connector_id: request.connector_id,
            capability_id: request.capability_id,
            output,
            status,
            error,
            actual_runtime: None,
        })
    }
}

/// §4 — authoritative facade declarations for `todo.*` capabilities.
///
/// `BuiltinToolRegistry::default_facades` MUST NOT duplicate these entries;
/// the host binary registers them via `capability_facades()`.
#[allow(dead_code)]
pub fn capability_facades() -> Vec<(CapabilityFacade, ToolsetCategory)> {
    let connector_id = ConnectorId::from_string("todo".to_string()).unwrap();
    vec![
        (
            CapabilityFacade {
                name: "todo_read".to_string(),
                description: "Read the current todo list for the agent. Returns all pending, \
                    in-progress, and completed items."
                    .to_string(),
                connector_id: connector_id.clone(),
                capability_id: CapabilityId::from_string("agent:todo:read".to_string()).unwrap(),
                risk_level: RiskLevel::Low,
                effects: vec!["read".to_string()],
                read_only: true,
                destructive: false,
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "agent_id": {
                            "type": "string",
                            "description": "Agent identifier whose todos to read (defaults to current agent)"
                        }
                    },
                    "required": []
                })),
                exposure: FacadeExposure::LlmDefault,
                workspace_root: None,
            },
            ToolsetCategory::SkillManagement,
        ),
        (
            CapabilityFacade {
                name: "todo_write".to_string(),
                description: "Add, update, or remove a todo item. Use to track tasks, \
                    sub-goals, and progress checkpoints during long-running work."
                    .to_string(),
                connector_id: connector_id.clone(),
                capability_id: CapabilityId::from_string("agent:todo:write".to_string()).unwrap(),
                risk_level: RiskLevel::Low,
                effects: vec!["read".to_string()],
                read_only: true,
                destructive: false,
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "agent_id": {
                            "type": "string",
                            "description": "Agent identifier (defaults to current agent)"
                        },
                        "action": {
                            "type": "string",
                            "enum": ["add", "update", "remove"],
                            "description": "Operation: add a new item, update an existing one, or remove it"
                        },
                        "id": {
                            "type": "string",
                            "description": "Item ID for update/remove actions"
                        },
                        "title": {
                            "type": "string",
                            "description": "Item title for add/update actions"
                        },
                        "status": {
                            "type": "string",
                            "enum": ["pending", "in_progress", "completed"],
                            "description": "Item status for add/update actions"
                        }
                    },
                    "required": ["action"]
                })),
                exposure: FacadeExposure::LlmDefault,
                workspace_root: None,
            },
            ToolsetCategory::SkillManagement,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(cap: &str, input: serde_json::Value) -> CapabilityExecutionRequest {
        use cyberclaw_core::identity::{ActorRef, ActorType};
        use cyberclaw_core::ids::{ActorId, CapabilityId, ConnectorId, ExecutionId, WorkspaceId};
        use cyberclaw_core::workspace::{WorkspaceMode, WorkspaceRef};

        CapabilityExecutionRequest {
            execution_id: ExecutionId::new(),
            trace_id: "test-trace".to_string(),
            actor: ActorRef {
                id: ActorId::new(),
                actor_type: ActorType::System,
                tenant_id: None,
                home_node_id: None,
                display_name: "test".to_string(),
            },
            workspace: WorkspaceRef {
                id: WorkspaceId::new(),
                mode: WorkspaceMode::Ephemeral,
                materialization_mode: None,
                home_node_id: None,
                backing_store: None,
                root: "/tmp/test".to_string(),
                writable_roots: vec![],
            },
            connector_id: ConnectorId::from_string("todo".to_string()).unwrap(),
            capability_id: CapabilityId::from_string(cap.to_string()).unwrap(),
            input,
        }
    }

    #[test]
    fn capabilities_list_has_two_entries() {
        let conn = TodoConnector::new();
        let caps = conn.capabilities();
        assert_eq!(caps.len(), 2);
        let ids: Vec<&str> = caps.iter().map(|c| c.id.as_str()).collect();
        assert!(ids.contains(&"agent:todo:read"));
        assert!(ids.contains(&"agent:todo:write"));
    }

    #[test]
    fn risk_levels_match_facades() {
        let conn = TodoConnector::new();
        let caps = conn.capabilities();
        let read = caps.iter().find(|c| c.id == "agent:todo:read").unwrap();
        let write = caps.iter().find(|c| c.id == "agent:todo:write").unwrap();
        assert_eq!(read.risk, RiskLevel::Low);
        assert_eq!(write.risk, RiskLevel::Medium);
    }

    #[tokio::test]
    async fn read_missing_agent_id_returns_failed_with_error_payload() {
        let conn = TodoConnector::new();
        let resp = conn
            .execute(req("agent:todo:read", serde_json::json!({})))
            .await
            .unwrap();
        assert_eq!(resp.status, ExecutionStatus::Failed);
        assert!(resp.error.unwrap().contains("agent_id"));
    }

    #[tokio::test]
    async fn unknown_capability_returns_failed() {
        let conn = TodoConnector::new();
        let resp = conn
            .execute(req("agent:todo:nope", serde_json::json!({})))
            .await
            .unwrap();
        assert_eq!(resp.status, ExecutionStatus::Failed);
        assert!(resp.error.unwrap().contains("unknown capability"));
    }
}
