//! 🧠 Cluster 管理命令（F4）
//!
//! Manage cluster brain nodes (multi-node coordination).
//!
//! Examples:
//!   cyberclaw cluster register --brain-id node-2 --address 10.0.0.2 --port 38090 --max-concurrent 50
//!   cyberclaw cluster heartbeat --brain-id node-2 --active-sessions 3 --cpu-pct 0.4 --mem-pct 0.6
//!   cyberclaw cluster assign --session-id sess-001
//!   cyberclaw cluster state                # show all brains + sessions
//!   cyberclaw cluster state --format json  # JSON for piping
//!   cyberclaw cluster watch                # live refresh every 5s (Ctrl+C exits)

use anyhow::Result;
use clap::{Args, Subcommand, ValueEnum};

use crate::http_client::{get_json, post_json, server_url};

// ─── ANSI colour helpers (no external crate) ──────────────────────────────────

const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const CYAN: &str = "\x1b[36m";
const BOLD: &str = "\x1b[1m";
const RESET: &str = "\x1b[0m";

fn green(s: &str) -> String {
    format!("{}{}{}", GREEN, s, RESET)
}
fn red(s: &str) -> String {
    format!("{}{}{}", RED, s, RESET)
}
fn cyan(s: &str) -> String {
    format!("{}{}{}", CYAN, s, RESET)
}
fn bold(s: &str) -> String {
    format!("{}{}{}", BOLD, s, RESET)
}

// ─── ASCII table helper ────────────────────────────────────────────────────────

