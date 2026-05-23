//! `cyberclaw doctor` — platform health checks.
//!
//! Runs up to 7 diagnostic checks and prints a table summary. Each check
//! is independently safe to run: read-only, no side-effects on the platform.

use anyhow::Result;
use clap::Args;
use std::path::PathBuf;

// ============================================================================
// CLI args
// ============================================================================

#[derive(Debug, Args, Clone)]
pub struct DoctorArgs {
    /// Run only the specified check(s). Comma-separated.
    /// Valid values: llm, config, users, governance, connectors, drift, server, ecosystem
    #[arg(long, value_delimiter = ',')]
    pub check: Vec<String>,

    /// Output format: table (default) or json.
    #[arg(long, default_value = "table")]
    pub format: String,

    /// Config directory to inspect (defaults to `~/.cyberclaw`).
    #[arg(long)]
    pub config_dir: Option<PathBuf>,
}

// ============================================================================
// Check result
// ============================================================================

#[derive(Debug, Clone)]
struct CheckResult {
    name: &'static str,
    emoji: &'static str,
    status: Status,
    detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Status {
    Ok,
    Warn,
    Fail,
}

impl Status {
    fn label(&self) -> &'static str {
        match self {
            Status::Ok => "OK",
            Status::Warn => "WARN",
            Status::Fail => "FAIL",
        }
    }
}

// ============================================================================
// Individual checks
// ============================================================================

fn check_config(config_dir: &std::path::Path) -> CheckResult {
    let path = config_dir.join("config.toml");
    if !path.exists() {
        return CheckResult {
            name: "config",
            emoji: "📄",
            status: Status::Fail,
            detail: format!("{} not found", path.display()),
        };
    }
    match std::fs::read_to_string(&path) {
        Ok(content) => match content.parse::<toml::Value>() {
            Ok(_) => CheckResult {
                name: "config",
                emoji: "📄",
                status: Status::Ok,
                detail: format!("{} valid TOML", path.display()),
            },
            Err(e) => CheckResult {
                name: "config",
                emoji: "📄",
                status: Status::Fail,
                detail: format!("TOML parse error: {}", e),
            },
        },
        Err(e) => CheckResult {
            name: "config",
            emoji: "📄",
            status: Status::Fail,
            detail: format!("read error: {}", e),
        },
    }
}

/// Parse a simple KEY=VALUE env file (one assignment per line, optional
/// `export` prefix, optional `# comment` lines). Returns None if the file
/// doesn't exist or contains no `key` match. Used to read `~/.cyberclaw/
/// llm.env` so `cyberclaw doctor` works the same whether or not the user
/// already sourced the file in their shell.
///
/// 2026-05-17: previously check_llm only consulted `std::env`, so running
/// `cyberclaw doctor` in a fresh terminal (without `source ~/.cyberclaw/
/// llm.env`) always reported `no LLM API key found` even when the start
/// script writes one for every server boot. Now we fall back to the file.
fn read_env_file_var(path: &std::path::Path, key: &str) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    for line in content.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // Strip optional `export ` prefix.
        let body = trimmed.strip_prefix("export ").unwrap_or(trimmed);
        let (lhs, rhs) = body.split_once('=')?;
        if lhs.trim() != key {
            continue;
        }
        // Strip surrounding quotes if present.
        let raw = rhs.trim();
        let value = raw
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .or_else(|| raw.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
            .unwrap_or(raw);
        return Some(value.to_string());
    }
    None
}

