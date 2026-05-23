//! Task management commands.
//!
//! All subcommands talk to the server (`/api/v1/tasks*`) — there is no
//! local task registry. The previous in-process `TaskManager` only ever
//! held data for a single CLI invocation (`task create` wrote to memory,
//! `task list` in a fresh process saw an empty registry); that
//! dead-end pattern was removed alongside the package-registry refactor.

use crate::cli_state::CliState;
use crate::http_client::{get_json, post_json, server_url};
use crate::output::OutputFormat;
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};

#[derive(Debug, Subcommand)]
pub enum TaskCommand {
    /// 创建新任务（POST /api/v1/tasks）
    Create(TaskCreateArgs),
    /// 列出任务（GET /api/v1/tasks）
    List(TaskListArgs),
    /// 查看任务详情（GET /api/v1/tasks/:id）
    Get(TaskGetArgs),
}

#[derive(Debug, Args)]
pub struct TaskCreateArgs {
    /// 任务标题
    #[arg(short, long)]
    pub title: String,
    /// 任务摘要
    #[arg(short, long)]
    pub summary: String,
    /// 任务类型（analysis / investigation / review / execution / reporting / automation / custom:<name>）
    #[arg(short, long, default_value = "analysis")]
    pub kind: String,
    /// 优先级（critical / high / medium / low）
    #[arg(short, long, default_value = "medium")]
    pub priority: String,
    /// 标签（逗号分隔，可重复 --label）
    #[arg(long, value_delimiter = ',')]
    pub label: Vec<String>,
}

#[derive(Debug, Args)]
pub struct TaskListArgs {
    /// 过滤状态：pending / running / completed / failed / cancelled
    #[arg(long)]
    pub status: Option<String>,
    /// 输出格式
    #[arg(short, long, value_enum, default_value = "text")]
    pub format: OutputFormat,
}

#[derive(Debug, Args)]
pub struct TaskGetArgs {
    /// 任务 ID（task-<uuid>）
    pub task_id: String,
    /// 输出格式
    #[arg(short, long, value_enum, default_value = "text")]
    pub format: OutputFormat,
}

#[derive(Debug, Serialize)]
struct CreateTaskBody {
    title: String,
    summary: String,
    kind: serde_json::Value,
    priority: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    labels: Vec<String>,
}

/// Server's TaskResponse is rich (admin/audit fields), but the CLI only
/// needs the visible columns. Borrow what we use and let serde drop the
/// rest. Falling back to `serde_json::Value` keeps the contract surface
/// minimal so a TaskResponse field rename on the server doesn't break the
/// CLI build.
#[derive(Debug, Deserialize)]
struct TaskSummaryView {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    kind: Option<serde_json::Value>,
    #[serde(default)]
    priority: Option<serde_json::Value>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    requested_by: Option<serde_json::Value>,
}

pub async fn handle_task_command(cmd: TaskCommand, _state: &CliState) -> anyhow::Result<()> {
    match cmd {
        TaskCommand::Create(args) => handle_create(args).await,
        TaskCommand::List(args) => handle_list(args).await,
        TaskCommand::Get(args) => handle_get(args).await,
    }
}

fn friendly_task_error(e: &anyhow::Error) -> String {
    let msg = e.to_string();
    if msg.contains("401") || msg.contains("403") {
        format!(
            "{} (token expired? mint a fresh one via `curl -s -X POST \
             $CYBERCLAW_SERVER/admin/login -H 'Content-Type: application/json' \
             -d '{{\"user_id\":\"qa-admin\"}}' | jq -r .jwt > ~/.cyberclaw/cli-token`)",
            msg
        )
    } else if msg.contains("connection refused") || msg.contains("connect error") {
        format!("{} (is the server running? check CYBERCLAW_SERVER)", msg)
    } else {
        msg
    }
}

fn parse_kind_for_server(raw: &str) -> serde_json::Value {
    // Server's TaskKind enum is serde-tagged; "custom" uses {"Custom": "<name>"},
    // canonical kinds are bare strings (e.g. "Analysis"). Accept lowercase
    // input and map to the canonical capitalised form the server expects.
    let lower = raw.to_lowercase();
    if let Some(custom) = lower.strip_prefix("custom:") {
        serde_json::json!({ "Custom": custom })
    } else {
        let capitalised = match lower.as_str() {
            "analysis" => "Analysis",
            "investigation" => "Investigation",
            "review" => "Review",
            "execution" => "Execution",
            "reporting" => "Reporting",
            "automation" => "Automation",
            other => return serde_json::json!({ "Custom": other }),
        };
        serde_json::Value::String(capitalised.to_string())
    }
}

