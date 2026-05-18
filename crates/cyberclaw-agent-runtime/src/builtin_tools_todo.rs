//! Todo builtin tools: `todo_read` and `todo_write`
//!
//! Provides two `CapabilityFacade`-compatible tool definitions and their
//! pure-Rust execution logic.  State is persisted under
//! `~/.cyberclaw/todos/<agent_id>.json`.  Writes are atomic (tempfile + rename).
//!
//! # Tool IDs (capability_id)
//! - `agent:todo:read`
//! - `agent:todo:write`

use crate::builtin_tools::ToolsetCategory;
use crate::tool_description::CapabilityFacade;
use cyberclaw_core::capability::RiskLevel;
use cyberclaw_core::facade::FacadeExposure;
use cyberclaw_core::ids::{CapabilityId, ConnectorId};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Data model
// ---------------------------------------------------------------------------

/// Status of a todo item.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    #[default]
    Pending,
    InProgress,
    Completed,
}

impl std::fmt::Display for TodoStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TodoStatus::Pending => write!(f, "pending"),
            TodoStatus::InProgress => write!(f, "in_progress"),
            TodoStatus::Completed => write!(f, "completed"),
        }
    }
}

/// A single todo item stored in the agent's todo list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub title: String,
    pub status: TodoStatus,
    pub created_at: String,
    pub updated_at: String,
}

// ---------------------------------------------------------------------------
// Persistence helpers
// ---------------------------------------------------------------------------

/// Resolve the base directory for todo storage.
///
/// Priority: explicit `base_dir` override → `$HOME/.cyberclaw/todos/`.
fn resolve_base_dir(base_dir: Option<&Path>) -> PathBuf {
    if let Some(dir) = base_dir {
        return dir.to_path_buf();
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join(".cyberclaw").join("todos")
}

/// Returns the path for storing todos for `agent_id`.
fn todo_path_in(agent_id: &str, base_dir: Option<&Path>) -> PathBuf {
    resolve_base_dir(base_dir).join(format!("{agent_id}.json"))
}

/// Load todos from disk.  Returns an empty vec if the file does not exist.
fn load_todos_in(agent_id: &str, base_dir: Option<&Path>) -> Result<Vec<TodoItem>, String> {
    let path = todo_path_in(agent_id, base_dir);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    serde_json::from_str::<Vec<TodoItem>>(&raw)
        .map_err(|e| format!("failed to parse {}: {e}", path.display()))
}

/// Write todos to disk atomically via a temporary file + rename.
fn save_todos_in(
    agent_id: &str,
    todos: &[TodoItem],
    base_dir: Option<&Path>,
) -> Result<(), String> {
    let path = todo_path_in(agent_id, base_dir);

    // Ensure parent directory exists.
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create dirs {}: {e}", parent.display()))?;
    }

    let json = serde_json::to_string_pretty(todos)
        .map_err(|e| format!("failed to serialise todos: {e}"))?;

    // Write to a sibling temp file then rename for atomicity.
    let tmp_path = path.with_extension("tmp");
    std::fs::write(&tmp_path, &json)
        .map_err(|e| format!("failed to write tmp {}: {e}", tmp_path.display()))?;
    std::fs::rename(&tmp_path, &path)
        .map_err(|e| format!("failed to rename tmp to final {}: {e}", path.display()))?;

    Ok(())
}

/// Current UTC timestamp as an ISO-8601 string (seconds precision).
fn now_iso8601() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

// ---------------------------------------------------------------------------
// Action enum
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoAction {
    Add,
    Update,
    Remove,
}

impl std::fmt::Display for TodoAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TodoAction::Add => write!(f, "add"),
            TodoAction::Update => write!(f, "update"),
            TodoAction::Remove => write!(f, "remove"),
        }
    }
}

// ---------------------------------------------------------------------------
// Core logic (base_dir-parameterised)
// ---------------------------------------------------------------------------

fn todo_read_inner(input: &serde_json::Value, base_dir: Option<&Path>) -> serde_json::Value {
    let agent_id = match input.get("agent_id").and_then(|v| v.as_str()) {
        Some(id) => id,
        None => return serde_json::json!({ "error": "missing required field: agent_id" }),
    };

    match load_todos_in(agent_id, base_dir) {
        Ok(todos) => serde_json::json!({ "todos": todos }),
        Err(e) => serde_json::json!({ "error": e }),
    }
}

