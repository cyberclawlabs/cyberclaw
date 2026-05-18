//! Capability 管理命令

use crate::cli_state::CliState;
use crate::http_client::{post_json, server_url};
use crate::output::OutputFormat;
use clap::{Args, Subcommand};

#[derive(Debug, Subcommand)]
pub enum CapabilityCommand {
    /// 列出所有可用的 Capabilities
    List(CapabilityListArgs),
    /// 根据目标自动发现匹配的 Capabilities（F3）
    DiscoverForGoal(DiscoverForGoalArgs),
}

#[derive(Debug, Args)]
pub struct CapabilityListArgs {
    /// 输出格式
    #[arg(short, long, value_enum, default_value = "text")]
    pub format: OutputFormat,
}

#[derive(Debug, Args)]
pub struct DiscoverForGoalArgs {
    /// 目标类型（如 deliverable / analysis / automation）
    #[arg(long)]
    pub kind: String,
    /// 附加搜索关键词（逗号分隔）
    #[arg(long, value_delimiter = ',')]
    pub terms: Vec<String>,
    /// 同时包含远程 Capabilities
    #[arg(long, default_value_t = false)]
    pub include_remote: bool,
    /// CyberClaw server URL
    #[arg(long)]
    pub server: Option<String>,
}

pub async fn handle_capability_command(
    cmd: CapabilityCommand,
    state: &CliState,
) -> anyhow::Result<()> {
    match cmd {
        CapabilityCommand::List(args) => handle_list(args, state).await,
        CapabilityCommand::DiscoverForGoal(args) => discover_for_goal(args).await,
    }
}

async fn discover_for_goal(args: DiscoverForGoalArgs) -> anyhow::Result<()> {
    let server = server_url(args.server.as_deref());
    let body = serde_json::json!({
        "kind": args.kind,
        "terms": args.terms,
        "include_remote": args.include_remote,
    });
    let resp: serde_json::Value =
        post_json(&server, "/api/v1/capabilities/discover_for_goal", &body)
            .await
            .map_err(|e| {
                anyhow::anyhow!("❌ discover-for-goal failed: {}", friendly_cap_error(&e))
            })?;

    // Try to render as table; fall back to JSON
    if let Some(caps) = resp.get("capabilities").and_then(|v| v.as_array()) {
        if caps.is_empty() {
            println!(
                "🔍 No matching capabilities found for kind='{}'.",
                args.kind
            );
        } else {
            println!(
                "🔍 Found {} capability/capabilities for kind='{}':\n",
                caps.len(),
                args.kind
            );
            for cap in caps {
                let name = cap.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                let connector = cap.get("connector").and_then(|v| v.as_str()).unwrap_or("?");
                let desc = cap
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                println!(
                    "  \x1b[36m{}\x1b[0m  (connector: {})  {}",
                    name, connector, desc
                );
            }
        }
    } else {
        println!("{}", serde_json::to_string_pretty(&resp)?);
    }
    Ok(())
}

fn friendly_cap_error(e: &anyhow::Error) -> String {
    let msg = e.to_string();
    if msg.contains("401") || msg.contains("403") {
        format!("{} (check CYBERCLAW_TOKEN or run `cyberclaw chat`)", msg)
    } else if msg.contains("connection refused") || msg.contains("connect error") {
        format!("{} (is the server running? check CYBERCLAW_SERVER)", msg)
    } else {
        msg
    }
}

async fn handle_list(args: CapabilityListArgs, state: &CliState) -> anyhow::Result<()> {
    // 从 connector_registry 获取所有 capabilities
    let capabilities = state.connector_registry.list_capabilities();

    match args.format {
        OutputFormat::Json => {
            // 简单的 JSON 输出
            let json_data: Vec<_> = capabilities
                .iter()
                .map(|(connector, cap)| {
                    serde_json::json!({
                        "connector": connector,
                        "capability": cap
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&json_data)?);
        }
        OutputFormat::Text => {
            if capabilities.is_empty() {
                println!("No capabilities available.");
                println!("  Register connectors first using 'cyberclaw connector register'");
            } else {
                println!("Total capabilities: {}\n", capabilities.len());
                for (connector, cap) in capabilities {
                    println!("Connector: {}", connector);
                    println!("Capability: {}", cap);
                    println!("---");
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct TestCli {
        #[command(subcommand)]
        cmd: CapabilityCommand,
    }

    #[test]
    fn test_discover_for_goal_args_parse() {
        let cli = TestCli::parse_from([
            "test",
            "discover-for-goal",
            "--kind",
            "deliverable",
            "--terms",
            "rust,build",
            "--include-remote",
        ]);
        match cli.cmd {
            CapabilityCommand::DiscoverForGoal(args) => {
                assert_eq!(args.kind, "deliverable");
                assert_eq!(args.terms, vec!["rust", "build"]);
                assert!(args.include_remote);
            }
            _ => panic!("expected DiscoverForGoal"),
        }
    }

    #[test]
    fn test_discover_for_goal_defaults() {
        let cli = TestCli::parse_from(["test", "discover-for-goal", "--kind", "analysis"]);
        match cli.cmd {
            CapabilityCommand::DiscoverForGoal(args) => {
                assert_eq!(args.kind, "analysis");
                assert!(args.terms.is_empty());
                assert!(!args.include_remote);
            }
            _ => panic!("expected DiscoverForGoal"),
        }
    }
}
