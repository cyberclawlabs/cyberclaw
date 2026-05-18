//! Workflow CLI surface (BT-40 — task chain config).
//!
//! Subcommands:
//!   - `chain`   — create a sequential task chain (A → B → C)
//!   - `status`  — fetch workflow instance status

use anyhow::{anyhow, Result};
use clap::{Args, Subcommand};
use serde::Deserialize;

use crate::http_client::{get_json, post_json, server_url};

#[derive(Debug, Subcommand)]
pub enum WorkflowCommand {
    /// Create a sequential task chain (BT-40).
    /// Each task automatically depends on the previous task's completion.
    Chain(ChainArgs),
    /// Get workflow instance status.
    Status(StatusArgs),
}

#[derive(Debug, Args)]
pub struct ChainArgs {
    /// Workflow name.
    #[arg(long)]
    pub name: String,
    /// Workflow version (default 1.0.0).
    #[arg(long, default_value = "1.0.0")]
    pub version: String,
    /// Tasks in chain order, format: `id:name` (id is unique within chain).
    /// Use `--task` repeatedly: `--task gen:Generate --task test:Run`.
    #[arg(long = "task", required = true)]
    pub tasks: Vec<String>,
    #[arg(long)]
    pub server: Option<String>,
}

#[derive(Debug, Args)]
pub struct StatusArgs {
    /// Workflow instance id.
    pub instance_id: String,
    #[arg(long)]
    pub server: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChainResponse {
    success: bool,
    workflow_id: String,
    instance_id: String,
}

#[derive(Debug, Deserialize)]
struct StatusResponse {
    instance_id: String,
    workflow_id: String,
    status: String,
    #[serde(default)]
    current_step: Option<String>,
}

pub async fn handle_workflow_command(cmd: WorkflowCommand) -> Result<()> {
    match cmd {
        WorkflowCommand::Chain(args) => chain(args).await,
        WorkflowCommand::Status(args) => status(args).await,
    }
}

async fn chain(args: ChainArgs) -> Result<()> {
    let server = server_url(args.server.as_deref());
    let mut tasks = Vec::with_capacity(args.tasks.len());
    for spec in &args.tasks {
        let (id, name) = spec
            .split_once(':')
            .ok_or_else(|| anyhow!("task spec must be 'id:name', got '{}'", spec))?;
        tasks.push(serde_json::json!({
            "id": id.trim(),
            "name": name.trim(),
        }));
    }
    let body = serde_json::json!({
        "name": args.name,
        "version": args.version,
        "tasks": tasks,
    });
    let resp: ChainResponse = post_json(&server, "/api/v1/admin/workflows/chain", &body).await?;
    println!(
        "✓ Chain registered: workflow_id={}, instance_id={}",
        resp.workflow_id, resp.instance_id
    );
    println!(
        "  {} task(s): {}",
        args.tasks.len(),
        args.tasks
            .iter()
            .map(|s| s.split(':').next().unwrap_or(s))
            .collect::<Vec<_>>()
            .join(" → ")
    );
    if !resp.success {
        anyhow::bail!("server reported failure");
    }
    Ok(())
}

async fn status(args: StatusArgs) -> Result<()> {
    let server = server_url(args.server.as_deref());
    let path = format!("/api/v1/workflows/{}", args.instance_id);
    let resp: StatusResponse = get_json(&server, &path).await?;
    println!("workflow_id:   {}", resp.workflow_id);
    println!("instance_id:   {}", resp.instance_id);
    println!("status:        {}", resp.status);
    if let Some(step) = resp.current_step {
        println!("current_step:  {}", step);
    }
    Ok(())
}
