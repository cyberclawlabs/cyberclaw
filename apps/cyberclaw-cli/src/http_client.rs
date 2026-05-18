//! Shared HTTP client helpers for CLI commands that talk to a running
//! cyberclaw-server via `/api/v1/*`.
//!
//! Auth precedence mirrors `commands/chat.rs`: env `CYBERCLAW_TOKEN` first,
//! then `~/.cyberclaw/cli-token`. Server URL precedence: `--server` flag
//! (per-command) > env `CYBERCLAW_SERVER` > `http://127.0.0.1:38090`.

use anyhow::{Context, Result};
use reqwest::{Client, Method, RequestBuilder, StatusCode};
use serde::{de::DeserializeOwned, Serialize};
use std::path::PathBuf;
use std::time::Duration;

/// Convert a non-success HTTP status into a friendly error message.
/// 401 → "session expired" with re-login hint (covers the JWT_SECRET-drift
/// case where server restarted with a new secret and existing CLI tokens
/// no longer validate).
fn explain_status(method: Method, url: &str, status: StatusCode, body: &str) -> anyhow::Error {
    match status {
        StatusCode::UNAUTHORIZED => anyhow::anyhow!(
            "{} {} returned 401 Unauthorized — your CLI token is rejected.\n\
             \n\
             Likely causes:\n\
               · token expired (JWT TTL passed)\n\
               · server restarted with a new JWT_SECRET (token signature no longer valid)\n\
               · the stored token belongs to a different user account\n\
             \n\
             Fix: run `cyberclaw chat` to re-login interactively, or update\n\
             ~/.cyberclaw/cli-token (or the CYBERCLAW_TOKEN env var) with a fresh JWT.",
            method,
            url
        ),
        StatusCode::FORBIDDEN => anyhow::anyhow!(
            "{} {} returned 403 Forbidden — token is valid but lacks the required role.\n\
             Body: {}",
            method,
            url,
            body
        ),
        _ => anyhow::anyhow!("{} {} failed ({}): {}", method, url, status, body),
    }
}

const DEFAULT_SERVER: &str = "http://127.0.0.1:38090";

fn cyberclaw_config_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    Ok(PathBuf::from(home).join(".cyberclaw"))
}

pub fn server_url(override_url: Option<&str>) -> String {
    if let Some(s) = override_url {
        if !s.is_empty() {
            return s.to_string();
        }
    }
    std::env::var("CYBERCLAW_SERVER").unwrap_or_else(|_| DEFAULT_SERVER.to_string())
}

pub fn load_token() -> Option<String> {
    if let Ok(t) = std::env::var("CYBERCLAW_TOKEN") {
        if !t.is_empty() {
            return Some(t);
        }
    }
    let path = cyberclaw_config_dir().ok()?.join("cli-token");
    std::fs::read_to_string(&path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn build_client() -> Result<Client> {
    // Bypass any system-level HTTP/HTTPS proxy. CLI requests target a
    // user-supplied CYBERCLAW_SERVER URL (often loopback), and a system
    // proxy (e.g. Clash on macOS, returning 403 with cache-control
    // public for unrecognized loopback ports) silently breaks every
    // request. curl/python don't honor system proxy by default; reqwest
    // does, so we opt out explicitly.
    Client::builder()
        .timeout(Duration::from_secs(30))
        .no_proxy()
        .build()
        .context("build reqwest client")
}

/// Construct an authenticated request. Returns `Err` if no token is found —
/// the user should run `cyberclaw chat` once to acquire and persist a token,
/// or set `CYBERCLAW_TOKEN`.
pub fn authed(client: &Client, method: Method, url: &str) -> Result<RequestBuilder> {
    let token = load_token().ok_or_else(|| {
        anyhow::anyhow!(
            "no JWT token found — run `cyberclaw chat` first to log in, \
             or set CYBERCLAW_TOKEN env var"
        )
    })?;
    Ok(client
        .request(method, url)
        .bearer_auth(token)
        .header("Accept", "application/json"))
}

/// POST JSON, expect JSON response.
pub async fn post_json<B: Serialize, R: DeserializeOwned>(
    server: &str,
    path: &str,
    body: &B,
) -> Result<R> {
    let client = build_client()?;
    let url = format!("{}{}", server.trim_end_matches('/'), path);
    let resp = authed(&client, Method::POST, &url)?
        .json(body)
        .send()
        .await
        .with_context(|| format!("POST {}", url))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(explain_status(Method::POST, &url, status, &text));
    }
    serde_json::from_str(&text).with_context(|| format!("parse JSON response from {}", url))
}

/// GET, expect JSON response.
pub async fn get_json<R: DeserializeOwned>(server: &str, path: &str) -> Result<R> {
    let client = build_client()?;
    let url = format!("{}{}", server.trim_end_matches('/'), path);
    let resp = authed(&client, Method::GET, &url)?
        .send()
        .await
        .with_context(|| format!("GET {}", url))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(explain_status(Method::GET, &url, status, &text));
    }
    serde_json::from_str(&text).with_context(|| format!("parse JSON response from {}", url))
}

/// DELETE, expect 200/204.
pub async fn delete(server: &str, path: &str) -> Result<()> {
    let client = build_client()?;
    let url = format!("{}{}", server.trim_end_matches('/'), path);
    let resp = authed(&client, Method::DELETE, &url)?
        .send()
        .await
        .with_context(|| format!("DELETE {}", url))?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(explain_status(Method::DELETE, &url, status, &text));
    }
    Ok(())
}
