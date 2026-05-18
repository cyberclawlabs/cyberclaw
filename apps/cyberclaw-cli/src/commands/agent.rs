//! Agent 运行时管理命令

use crate::cli_state::CliState;
use crate::http_client::{post_json, server_url};
use crate::output::{print_field, print_separator, OutputFormat};
use anyhow::Context;
use chrono::Utc;
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Subcommand)]
pub enum AgentCommand {
    /// 列出所有运行中的 Agent 实例
    List(AgentListArgs),
    /// 启动一个 Agent 实例
    Run(AgentRunArgs),
    /// 停止一个 Agent 实例
    Stop(AgentStopArgs),
    /// 🤝 委托任务给子 Agent（F5）
    ///
    /// POST /api/v1/agents/delegate
    ///
    /// Examples:
    ///   cyberclaw agent delegate --task "Generate a Rust crate README"
    ///   cyberclaw agent delegate --task "Audit security" --max-iterations 16 --parent agent-root
    Delegate(AgentDelegateArgs),
}

#[derive(Debug, Args)]
pub struct AgentListArgs {
    /// 输出格式
    #[arg(short, long, value_enum, default_value = "text")]
    pub format: OutputFormat,
}

#[derive(Debug, Args)]
pub struct AgentRunArgs {
    /// Agent 包 ID
    pub agent_id: String,

    /// 可选的实例标签
    #[arg(short, long)]
    pub label: Option<String>,
}

#[derive(Debug, Args)]
pub struct AgentStopArgs {
    /// 运行中的 Agent 实例 ID
    pub instance_id: String,
}

#[derive(Debug, Args)]
pub struct AgentDelegateArgs {
    /// 委托给子 Agent 的任务描述
    #[arg(long)]
    pub task: String,
    /// 最大迭代次数
    #[arg(long, default_value_t = 8)]
    pub max_iterations: u32,
    /// 父 Agent ID（可选）
    #[arg(long)]
    pub parent: Option<String>,
    /// CyberClaw server URL
    #[arg(long)]
    pub server: Option<String>,
}

/// Agent 实例的展示数据（用于列表输出）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInstanceRecord {
    pub instance_id: String,
    pub agent_id: String,
    pub status: String,
    pub started_at: String,
    pub label: Option<String>,
}

pub async fn handle_agent_command(cmd: AgentCommand, state: &CliState) -> anyhow::Result<()> {
    match cmd {
        AgentCommand::List(args) => handle_list(args, state).await,
        AgentCommand::Run(args) => handle_run(args, state).await,
        AgentCommand::Stop(args) => handle_stop(args, state).await,
        AgentCommand::Delegate(args) => handle_delegate(args).await,
    }
}

async fn handle_delegate(args: AgentDelegateArgs) -> anyhow::Result<()> {
    let server = server_url(args.server.as_deref());
    let mut body = serde_json::json!({
        "task": args.task,
        "max_iterations": args.max_iterations,
    });
    if let Some(parent) = &args.parent {
        body["parent_agent_id"] = serde_json::Value::String(parent.clone());
    }
    let resp: serde_json::Value = post_json(&server, "/api/v1/agents/delegate", &body)
        .await
        .map_err(|e| {
            let msg = e.to_string();
            let hint = if msg.contains("401") || msg.contains("403") {
                " (check CYBERCLAW_TOKEN or run `cyberclaw chat`)"
            } else if msg.contains("connection refused") || msg.contains("connect error") {
                " (is the server running? check CYBERCLAW_SERVER)"
            } else {
                ""
            };
            anyhow::anyhow!("❌ Delegation failed: {}{}", msg, hint)
        })?;

    println!(
        "🤝 Task delegated: session=\x1b[36m{}\x1b[0m agent=\x1b[36m{}\x1b[0m",
        resp.get("session_id")
            .and_then(|v| v.as_str())
            .unwrap_or("?"),
        resp.get("agent_id").and_then(|v| v.as_str()).unwrap_or("?")
    );
    Ok(())
}

async fn handle_list(args: AgentListArgs, state: &CliState) -> anyhow::Result<()> {
    let _ = state; // list 只读取本地实例状态文件
    let instances = load_agent_instances()?;
    let running: Vec<AgentInstanceRecord> = instances
        .into_iter()
        .filter(|instance| instance.status == "running")
        .collect();

    match args.format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&running)?);
        }
        OutputFormat::Text => {
            if running.is_empty() {
                println!("No running agent instances.");
            } else {
                println!("Total running agents: {}\n", running.len());
                for instance in running {
                    print_field("Instance", &instance.instance_id);
                    print_field("Agent", &instance.agent_id);
                    print_field("Status", &instance.status);
                    print_field("StartedAt", &instance.started_at);
                    if let Some(label) = &instance.label {
                        print_field("Label", label);
                    }
                    print_separator();
                }
            }
        }
    }

    Ok(())
}

async fn handle_run(args: AgentRunArgs, state: &CliState) -> anyhow::Result<()> {
    use cyberclaw_core::manifests::PackageKind;
    let exists = state
        .package_registry
        .get(PackageKind::Agent, &args.agent_id)
        .await?
        .is_some();
    if !exists {
        anyhow::bail!(
            "agent package not found in registry: {} (run `cyberclaw package install` first)",
            args.agent_id
        );
    }

    let mut instances = load_agent_instances()?;
    let instance_id = format!(
        "{}-{}",
        sanitize_identifier(&args.agent_id),
        Utc::now().timestamp_millis()
    );
    let record = AgentInstanceRecord {
        instance_id: instance_id.clone(),
        agent_id: args.agent_id.clone(),
        status: "running".to_string(),
        started_at: Utc::now().to_rfc3339(),
        label: args.label.clone(),
    };
    instances.push(record);
    save_agent_instances(&instances)?;

    println!("Agent instance started.");
    println!("  agent_id: {}", args.agent_id);
    println!("  instance_id: {}", instance_id);
    if let Some(label) = args.label {
        println!("  label: {}", label);
    }
    Ok(())
}

