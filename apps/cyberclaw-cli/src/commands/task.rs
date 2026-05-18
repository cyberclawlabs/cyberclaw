//! Task 管理命令

use crate::cli_state::CliState;
use crate::output::OutputFormat;
use chrono::Utc;
use clap::{Args, Subcommand};
use cyberclaw_core::identity::ActorType;
use cyberclaw_core::ids::{ActorId, TaskId};
use cyberclaw_core::prelude::*;

#[derive(Debug, Subcommand)]
pub enum TaskCommand {
    /// 创建新任务
    Create(TaskCreateArgs),
    /// 列出所有任务
    List(TaskListArgs),
    /// 查看任务详情
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

    /// 任务类型
    #[arg(short, long, default_value = "analysis")]
    pub kind: String,

    /// 优先级
    #[arg(short, long, default_value = "medium")]
    pub priority: String,

    /// 请求者名称
    #[arg(short = 'u', long, default_value = "cli-user")]
    pub requested_by: String,
}

#[derive(Debug, Args)]
pub struct TaskListArgs {
    /// 输出格式
    #[arg(short, long, value_enum, default_value = "text")]
    pub format: OutputFormat,
}

#[derive(Debug, Args)]
pub struct TaskGetArgs {
    /// 任务 ID
    pub task_id: String,

    /// 输出格式
    #[arg(short, long, value_enum, default_value = "text")]
    pub format: OutputFormat,
}

pub async fn handle_task_command(cmd: TaskCommand, state: &CliState) -> anyhow::Result<()> {
    match cmd {
        TaskCommand::Create(args) => handle_create(args, state).await,
        TaskCommand::List(args) => handle_list(args, state).await,
        TaskCommand::Get(args) => handle_get(args, state).await,
    }
}

async fn handle_create(args: TaskCreateArgs, state: &CliState) -> anyhow::Result<()> {
    // 解析 TaskKind
    let kind = match args.kind.to_lowercase().as_str() {
        "analysis" => TaskKind::Analysis,
        "investigation" => TaskKind::Investigation,
        "review" => TaskKind::Review,
        "execution" => TaskKind::Execution,
        "reporting" => TaskKind::Reporting,
        "automation" => TaskKind::Automation,
        _ => anyhow::bail!("invalid task kind: {}. valid values: analysis, investigation, review, execution, reporting, automation", args.kind),
    };

    // 解析 Priority
    let priority = match args.priority.to_lowercase().as_str() {
        "critical" => Priority::Critical,
        "high" => Priority::High,
        "medium" => Priority::Medium,
        "low" => Priority::Low,
        _ => anyhow::bail!("invalid priority: {}", args.priority),
    };

    // 创建 Actor
    let actor = ActorRef {
        id: ActorId::from_string(args.requested_by.clone())?,
        actor_type: ActorType::Human,
        tenant_id: None,
        home_node_id: None,
        display_name: args.requested_by.clone(),
    };

    // 创建任务
    let task = Task {
        id: TaskId::new(),
        case_id: None,
        title: args.title,
        summary: args.summary,
        kind,
        priority,
        requested_by: actor,
        requested_at: Utc::now(),
        trigger: TriggerRef {
            kind: "cli".to_string(),
            source: "cyberclaw-cli".to_string(),
        },
        input: TaskInput::default(),
        desired_outputs: vec![],
        labels: vec![],
        preferred_agent_id: None,
    };

    let task_id = task.id.clone();
    state.task_manager.create_task(task).await?;

    println!("✓ Task created successfully: {}", task_id);
    Ok(())
}

async fn handle_list(args: TaskListArgs, state: &CliState) -> anyhow::Result<()> {
    let tasks = state.task_manager.list_tasks().await?;

    match args.format {
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&tasks)?;
            println!("{}", json);
        }
        OutputFormat::Text => {
            if tasks.is_empty() {
                println!("No tasks found.");
            } else {
                println!("Total tasks: {}\n", tasks.len());
                for task in tasks {
                    println!("ID:       {}", task.id);
                    println!("Title:    {}", task.title);
                    println!("Kind:     {:?}", task.kind);
                    println!("Priority: {:?}", task.priority);
                    println!("By:       {}", task.requested_by.display_name);
                    println!("At:       {}", task.requested_at);
                    println!("---");
                }
            }
        }
    }

    Ok(())
}

async fn handle_get(args: TaskGetArgs, state: &CliState) -> anyhow::Result<()> {
    let task_id = TaskId::from_string(args.task_id)?;
    let task = state
        .task_manager
        .get_task(&task_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("task not found: {}", task_id))?;

    match args.format {
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&task)?;
            println!("{}", json);
        }
        OutputFormat::Text => {
            println!("Task Details:");
            println!("  ID:         {}", task.id);
            println!("  Title:      {}", task.title);
            println!("  Summary:    {}", task.summary);
            println!("  Kind:       {:?}", task.kind);
            println!("  Priority:   {:?}", task.priority);
            println!(
                "  Requested:  {} by {}",
                task.requested_at, task.requested_by.display_name
            );
            println!(
                "  Trigger:    {} ({})",
                task.trigger.kind, task.trigger.source
            );
        }
    }

    Ok(())
}
