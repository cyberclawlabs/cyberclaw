mod cli_state;
mod commands;
mod http_client;
mod output;

use anyhow::Result;
use clap::{Parser, Subcommand};
use cli_state::CliState;

#[derive(Parser)]
#[command(name = "cyberclaw", version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// 显示平台状态
    Status,
    /// 检查系统状态（同 status）
    Inspect,
    /// 与 agent 交互对话（终端 REPL）
    Chat(commands::ChatArgs),
    /// Task 管理命令
    #[command(subcommand)]
    Task(commands::TaskCommand),
    /// Connector 管理命令
    #[command(subcommand)]
    Connector(commands::ConnectorCommand),
    /// Package 管理命令
    #[command(subcommand)]
    Package(commands::PackageCommand),
    /// Agent 运行时管理命令
    #[command(subcommand)]
    Agent(commands::AgentCommand),
    /// Skill 管理命令
    #[command(subcommand)]
    Skill(commands::SkillCommand),
    /// Capability 管理命令
    #[command(subcommand)]
    Capability(commands::CapabilityCommand),
    /// 审计日志归档与校验命令
    #[command(subcommand)]
    Audit(commands::AuditCommand),
    /// Review 审批命令（list / approve / reject）
    #[command(subcommand)]
    Review(commands::ReviewCommand),
    /// Memory CRUD（含 BT-09 tag 过滤）
    #[command(subcommand)]
    Memory(commands::MemoryCommand),
    /// Workflow 管理（含 BT-40 chain）
    #[command(subcommand)]
    Workflow(commands::WorkflowCommand),
    /// MCP server 热加载（BT-37）
    #[command(subcommand)]
    Mcp(commands::McpCommand),
    /// 交互式引导配置向导
    Onboard(commands::OnboardArgs),
    /// 系统健康检查（config / llm / users / governance / connectors / drift / server）
    Doctor(commands::DoctorArgs),
    /// 🧠 集群 Brain 节点管理（F4）
    #[command(subcommand)]
    Cluster(commands::ClusterCommand),
    /// 🔧 Tool 状态管理（active / deferred）（F7）
    #[command(subcommand)]
    Tools(commands::ToolsCommand),
    /// 📋 跨平台剪贴板读写（pbcopy / xclip / wl-copy / clip 包装）
    #[command(subcommand)]
    Clipboard(commands::ClipboardCommand),
    /// 🌐 浏览器自动化（通过 server BrowserConnector + 治理审计）
    #[command(subcommand)]
    Browser(commands::BrowserCommand),
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let state = CliState::new()?;

    match cli.command {
        Some(Commands::Status) | Some(Commands::Inspect) => {
            show_system_status(&state).await?;
        }
        Some(Commands::Chat(args)) => {
            commands::run_chat(args).await?;
        }
        Some(Commands::Task(cmd)) => {
            commands::handle_task_command(cmd, &state).await?;
        }
        Some(Commands::Connector(cmd)) => {
            commands::handle_connector_command(cmd, &state).await?;
        }
        Some(Commands::Package(cmd)) => {
            commands::handle_package_command(cmd, &state).await?;
        }
        Some(Commands::Agent(cmd)) => {
            commands::handle_agent_command(cmd, &state).await?;
        }
        Some(Commands::Skill(cmd)) => {
            commands::handle_skill_command(cmd, &state).await?;
        }
        Some(Commands::Capability(cmd)) => {
            commands::handle_capability_command(cmd, &state).await?;
        }
        Some(Commands::Audit(cmd)) => {
            commands::handle_audit_command(cmd).await?;
        }
        Some(Commands::Review(cmd)) => {
            commands::handle_review_command(cmd).await?;
        }
        Some(Commands::Memory(cmd)) => {
            commands::handle_memory_command(cmd).await?;
        }
        Some(Commands::Workflow(cmd)) => {
            commands::handle_workflow_command(cmd).await?;
        }
        Some(Commands::Mcp(cmd)) => {
            commands::handle_mcp_command(cmd).await?;
        }
        Some(Commands::Onboard(args)) => {
            commands::handle_onboard(args).await?;
        }
        Some(Commands::Doctor(args)) => {
            commands::handle_doctor(args).await?;
        }
        Some(Commands::Cluster(cmd)) => {
            commands::handle_cluster_command(cmd).await?;
        }
        Some(Commands::Tools(cmd)) => {
            commands::handle_tools_command(cmd).await?;
        }
        Some(Commands::Clipboard(cmd)) => {
            commands::handle_clipboard_command(cmd).await?;
        }
        Some(Commands::Browser(cmd)) => {
            commands::handle_browser_command(cmd).await?;
        }
        None => {
            println!("cyberclaw cli ready. Use --help for available commands.");
        }
    }

    Ok(())
}

async fn show_system_status(state: &CliState) -> Result<()> {
    use cyberclaw_core::manifests::PackageKind;

    println!("=== CyberClaw Platform Status ===\n");

    // 查询各类包的数量
    let agents = state
        .package_registry
        .list(Some(PackageKind::Agent))
        .await?;
    let skills = state
        .package_registry
        .list(Some(PackageKind::Skill))
        .await?;
    let connectors = state
        .package_registry
        .list(Some(PackageKind::Connector))
        .await?;

    // 统计 capabilities
    let all_capabilities = state.connector_registry.list_capabilities();
    let total_capabilities = all_capabilities.len();

    println!("Registered Packages:");
    println!("  Agents:      {}", agents.len());
    println!("  Skills:      {}", skills.len());
    println!("  Connectors:  {}", connectors.len());
    println!("  Capabilities: {}", total_capabilities);
    println!();

    if !agents.is_empty() {
        println!("Agent Packages:");
        for agent in agents.iter().take(5) {
            println!("  - {} (v{})", agent.id, agent.latest_version);
        }
        if agents.len() > 5 {
            println!("  ... and {} more", agents.len() - 5);
        }
        println!();
    }

    if !skills.is_empty() {
        println!("Skill Packages:");
        for skill in skills.iter().take(5) {
            println!("  - {} (v{})", skill.id, skill.latest_version);
        }
        if skills.len() > 5 {
            println!("  ... and {} more", skills.len() - 5);
        }
        println!();
    }

    if !connectors.is_empty() {
        println!("Connector Packages:");
        for connector in connectors.iter().take(5) {
            println!("  - {} (v{})", connector.id, connector.latest_version);
        }
        if connectors.len() > 5 {
            println!("  ... and {} more", connectors.len() - 5);
        }
        println!();
    }

    println!("Platform: Ready");
    println!("Use 'cyberclaw <resource> list' for detailed information.");

    Ok(())
}