fn check_llm(config_dir: &std::path::Path) -> CheckResult {
    let path = config_dir.join("config.toml");
    let llm_env_path = config_dir.join("llm.env");
    let api_key = match std::fs::read_to_string(&path) {
        Ok(content) => match content.parse::<toml::Value>() {
            Ok(v) => v
                .get("llm_api_key")
                .and_then(|k| k.as_str())
                .map(|s| s.to_string()),
            Err(_) => None,
        },
        Err(_) => None,
    }
    .or_else(|| std::env::var("LLM_API_KEY").ok())
    .or_else(|| std::env::var("ANTHROPIC_API_KEY").ok())
    .or_else(|| std::env::var("OPENAI_API_KEY").ok())
    // Fall back to the llm.env file the start script reads (so doctor
    // works even when run outside the wrapper, see read_env_file_var doc).
    .or_else(|| read_env_file_var(&llm_env_path, "LLM_API_KEY"))
    .or_else(|| read_env_file_var(&llm_env_path, "ANTHROPIC_API_KEY"))
    .or_else(|| read_env_file_var(&llm_env_path, "OPENAI_API_KEY"))
    .or_else(|| read_env_file_var(&llm_env_path, "ARK_API_KEY"))
    .or_else(|| read_env_file_var(&llm_env_path, "MINIMAX_API_KEY"));

    match api_key {
        None => CheckResult {
            name: "llm",
            emoji: "🤖",
            status: Status::Fail,
            detail: "no LLM API key found in config or environment".into(),
        },
        Some(k) if k.trim().is_empty() => CheckResult {
            name: "llm",
            emoji: "🤖",
            status: Status::Fail,
            detail: "LLM API key is empty".into(),
        },
        Some(_) => CheckResult {
            name: "llm",
            emoji: "🤖",
            status: Status::Ok,
            detail: "API key present (connectivity not tested in offline mode)".into(),
        },
    }
}

fn check_users(config_dir: &std::path::Path) -> CheckResult {
    let path = config_dir.join("users.toml");
    if !path.exists() {
        return CheckResult {
            name: "users",
            emoji: "👤",
            status: Status::Fail,
            detail: "users.toml not found — run `cyberclaw onboard` first".into(),
        };
    }
    match std::fs::read_to_string(&path) {
        Ok(content) => match content.parse::<toml::Value>() {
            Ok(v) => {
                let admins = v
                    .get("users")
                    .and_then(|u| u.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter(|u| u.get("role").and_then(|r| r.as_str()) == Some("admin"))
                            .count()
                    })
                    .unwrap_or(0);
                if admins >= 1 {
                    CheckResult {
                        name: "users",
                        emoji: "👤",
                        status: Status::Ok,
                        detail: format!("{} admin(s) configured", admins),
                    }
                } else {
                    CheckResult {
                        name: "users",
                        emoji: "👤",
                        status: Status::Warn,
                        detail: "no admin users found in users.toml".into(),
                    }
                }
            }
            Err(e) => CheckResult {
                name: "users",
                emoji: "👤",
                status: Status::Fail,
                detail: format!("TOML parse error: {}", e),
            },
        },
        Err(e) => CheckResult {
            name: "users",
            emoji: "👤",
            status: Status::Fail,
            detail: format!("read error: {}", e),
        },
    }
}

fn check_governance(config_dir: &std::path::Path) -> CheckResult {
    // Look for governance rules in config or a dedicated governance.toml.
    let path = config_dir.join("governance.toml");
    if !path.exists() {
        // Acceptable: governance may be embedded in config.toml.
        let cfg_path = config_dir.join("config.toml");
        let rule_count = std::fs::read_to_string(&cfg_path)
            .ok()
            .and_then(|c| c.parse::<toml::Value>().ok())
            .and_then(|v| {
                v.get("governance")
                    .and_then(|g| g.get("rules"))
                    .and_then(|r| r.as_array())
                    .map(|a| a.len())
            })
            .unwrap_or(0);
        return if rule_count >= 5 {
            CheckResult {
                name: "governance",
                emoji: "🛡️",
                status: Status::Ok,
                detail: format!("{} governance rules in config.toml", rule_count),
            }
        } else {
            CheckResult {
                name: "governance",
                emoji: "🛡️",
                status: Status::Warn,
                detail: "governance.toml absent; fewer than 5 rules in config.toml".into(),
            }
        };
    }
    match std::fs::read_to_string(&path)
        .ok()
        .and_then(|c| c.parse::<toml::Value>().ok())
        .and_then(|v| v.get("rules").and_then(|r| r.as_array()).map(|a| a.len()))
    {
        Some(n) if n >= 5 => CheckResult {
            name: "governance",
            emoji: "🛡️",
            status: Status::Ok,
            detail: format!("{} governance rules", n),
        },
        Some(n) => CheckResult {
            name: "governance",
            emoji: "🛡️",
            status: Status::Warn,
            detail: format!("only {} governance rules (recommended ≥ 5)", n),
        },
        None => CheckResult {
            name: "governance",
            emoji: "🛡️",
            status: Status::Warn,
            detail: "could not count governance rules".into(),
        },
    }
}