/// Print a simple bordered ascii table.
/// `headers`: column names. `rows`: each row is a Vec<String>.
fn print_table(headers: &[&str], rows: &[Vec<String>]) {
    // compute column widths
    let mut widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            // strip ANSI escape codes for width calculation
            let visible_len = strip_ansi(cell).len();
            if i < widths.len() && visible_len > widths[i] {
                widths[i] = visible_len;
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

    // header
    let header_cells: Vec<String> = headers
        .iter()
        .enumerate()
        .map(|(i, h)| format!(" {:<width$} ", bold(h), width = widths[i]))
        .collect();
    println!("│{}│", header_cells.join("│"));
    println!("├{}┤", bar_mid);

    // rows
    for row in rows {
        let cells: Vec<String> = row
            .iter()
            .enumerate()
            .map(|(i, cell)| {
                let visible = strip_ansi(cell);
                let pad = if widths[i] > visible.len() {
                    widths[i] - visible.len()
                } else {
                    0
                };
                format!(" {}{} ", cell, " ".repeat(pad))
            })
            .collect();
        println!("│{}│", cells.join("│"));
    }

    println!("└{}┘", bar_bot);
}

/// Strip ANSI escape codes to get visible character count.
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

// ─── Clap types ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default)]
pub enum ClusterOutputFormat {
    #[default]
    Table,
    Json,
}

#[derive(Debug, Subcommand)]
pub enum ClusterCommand {
    /// 注册一个新的 Brain 节点到集群
    ///
    /// POST /api/v1/cluster/brain/register
    Register(RegisterArgs),
    /// 上报 Brain 心跳（活跃会话数 / CPU / 内存）
    ///
    /// POST /api/v1/cluster/heartbeat/:brain_id
    Heartbeat(HeartbeatArgs),
    /// 将新会话分配到最合适的节点
    ///
    /// POST /api/v1/cluster/sessions/assign
    Assign(AssignArgs),
    /// 查看集群当前状态（brains + sessions）
    ///
    /// GET /api/v1/cluster/state
    ///
    /// `list` 是别名（2026-05-17 — 统一 cli 子命令习惯，让 `cluster list`
    /// 也工作，避免用户撞到 unknown subcommand 错误）。
    #[command(alias = "list")]
    State(StateArgs),
    /// 实时监控集群状态（每 5s 刷新，Ctrl+C 退出）
    Watch(WatchArgs),
}

#[derive(Debug, Args)]
pub struct RegisterArgs {
    /// Brain 节点 ID
    #[arg(long)]
    pub brain_id: String,
    /// 节点监听地址（IP 或主机名）
    #[arg(long)]
    pub address: String,
    /// 节点监听端口
    #[arg(long)]
    pub port: u16,
    /// 最大并发会话数
    #[arg(long, default_value_t = 10)]
    pub max_concurrent: u32,
    /// CyberClaw server URL（覆盖 CYBERCLAW_SERVER）
    #[arg(long)]
    pub server: Option<String>,
}

#[derive(Debug, Args)]
pub struct HeartbeatArgs {
    /// Brain 节点 ID
    #[arg(long)]
    pub brain_id: String,
    /// 当前活跃会话数
    #[arg(long, default_value_t = 0)]
    pub active_sessions: u32,
    /// CPU 使用率（0.0–1.0）
    #[arg(long, default_value_t = 0.0)]
    pub cpu_pct: f64,
    /// 内存使用率（0.0–1.0）
    #[arg(long, default_value_t = 0.0)]
    pub mem_pct: f64,
    /// 节点容量（最大并发会话数）— 必须随每次心跳重新声明，
    /// 因为 server 端 NodeLoad 会被整体覆盖。常规写法：
    /// 与 register 时的 --max-concurrent 保持一致。
    #[arg(long, default_value_t = 10)]
    pub capacity: u32,
    /// CyberClaw server URL
    #[arg(long)]
    pub server: Option<String>,
}

#[derive(Debug, Args)]
pub struct AssignArgs {
    /// 需要分配节点的会话 ID
    #[arg(long)]
    pub session_id: String,
    /// CyberClaw server URL
    #[arg(long)]
    pub server: Option<String>,
}

#[derive(Debug, Args)]
pub struct StateArgs {
    /// 输出格式（table / json）
    #[arg(long, value_enum, default_value = "table")]
    pub format: ClusterOutputFormat,
    /// CyberClaw server URL
    #[arg(long)]
    pub server: Option<String>,
}

#[derive(Debug, Args)]
pub struct WatchArgs {
    /// 刷新间隔（秒）
    #[arg(long, default_value_t = 5)]
    pub interval: u64,
    /// CyberClaw server URL
    #[arg(long)]
    pub server: Option<String>,
}

// ─── Dispatch ─────────────────────────────────────────────────────────────────

pub async fn handle_cluster_command(cmd: ClusterCommand) -> Result<()> {
    match cmd {
        ClusterCommand::Register(args) => register(args).await,
        ClusterCommand::Heartbeat(args) => heartbeat(args).await,
        ClusterCommand::Assign(args) => assign(args).await,
        ClusterCommand::State(args) => state(args).await,
        ClusterCommand::Watch(args) => watch(args).await,
    }
}

// ─── Handlers ─────────────────────────────────────────────────────────────────

async fn register(args: RegisterArgs) -> Result<()> {
    let server = server_url(args.server.as_deref());
    let body = serde_json::json!({
        "brain_id": args.brain_id,
        "address": args.address,
        "port": args.port,
        "max_concurrent": args.max_concurrent,
    });
    let resp: serde_json::Value = post_json(&server, "/api/v1/cluster/brain/register", &body)
        .await
        .map_err(|e| anyhow::anyhow!("❌ Failed to register brain: {}", friendly_error(&e)))?;
    println!(
        "{} Brain registered: {}{}{}",
        green("✅"),
        CYAN,
        resp.get("brain_id")
            .and_then(|v| v.as_str())
            .unwrap_or(&args.brain_id),
        RESET
    );
    Ok(())
}

async fn heartbeat(args: HeartbeatArgs) -> Result<()> {
    let server = server_url(args.server.as_deref());
    let path = format!("/api/v1/cluster/heartbeat/{}", args.brain_id);
    // Server schema: HeartbeatRequest { load: NodeLoad { cpu_percent, memory_percent, active_sessions, capacity } }
    let body = serde_json::json!({
        "load": {
            "cpu_percent": args.cpu_pct,
            "memory_percent": args.mem_pct,
            "active_sessions": args.active_sessions,
            "capacity": args.capacity,
        }
    });
    let resp: serde_json::Value = post_json(&server, &path, &body).await.map_err(|e| {
        anyhow::anyhow!(
            "❌ Heartbeat failed for {}: {}",
            args.brain_id,
            friendly_error(&e)
        )
    })?;
    println!(
        "{} Heartbeat accepted: brain={} status={}",
        green("✅"),
        cyan(&args.brain_id),
        resp.get("status").and_then(|v| v.as_str()).unwrap_or("ok")
    );
    Ok(())
}

async fn assign(args: AssignArgs) -> Result<()> {
    let server = server_url(args.server.as_deref());
    let body = serde_json::json!({ "session_id": args.session_id });
    let resp: serde_json::Value = post_json(&server, "/api/v1/cluster/sessions/assign", &body)
        .await
        .map_err(|e| anyhow::anyhow!("❌ Session assignment failed: {}", friendly_error(&e)))?;
    println!(
        "{} Session assigned: {} → brain {}",
        green("✅"),
        cyan(&args.session_id),
        resp.get("brain_id").and_then(|v| v.as_str()).unwrap_or("?")
    );
    Ok(())
}

async fn state(args: StateArgs) -> Result<()> {
    let server = server_url(args.server.as_deref());
    let resp: serde_json::Value = get_json(&server, "/api/v1/cluster/state")
        .await
        .map_err(|e| anyhow::anyhow!("❌ Could not fetch cluster state: {}", friendly_error(&e)))?;

    match args.format {
        ClusterOutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&resp)?);
        }
        ClusterOutputFormat::Table => {
            render_cluster_state(&resp);
        }
    }
    Ok(())
}