fn parse_priority_for_server(raw: &str) -> anyhow::Result<&'static str> {
    // Server's Priority enum is `#[serde(rename_all = "lowercase")]` —
    // wire format is lowercase regardless of how it's printed in TaskKind.
    // Keep this function aligned with the enum's rename rule, not Rust casing.
    Ok(match raw.to_lowercase().as_str() {
        "critical" => "critical",
        "high" => "high",
        "medium" => "medium",
        "low" => "low",
        other => anyhow::bail!("invalid priority '{}': use critical|high|medium|low", other),
    })
}

async fn handle_create(args: TaskCreateArgs) -> anyhow::Result<()> {
    let server = server_url(None);
    let body = CreateTaskBody {
        title: args.title,
        summary: args.summary,
        kind: parse_kind_for_server(&args.kind),
        priority: parse_priority_for_server(&args.priority)?.to_string(),
        labels: args.label,
    };

    let resp: TaskSummaryView = post_json(&server, "/api/v1/tasks", &body)
        .await
        .map_err(|e| anyhow::anyhow!("❌ task create failed: {}", friendly_task_error(&e)))?;

    println!("✓ Task created: {}", resp.id);
    if let Some(title) = resp.title {
        println!("  title:    {}", title);
    }
    if let Some(status) = resp.status {
        println!("  status:   {}", status);
    }
    Ok(())
}

async fn handle_list(args: TaskListArgs) -> anyhow::Result<()> {
    let server = server_url(None);
    let path = match args.status.as_deref() {
        // Status is a tiny enum (pending/running/completed/failed/cancelled),
        // so a percent-encoder is overkill — reject anything outside the
        // ASCII safe set explicitly.
        Some(s) if !s.is_empty() => {
            if s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
                format!("/api/v1/tasks?status={}", s)
            } else {
                anyhow::bail!(
                    "invalid --status value '{}': must be ascii alphanumeric / dash / underscore",
                    s
                );
            }
        }
        _ => "/api/v1/tasks".to_string(),
    };

    let resp: serde_json::Value = get_json(&server, &path)
        .await
        .map_err(|e| anyhow::anyhow!("❌ task list failed: {}", friendly_task_error(&e)))?;

    // The server returns either a {"tasks":[...]} object or a bare array
    // depending on filter/fallback path; normalise to a list of views.
    let raw_list: Vec<serde_json::Value> = match &resp {
        serde_json::Value::Array(arr) => arr.clone(),
        serde_json::Value::Object(obj) => obj
            .get("tasks")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default(),
        _ => Vec::new(),
    };

    let tasks: Vec<TaskSummaryView> = raw_list
        .iter()
        .filter_map(|v| serde_json::from_value::<TaskSummaryView>(v.clone()).ok())
        .collect();

    match args.format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&raw_list)?);
        }
        OutputFormat::Text => {
            if tasks.is_empty() {
                println!("No tasks found.");
            } else {
                println!("Total tasks: {}\n", tasks.len());
                for task in tasks {
                    println!("ID:       {}", task.id);
                    if let Some(title) = task.title {
                        println!("Title:    {}", title);
                    }
                    if let Some(k) = task.kind {
                        println!("Kind:     {}", short_value(&k));
                    }
                    if let Some(p) = task.priority {
                        println!("Priority: {}", short_value(&p));
                    }
                    if let Some(status) = task.status {
                        println!("Status:   {}", status);
                    }
                    if let Some(created) = task.created_at {
                        println!("Created:  {}", created);
                    }
                    if let Some(requester) = task.requested_by {
                        println!("By:       {}", short_value(&requester));
                    }
                    println!("---");
                }
            }
        }
    }

    Ok(())
}

