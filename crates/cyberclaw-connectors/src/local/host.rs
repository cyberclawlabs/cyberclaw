use super::LocalConnector;
use crate::local::cmd;
use crate::types::CapabilityExecutionRequest;
use chrono::{DateTime, Utc};
use cyberclaw_skill_runtime::loaders::{LoadedSkill, UnifiedSkillLoader};
use cyberclaw_skill_runtime::runtime::{MinimalSkillRuntime, SkillRuntime};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::Mutex;
use uuid::Uuid;

static HOST_STATE_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));
static HOST_SKILL_RUNTIME: Lazy<MinimalSkillRuntime> = Lazy::new(MinimalSkillRuntime::new);
static HOST_SKILL_LOADER: Lazy<UnifiedSkillLoader> = Lazy::new(UnifiedSkillLoader::new);
static SYMBOL_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"^\s*(?:pub\s+)?(?:async\s+)?(fn|struct|enum|trait|impl)\s+([A-Za-z_][A-Za-z0-9_]*)",
    )
    .expect("valid symbol regex")
});

const MAX_BRIEF_CHARS: usize = 800;
const MAX_REMOTE_BODY_CHARS: usize = 8192;
const DEFAULT_SLEEP_MS: u64 = 1000;

const CLAUDE_COMPAT_TOOL_NAMES: &[&str] = &[
    // Core file/search/shell
    "Read",
    "Write",
    "Edit",
    "Grep",
    "Glob",
    "Bash",
    "PowerShell",
    "WebFetch",
    "WebSearch",
    // Existing aliases
    "FileReadTool",
    "FileWriteTool",
    "FileEditTool",
    "GrepTool",
    "GlobTool",
    "BashTool",
    "PowerShellTool",
    "WebFetchTool",
    "WebSearchTool",
    "ListMcpResourcesTool",
    "ReadMcpResourceTool",
    "MCPTool",
    "mcp",
    // Host/session orchestration tools
    "Agent",
    "Task",
    "AgentTool",
    "AskUserQuestion",
    "AskUserQuestionTool",
    "SendMessage",
    "SendUserMessage",
    "Brief",
    "BriefTool",
    "Config",
    "ConfigTool",
    "EnterPlanMode",
    "EnterPlanModeTool",
    "ExitPlanMode",
    "ExitPlanModeTool",
    "EnterWorktree",
    "EnterWorktreeTool",
    "ExitWorktree",
    "ExitWorktreeTool",
    "LSP",
    "LSPTool",
    "McpAuthTool",
    "NotebookEdit",
    "NotebookEditTool",
    "REPL",
    "REPLTool",
    "RemoteTrigger",
    "RemoteTriggerTool",
    "Skill",
    "SkillTool",
    "Sleep",
    "SleepTool",
    "StructuredOutput",
    "SyntheticOutputTool",
    "TaskCreate",
    "TaskCreateTool",
    "TaskGet",
    "TaskGetTool",
    "TaskList",
    "TaskListTool",
    "TaskOutput",
    "TaskOutputTool",
    "TaskStop",
    "TaskStopTool",
    "TaskUpdate",
    "TaskUpdateTool",
    "TeamCreate",
    "TeamCreateTool",
    "TeamDelete",
    "TeamDeleteTool",
    "TodoWrite",
    "TodoWriteTool",
    "ToolSearch",
    "ToolSearchTool",
    "CronCreate",
    "CronDelete",
    "CronList",
    "ScheduleCronTool",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HostTaskRecord {
    id: String,
    subject: String,
    description: Option<String>,
    status: String,
    active_form: Option<String>,
    metadata: Option<Value>,
    output: Vec<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HostTeamRecord {
    name: String,
    description: Option<String>,
    members: Vec<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HostTodoItem {
    content: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    active_form: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HostAgentSession {
    id: String,
    agent_type: String,
    message: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HostCronJob {
    id: String,
    name: String,
    cron: String,
    command: String,
    enabled: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct HostState {
    tasks: BTreeMap<String, HostTaskRecord>,
    teams: BTreeMap<String, HostTeamRecord>,
    todos: Vec<HostTodoItem>,
    config: BTreeMap<String, Value>,
    plan_mode: bool,
    current_worktree: Option<String>,
    mcp_auth: BTreeMap<String, String>,
    agents: BTreeMap<String, HostAgentSession>,
    cron_jobs: BTreeMap<String, HostCronJob>,
}

pub async fn execute(
    connector: &LocalConnector,
    request: CapabilityExecutionRequest,
) -> anyhow::Result<Value> {
    match request.capability_id.as_str() {
        "host.agent.run" => handle_agent(connector, &request).await,
        "host.ask_user_question" => handle_ask_user_question(&request.input),
        "host.brief" => handle_brief(&request.input),
        "host.config" => handle_config(connector, &request.input).await,
        "host.plan.enter" => handle_plan_mode(connector, true).await,
        "host.plan.exit" => handle_plan_mode(connector, false).await,
        "host.worktree.enter" => handle_worktree_enter(connector, &request.input).await,
        "host.worktree.exit" => handle_worktree_exit(connector).await,
        "host.lsp" => handle_lsp(connector, &request.input).await,
        "host.mcp.auth" => handle_mcp_auth(connector, &request.input).await,
        "host.notebook.edit" => handle_notebook_edit(connector, &request.input).await,
        "host.repl" => handle_repl(connector, &request).await,
        "host.remote.trigger" => handle_remote_trigger(&request.input).await,
        "host.skill.invoke" => handle_skill(connector, &request.input).await,
        "host.sleep" => handle_sleep(&request.input).await,
        "host.synthetic.output" => handle_synthetic_output(&request.input),
        "host.task.create" => handle_task_create(connector, &request.input).await,
        "host.task.get" => handle_task_get(connector, &request.input).await,
        "host.task.list" => handle_task_list(connector, &request.input).await,
        "host.task.output" => handle_task_output(connector, &request.input).await,
        "host.task.stop" => handle_task_stop(connector, &request.input).await,
        "host.task.update" => handle_task_update(connector, &request.input).await,
        "host.team.create" => handle_team_create(connector, &request.input).await,
        "host.team.delete" => handle_team_delete(connector, &request.input).await,
        "host.todo.write" => handle_todo_write(connector, &request.input).await,
        "host.tool.search" => handle_tool_search(&request.input),
        "host.cron.create" => handle_cron_create(connector, &request.input).await,
        "host.cron.delete" => handle_cron_delete(connector, &request.input).await,
        "host.cron.list" => handle_cron_list(connector).await,
        other => Err(anyhow::anyhow!("Unsupported host capability: {}", other)),
    }
}

fn state_file_path(connector: &LocalConnector) -> PathBuf {
    connector
        .workspace
        .join(".cyberclaw")
        .join("host_state.json")
}

async fn load_state(connector: &LocalConnector) -> anyhow::Result<HostState> {
    let path = state_file_path(connector);
    if !path.exists() {
        return Ok(HostState::default());
    }

    let raw = tokio::fs::read_to_string(path).await?;
    let state = serde_json::from_str::<HostState>(&raw)?;
    Ok(state)
}

async fn save_state(connector: &LocalConnector, state: &HostState) -> anyhow::Result<()> {
    let path = state_file_path(connector);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let body = serde_json::to_vec_pretty(state)?;
    tokio::fs::write(path, body).await?;
    Ok(())
}

async fn with_state<T, F>(connector: &LocalConnector, f: F) -> anyhow::Result<T>
where
    F: FnOnce(&HostState) -> anyhow::Result<T>,
{
    let _guard = HOST_STATE_LOCK.lock().await;
    let state = load_state(connector).await?;
    f(&state)
}

async fn with_state_mut<T, F>(connector: &LocalConnector, f: F) -> anyhow::Result<T>
where
    F: FnOnce(&mut HostState) -> anyhow::Result<T>,
{
    let _guard = HOST_STATE_LOCK.lock().await;
    let mut state = load_state(connector).await?;
    let out = f(&mut state)?;
    save_state(connector, &state).await?;
    Ok(out)
}

fn find_string(input: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| {
        input
            .get(*k)
            .and_then(Value::as_str)
            .map(ToString::to_string)
    })
}

fn find_u64(input: &Value, keys: &[&str]) -> Option<u64> {
    keys.iter()
        .find_map(|k| input.get(*k).and_then(Value::as_u64))
}

fn find_bool(input: &Value, keys: &[&str]) -> Option<bool> {
    keys.iter()
        .find_map(|k| input.get(*k).and_then(Value::as_bool))
}

fn find_object(input: &Value, keys: &[&str]) -> Option<Value> {
    keys.iter().find_map(|k| input.get(*k).cloned())
}

fn parse_todo_item(value: &Value) -> Option<HostTodoItem> {
    let content = value
        .get("content")
        .and_then(Value::as_str)?
        .trim()
        .to_string();
    if content.is_empty() {
        return None;
    }

    let status = value
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("pending")
        .to_string();

    let active_form = value
        .get("activeForm")
        .and_then(Value::as_str)
        .map(ToString::to_string);

    Some(HostTodoItem {
        content,
        status,
        active_form,
    })
}

fn source_to_string(source: &Value) -> String {
    match source {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

fn string_to_source_lines(content: &str) -> Vec<Value> {
    if content.is_empty() {
        return Vec::new();
    }

    if content.contains('\n') {
        content
            .split_inclusive('\n')
            .map(|s| Value::String(s.to_string()))
            .collect()
    } else {
        vec![Value::String(content.to_string())]
    }
}

fn mask_token(token: &str) -> String {
    if token.len() <= 8 {
        return "****".to_string();
    }
    let start = &token[..4];
    let end = &token[token.len() - 4..];
    format!("{}...{}", start, end)
}

fn ensure_object(value: &Value, field_name: &str) -> anyhow::Result<()> {
    if value.is_object() {
        return Ok(());
    }
    Err(anyhow::anyhow!("{} must be a JSON object", field_name))
}

fn default_subject_from_message(message: &str) -> String {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return "Untitled task".to_string();
    }
    trimmed
        .split_whitespace()
        .take(8)
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_method(input: &Value) -> anyhow::Result<reqwest::Method> {
    let method = find_string(input, &["method"]).unwrap_or_else(|| "POST".to_string());
    method
        .parse::<reqwest::Method>()
        .map_err(|e| anyhow::anyhow!("Invalid HTTP method '{}': {}", method, e))
}

fn parse_skill_candidates(connector: &LocalConnector, skill_name: &str) -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    let workspace_skill = connector.workspace.join("skills").join(skill_name);
    candidates.push(workspace_skill);

    let ecosystem_skill = connector
        .workspace
        .join("ecosystem")
        .join("skills")
        .join(skill_name);
    candidates.push(ecosystem_skill);

    let superpowers_skill =
        PathBuf::from("/Users/cyber/.codex/superpowers/skills").join(skill_name);
    candidates.push(superpowers_skill);

    let codex_skill = PathBuf::from("/Users/cyber/.codex/skills").join(skill_name);
    candidates.push(codex_skill);

    candidates
}

fn resolve_skill_directory(connector: &LocalConnector, input: &Value) -> anyhow::Result<PathBuf> {
    if let Some(path) = find_string(input, &["skill_path", "path"]) {
        let validated = connector.validate_path(&path)?;
        let skill_dir = if validated.is_file() {
            validated
                .parent()
                .map(PathBuf::from)
                .ok_or_else(|| anyhow::anyhow!("invalid skill path '{}'", path))?
        } else {
            validated
        };
        return Ok(skill_dir);
    }

    let skill_name = find_string(input, &["skill", "skill_name", "name"])
        .ok_or_else(|| anyhow::anyhow!("skill_name is required"))?;
    for candidate in parse_skill_candidates(connector, &skill_name) {
        if candidate.exists() && candidate.is_dir() {
            return Ok(candidate);
        }
    }

    Err(anyhow::anyhow!("Skill '{}' not found", skill_name))
}

fn build_skill_invoke_payload(input: &Value) -> Value {
    if let Some(explicit) = input.get("input") {
        return explicit.clone();
    }

    if let Some(obj) = input.as_object() {
        let mut payload = serde_json::Map::new();
        for (key, value) in obj {
            if matches!(
                key.as_str(),
                "skill" | "skill_name" | "name" | "skill_path" | "path" | "action" | "inspect"
            ) {
                continue;
            }
            payload.insert(key.clone(), value.clone());
        }
        return Value::Object(payload);
    }

    Value::Null
}

fn build_skill_response(loaded: &LoadedSkill, output: Option<Value>) -> Value {
    let mut response = json!({
        "found": true,
        "skill": loaded.id.as_str(),
        "path": loaded.path.to_string_lossy(),
        "metadata": {
            "name": loaded.metadata.name,
            "version": loaded.metadata.version,
            "description": loaded.metadata.description,
            "author": loaded.metadata.author,
            "homepage": loaded.metadata.homepage,
            "tags": loaded.metadata.tags,
        }
    });

    if let Some(output) = output {
        response["invoked"] = Value::Bool(true);
        response["output"] = output;
    } else {
        response["invoked"] = Value::Bool(false);
    }

    response
}

async fn handle_agent(
    connector: &LocalConnector,
    request: &CapabilityExecutionRequest,
) -> anyhow::Result<Value> {
    let input = &request.input;
    let action = find_string(input, &["action"]).unwrap_or_else(|| "run".to_string());
    let target_agent_id = find_string(input, &["agent_id", "id", "to", "target"]);

    if action == "get" {
        let agent_id = target_agent_id
            .ok_or_else(|| anyhow::anyhow!("agent_id is required for action=get"))?;

        return with_state(connector, |state| {
            let agent = state
                .agents
                .get(&agent_id)
                .ok_or_else(|| anyhow::anyhow!("Agent session '{}' not found", agent_id))?;
            Ok(json!({ "agent": agent }))
        })
        .await;
    }

    if action == "send" || target_agent_id.is_some() {
        let agent_id = target_agent_id
            .ok_or_else(|| anyhow::anyhow!("agent_id is required for action=send"))?;
        let message =
            find_string(input, &["message", "prompt", "task", "text"]).unwrap_or_default();
        if message.trim().is_empty() {
            return Err(anyhow::anyhow!("message is required for action=send"));
        }

        return with_state_mut(connector, |state| {
            let session = state
                .agents
                .get_mut(&agent_id)
                .ok_or_else(|| anyhow::anyhow!("Agent session '{}' not found", agent_id))?;

            session.message = message.clone();
            session.updated_at = Utc::now();
            session.status = "completed".to_string();
            session.result = Some(json!({
                "mode": "message-dispatch",
                "delivered": true,
                "content": message,
            }));

            Ok(json!({
                "agent": session,
                "note": "Message delivered to existing host agent session"
            }))
        })
        .await;
    }

    let agent_type = find_string(input, &["agent_type", "subagent_type", "type"])
        .unwrap_or_else(|| "default".to_string());
    let message = find_string(input, &["message", "prompt", "task"]).unwrap_or_default();
    let command = find_string(input, &["command"]);
    let workdir = find_string(input, &["workdir", "cwd"]);
    let timeout_ms = find_u64(input, &["timeout_ms"]);

    let (agent_result, status) = if let Some(command) = command {
        let mut sub_request = request.clone();
        sub_request.input = json!({
            "command": command,
            "workdir": workdir,
            "timeout_ms": timeout_ms,
        });
        (Some(cmd::exec(connector, sub_request).await?), "completed")
    } else {
        if message.trim().is_empty() {
            return Err(anyhow::anyhow!(
                "host.agent.run requires either command or non-empty message"
            ));
        }

        (
            Some(json!({
                "mode": "message-dispatch",
                "accepted": true,
                "content": message,
            })),
            "completed",
        )
    };

    let session_id = target_agent_id
        .clone()
        .unwrap_or_else(|| format!("agent-{}", Uuid::new_v4().simple()));

    with_state_mut(connector, |state| {
        let now = Utc::now();
        let session = HostAgentSession {
            id: session_id.clone(),
            agent_type: agent_type.clone(),
            message: message.clone(),
            status: status.to_string(),
            result: agent_result.clone(),
            created_at: now,
            updated_at: now,
        };

        state.agents.insert(session_id.clone(), session.clone());

        Ok(json!({
            "agent": session,
            "note": "Agent capability executed in local host runtime"
        }))
    })
    .await
}

async fn load_and_register_skill(skill_dir: &Path) -> anyhow::Result<LoadedSkill> {
    let loaded = HOST_SKILL_LOADER.load_skill(skill_dir).await?;
    HOST_SKILL_RUNTIME
        .register_skill(loaded.id.clone(), loaded.handler.clone())
        .await;
    Ok(loaded)
}

async fn handle_skill(connector: &LocalConnector, input: &Value) -> anyhow::Result<Value> {
    let skill_dir = resolve_skill_directory(connector, input)?;
    let loaded = load_and_register_skill(&skill_dir).await?;

    let inspect = find_bool(input, &["inspect"]).unwrap_or(false)
        || find_string(input, &["action"])
            .map(|action| action == "inspect")
            .unwrap_or(false);

    if inspect {
        return Ok(build_skill_response(&loaded, None));
    }

    let invoke_payload = build_skill_invoke_payload(input);
    #[allow(deprecated)]
    let output = HOST_SKILL_RUNTIME
        .invoke(&loaded.id, invoke_payload)
        .await?;

    Ok(build_skill_response(&loaded, Some(output)))
}

fn handle_ask_user_question(input: &Value) -> anyhow::Result<Value> {
    let question = find_string(input, &["question", "prompt"]).unwrap_or_else(|| {
        "Please provide additional guidance for the next execution step.".to_string()
    });
    let options = input
        .get("options")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    Ok(json!({
        "awaiting_user_input": true,
        "question": question,
        "options": options,
        "status": "pending-user-input"
    }))
}

fn handle_brief(input: &Value) -> anyhow::Result<Value> {
    let text = if let Some(raw) = find_string(input, &["text", "content", "message"]) {
        raw
    } else if let Some(messages) = input.get("messages").and_then(Value::as_array) {
        messages
            .iter()
            .map(|m| {
                let role = m.get("role").and_then(Value::as_str).unwrap_or("unknown");
                let content = m.get("content").and_then(Value::as_str).unwrap_or("");
                format!("[{}] {}", role, content)
            })
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        String::new()
    };

    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(json!({
            "summary": "(empty)",
            "bullets": [],
            "chars": 0
        }));
    }

    let summary = if trimmed.chars().count() > MAX_BRIEF_CHARS {
        let prefix: String = trimmed.chars().take(MAX_BRIEF_CHARS).collect();
        format!("{}...", prefix)
    } else {
        trimmed.to_string()
    };

    let bullets: Vec<String> = trimmed
        .split(['。', '.', '\n'])
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(5)
        .map(ToString::to_string)
        .collect();

    Ok(json!({
        "summary": summary,
        "bullets": bullets,
        "chars": trimmed.chars().count()
    }))
}

async fn handle_config(connector: &LocalConnector, input: &Value) -> anyhow::Result<Value> {
    let action = find_string(input, &["action"]).unwrap_or_else(|| "get".to_string());
    let key = find_string(input, &["key"]);

    match action.as_str() {
        "set" => {
            let key = key.ok_or_else(|| anyhow::anyhow!("key is required for config set"))?;
            let value = find_object(input, &["value"]).unwrap_or(Value::Null);
            with_state_mut(connector, |state| {
                state.config.insert(key.clone(), value.clone());
                Ok(json!({ "action": "set", "key": key, "value": value }))
            })
            .await
        }
        "delete" => {
            let key = key.ok_or_else(|| anyhow::anyhow!("key is required for config delete"))?;
            with_state_mut(connector, |state| {
                let existed = state.config.remove(&key).is_some();
                Ok(json!({ "action": "delete", "key": key, "deleted": existed }))
            })
            .await
        }
        "list" => with_state(connector, |state| Ok(json!({ "config": state.config }))).await,
        _ => {
            if let Some(key) = key {
                with_state(connector, |state| {
                    Ok(json!({
                        "key": key,
                        "value": state.config.get(&key).cloned()
                    }))
                })
                .await
            } else {
                with_state(connector, |state| Ok(json!({ "config": state.config }))).await
            }
        }
    }
}

async fn handle_plan_mode(connector: &LocalConnector, enabled: bool) -> anyhow::Result<Value> {
    with_state_mut(connector, |state| {
        let previous = state.plan_mode;
        state.plan_mode = enabled;
        Ok(json!({
            "previous": previous,
            "current": enabled
        }))
    })
    .await
}

async fn handle_worktree_enter(connector: &LocalConnector, input: &Value) -> anyhow::Result<Value> {
    let requested = find_string(input, &["path", "worktree", "name"])
        .unwrap_or_else(|| ".cyberclaw/worktrees/default".to_string());
    let should_create = find_bool(input, &["create"]).unwrap_or(true);

    let validated = connector.validate_path(&requested)?;
    if should_create {
        tokio::fs::create_dir_all(&validated).await?;
    }

    with_state_mut(connector, |state| {
        let previous = state.current_worktree.clone();
        let current = validated.to_string_lossy().to_string();
        state.current_worktree = Some(current.clone());

        Ok(json!({
            "previous": previous,
            "current": current,
            "created": should_create
        }))
    })
    .await
}

async fn handle_worktree_exit(connector: &LocalConnector) -> anyhow::Result<Value> {
    with_state_mut(connector, |state| {
        let previous = state.current_worktree.clone();
        state.current_worktree = None;
        Ok(json!({
            "previous": previous,
            "current": Value::Null
        }))
    })
    .await
}

async fn handle_lsp(connector: &LocalConnector, input: &Value) -> anyhow::Result<Value> {
    let operation =
        find_string(input, &["operation", "op"]).unwrap_or_else(|| "symbols".to_string());

    match operation.as_str() {
        "symbols" => {
            let path = find_string(input, &["path", "file_path"])
                .ok_or_else(|| anyhow::anyhow!("path is required for LSP symbols"))?;
            let validated = connector.validate_path(&path)?;
            let content = tokio::fs::read_to_string(validated).await?;

            let symbols: Vec<Value> = content
                .lines()
                .enumerate()
                .filter_map(|(idx, line)| {
                    SYMBOL_RE.captures(line).map(|caps| {
                        json!({
                            "kind": caps.get(1).map(|m| m.as_str()).unwrap_or("unknown"),
                            "name": caps.get(2).map(|m| m.as_str()).unwrap_or(""),
                            "line": idx + 1
                        })
                    })
                })
                .collect();

            Ok(json!({ "operation": "symbols", "symbols": symbols }))
        }
        "definition" => {
            let path = find_string(input, &["path", "file_path"])
                .ok_or_else(|| anyhow::anyhow!("path is required for LSP definition"))?;
            let symbol = find_string(input, &["symbol", "name"])
                .ok_or_else(|| anyhow::anyhow!("symbol is required for LSP definition"))?;

            let validated = connector.validate_path(&path)?;
            let content = tokio::fs::read_to_string(validated).await?;

            let location = content.lines().enumerate().find_map(|(idx, line)| {
                if line.contains(&symbol) {
                    Some(json!({ "line": idx + 1, "content": line.trim() }))
                } else {
                    None
                }
            });

            Ok(json!({ "operation": "definition", "symbol": symbol, "location": location }))
        }
        "references" => {
            let symbol = find_string(input, &["symbol", "name"])
                .ok_or_else(|| anyhow::anyhow!("symbol is required for LSP references"))?;
            let path =
                find_string(input, &["path", "directory"]).unwrap_or_else(|| ".".to_string());
            let validated = connector.validate_path(&path)?;

            let output = tokio::process::Command::new("rg")
                .arg("-n")
                .arg(&symbol)
                .arg(&validated)
                .output()
                .await?;

            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let lines: Vec<&str> = stdout.lines().take(200).collect();
            Ok(json!({
                "operation": "references",
                "symbol": symbol,
                "count": lines.len(),
                "matches": lines
            }))
        }
        _ => Err(anyhow::anyhow!(
            "Unsupported LSP operation '{}'. Supported: symbols, definition, references",
            operation
        )),
    }
}

async fn handle_mcp_auth(connector: &LocalConnector, input: &Value) -> anyhow::Result<Value> {
    let action = find_string(input, &["action"]).unwrap_or_else(|| "set".to_string());
    let server = find_string(input, &["server", "server_name"])
        .ok_or_else(|| anyhow::anyhow!("server is required for mcp auth"))?;

    match action.as_str() {
        "set" => {
            let token = find_string(input, &["token", "access_token", "auth_token"])
                .ok_or_else(|| anyhow::anyhow!("token is required for mcp auth set"))?;

            with_state_mut(connector, |state| {
                state.mcp_auth.insert(server.clone(), token.clone());
                Ok(json!({
                    "server": server,
                    "status": "stored",
                    "token_masked": mask_token(&token)
                }))
            })
            .await
        }
        "delete" => {
            with_state_mut(connector, |state| {
                let removed = state.mcp_auth.remove(&server).is_some();
                Ok(json!({
                    "server": server,
                    "deleted": removed
                }))
            })
            .await
        }
        _ => {
            with_state(connector, |state| {
                let token = state.mcp_auth.get(&server).cloned();
                Ok(json!({
                    "server": server,
                    "exists": token.is_some(),
                    "token_masked": token.map(|t| mask_token(&t))
                }))
            })
            .await
        }
    }
}

async fn handle_notebook_edit(connector: &LocalConnector, input: &Value) -> anyhow::Result<Value> {
    let notebook_path = find_string(input, &["notebook_path", "path", "file_path"])
        .ok_or_else(|| anyhow::anyhow!("notebook_path is required"))?;
    let new_source = find_string(input, &["new_source", "content"])
        .ok_or_else(|| anyhow::anyhow!("new_source is required"))?;
    let edit_mode = find_string(input, &["edit_mode"]).unwrap_or_else(|| "replace".to_string());

    let path = connector.validate_path(&notebook_path)?;
    let raw = tokio::fs::read_to_string(&path).await?;
    let mut notebook: Value = serde_json::from_str(&raw)?;

    ensure_object(&notebook, "notebook")?;

    let cells = notebook
        .get_mut("cells")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| anyhow::anyhow!("Invalid notebook: missing cells array"))?;

    if cells.is_empty() {
        return Err(anyhow::anyhow!("Notebook has no cells"));
    }

    let index = if let Some(idx) = find_u64(input, &["cell_index"]) {
        usize::try_from(idx).unwrap_or(0)
    } else if let Some(cell_id) = find_string(input, &["cell_id"]) {
        cells
            .iter()
            .position(|cell| cell.get("id").and_then(Value::as_str) == Some(cell_id.as_str()))
            .unwrap_or(0)
    } else {
        0
    };

    if index >= cells.len() {
        return Err(anyhow::anyhow!("cell_index {} out of range", index));
    }

    let target = cells
        .get_mut(index)
        .ok_or_else(|| anyhow::anyhow!("cell_index {} not found", index))?;

    let current = source_to_string(target.get("source").unwrap_or(&Value::Null));
    let merged = match edit_mode.as_str() {
        "append" => format!("{}{}", current, new_source),
        "prepend" => format!("{}{}", new_source, current),
        _ => new_source,
    };

    if let Some(obj) = target.as_object_mut() {
        obj.insert(
            "source".to_string(),
            Value::Array(string_to_source_lines(&merged)),
        );
    }

    let serialized = serde_json::to_vec_pretty(&notebook)?;
    tokio::fs::write(path, serialized).await?;

    Ok(json!({
        "path": notebook_path,
        "cell_index": index,
        "edit_mode": edit_mode,
        "source_length": merged.len()
    }))
}

async fn handle_repl(
    connector: &LocalConnector,
    request: &CapabilityExecutionRequest,
) -> anyhow::Result<Value> {
    let input = &request.input;
    let command = find_string(input, &["command", "code"])
        .ok_or_else(|| anyhow::anyhow!("command/code is required for REPL"))?;

    let mut cmd_request = request.clone();
    cmd_request.input = json!({
        "command": command,
        "workdir": find_string(input, &["workdir", "cwd"]),
        "timeout_ms": find_u64(input, &["timeout_ms"])
    });

    let output = cmd::exec(connector, cmd_request).await?;
    Ok(json!({
        "mode": "command",
        "result": output
    }))
}

async fn handle_remote_trigger(input: &Value) -> anyhow::Result<Value> {
    let url = find_string(input, &["url", "endpoint"])
        .ok_or_else(|| anyhow::anyhow!("url is required"))?;
    let method = parse_method(input)?;
    let timeout_ms = find_u64(input, &["timeout_ms"]).unwrap_or(10_000);

    let mut builder = reqwest::Client::builder().timeout(Duration::from_millis(timeout_ms));
    if find_bool(input, &["no_proxy"]).unwrap_or(true) {
        builder = builder.no_proxy();
    }
    let client = builder.build()?;

    let mut request = client.request(method.clone(), &url);

    if let Some(headers) = input.get("headers").and_then(Value::as_object) {
        for (key, value) in headers {
            if let Some(text) = value.as_str() {
                request = request.header(key, text);
            }
        }
    }

    if let Some(payload) = input.get("payload") {
        request = request.json(payload);
    }

    let response = request.send().await?;
    let status = response.status().as_u16();
    let text = response.text().await?;
    let truncated = text.chars().count() > MAX_REMOTE_BODY_CHARS;
    let body = if truncated {
        text.chars().take(MAX_REMOTE_BODY_CHARS).collect::<String>()
    } else {
        text
    };

    Ok(json!({
        "url": url,
        "method": method.as_str(),
        "status": status,
        "body": body,
        "truncated": truncated
    }))
}

async fn handle_sleep(input: &Value) -> anyhow::Result<Value> {
    let duration_ms = if let Some(ms) = find_u64(input, &["duration_ms", "milliseconds", "ms"]) {
        ms
    } else if let Some(seconds) = find_u64(input, &["seconds", "duration_s"]) {
        seconds.saturating_mul(1000)
    } else {
        DEFAULT_SLEEP_MS
    };

    tokio::time::sleep(Duration::from_millis(duration_ms)).await;
    Ok(json!({ "slept_ms": duration_ms }))
}

fn validate_json_schema(schema: &Value, data: &Value, path: &str, errors: &mut Vec<String>) {
    let schema_type = schema
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("object");

    match schema_type {
        "object" => {
            if !data.is_object() {
                errors.push(format!("{} expected object", path));
                return;
            }

            if let Some(required) = schema.get("required").and_then(Value::as_array) {
                for key in required.iter().filter_map(Value::as_str) {
                    if data.get(key).is_none() {
                        errors.push(format!("{}.{} is required", path, key));
                    }
                }
            }

            if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
                for (key, prop_schema) in properties {
                    if let Some(prop_data) = data.get(key) {
                        let child_path = format!("{}.{}", path, key);
                        validate_json_schema(prop_schema, prop_data, &child_path, errors);
                    }
                }
            }
        }
        "array" => {
            if let Some(items) = data.as_array() {
                if let Some(item_schema) = schema.get("items") {
                    for (idx, item) in items.iter().enumerate() {
                        let child_path = format!("{}[{}]", path, idx);
                        validate_json_schema(item_schema, item, &child_path, errors);
                    }
                }
            } else {
                errors.push(format!("{} expected array", path));
            }
        }
        "string" if !data.is_string() => {
            errors.push(format!("{} expected string", path));
        }
        "boolean" if !data.is_boolean() => {
            errors.push(format!("{} expected boolean", path));
        }
        "integer" if !(data.is_i64() || data.is_u64()) => {
            errors.push(format!("{} expected integer", path));
        }
        "number" if !(data.is_i64() || data.is_u64() || data.is_f64()) => {
            errors.push(format!("{} expected number", path));
        }
        _ => {}
    }
}

fn handle_synthetic_output(input: &Value) -> anyhow::Result<Value> {
    let schema = input
        .get("schema")
        .or_else(|| input.get("json_schema"))
        .ok_or_else(|| anyhow::anyhow!("schema/json_schema is required"))?;
    let data = input
        .get("data")
        .or_else(|| input.get("output"))
        .ok_or_else(|| anyhow::anyhow!("data/output is required"))?;

    let mut errors = Vec::new();
    validate_json_schema(schema, data, "$", &mut errors);

    Ok(json!({
        "valid": errors.is_empty(),
        "errors": errors,
        "data": data
    }))
}

async fn handle_task_create(connector: &LocalConnector, input: &Value) -> anyhow::Result<Value> {
    let subject = find_string(input, &["subject", "title"]).unwrap_or_else(|| {
        default_subject_from_message(
            &find_string(input, &["description", "message", "task"]).unwrap_or_default(),
        )
    });
    let description = find_string(input, &["description", "message", "task"]);
    let active_form = find_string(input, &["activeForm", "active_form"]);
    let metadata = find_object(input, &["metadata"]);

    with_state_mut(connector, |state| {
        let now = Utc::now();
        let id = format!("task-{}", Uuid::new_v4().simple());

        let task = HostTaskRecord {
            id: id.clone(),
            subject,
            description,
            status: "pending".to_string(),
            active_form,
            metadata,
            output: Vec::new(),
            created_at: now,
            updated_at: now,
        };

        state.tasks.insert(id.clone(), task.clone());

        Ok(json!({
            "task": {
                "id": id,
                "subject": task.subject,
                "status": task.status
            }
        }))
    })
    .await
}

async fn handle_task_get(connector: &LocalConnector, input: &Value) -> anyhow::Result<Value> {
    let task_id = find_string(input, &["task_id", "id"])
        .ok_or_else(|| anyhow::anyhow!("task_id/id is required"))?;

    with_state(connector, |state| {
        let task = state
            .tasks
            .get(&task_id)
            .ok_or_else(|| anyhow::anyhow!("Task '{}' not found", task_id))?;
        Ok(json!({ "task": task }))
    })
    .await
}

async fn handle_task_list(connector: &LocalConnector, input: &Value) -> anyhow::Result<Value> {
    let status_filter = find_string(input, &["status"]);
    let limit = find_u64(input, &["limit"])
        .and_then(|v| usize::try_from(v).ok())
        .unwrap_or(100);

    with_state(connector, |state| {
        let mut tasks: Vec<&HostTaskRecord> = state.tasks.values().collect();
        tasks.sort_by_key(|task| task.created_at);

        if let Some(status) = status_filter.as_deref() {
            tasks.retain(|task| task.status == status);
        }

        let tasks: Vec<&HostTaskRecord> = tasks.into_iter().take(limit).collect();

        Ok(json!({
            "tasks": tasks,
            "count": tasks.len()
        }))
    })
    .await
}

async fn handle_task_output(connector: &LocalConnector, input: &Value) -> anyhow::Result<Value> {
    let task_id = find_string(input, &["task_id", "id"])
        .ok_or_else(|| anyhow::anyhow!("task_id/id is required"))?;
    let output = find_string(input, &["output", "text", "message"])
        .ok_or_else(|| anyhow::anyhow!("output/text/message is required"))?;

    with_state_mut(connector, |state| {
        let task = state
            .tasks
            .get_mut(&task_id)
            .ok_or_else(|| anyhow::anyhow!("Task '{}' not found", task_id))?;
        task.output.push(output.clone());
        task.updated_at = Utc::now();

        Ok(json!({
            "task_id": task_id,
            "outputs": task.output.len()
        }))
    })
    .await
}

async fn handle_task_stop(connector: &LocalConnector, input: &Value) -> anyhow::Result<Value> {
    let task_id = find_string(input, &["task_id", "id"])
        .ok_or_else(|| anyhow::anyhow!("task_id/id is required"))?;

    with_state_mut(connector, |state| {
        let task = state
            .tasks
            .get_mut(&task_id)
            .ok_or_else(|| anyhow::anyhow!("Task '{}' not found", task_id))?;

        task.status = "stopped".to_string();
        task.updated_at = Utc::now();

        Ok(json!({
            "task": {
                "id": task_id,
                "status": task.status
            }
        }))
    })
    .await
}

async fn handle_task_update(connector: &LocalConnector, input: &Value) -> anyhow::Result<Value> {
    let task_id = find_string(input, &["task_id", "id"])
        .ok_or_else(|| anyhow::anyhow!("task_id/id is required"))?;

    let status = find_string(input, &["status"]);
    let subject = find_string(input, &["subject", "title"]);
    let description = find_string(input, &["description"]);
    let active_form = find_string(input, &["activeForm", "active_form"]);
    let metadata = find_object(input, &["metadata"]);

    with_state_mut(connector, |state| {
        let task = state
            .tasks
            .get_mut(&task_id)
            .ok_or_else(|| anyhow::anyhow!("Task '{}' not found", task_id))?;

        if let Some(status) = status {
            task.status = status;
        }
        if let Some(subject) = subject {
            task.subject = subject;
        }
        if let Some(description) = description {
            task.description = Some(description);
        }
        if let Some(active_form) = active_form {
            task.active_form = Some(active_form);
        }
        if metadata.is_some() {
            task.metadata = metadata;
        }

        task.updated_at = Utc::now();

        Ok(json!({ "task": task }))
    })
    .await
}

async fn handle_team_create(connector: &LocalConnector, input: &Value) -> anyhow::Result<Value> {
    let name = find_string(input, &["name", "team_name"])
        .ok_or_else(|| anyhow::anyhow!("name/team_name is required"))?;

    let description = find_string(input, &["description"]);
    let members: Vec<String> = input
        .get("members")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default();

    with_state_mut(connector, |state| {
        let now = Utc::now();
        let record = HostTeamRecord {
            name: name.clone(),
            description,
            members,
            created_at: now,
            updated_at: now,
        };

        state.teams.insert(name.clone(), record.clone());
        Ok(json!({ "team": record }))
    })
    .await
}

async fn handle_team_delete(connector: &LocalConnector, input: &Value) -> anyhow::Result<Value> {
    let name = find_string(input, &["name", "team_name"])
        .ok_or_else(|| anyhow::anyhow!("name/team_name is required"))?;

    with_state_mut(connector, |state| {
        let deleted = state.teams.remove(&name).is_some();
        Ok(json!({
            "name": name,
            "deleted": deleted
        }))
    })
    .await
}

async fn write_todo_markdown(
    connector: &LocalConnector,
    todos: &[HostTodoItem],
) -> anyhow::Result<PathBuf> {
    let dir = connector.workspace.join(".cyberclaw");
    tokio::fs::create_dir_all(&dir).await?;
    let path = dir.join("TODO.md");

    let mut lines = Vec::with_capacity(todos.len() + 1);
    lines.push("# TODO\n".to_string());
    for item in todos {
        let checked = if item.status == "done" || item.status == "completed" {
            "x"
        } else {
            " "
        };
        lines.push(format!("- [{}] {}", checked, item.content));
    }

    tokio::fs::write(&path, lines.join("\n")).await?;
    Ok(path)
}

async fn handle_todo_write(connector: &LocalConnector, input: &Value) -> anyhow::Result<Value> {
    let mut todos = Vec::new();

    if let Some(items) = input.get("todos").and_then(Value::as_array) {
        todos.extend(items.iter().filter_map(parse_todo_item));
    } else if let Some(content) = find_string(input, &["content", "task", "subject"]) {
        if !content.trim().is_empty() {
            todos.push(HostTodoItem {
                content,
                status: "pending".to_string(),
                active_form: find_string(input, &["activeForm", "active_form"]),
            });
        }
    }

    if todos.is_empty() {
        return Err(anyhow::anyhow!("No todo items provided"));
    }

    with_state_mut(connector, |state| {
        state.todos = todos.clone();
        Ok(())
    })
    .await?;

    let todo_file = write_todo_markdown(connector, &todos).await?;

    Ok(json!({
        "count": todos.len(),
        "todos": todos,
        "file": todo_file.to_string_lossy()
    }))
}

fn handle_tool_search(input: &Value) -> anyhow::Result<Value> {
    let query = find_string(input, &["query", "q"]).unwrap_or_default();
    let query_lower = query.to_ascii_lowercase();
    let limit = find_u64(input, &["limit"])
        .and_then(|v| usize::try_from(v).ok())
        .unwrap_or(20);

    let mut results: Vec<String> = CLAUDE_COMPAT_TOOL_NAMES
        .iter()
        .filter(|name| {
            if query_lower.is_empty() {
                true
            } else {
                name.to_ascii_lowercase().contains(&query_lower)
            }
        })
        .take(limit)
        .map(|s| (*s).to_string())
        .collect();

    results.sort();

    Ok(json!({
        "query": query,
        "total": results.len(),
        "results": results
    }))
}

async fn handle_cron_create(connector: &LocalConnector, input: &Value) -> anyhow::Result<Value> {
    let name =
        find_string(input, &["name", "job_name"]).unwrap_or_else(|| "scheduled-job".to_string());
    let cron = find_string(input, &["cron", "schedule"])
        .ok_or_else(|| anyhow::anyhow!("cron/schedule is required"))?;
    let command = find_string(input, &["command", "task"])
        .ok_or_else(|| anyhow::anyhow!("command/task is required"))?;

    with_state_mut(connector, |state| {
        let now = Utc::now();
        let id = format!("cron-{}", Uuid::new_v4().simple());
        let job = HostCronJob {
            id: id.clone(),
            name,
            cron,
            command,
            enabled: true,
            created_at: now,
            updated_at: now,
        };

        state.cron_jobs.insert(id.clone(), job.clone());
        Ok(json!({ "job": job }))
    })
    .await
}

async fn handle_cron_delete(connector: &LocalConnector, input: &Value) -> anyhow::Result<Value> {
    let job_id = find_string(input, &["job_id", "id"])
        .ok_or_else(|| anyhow::anyhow!("job_id/id is required"))?;

    with_state_mut(connector, |state| {
        let deleted = state.cron_jobs.remove(&job_id).is_some();
        Ok(json!({
            "job_id": job_id,
            "deleted": deleted
        }))
    })
    .await
}

async fn handle_cron_list(connector: &LocalConnector) -> anyhow::Result<Value> {
    with_state(connector, |state| {
        let jobs: Vec<&HostCronJob> = state.cron_jobs.values().collect();
        Ok(json!({
            "jobs": jobs,
            "count": jobs.len()
        }))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Connector;
    use tempfile::TempDir;

    #[test]
    fn test_mask_token() {
        assert_eq!(mask_token("short"), "****");
        assert_eq!(mask_token("abcd1234wxyz"), "abcd...wxyz");
    }

    #[test]
    fn test_synthetic_output_validation() {
        let input = json!({
            "schema": {
                "type": "object",
                "required": ["name"],
                "properties": {
                    "name": { "type": "string" },
                    "age": { "type": "integer" }
                }
            },
            "data": {
                "name": "cyber",
                "age": 18
            }
        });

        let result = handle_synthetic_output(&input).unwrap();
        assert_eq!(result["valid"], true);
    }

    fn build_request(capability: &str, input: Value) -> CapabilityExecutionRequest {
        CapabilityExecutionRequest {
            execution_id: cyberclaw_core::ids::ExecutionId::new(),
            trace_id: Uuid::new_v4().to_string(),
            actor: cyberclaw_core::identity::ActorRef {
                id: cyberclaw_core::ids::ActorId::new(),
                actor_type: cyberclaw_core::identity::ActorType::System,
                tenant_id: None,
                home_node_id: None,
                display_name: "host-test".to_string(),
            },
            workspace: cyberclaw_core::workspace::WorkspaceRef {
                id: cyberclaw_core::ids::WorkspaceId::new(),
                mode: cyberclaw_core::workspace::WorkspaceMode::Isolated,
                materialization_mode: None,
                home_node_id: None,
                backing_store: None,
                root: ".".to_string(),
                writable_roots: vec![".".to_string()],
            },
            connector_id: cyberclaw_core::ids::ConnectorId::from_string("local".to_string())
                .expect("valid connector id"),
            capability_id: cyberclaw_core::ids::CapabilityId::from_string(capability.to_string())
                .expect("valid capability id"),
            input,
        }
    }

    #[tokio::test]
    async fn test_host_agent_run_message_dispatch_no_echo_mode() {
        let tmp = TempDir::new().expect("tempdir");
        let connector = LocalConnector::new(tmp.path().to_path_buf());
        let request = build_request(
            "host.agent.run",
            json!({
                "agent_type": "default",
                "message": "hello host agent",
            }),
        );

        let result = connector.execute(request).await.expect("execute");
        assert_eq!(result.status, crate::types::ExecutionStatus::Success);
        assert_eq!(result.output["agent"]["result"]["mode"], "message-dispatch");
        assert_eq!(result.output["agent"]["result"]["accepted"], true);
        assert!(result.output["agent"]["result"].get("content").is_some());
    }

    #[tokio::test]
    async fn test_host_skill_invoke_executes_handler() {
        let tmp = TempDir::new().expect("tempdir");
        let workspace = tmp.path().to_path_buf();
        let connector = LocalConnector::new(workspace.clone());

        let skill_dir = workspace.join("skills").join("unit-skill");
        tokio::fs::create_dir_all(&skill_dir)
            .await
            .expect("create skill dir");
        tokio::fs::write(
            skill_dir.join("SKILL.md"),
            r#"---
name: Unit Skill
version: 1.0.0
description: Unit test skill
---

# Unit Skill
"#,
        )
        .await
        .expect("write skill");

        let request = build_request(
            "host.skill.invoke",
            json!({
                "skill_name": "unit-skill",
                "input": {
                    "k": "v"
                }
            }),
        );

        let result = connector.execute(request).await.expect("execute");
        assert_eq!(result.status, crate::types::ExecutionStatus::Success);
        assert_eq!(result.output["found"], true);
        assert_eq!(result.output["invoked"], true);
        assert_eq!(result.output["output"]["status"], "processed");
        assert!(result.output.get("preview").is_none());
    }
}