/// Server-mediated connector count. Replaces the old config.toml-scanning
/// variant — connectors load from `ecosystem/connectors/` at server boot,
/// not from `~/.cyberclaw/config.toml`, so the file scan was structurally
/// misleading (always 0 in healthy installs).
async fn check_connectors(_config_dir: &std::path::Path) -> CheckResult {
    #[derive(serde::Deserialize)]
    struct StatusBrief {
        #[serde(default)]
        connectors: usize,
    }

    let server =
        std::env::var("CYBERCLAW_SERVER").unwrap_or_else(|_| "http://127.0.0.1:38090".into());
    let status_url = format!("{}/api/v2/status", server.trim_end_matches('/'));
    let token_path = cyberclaw_control_plane::wizard_engine::default_config_dir().join("cli-token");

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .no_proxy()
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return CheckResult {
                name: "connectors",
                emoji: "🔌",
                status: Status::Fail,
                detail: format!("could not build HTTP client: {}", e),
            };
        }
    };
    let mut req = client.get(&status_url);
    if let Ok(tok) = std::fs::read_to_string(&token_path) {
        let t = tok.trim();
        if !t.is_empty() {
            req = req.bearer_auth(t);
        }
    }
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            return CheckResult {
                name: "connectors",
                emoji: "🔌",
                status: Status::Warn,
                detail: format!("server unreachable at {}: {}", status_url, e),
            };
        }
    };
    if !resp.status().is_success() {
        return CheckResult {
            name: "connectors",
            emoji: "🔌",
            status: Status::Warn,
            detail: format!("/api/v2/status returned HTTP {}", resp.status()),
        };
    }
    let status: StatusBrief = match resp.json().await {
        Ok(s) => s,
        Err(e) => {
            return CheckResult {
                name: "connectors",
                emoji: "🔌",
                status: Status::Warn,
                detail: format!("status JSON parse failed: {}", e),
            };
        }
    };
    let count = status.connectors;
    if count >= 6 {
        CheckResult {
            name: "connectors",
            emoji: "🔌",
            status: Status::Ok,
            detail: format!("{} connectors registered (server)", count),
        }
    } else if count >= 1 {
        CheckResult {
            name: "connectors",
            emoji: "🔌",
            status: Status::Warn,
            detail: format!(
                "{} connectors registered (recommended ≥ 6); check ecosystem/connectors",
                count
            ),
        }
    } else {
        CheckResult {
            name: "connectors",
            emoji: "🔌",
            status: Status::Fail,
            detail: "0 connectors registered (ecosystem scan likely failed); check server log"
                .into(),
        }
    }
}

fn check_drift(config_dir: &std::path::Path) -> CheckResult {
    // Drift = mismatch between registered packages and on-disk manifests.
    // Without a running server we do a simple heuristic: check if
    // `.cyberclaw/drift.json` exists (written by server on reconcile).
    let path = config_dir.join("drift.json");
    if !path.exists() {
        return CheckResult {
            name: "drift",
            emoji: "📊",
            status: Status::Ok,
            detail: "no drift report found; assumed clean".into(),
        };
    }
    match std::fs::read_to_string(&path) {
        Ok(content) => match serde_json::from_str::<serde_json::Value>(&content) {
            Ok(v) => {
                let drift = v.get("drift_count").and_then(|d| d.as_u64()).unwrap_or(0);
                if drift == 0 {
                    CheckResult {
                        name: "drift",
                        emoji: "📊",
                        status: Status::Ok,
                        detail: "drift = 0".into(),
                    }
                } else {
                    CheckResult {
                        name: "drift",
                        emoji: "📊",
                        status: Status::Warn,
                        detail: format!("drift = {} package(s) out of sync", drift),
                    }
                }
            }
            Err(_) => CheckResult {
                name: "drift",
                emoji: "📊",
                status: Status::Warn,
                detail: "drift.json present but not valid JSON".into(),
            },
        },
        Err(e) => CheckResult {
            name: "drift",
            emoji: "📊",
            status: Status::Warn,
            detail: format!("could not read drift.json: {}", e),
        },
    }
}

