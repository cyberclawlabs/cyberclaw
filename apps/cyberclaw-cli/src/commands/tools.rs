//! 🔧 Tool 状态管理命令（F7）
//!
//! Manage the DeferredToolRegistry — promote / demote tools between active
//! and deferred states.
//!
//! Examples:
//!   cyberclaw tools state                      # show active + deferred (table)
//!   cyberclaw tools state --format json        # JSON for piping
//!   cyberclaw tools promote bash               # move bash → active
//!   cyberclaw tools demote read_file           # move read_file → deferred

use anyhow::Result;
use clap::{Args, Subcommand, ValueEnum};

use crate::http_client::{get_json, post_json, server_url};

// ─── ANSI colour helpers ──────────────────────────────────────────────────────

const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const BOLD: &str = "\x1b[1m";
const RESET: &str = "\x1b[0m";

fn green(s: &str) -> String {
    format!("{}{}{}", GREEN, s, RESET)
}
fn yellow(s: &str) -> String {
    format!("{}{}{}", YELLOW, s, RESET)
}
fn bold(s: &str) -> String {
    format!("{}{}{}", BOLD, s, RESET)
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::new();
    let mut in_escape = false;
    for ch in s.chars() {
        if ch == '\x1b' {
            in_escape = true;
        } else if in_escape && ch == 'm' {
            in_escape = false;
        } else if !in_escape {
            out.push(ch);
        }
    }
    out
}

fn print_table(headers: &[&str], rows: &[Vec<String>]) {
    let mut widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            let vlen = strip_ansi(cell).len();
            if i < widths.len() && vlen > widths[i] {
                widths[i] = vlen;
            }
        }
    }
    let bar = widths
        .iter()
        .map(|w| "─".repeat(w + 2))
        .collect::<Vec<_>>()
        .join("┬");
    let bar_mid = widths
        .iter()
        .map(|w| "─".repeat(w + 2))
        .collect::<Vec<_>>()
        .join("┼");
    let bar_bot = widths
        .iter()
        .map(|w| "─".repeat(w + 2))
        .collect::<Vec<_>>()
        .join("┴");

    println!("┌{}┐", bar);
    let header_cells: Vec<String> = headers
        .iter()
        .enumerate()
        .map(|(i, h)| format!(" {:<width$} ", bold(h), width = widths[i]))
        .collect();
    println!("│{}│", header_cells.join("│"));
    println!("├{}┤", bar_mid);
    for row in rows {
        let cells: Vec<String> = row
            .iter()
            .enumerate()
            .map(|(i, cell)| {
                let visible = strip_ansi(cell);
                let pad = widths[i].saturating_sub(visible.len());
                format!(" {}{} ", cell, " ".repeat(pad))
            })
            .collect();
        println!("│{}│", cells.join("│"));
    }
    println!("└{}┘", bar_bot);
}

// ─── Clap types ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default)]
pub enum ToolsOutputFormat {
    #[default]
    Table,
    Json,
}

#[derive(Debug, Subcommand)]
pub enum ToolsCommand {
    /// 📊 列出 active / deferred tool 状态
    ///
    /// GET /api/v1/tools/state
    ///
    /// `list` 是别名（2026-05-17 — 统一 cli 子命令习惯：每个 *list* 风格的
    /// 资源页都用 `list` 触发，避免 `tools list` 报错 unknown subcommand）。
    #[command(alias = "list")]
    State(StateArgs),
    /// ⬆️  将 tool 从 deferred 提升为 active
    ///
    /// POST /api/v1/tools/promote/:name
    Promote(PromoteArgs),
    /// ⬇️  将 tool 从 active 降级为 deferred
    ///
    /// POST /api/v1/tools/demote/:name
    Demote(DemoteArgs),
}

#[derive(Debug, Args)]
pub struct StateArgs {
    /// 输出格式（table / json）
    #[arg(long, value_enum, default_value = "table")]
    pub format: ToolsOutputFormat,
    /// CyberClaw server URL
    #[arg(long)]
    pub server: Option<String>,
}

#[derive(Debug, Args)]
pub struct PromoteArgs {
    /// Tool 名称（如 bash / read_file）
    pub name: String,
    /// CyberClaw server URL
    #[arg(long)]
    pub server: Option<String>,
}

#[derive(Debug, Args)]
pub struct DemoteArgs {
    /// Tool 名称（如 bash / read_file）
    pub name: String,
    /// CyberClaw server URL
    #[arg(long)]
    pub server: Option<String>,
}

// ─── Dispatch ─────────────────────────────────────────────────────────────────

pub async fn handle_tools_command(cmd: ToolsCommand) -> Result<()> {
    match cmd {
        ToolsCommand::State(args) => state(args).await,
        ToolsCommand::Promote(args) => promote(args).await,
        ToolsCommand::Demote(args) => demote(args).await,
    }
}

// ─── Handlers ─────────────────────────────────────────────────────────────────

async fn state(args: StateArgs) -> Result<()> {
    let server = server_url(args.server.as_deref());
    let resp: serde_json::Value = get_json(&server, "/api/v1/tools/state")
        .await
        .map_err(|e| anyhow::anyhow!("❌ Could not fetch tools state: {}", friendly_error(&e)))?;

    match args.format {
        ToolsOutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&resp)?);
        }
        ToolsOutputFormat::Table => {
            render_tools_state(&resp);
        }
    }
    Ok(())
}

