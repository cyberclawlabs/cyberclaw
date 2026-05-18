//! Review approval CLI surface (BT-21 + Sprint 21 multi-tenant Phase 3).
//!
//! Subcommands:
//!   - `list`     — list pending review requests
//!   - `approve`  — approve a review by id
//!   - `reject`   — reject a review by id with reason

use anyhow::Result;
use clap::{Args, Subcommand};
use serde::Deserialize;
use serde_json::Value;

use crate::http_client::{get_json, post_json, server_url};

#[derive(Debug, Subcommand)]
pub enum ReviewCommand {
    /// List pending review requests.
    List(ListArgs),
    /// Approve a review by id.
    Approve(ApproveArgs),
    /// Reject a review by id.
    Reject(RejectArgs),
}

#[derive(Debug, Args)]
pub struct ListArgs {
    /// Filter by status (pending / approved / rejected). Default: pending.
    #[arg(long, default_value = "pending")]
    pub status: String,
    /// Server URL override.
    #[arg(long)]
    pub server: Option<String>,
}

#[derive(Debug, Args)]
pub struct ApproveArgs {
    /// Review id, e.g. `rev_<uuid>`.
    pub id: String,
    /// Optional approval comment.
    #[arg(long)]
    pub comment: Option<String>,
    #[arg(long)]
    pub server: Option<String>,
}

#[derive(Debug, Args)]
pub struct RejectArgs {
    /// Review id.
    pub id: String,
    /// Required rejection reason.
    #[arg(long)]
    pub reason: String,
    #[arg(long)]
    pub server: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ReviewRow {
    id: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    summary: String,
}

pub async fn handle_review_command(cmd: ReviewCommand) -> Result<()> {
    match cmd {
        ReviewCommand::List(args) => list(args).await,
        ReviewCommand::Approve(args) => approve(args).await,
        ReviewCommand::Reject(args) => reject(args).await,
    }
}

async fn list(args: ListArgs) -> Result<()> {
    let server = server_url(args.server.as_deref());
    let path = format!("/api/v1/reviews?status={}", args.status);
    let rows: Vec<ReviewRow> = get_json(&server, &path).await?;
    if rows.is_empty() {
        println!("No reviews with status='{}'", args.status);
        return Ok(());
    }
    println!("{:<24}  {:<14}  {:<10}  summary", "id", "kind", "status");
    for r in &rows {
        let summary = if r.summary.len() > 60 {
            format!("{}...", &r.summary[..57])
        } else {
            r.summary.clone()
        };
        println!(
            "{:<24}  {:<14}  {:<10}  {}",
            r.id, r.kind, r.status, summary
        );
    }
    println!("\n{} review(s)", rows.len());
    Ok(())
}

async fn approve(args: ApproveArgs) -> Result<()> {
    let server = server_url(args.server.as_deref());
    let path = format!("/api/v1/reviews/{}/approve", args.id);
    let body = serde_json::json!({ "comment": args.comment });
    let resp: Value = post_json(&server, &path, &body).await?;
    println!("✓ Approved {}", args.id);
    if let Some(target) = resp.get("target") {
        println!("  target: {}", target);
    }
    Ok(())
}

async fn reject(args: RejectArgs) -> Result<()> {
    let server = server_url(args.server.as_deref());
    let path = format!("/api/v1/reviews/{}/reject", args.id);
    let body = serde_json::json!({ "reason": args.reason });
    let _: Value = post_json(&server, &path, &body).await?;
    println!("✗ Rejected {} — {}", args.id, args.reason);
    Ok(())
}
