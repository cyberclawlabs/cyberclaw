//! Memory CRUD CLI surface (BT-32/33/34 + BT-09 tags).
//!
//! Subcommands:
//!   - `set`     — write a memory entry (with optional tags)
//!   - `get`     — fetch a memory entry by id
//!   - `search`  — full-text + tag-filtered search
//!   - `delete`  — delete a memory entry

use anyhow::Result;
use clap::{Args, Subcommand};
use serde::Deserialize;
use serde_json::Value;

use crate::http_client::{delete as http_delete, get_json, post_json, server_url};

#[derive(Debug, Subcommand)]
pub enum MemoryCommand {
    /// Write a memory entry.
    Set(SetArgs),
    /// Fetch a memory entry by id.
    Get(GetArgs),
    /// Search memory entries (BT-34 BM25/FTS5 + BT-09 tag filter).
    Search(SearchArgs),
    /// Delete a memory entry.
    Delete(DeleteArgs),
    /// List recent memory entries (alias for `search` with empty query).
    ///
    /// 2026-05-17 — added so `cyberclaw-cli memory list` works consistently
    /// with `tools list` / `cluster list`. Internally re-uses the search
    /// endpoint with `q=` (empty), which falls back to recency-ordered list.
    List(ListArgs),
}

#[derive(Debug, Args)]
pub struct SetArgs {
    /// Agent that owns the memory.
    #[arg(long)]
    pub agent_id: String,
    /// Wire level: L0 / L1 / L2.
    #[arg(long, default_value = "L1")]
    pub level: String,
    /// Content body.
    pub content: String,
    /// Optional tags (repeatable). Closes BT-09.
    #[arg(long = "tag")]
    pub tags: Vec<String>,
    #[arg(long)]
    pub server: Option<String>,
}

#[derive(Debug, Args)]
pub struct GetArgs {
    pub id: String,
    #[arg(long)]
    pub server: Option<String>,
}

#[derive(Debug, Args)]
pub struct SearchArgs {
    /// Query string (matched via FTS5 BM25).
    pub query: String,
    /// Filter by tag (repeatable). Closes BT-09 retrieval.
    #[arg(long = "tag")]
    pub tags: Vec<String>,
    /// Max results.
    #[arg(long, default_value_t = 10)]
    pub limit: u32,
    #[arg(long)]
    pub server: Option<String>,
}

#[derive(Debug, Args)]
pub struct ListArgs {
    /// Filter by tag (repeatable).
    #[arg(long = "tag")]
    pub tags: Vec<String>,
    /// Max results.
    #[arg(long, default_value_t = 20)]
    pub limit: u32,
    #[arg(long)]
    pub server: Option<String>,
}

#[derive(Debug, Args)]
pub struct DeleteArgs {
    pub id: String,
    #[arg(long)]
    pub server: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MemoryRow {
    id: String,
    #[serde(default)]
    agent_id: String,
    #[serde(default)]
    level: String,
    #[serde(default)]
    content: String,
}

/// `GET /api/v1/memory?q=...` returns `{ records: [MemorySearchRow, ...] }`,
/// where each row carries a server-truncated `preview` instead of the full
/// `content` body. Mirrors the response shape; do NOT reuse `MemoryRow`.
#[derive(Debug, Deserialize)]
struct MemorySearchResponse {
    #[serde(default)]
    records: Vec<MemorySearchRow>,
}

#[derive(Debug, Deserialize)]
struct MemorySearchRow {
    id: String,
    #[serde(default)]
    agent_id: String,
    #[serde(default)]
    level: String,
    #[serde(default)]
    preview: String,
}

pub async fn handle_memory_command(cmd: MemoryCommand) -> Result<()> {
    match cmd {
        MemoryCommand::Set(args) => set(args).await,
        MemoryCommand::Get(args) => get(args).await,
        MemoryCommand::Search(args) => search(args).await,
        MemoryCommand::Delete(args) => delete_one(args).await,
        MemoryCommand::List(args) => {
            // Reuse the search handler with an empty query string — server
            // falls back to recency-ordered listing for empty `q`.
            search(SearchArgs {
                query: String::new(),
                tags: args.tags,
                limit: args.limit,
                server: args.server,
            })
            .await
        }
    }
}

async fn set(args: SetArgs) -> Result<()> {
    let server = server_url(args.server.as_deref());
    let body = serde_json::json!({
        "agent_id": args.agent_id,
        "level": args.level,
        "content": args.content,
        "tags": args.tags,
    });
    let resp: Value = post_json(&server, "/api/v1/memory", &body).await?;
    println!(
        "✓ Memory written: id={}",
        resp.get("id").and_then(|v| v.as_str()).unwrap_or("?")
    );
    if !args.tags.is_empty() {
        println!("  tags: {}", args.tags.join(", "));
    }
    Ok(())
}

async fn get(args: GetArgs) -> Result<()> {
    let server = server_url(args.server.as_deref());
    let path = format!("/api/v1/memory/{}", args.id);
    let row: MemoryRow = get_json(&server, &path).await?;
    println!("id:       {}", row.id);
    println!("agent_id: {}", row.agent_id);
    println!("level:    {}", row.level);
    println!("content:  {}", row.content);
    Ok(())
}

async fn search(args: SearchArgs) -> Result<()> {
    let server = server_url(args.server.as_deref());
    let mut path = format!("/api/v1/memory?q={}&limit={}", args.query, args.limit);
    for t in &args.tags {
        path.push_str("&tag=");
        path.push_str(t);
    }
    let resp: MemorySearchResponse = get_json(&server, &path).await?;
    let rows = resp.records;
    if rows.is_empty() {
        println!("No memory entries match query='{}'", args.query);
        return Ok(());
    }
    for r in &rows {
        // Server-side preview is already truncated; do not re-truncate.
        println!("[{}] {} ({}): {}", r.level, r.id, r.agent_id, r.preview);
    }
    println!("\n{} result(s)", rows.len());
    Ok(())
}

async fn delete_one(args: DeleteArgs) -> Result<()> {
    let server = server_url(args.server.as_deref());
    let path = format!("/api/v1/memory/{}", args.id);
    http_delete(&server, &path).await?;
    println!("✓ Deleted {}", args.id);
    Ok(())
}