async fn handle_stop(args: AgentStopArgs, _state: &CliState) -> anyhow::Result<()> {
    let mut instances = load_agent_instances()?;
    let mut found = false;
    for instance in &mut instances {
        if instance.instance_id == args.instance_id && instance.status == "running" {
            instance.status = "stopped".to_string();
            found = true;
            break;
        }
    }
    if !found {
        anyhow::bail!("running agent instance not found: {}", args.instance_id);
    }
    save_agent_instances(&instances)?;
    println!("Agent instance stopped: {}", args.instance_id);
    Ok(())
}

fn sanitize_identifier(raw: &str) -> String {
    raw.chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect()
}

fn agent_state_file() -> anyhow::Result<PathBuf> {
    if let Ok(path) = std::env::var("CYBERCLAW_AGENT_STATE_FILE") {
        return Ok(PathBuf::from(path));
    }
    Ok(std::env::current_dir()?.join(".cyberclaw/agent_instances.json"))
}

fn load_agent_instances() -> anyhow::Result<Vec<AgentInstanceRecord>> {
    let path = agent_state_file()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(&path)
        .with_context(|| format!("failed to read agent state file {}", path.display()))?;
    if content.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(&content)
        .with_context(|| format!("failed to parse agent state file {}", path.display()))
}

fn save_agent_instances(instances: &[AgentInstanceRecord]) -> anyhow::Result<()> {
    let path = agent_state_file()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
    }
    let content = serde_json::to_string_pretty(instances)?;
    fs::write(&path, content)
        .with_context(|| format!("failed to write agent state file {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli_state::CliState;
    use cyberclaw_control_plane::{PackageRecord, PackageSource, RegistryState};

    fn repo_ecosystem_path(segment: &str) -> String {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../ecosystem")
            .join(segment)
            .to_string_lossy()
            .into_owned()
    }

    #[tokio::test]
    async fn test_agent_run_list_stop_lifecycle() {
        let state_file = std::env::temp_dir().join(format!(
            "cyberclaw-agent-instances-{}.json",
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        unsafe {
            std::env::set_var(
                "CYBERCLAW_AGENT_STATE_FILE",
                state_file.to_string_lossy().to_string(),
            )
        };

        let state = CliState::new().expect("state");
        let loaded = crate::commands::package::resolve_package_reference(&repo_ecosystem_path(
            "agents/master-agent",
        ))
        .await
        .expect("resolve agent package");
        let record = PackageRecord {
            kind: loaded.manifest.kind.clone(),
            id: loaded.manifest.id.clone(),
            latest_version: loaded.manifest.version.clone(),
            installed_versions: vec![loaded.manifest.version.clone()],
            active_version: Some(loaded.manifest.version.clone()),
            source: PackageSource::LocalPath(repo_ecosystem_path("agents/master-agent")),
            state: RegistryState::Active,
            available_nodes: vec![],
            runtime_requirements: loaded.manifest.compatibility.runtime.clone(),
            manifest: loaded.manifest.clone(),
        };
        state.package_registry.upsert(record).await.expect("upsert");

        handle_run(
            AgentRunArgs {
                agent_id: loaded.manifest.id.clone(),
                label: Some("test".to_string()),
            },
            &state,
        )
        .await
        .expect("run");

        let all_instances = load_agent_instances().expect("load instances");
        assert_eq!(all_instances.len(), 1);
        assert_eq!(all_instances[0].status, "running");

        handle_stop(
            AgentStopArgs {
                instance_id: all_instances[0].instance_id.clone(),
            },
            &state,
        )
        .await
        .expect("stop");

        let final_instances = load_agent_instances().expect("load instances after stop");
        assert_eq!(final_instances.len(), 1);
        assert_eq!(final_instances[0].status, "stopped");

        unsafe { std::env::remove_var("CYBERCLAW_AGENT_STATE_FILE") };
        let _ = std::fs::remove_file(&state_file);
    }

    #[test]
    fn test_delegate_args_parse() {
        use clap::Parser;

        #[derive(Parser)]
        struct TestCli {
            #[command(subcommand)]
            cmd: AgentCommand,
        }

        let cli = TestCli::parse_from([
            "test",
            "delegate",
            "--task",
            "Generate a README",
            "--max-iterations",
            "16",
            "--parent",
            "agent-root",
        ]);
        match cli.cmd {
            AgentCommand::Delegate(args) => {
                assert_eq!(args.task, "Generate a README");
                assert_eq!(args.max_iterations, 16);
                assert_eq!(args.parent.as_deref(), Some("agent-root"));
            }
            _ => panic!("expected Delegate"),
        }
    }

    #[test]
    fn test_delegate_args_defaults() {
        use clap::Parser;

        #[derive(Parser)]
        struct TestCli {
            #[command(subcommand)]
            cmd: AgentCommand,
        }

        let cli = TestCli::parse_from(["test", "delegate", "--task", "do something"]);
        match cli.cmd {
            AgentCommand::Delegate(args) => {
                assert_eq!(args.max_iterations, 8);
                assert!(args.parent.is_none());
            }
            _ => panic!("expected Delegate"),
        }
    }
}