async fn watch(args: WatchArgs) -> Result<()> {
    let server = server_url(args.server.as_deref());
    println!(
        "🔄 Watching cluster state (every {}s — Ctrl+C to exit)…",
        args.interval
    );
    loop {
        // clear screen
        print!("\x1b[2J\x1b[H");
        println!(
            "🔄 {} Cluster watch — refreshing every {}s",
            bold("CyberClaw"),
            args.interval
        );
        println!();
        match get_json::<serde_json::Value>(&server, "/api/v1/cluster/state").await {
            Ok(resp) => render_cluster_state(&resp),
            Err(e) => println!("{} {}", red("❌"), friendly_error(&e)),
        }
        tokio::time::sleep(tokio::time::Duration::from_secs(args.interval)).await;
    }
}

// ─── Rendering ────────────────────────────────────────────────────────────────

fn render_cluster_state(resp: &serde_json::Value) {
    println!("{} Cluster brains:", bold("🧠"));

    // Try to extract brains array; fall back to raw JSON
    if let Some(brains) = resp.get("brains").and_then(|v| v.as_array()) {
        if brains.is_empty() {
            println!("  (no brains registered)");
        } else {
            let headers = &["id", "status", "last_seen", "load"];
            let rows: Vec<Vec<String>> = brains
                .iter()
                .map(|b| {
                    // Server schema: BrainStateView { id, status, last_seen, load: NodeLoad }
                    let id = b
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("?")
                        .to_string();
                    let raw_status = b
                        .get("status")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    let status = if raw_status == "healthy" || raw_status == "active" {
                        green(&format!("✅ {}", raw_status))
                    } else {
                        red(&format!("⚠️  {}", raw_status))
                    };
                    let last_seen = b
                        .get("last_seen")
                        .and_then(|v| v.as_str())
                        .unwrap_or("—")
                        .to_string();
                    let load_obj = b.get("load");
                    let cpu = load_obj
                        .and_then(|l| l.get("cpu_percent"))
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);
                    let mem = load_obj
                        .and_then(|l| l.get("memory_percent"))
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);
                    let load = format!("CPU {:.0}% MEM {:.0}%", cpu * 100.0, mem * 100.0);
                    vec![id, status, last_seen, load]
                })
                .collect();
            print_table(headers, &rows);
        }
    } else {
        // unknown shape — dump raw
        println!("{}", serde_json::to_string_pretty(resp).unwrap_or_default());
        return;
    }

    // Sessions
    if let Some(sessions) = resp.get("sessions").and_then(|v| v.as_array()) {
        println!();
        println!("{} Sessions:", bold("📊"));
        if sessions.is_empty() {
            println!("  (no active sessions)");
        } else {
            for s in sessions {
                // Server schema: SessionStateView { id, brain, last_touched }
                let sid = s.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                let brain = s.get("brain").and_then(|v| v.as_str()).unwrap_or("—");
                let ts = s
                    .get("last_touched")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                println!("  - {} → {} (last_touched {})", cyan(sid), brain, ts);
            }
        }
    }
}