async fn check_server() -> CheckResult {
    // Align with the rest of the CLI (see http_client.rs):
    //   env CYBERCLAW_SERVER (not *_URL), default 127.0.0.1:38090.
    let url = std::env::var("CYBERCLAW_SERVER").unwrap_or_else(|_| "http://127.0.0.1:38090".into());
    let health_url = format!("{}/health", url.trim_end_matches('/'));
    match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        // Bypass system proxy — same reasoning as http_client::build_client.
        .no_proxy()
        .build()
    {
        Ok(client) => match client.get(&health_url).send().await {
            Ok(resp) if resp.status().is_success() => CheckResult {
                name: "server",
                emoji: "🖥️",
                status: Status::Ok,
                detail: format!("{} reachable (HTTP {})", health_url, resp.status()),
            },
            Ok(resp) => {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                let body_preview = body.chars().take(120).collect::<String>();
                CheckResult {
                    name: "server",
                    emoji: "🖥️",
                    status: Status::Warn,
                    detail: format!(
                        "{} returned HTTP {} (body: {})",
                        health_url, status, body_preview
                    ),
                }
            }
            Err(e) => CheckResult {
                name: "server",
                emoji: "🖥️",
                status: Status::Warn,
                detail: format!("server unreachable at {}: {}", health_url, e),
            },
        },
        Err(e) => CheckResult {
            name: "server",
            emoji: "🖥️",
            status: Status::Fail,
            detail: format!("could not build HTTP client: {}", e),
        },
    }
}

/// Hit GET /api/v2/status (the server-side aggregate) so doctor reports the
/// **live** ecosystem load instead of just what config files claim. This
/// catches the failure mode where everything looks right on disk but server
/// boot dropped packages.
async fn check_ecosystem() -> CheckResult {
    #[derive(serde::Deserialize)]
    struct StatusBrief {
        #[serde(default)]
        agents: usize,
        #[serde(default)]
        skills: usize,
        #[serde(default)]
        connectors: usize,
        #[serde(default)]
        plugins: usize,
        #[serde(default)]
        capabilities: usize,
    }

    let server =
        std::env::var("CYBERCLAW_SERVER").unwrap_or_else(|_| "http://127.0.0.1:38090".into());
    let status_url = format!("{}/api/v2/status", server.trim_end_matches('/'));

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .no_proxy()
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return CheckResult {
                name: "ecosystem",
                emoji: "📦",
                status: Status::Fail,
                detail: format!("could not build HTTP client: {}", e),
            };
        }
    };

    // Inline token load: doctor doesn't carry a CliState here, so re-read
    // ~/.cyberclaw/cli-token directly. Anonymous probe still works if the
    // server permits unauthed status reads; if not, we'll see 401.
    let token_path = cyberclaw_control_plane::wizard_engine::default_config_dir().join("cli-token");
    let mut req = client.get(&status_url);
    if let Ok(tok) = std::fs::read_to_string(&token_path) {
        let t = tok.trim();
        if !t.is_empty() {
            req = req.bearer_auth(t);
        }
    }

    match req.send().await {
        Ok(resp) if resp.status().is_success() => match resp.json::<StatusBrief>().await {
            Ok(s) => {
                let detail = format!(
                    "agents={} skills={} connectors={} plugins={} capabilities={}",
                    s.agents, s.skills, s.connectors, s.plugins, s.capabilities
                );
                // Heuristic: an empty registry on a fresh boot is suspicious;
                // 0 connectors means even the built-in local connector failed
                // to load, which is always a real failure.
                let status = if s.connectors == 0 || s.capabilities == 0 {
                    Status::Fail
                } else if s.skills == 0 || s.agents == 0 {
                    Status::Warn
                } else {
                    Status::Ok
                };
                CheckResult {
                    name: "ecosystem",
                    emoji: "📦",
                    status,
                    detail,
                }
            }
            Err(e) => CheckResult {
                name: "ecosystem",
                emoji: "📦",
                status: Status::Warn,
                detail: format!("status JSON parse failed: {}", e),
            },
        },
        Ok(resp) => CheckResult {
            name: "ecosystem",
            emoji: "📦",
            status: Status::Warn,
            detail: format!("/api/v2/status returned HTTP {}", resp.status()),
        },
        Err(e) => CheckResult {
            name: "ecosystem",
            emoji: "📦",
            status: Status::Warn,
            detail: format!("server unreachable at {}: {}", status_url, e),
        },
    }
}

