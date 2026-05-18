//! MCP server management CLI surface (BT-37 — hot-reload).
//!
//! Subcommands:
//!   - `list`        — list registered MCP servers
//!   - `register`    — add a new MCP server (HTTP transport)
//!   - `unregister`  — remove a registered MCP server
//!
//! Backed by `POST /api/v1/admin/mcp/servers` (register), `GET /…servers`
//! (list), `DELETE /…servers/:id` (unregister). Server side calls into
//! `ConnectorRegistry::register` / `unregister`.

use anyhow::Result;
use clap::{Args, Subcommand};
use serde::Deserialize;

use crate::http_client::{delete as http_delete, get_json, post_json, server_url};

#[derive(Debug, Subcommand)]
pub enum McpCommand {
    /// List registered MCP servers.
    List(ListArgs),
    /// Register a new MCP server (HTTP transport).
    Register(RegisterArgs),
    /// Unregister an MCP server by name.
    Unregister(UnregisterArgs),
    /// 🌉 Bridge an MCP server tool into the CyberClaw capability system（F6）.
    ///
    /// POST /api/v1/mcp/bridge_tool
    ///
    /// Examples:
    ///   cyberclaw mcp bridge-tool --server my-mcp --tool run_query \
    ///     --description "Execute a SQL query" --schema '{"sql":{"type":"string"}}'
    BridgeTool(BridgeToolArgs),
}

#[derive(Debug, Args)]
pub struct ListArgs {
    #[arg(long)]
    pub server: Option<String>,
}

#[derive(Debug, Args)]
pub struct RegisterArgs {
    /// Logical name for the MCP server (becomes connector id `mcp-<name>`).
    #[arg(long)]
    pub name: String,
    /// HTTP endpoint of the MCP server, e.g. `http://localhost:9001/mcp`.
    #[arg(long)]
    pub url: String,
    /// Optional timeout in seconds (default 30).
    #[arg(long, default_value_t = 30)]
    pub timeout_secs: u64,
    #[arg(long)]
    pub server: Option<String>,
}

#[derive(Debug, Args)]
pub struct UnregisterArgs {
    /// MCP server name (the part after `mcp-` in the connector id).
    pub name: String,
    #[arg(long)]
    pub server: Option<String>,
}

#[derive(Debug, Args)]
pub struct BridgeToolArgs {
    /// MCP server name (logical name used during registration).
    #[arg(long)]
    pub server_name: String,
    /// Tool name as exposed by the MCP server.
    #[arg(long)]
    pub tool: String,
    /// Human-readable description of what the tool does.
    #[arg(long)]
    pub description: String,
    /// JSON Schema string for the tool's input parameters (e.g. '{"sql":{"type":"string"}}').
    #[arg(long)]
    pub schema: String,
    /// CyberClaw server URL
    #[arg(long)]
    pub server: Option<String>,
}

#[derive(Debug, Deserialize)]
struct McpServerRow {
    name: String,
    #[serde(default)]
    connector_id: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    capabilities: u32,
}

pub async fn handle_mcp_command(cmd: McpCommand) -> Result<()> {
    match cmd {
        McpCommand::List(args) => list(args).await,
        McpCommand::Register(args) => register(args).await,
        McpCommand::Unregister(args) => unregister(args).await,
        McpCommand::BridgeTool(args) => bridge_tool(args).await,
    }
}

async fn list(args: ListArgs) -> Result<()> {
    let server = server_url(args.server.as_deref());
    let rows: Vec<McpServerRow> = get_json(&server, "/api/v1/admin/mcp/servers").await?;
    if rows.is_empty() {
        println!("No MCP servers registered");
        return Ok(());
    }
    println!("{:<20}  {:<24}  {:<7}  url", "name", "connector_id", "caps");
    for r in &rows {
        println!(
            "{:<20}  {:<24}  {:<7}  {}",
            r.name, r.connector_id, r.capabilities, r.url
        );
    }
    println!("\n{} server(s)", rows.len());
    Ok(())
}

async fn register(args: RegisterArgs) -> Result<()> {
    let server = server_url(args.server.as_deref());
    let body = serde_json::json!({
        "name": args.name,
        "transport": { "type": "http", "url": args.url, "headers": {} },
        "timeout_secs": args.timeout_secs,
    });
    let resp: serde_json::Value = post_json(&server, "/api/v1/admin/mcp/servers", &body).await?;
    println!(
        "✓ MCP server registered: connector_id={}",
        resp.get("connector_id")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
    );
    Ok(())
}

async fn unregister(args: UnregisterArgs) -> Result<()> {
    let server = server_url(args.server.as_deref());
    let path = format!("/api/v1/admin/mcp/servers/{}", args.name);
    http_delete(&server, &path).await?;
    println!("✓ MCP server unregistered: {}", args.name);
    Ok(())
}

async fn bridge_tool(args: BridgeToolArgs) -> Result<()> {
    let server = server_url(args.server.as_deref());

    // Parse --schema as JSON; provide a helpful error if it's malformed
    let schema_value: serde_json::Value = serde_json::from_str(&args.schema).map_err(|e| {
        anyhow::anyhow!(
            "❌ --schema is not valid JSON: {} (got: {})",
            e,
            &args.schema
        )
    })?;

    let body = serde_json::json!({
        "server_name": args.server_name,
        "tool_name":   args.tool,
        "description": args.description,
        "schema":      schema_value,
    });

    let resp: serde_json::Value = post_json(&server, "/api/v1/mcp/bridge_tool", &body)
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "❌ bridge-tool failed (server={}, tool={}): {}",
                args.server_name,
                args.tool,
                e
            )
        })?;

    println!(
        "🌉 Tool bridged: \x1b[36m{}\x1b[0m/\x1b[36m{}\x1b[0m → capability_id={}",
        args.server_name,
        args.tool,
        resp.get("capability_id")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct TestCli {
        #[command(subcommand)]
        cmd: McpCommand,
    }

    #[test]
    fn test_bridge_tool_args_parse() {
        let cli = TestCli::parse_from([
            "test",
            "bridge-tool",
            "--server-name",
            "my-mcp",
            "--tool",
            "run_query",
            "--description",
            "Execute SQL",
            "--schema",
            r#"{"sql":{"type":"string"}}"#,
        ]);
        match cli.cmd {
            McpCommand::BridgeTool(args) => {
                assert_eq!(args.server_name, "my-mcp");
                assert_eq!(args.tool, "run_query");
                assert_eq!(args.description, "Execute SQL");
                assert!(args.schema.contains("sql"));
            }
            _ => panic!("expected BridgeTool"),
        }
    }
}