// ─── Error prettifier ─────────────────────────────────────────────────────────

fn friendly_error(e: &anyhow::Error) -> String {
    let msg = e.to_string();
    if msg.contains("500") {
        format!("{} (try CYBERCLAW_STRICT_DRIFT=0 if connector drift)", msg)
    } else if msg.contains("401") || msg.contains("403") {
        format!(
            "{} (check CYBERCLAW_TOKEN or run `cyberclaw chat` to log in)",
            msg
        )
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
        cmd: ClusterCommand,
    }

    #[test]
    fn test_register_args_parse() {
        let cli = TestCli::parse_from([
            "test",
            "register",
            "--brain-id",
            "brain-1",
            "--address",
            "127.0.0.1",
            "--port",
            "9000",
            "--max-concurrent",
            "5",
        ]);
        match cli.cmd {
            ClusterCommand::Register(args) => {
                assert_eq!(args.brain_id, "brain-1");
                assert_eq!(args.address, "127.0.0.1");
                assert_eq!(args.port, 9000);
                assert_eq!(args.max_concurrent, 5);
            }
            _ => panic!("expected Register"),
        }
    }

    #[test]
    fn test_heartbeat_args_parse() {
        let cli = TestCli::parse_from([
            "test",
            "heartbeat",
            "--brain-id",
            "brain-1",
            "--active-sessions",
            "3",
            "--cpu-pct",
            "0.42",
            "--mem-pct",
            "0.61",
        ]);
        match cli.cmd {
            ClusterCommand::Heartbeat(args) => {
                assert_eq!(args.brain_id, "brain-1");
                assert_eq!(args.active_sessions, 3);
                assert!((args.cpu_pct - 0.42).abs() < 1e-6);
            }
            _ => panic!("expected Heartbeat"),
        }
    }

    #[test]
    fn test_assign_args_parse() {
        let cli = TestCli::parse_from(["test", "assign", "--session-id", "sess-abc"]);
        match cli.cmd {
            ClusterCommand::Assign(args) => {
                assert_eq!(args.session_id, "sess-abc");
            }
            _ => panic!("expected Assign"),
        }
    }

    #[test]
    fn test_state_args_parse_default_format() {
        let cli = TestCli::parse_from(["test", "state"]);
        match cli.cmd {
            ClusterCommand::State(args) => {
                assert_eq!(args.format, ClusterOutputFormat::Table);
            }
            _ => panic!("expected State"),
        }
    }

    #[test]
    fn test_state_args_parse_json_format() {
        let cli = TestCli::parse_from(["test", "state", "--format", "json"]);
        match cli.cmd {
            ClusterCommand::State(args) => {
                assert_eq!(args.format, ClusterOutputFormat::Json);
            }
            _ => panic!("expected State with json format"),
        }
    }

    #[test]
    fn test_watch_args_parse() {
        let cli = TestCli::parse_from(["test", "watch", "--interval", "10"]);
        match cli.cmd {
            ClusterCommand::Watch(args) => {
                assert_eq!(args.interval, 10);
            }
            _ => panic!("expected Watch"),
        }
    }

    #[test]
    fn test_strip_ansi() {
        let s = format!("{}hello{}", GREEN, RESET);
        assert_eq!(strip_ansi(&s), "hello");
    }

    #[test]
    fn test_render_cluster_state_no_panic() {
        // Mirrors the server's BrainStateView/SessionStateView shape — see
        // apps/cyberclaw-server/src/api/cluster_brain.rs.
        let resp = serde_json::json!({
            "brains": [
                {
                    "id": "node-1",
                    "status": "healthy",
                    "last_seen": "2026-05-05T10:00:00Z",
                    "load": {
                        "cpu_percent": 0.3,
                        "memory_percent": 0.5,
                        "active_sessions": 2,
                        "capacity": 10,
                    }
                }
            ],
            "sessions": [
                {
                    "id": "sess-001",
                    "brain": "node-1",
                    "last_touched": "10:00:00"
                }
            ]
        });
        // should not panic
        render_cluster_state(&resp);
    }
}
