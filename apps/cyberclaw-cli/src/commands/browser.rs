//! `cyberclaw browser` — thin CLI wrapper around the server-side
//! BrowserConnector admin endpoints.
//!
//! Why "thin": the actual chromium driver lives in
//! `crates/cyberclaw-connectors/src/browser.rs` and is exposed via
//! `/api/v1/admin/browser/*` admin routes. Putting the CDP / chromium
//! launching here would duplicate that logic AND bypass governance.
//! Instead the CLI just builds the request body, POSTs to admin, and
//! prints the response — every action still goes through the platform's
//! PolicyEngine + audit chain.
//!
//! Prerequisite: server started with `CYBERCLAW_BROWSER_ENABLED=true`.
//! Without it, every subcommand returns 503 ServiceUnavailable with an
//! actionable error message.

use anyhow::{Context, Result};
use clap::Subcommand;
use serde_json::{json, Value};

use crate::http_client::{get_json, post_json, server_url};

#[derive(Subcommand, Debug)]
pub enum BrowserCommand {
    /// Probe browser connector status (returns { enabled, attached, targets }).
    Status {
        #[arg(long)]
        server: Option<String>,
    },
    /// Navigate the active page to a URL.
    Navigate {
        #[arg(long)]
        url: String,
        #[arg(long)]
        server: Option<String>,
    },
    /// Click an element matching a CSS selector.
    Click {
        #[arg(long)]
        selector: String,
        #[arg(long)]
        server: Option<String>,
    },
    /// Fill a form field. The field is identified by a CSS selector.
    Fill {
        #[arg(long)]
        selector: String,
        #[arg(long)]
        value: String,
        #[arg(long)]
        server: Option<String>,
    },
    /// Evaluate a JavaScript expression in the page. Result printed as JSON.
    Evaluate {
        #[arg(long)]
        script: String,
        #[arg(long)]
        server: Option<String>,
    },
    /// Capture a screenshot. Path is server-side (under the platform's
    /// workspace root); the admin response embeds image_b64 for retrieval.
    Screenshot {
        #[arg(long, default_value = "/tmp/cyberclaw-browser-shot.png")]
        path: String,
        #[arg(long)]
        full_page: bool,
        #[arg(long)]
        server: Option<String>,
    },
    /// Handle the next browser dialog (accept / dismiss).
    Dialog {
        #[arg(long, value_parser = ["accept", "dismiss"])]
        action: String,
        #[arg(long, default_value = "")]
        prompt_text: String,
        #[arg(long)]
        server: Option<String>,
    },
}

pub async fn handle_browser_command(cmd: BrowserCommand) -> Result<()> {
    match cmd {
        BrowserCommand::Status { server } => {
            let v: Value = get_json(
                &server_url(server.as_deref()),
                "/api/v1/admin/browser/status",
            )
            .await
            .context("GET /api/v1/admin/browser/status")?;
            println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
        }
        BrowserCommand::Navigate { url, server } => {
            invoke(
                server.as_deref(),
                "/api/v1/admin/browser/navigate",
                &json!({ "url": url }),
            )
            .await?;
        }
        BrowserCommand::Click { selector, server } => {
            invoke(
                server.as_deref(),
                "/api/v1/admin/browser/click",
                &json!({ "selector": selector }),
            )
            .await?;
        }
        BrowserCommand::Fill {
            selector,
            value,
            server,
        } => {
            invoke(
                server.as_deref(),
                "/api/v1/admin/browser/fill",
                &json!({ "selector": selector, "value": value }),
            )
            .await?;
        }
        BrowserCommand::Evaluate { script, server } => {
            invoke(
                server.as_deref(),
                "/api/v1/admin/browser/evaluate",
                // 2026-05-17: server BrowserEvaluateInput expects `expression`,
                // not `script` (the CLI flag is `--script` for UX but the JSON
                // payload field must match the connector struct).
                &json!({ "expression": script }),
            )
            .await?;
        }
        BrowserCommand::Screenshot {
            path,
            full_page,
            server,
        } => {
            invoke(
                server.as_deref(),
                "/api/v1/admin/browser/screenshot",
                &json!({ "path": path, "full_page": full_page }),
            )
            .await?;
        }
        BrowserCommand::Dialog {
            action,
            prompt_text,
            server,
        } => {
            invoke(
                server.as_deref(),
                "/api/v1/admin/browser/dialog",
                &json!({ "action": action, "prompt_text": prompt_text }),
            )
            .await?;
        }
    }
    Ok(())
}

/// POST `body` to `path` and pretty-print the JSON response.
async fn invoke(server: Option<&str>, path: &str, body: &Value) -> Result<()> {
    let v: Value = post_json(&server_url(server), path, body)
        .await
        .with_context(|| format!("POST {}", path))?;
    println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
    Ok(())
}