// ============================================================================
// Entry point
// ============================================================================

pub async fn handle_doctor(args: DoctorArgs) -> Result<()> {
    let config_dir = args
        .config_dir
        .unwrap_or_else(cyberclaw_control_plane::wizard_engine::default_config_dir);

    let filter: std::collections::HashSet<String> = args.check.into_iter().collect();
    let run_all = filter.is_empty();
    let should_run = |name: &str| run_all || filter.contains(name);

    let mut results: Vec<CheckResult> = Vec::new();

    if should_run("config") {
        results.push(check_config(&config_dir));
    }
    if should_run("llm") {
        results.push(check_llm(&config_dir));
    }
    if should_run("users") {
        results.push(check_users(&config_dir));
    }
    if should_run("governance") {
        results.push(check_governance(&config_dir));
    }
    if should_run("connectors") {
        results.push(check_connectors(&config_dir).await);
    }
    if should_run("drift") {
        results.push(check_drift(&config_dir));
    }
    if should_run("server") {
        results.push(check_server().await);
    }
    if should_run("ecosystem") {
        results.push(check_ecosystem().await);
    }

    match args.format.as_str() {
        "json" => print_json(&results),
        _ => print_table(&results),
    }

    let any_fail = results.iter().any(|r| r.status == Status::Fail);
    if any_fail {
        std::process::exit(1);
    }

    Ok(())
}

fn print_table(results: &[CheckResult]) {
    println!();
    println!("     CHECK          STATUS  DETAIL");
    println!("{}", "-".repeat(70));
    for r in results {
        println!(
            "{:<4} {:<14} {:<6}  {}",
            r.emoji,
            r.name,
            r.status.label(),
            r.detail
        );
    }
    println!();
    let ok = results.iter().filter(|r| r.status == Status::Ok).count();
    let warn = results.iter().filter(|r| r.status == Status::Warn).count();
    let fail = results.iter().filter(|r| r.status == Status::Fail).count();
    println!("Summary: {} OK  {} WARN  {} FAIL", ok, warn, fail);
    println!();
}

fn print_json(results: &[CheckResult]) {
    let arr: Vec<serde_json::Value> = results
        .iter()
        .map(|r| {
            serde_json::json!({
                "check": r.name,
                "status": r.status.label(),
                "detail": r.detail,
            })
        })
        .collect();
    println!("{}", serde_json::to_string_pretty(&arr).unwrap_or_default());
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn config_check_fails_when_absent() {
        let dir = tempdir().unwrap();
        let r = check_config(dir.path());
        assert_eq!(r.status, Status::Fail);
    }

    #[test]
    fn config_check_ok_with_valid_toml() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "llm_provider = \"anthropic\"\n",
        )
        .unwrap();
        let r = check_config(dir.path());
        assert_eq!(r.status, Status::Ok);
    }

    #[test]
    fn config_check_fails_with_invalid_toml() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "not [ valid toml\n").unwrap();
        let r = check_config(dir.path());
        assert_eq!(r.status, Status::Fail);
    }

    #[test]
    fn llm_check_ok_when_env_key_set() {
        let dir = tempdir().unwrap();
        std::env::set_var("ANTHROPIC_API_KEY", "sk-test");
        let r = check_llm(dir.path());
        std::env::remove_var("ANTHROPIC_API_KEY");
        assert_eq!(r.status, Status::Ok);
    }

    #[test]
    fn drift_check_ok_when_no_drift_file() {
        let dir = tempdir().unwrap();
        let r = check_drift(dir.path());
        assert_eq!(r.status, Status::Ok);
    }

    #[test]
    fn drift_check_warn_when_nonzero_drift() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("drift.json"), r#"{"drift_count": 3}"#).unwrap();
        let r = check_drift(dir.path());
        assert_eq!(r.status, Status::Warn);
    }
}