async fn promote(args: PromoteArgs) -> Result<()> {
    let server = server_url(args.server.as_deref());
    let path = format!("/api/v1/tools/promote/{}", args.name);
    let body = serde_json::json!({});
    let resp: serde_json::Value = post_json(&server, &path, &body).await.map_err(|e| {
        anyhow::anyhow!(
            "❌ Failed to promote tool '{}': {}",
            args.name,
            friendly_error(&e)
        )
    })?;
    println!(
        "{} Tool promoted: {} → {}",
        green("✅"),
        args.name,
        resp.get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("active")
    );
    Ok(())
}

async fn demote(args: DemoteArgs) -> Result<()> {
    let server = server_url(args.server.as_deref());
    let path = format!("/api/v1/tools/demote/{}", args.name);
    let body = serde_json::json!({});
    let resp: serde_json::Value = post_json(&server, &path, &body).await.map_err(|e| {
        anyhow::anyhow!(
            "❌ Failed to demote tool '{}': {}",
            args.name,
            friendly_error(&e)
        )
    })?;
    println!(
        "{} Tool demoted: {} → {}",
        yellow("⬇️ "),
        args.name,
        resp.get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("deferred")
    );
    Ok(())
}

// ─── Rendering ────────────────────────────────────────────────────────────────

fn render_tools_state(resp: &serde_json::Value) {
    let active_count = resp
        .get("active_count")
        .and_then(|v| v.as_u64())
        .unwrap_or_default();
    let deferred_count = resp
        .get("deferred_count")
        .and_then(|v| v.as_u64())
        .unwrap_or_default();

    println!(
        "{} Tools: {} active / {} deferred",
        bold("🔧"),
        active_count,
        deferred_count
    );
    println!();

    // Active tools table
    if let Some(active) = resp.get("active").and_then(|v| v.as_array()) {
        println!("{}", bold("Active tools:"));
        if active.is_empty() {
            println!("  (none)");
        } else {
            let headers = &["name", "state"];
            let rows: Vec<Vec<String>> = active
                .iter()
                .map(|t| {
                    let name = t.as_str().unwrap_or(&t.to_string()).to_string();
                    vec![name, green("active")]
                })
                .collect();
            print_table(headers, &rows);
        }
        println!();
    }

    // Deferred tools table
    if let Some(deferred) = resp.get("deferred").and_then(|v| v.as_array()) {
        println!("{}", bold("Deferred tools:"));
        if deferred.is_empty() {
            println!("  (none)");
        } else {
            let headers = &["name", "state"];
            let rows: Vec<Vec<String>> = deferred
                .iter()
                .map(|t| {
                    let name = t.as_str().unwrap_or(&t.to_string()).to_string();
                    vec![name, yellow("deferred")]
                })
                .collect();
            print_table(headers, &rows);
        }
    }

    // Fallback: unknown shape
    if resp.get("active").is_none() && resp.get("deferred").is_none() {
        println!("{}", serde_json::to_string_pretty(resp).unwrap_or_default());
    }
}

fn friendly_error(e: &anyhow::Error) -> String {
    let msg = e.to_string();
    if msg.contains("500") {
        format!("{} (try CYBERCLAW_STRICT_DRIFT=0 if connector drift)", msg)
    } else if msg.contains("401") || msg.contains("403") {
        format!("{} (check CYBERCLAW_TOKEN or run `cyberclaw chat`)", msg)
    } else if msg.contains("connection refused") || msg.contains("connect error") {
        format!("{} (is the server running? check CYBERCLAW_SERVER)", msg)
    } else {
        msg
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct TestCli {
        #[command(subcommand)]
        cmd: ToolsCommand,
    }

    #[test]
    fn test_state_args_default_format() {
        let cli = TestCli::parse_from(["test", "state"]);
        match cli.cmd {
            ToolsCommand::State(args) => assert_eq!(args.format, ToolsOutputFormat::Table),
            _ => panic!("expected State"),
        }
    }

    #[test]
    fn test_state_args_json_format() {
        let cli = TestCli::parse_from(["test", "state", "--format", "json"]);
        match cli.cmd {
            ToolsCommand::State(args) => assert_eq!(args.format, ToolsOutputFormat::Json),
            _ => panic!("expected State"),
        }
    }

    #[test]
    fn test_promote_args_parse() {
        let cli = TestCli::parse_from(["test", "promote", "my-tool"]);
        match cli.cmd {
            ToolsCommand::Promote(args) => assert_eq!(args.name, "my-tool"),
            _ => panic!("expected Promote"),
        }
    }

    #[test]
    fn test_demote_args_parse() {
        let cli = TestCli::parse_from(["test", "demote", "my-tool"]);
        match cli.cmd {
            ToolsCommand::Demote(args) => assert_eq!(args.name, "my-tool"),
            _ => panic!("expected Demote"),
        }
    }

    #[test]
    fn test_render_tools_state_no_panic() {
        let resp = serde_json::json!({
            "active_count": 2,
            "deferred_count": 1,
            "active": ["cmd_run", "file_read"],
            "deferred": ["write_file"]
        });
        render_tools_state(&resp);
    }
}