fn todo_write_inner(input: &serde_json::Value, base_dir: Option<&Path>) -> serde_json::Value {
    let agent_id = match input.get("agent_id").and_then(|v| v.as_str()) {
        Some(id) => id,
        None => return serde_json::json!({ "error": "missing required field: agent_id" }),
    };

    let action_str = match input.get("action").and_then(|v| v.as_str()) {
        Some(a) => a,
        None => return serde_json::json!({ "error": "missing required field: action" }),
    };

    let action: TodoAction = match action_str {
        "add" => TodoAction::Add,
        "update" => TodoAction::Update,
        "remove" => TodoAction::Remove,
        other => {
            return serde_json::json!({
                "error": format!("unknown action: {other}; expected add|update|remove")
            });
        }
    };

    let mut todos = match load_todos_in(agent_id, base_dir) {
        Ok(t) => t,
        Err(e) => return serde_json::json!({ "error": e }),
    };

    let affected_id: String;

    match action {
        TodoAction::Add => {
            let title = match input.get("title").and_then(|v| v.as_str()) {
                Some(t) => t.to_string(),
                None => {
                    return serde_json::json!({
                        "error": "missing required field for add: title"
                    });
                }
            };
            let status = parse_status(input.get("status").and_then(|v| v.as_str()));
            let id = input
                .get("id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            affected_id = id.clone();
            let now = now_iso8601();
            todos.push(TodoItem {
                id,
                title,
                status,
                created_at: now.clone(),
                updated_at: now,
            });
        }
        TodoAction::Update => {
            let id = match input.get("id").and_then(|v| v.as_str()) {
                Some(i) => i.to_string(),
                None => {
                    return serde_json::json!({
                        "error": "missing required field for update: id"
                    });
                }
            };
            affected_id = id.clone();
            let item = match todos.iter_mut().find(|t| t.id == id) {
                Some(i) => i,
                None => {
                    return serde_json::json!({ "error": format!("todo id not found: {id}") });
                }
            };
            if let Some(title) = input.get("title").and_then(|v| v.as_str()) {
                item.title = title.to_string();
            }
            if let Some(status_str) = input.get("status").and_then(|v| v.as_str()) {
                item.status = parse_status(Some(status_str));
            }
            item.updated_at = now_iso8601();
        }
        TodoAction::Remove => {
            let id = match input.get("id").and_then(|v| v.as_str()) {
                Some(i) => i.to_string(),
                None => {
                    return serde_json::json!({
                        "error": "missing required field for remove: id"
                    });
                }
            };
            affected_id = id.clone();
            let before = todos.len();
            todos.retain(|t| t.id != id);
            if todos.len() == before {
                return serde_json::json!({ "error": format!("todo id not found: {id}") });
            }
        }
    }

    if let Err(e) = save_todos_in(agent_id, &todos, base_dir) {
        return serde_json::json!({ "error": e });
    }

    serde_json::json!({
        "todos": todos,
        "action_taken": action.to_string(),
        "affected_id": affected_id,
    })
}

fn parse_status(s: Option<&str>) -> TodoStatus {
    match s {
        Some("in_progress") => TodoStatus::InProgress,
        Some("completed") => TodoStatus::Completed,
        _ => TodoStatus::Pending,
    }
}

// ---------------------------------------------------------------------------
// Public entry points (production — no base_dir override)
// ---------------------------------------------------------------------------

/// Execute `todo_read`.
///
/// Input JSON: `{ "agent_id": "..." }`
/// Output JSON: `{ "todos": [...] }`
pub fn execute_todo_read(input: &serde_json::Value) -> serde_json::Value {
    todo_read_inner(input, None)
}

/// Execute `todo_write`.
///
/// Input JSON:
/// ```json
/// {
///   "agent_id": "...",
///   "action": "add|update|remove",
///   "id": "...",
///   "title": "...",
///   "status": "pending|in_progress|completed"
/// }
/// ```
pub fn execute_todo_write(input: &serde_json::Value) -> serde_json::Value {
    todo_write_inner(input, None)
}

// ---------------------------------------------------------------------------
// CapabilityFacade builders (consumed by builtin_tools.rs)
// ---------------------------------------------------------------------------

/// Build the `todo_read` facade.
pub fn todo_read_facade() -> (CapabilityFacade, ToolsetCategory) {
    let facade = CapabilityFacade {
        name: "todo_read".to_string(),
        description: "Read the todo list for the specified agent. Returns all todo items with their id, title, status, and timestamps.".to_string(),
        input_schema: Some(serde_json::json!({
            "type": "object",
            "properties": {
                "agent_id": {
                    "type": "string",
                    "description": "The agent whose todo list to read"
                }
            },
            "required": ["agent_id"]
        })),
        connector_id: ConnectorId::from_string("todo".to_string()).unwrap(),
        capability_id: CapabilityId::from_string("agent:todo:read".to_string()).unwrap(),
        risk_level: RiskLevel::Low,
        effects: vec!["read".to_string()],
        read_only: true,
        destructive: false,
        exposure: FacadeExposure::LlmDefault,
    };
    (facade, ToolsetCategory::Memory)
}

/// Build the `todo_write` facade.
pub fn todo_write_facade() -> (CapabilityFacade, ToolsetCategory) {
    let facade = CapabilityFacade {
        name: "todo_write".to_string(),
        description: "Add, update, or remove a todo item for the specified agent. Changes are persisted atomically.".to_string(),
        input_schema: Some(serde_json::json!({
            "type": "object",
            "properties": {
                "agent_id": {
                    "type": "string",
                    "description": "The agent whose todo list to modify"
                },
                "action": {
                    "type": "string",
                    "enum": ["add", "update", "remove"],
                    "description": "The operation to perform"
                },
                "id": {
                    "type": "string",
                    "description": "Todo item ID; required for update and remove; optional for add (auto-generated if omitted)"
                },
                "title": {
                    "type": "string",
                    "description": "Title of the todo item; required for add, optional for update"
                },
                "status": {
                    "type": "string",
                    "enum": ["pending", "in_progress", "completed"],
                    "description": "Status of the todo item"
                }
            },
            "required": ["agent_id", "action"]
        })),
        connector_id: ConnectorId::from_string("todo".to_string()).unwrap(),
        capability_id: CapabilityId::from_string("agent:todo:write".to_string()).unwrap(),
        risk_level: RiskLevel::Medium,
        effects: vec!["write".to_string()],
        read_only: false,
        destructive: false,
        exposure: FacadeExposure::LlmDefault,
    };
    (facade, ToolsetCategory::Memory)
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Create an isolated tempdir and return a helper that uses it as base_dir.
    /// Each test calls this independently — no shared global state.
    fn tmp_base() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn test_todo_read_empty_returns_empty_list() {
        let base = tmp_base();
        let result = todo_read_inner(
            &serde_json::json!({ "agent_id": "agent-read-empty" }),
            Some(base.path()),
        );
        let todos = result["todos"].as_array().expect("todos array");
        assert!(todos.is_empty(), "expected empty list for new agent");
    }

    #[test]
    fn test_todo_write_add_persists() {
        let base = tmp_base();
        let result = todo_write_inner(
            &serde_json::json!({
                "agent_id": "agent-add",
                "action": "add",
                "id": "todo-1",
                "title": "Implement feature X",
                "status": "pending"
            }),
            Some(base.path()),
        );
        assert!(result.get("error").is_none(), "unexpected error: {result}");
        assert_eq!(result["action_taken"], "add");
        assert_eq!(result["affected_id"], "todo-1");

        // Read back using the same base_dir to verify persistence.
        let read = todo_read_inner(
            &serde_json::json!({ "agent_id": "agent-add" }),
            Some(base.path()),
        );
        let todos = read["todos"].as_array().unwrap();
        assert_eq!(todos.len(), 1);
        assert_eq!(todos[0]["id"], "todo-1");
        assert_eq!(todos[0]["title"], "Implement feature X");
        assert_eq!(todos[0]["status"], "pending");
    }

    #[test]
    fn test_todo_write_update_by_id() {
        let base = tmp_base();
        // Add first.
        todo_write_inner(
            &serde_json::json!({
                "agent_id": "agent-update",
                "action": "add",
                "id": "todo-2",
                "title": "Draft RFC",
                "status": "pending"
            }),
            Some(base.path()),
        );

        // Update status and title.
        let result = todo_write_inner(
            &serde_json::json!({
                "agent_id": "agent-update",
                "action": "update",
                "id": "todo-2",
                "title": "Draft RFC (revised)",
                "status": "in_progress"
            }),
            Some(base.path()),
        );
        assert!(result.get("error").is_none(), "unexpected error: {result}");
        assert_eq!(result["action_taken"], "update");

        let todos = result["todos"].as_array().unwrap();
        assert_eq!(todos.len(), 1);
        assert_eq!(todos[0]["title"], "Draft RFC (revised)");
        assert_eq!(todos[0]["status"], "in_progress");
    }

    #[test]
    fn test_todo_write_remove_by_id() {
        let base = tmp_base();
        // Add two items.
        todo_write_inner(
            &serde_json::json!({
                "agent_id": "agent-remove",
                "action": "add",
                "id": "todo-a",
                "title": "Task A"
            }),
            Some(base.path()),
        );
        todo_write_inner(
            &serde_json::json!({
                "agent_id": "agent-remove",
                "action": "add",
                "id": "todo-b",
                "title": "Task B"
            }),
            Some(base.path()),
        );

        // Remove the first.
        let result = todo_write_inner(
            &serde_json::json!({
                "agent_id": "agent-remove",
                "action": "remove",
                "id": "todo-a"
            }),
            Some(base.path()),
        );
        assert!(result.get("error").is_none(), "unexpected error: {result}");
        assert_eq!(result["action_taken"], "remove");
        assert_eq!(result["affected_id"], "todo-a");

        let todos = result["todos"].as_array().unwrap();
        assert_eq!(todos.len(), 1);
        assert_eq!(todos[0]["id"], "todo-b");
    }

    #[test]
    fn test_atomic_write_no_partial_file_on_crash() {
        // Verifies that the .tmp file is cleaned up and the final file is valid JSON.
        let base = tmp_base();
        todo_write_inner(
            &serde_json::json!({
                "agent_id": "agent-atomic",
                "action": "add",
                "id": "todo-x",
                "title": "Atomic test"
            }),
            Some(base.path()),
        );

        let path = todo_path_in("agent-atomic", Some(base.path()));
        let tmp_path = path.with_extension("tmp");

        // The final file must exist and be valid JSON.
        assert!(path.exists(), "final file should exist");
        let raw = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&raw).expect("valid JSON");
        assert!(parsed.is_array());

        // The .tmp file must NOT remain after a successful write.
        assert!(
            !tmp_path.exists(),
            ".tmp file should be renamed away after successful write"
        );
    }

    #[test]
    fn test_todo_read_missing_agent_id_returns_error() {
        let result = execute_todo_read(&serde_json::json!({}));
        assert!(result.get("error").is_some());
    }

    #[test]
    fn test_todo_write_missing_action_returns_error() {
        let result = execute_todo_write(&serde_json::json!({ "agent_id": "agent-x" }));
        assert!(result.get("error").is_some());
    }

    #[test]
    fn test_todo_write_unknown_action_returns_error() {
        let result = execute_todo_write(&serde_json::json!({
            "agent_id": "agent-x",
            "action": "purge"
        }));
        assert!(result.get("error").is_some());
    }

    #[test]
    fn test_todo_read_facade_schema() {
        let (facade, cat) = todo_read_facade();
        assert_eq!(facade.name, "todo_read");
        assert_eq!(facade.capability_id.as_str(), "agent:todo:read");
        assert_eq!(facade.risk_level, RiskLevel::Low);
        assert!(facade.read_only);
        assert!(!facade.destructive);
        assert_eq!(cat, ToolsetCategory::Memory);
        let schema = facade.input_schema.unwrap();
        assert_eq!(schema["required"][0], "agent_id");
    }

    #[test]
    fn test_todo_write_facade_schema() {
        let (facade, cat) = todo_write_facade();
        assert_eq!(facade.name, "todo_write");
        assert_eq!(facade.capability_id.as_str(), "agent:todo:write");
        assert_eq!(facade.risk_level, RiskLevel::Medium);
        assert!(!facade.read_only);
        assert!(!facade.destructive);
        assert_eq!(cat, ToolsetCategory::Memory);
        let schema = facade.input_schema.unwrap();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "agent_id"));
        assert!(required.iter().any(|v| v == "action"));
    }
}