async fn handle_get(args: TaskGetArgs) -> anyhow::Result<()> {
    let server = server_url(None);
    // Task IDs are server-minted UUID v4 (`task-<uuid>`) — safe to splice
    // directly. Reject anything else with a clear message before the round-trip.
    if !args
        .task_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        anyhow::bail!(
            "invalid task id '{}': must be ascii alphanumeric / dash / underscore",
            args.task_id
        );
    }
    let path = format!("/api/v1/tasks/{}", args.task_id);
    let resp: serde_json::Value = get_json(&server, &path)
        .await
        .map_err(|e| anyhow::anyhow!("❌ task get failed: {}", friendly_task_error(&e)))?;

    match args.format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&resp)?);
        }
        OutputFormat::Text => {
            let view: TaskSummaryView = serde_json::from_value(resp.clone()).unwrap_or_else(|_| {
                TaskSummaryView {
                    id: args.task_id.clone(),
                    title: None,
                    summary: None,
                    kind: None,
                    priority: None,
                    status: None,
                    created_at: None,
                    requested_by: None,
                }
            });
            println!("Task Details:");
            println!("  ID:        {}", view.id);
            if let Some(title) = view.title {
                println!("  Title:     {}", title);
            }
            if let Some(summary) = view.summary {
                println!("  Summary:   {}", summary);
            }
            if let Some(k) = view.kind {
                println!("  Kind:      {}", short_value(&k));
            }
            if let Some(p) = view.priority {
                println!("  Priority:  {}", short_value(&p));
            }
            if let Some(status) = view.status {
                println!("  Status:    {}", status);
            }
            if let Some(created) = view.created_at {
                println!("  Created:   {}", created);
            }
            if let Some(requester) = view.requested_by {
                println!("  Requested: {}", short_value(&requester));
            }
        }
    }
    Ok(())
}

/// Render a JSON value as a short single-line label suitable for column display.
/// Strings: as-is. Enum-like objects {"Custom":"foo"}: render as `Custom(foo)`.
/// ActorRef-shaped objects (multi-field with `display_name`): render the
/// display_name verbatim, falling back to `id` if missing. Anything else:
/// compact JSON.
fn short_value(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(map) if map.len() == 1 => {
            let (k, inner) = map.iter().next().expect("map.len() == 1");
            match inner {
                serde_json::Value::String(s) => format!("{}({})", k, s),
                _ => serde_json::to_string(v).unwrap_or_else(|_| String::new()),
            }
        }
        serde_json::Value::Object(map) => {
            // ActorRef-like: prefer display_name, fall back to id, then JSON dump.
            if let Some(name) = map.get("display_name").and_then(|x| x.as_str()) {
                return name.to_string();
            }
            if let Some(id) = map.get("id").and_then(|x| x.as_str()) {
                return id.to_string();
            }
            serde_json::to_string(v).unwrap_or_else(|_| String::new())
        }
        _ => serde_json::to_string(v).unwrap_or_else(|_| String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_kind_canonical() {
        assert_eq!(parse_kind_for_server("analysis"), serde_json::json!("Analysis"));
        assert_eq!(parse_kind_for_server("review"), serde_json::json!("Review"));
    }

    #[test]
    fn parse_kind_custom_explicit() {
        assert_eq!(
            parse_kind_for_server("custom:bug-fix"),
            serde_json::json!({ "Custom": "bug-fix" })
        );
    }

    #[test]
    fn parse_kind_unknown_falls_back_to_custom() {
        assert_eq!(
            parse_kind_for_server("compliance-audit"),
            serde_json::json!({ "Custom": "compliance-audit" })
        );
    }

    #[test]
    fn parse_priority_canonical() {
        assert_eq!(parse_priority_for_server("medium").unwrap(), "medium");
        assert_eq!(parse_priority_for_server("CRITICAL").unwrap(), "critical");
    }

    #[test]
    fn parse_priority_invalid_rejects() {
        assert!(parse_priority_for_server("urgent").is_err());
    }

    #[test]
    fn short_value_renders_string_bareword() {
        assert_eq!(short_value(&serde_json::json!("Medium")), "Medium");
    }

    #[test]
    fn short_value_renders_single_key_object_as_paren() {
        assert_eq!(
            short_value(&serde_json::json!({ "Custom": "bug-fix" })),
            "Custom(bug-fix)"
        );
    }
}
