//! Admin onboarding endpoints.
//!
//! Sprint 8 L4 (existing) + Sprint 9 onboarding wizard backend (new).
//!
//! # Routes
//!
//! | Route | Auth | Purpose |
//! |---|---|---|
//! | `GET  /admin/onboarding/status`       | JWT | Returns `{ needs_onboarding, onboarded_at }` |
//! | `POST /admin/onboarding/complete`     | JWT | Sets `onboarded_at = now` |
//! | `POST /admin/onboarding/test-llm`     | JWT | Ping LLM with minimal completion, returns latency |
//! | `POST /admin/onboarding/llm-config`   | JWT | Persist LLM config to `~/.cyberclaw/config.toml` |
//! | `POST /admin/onboarding/skill-bundle` | JWT | Choose skill bundle, persist to onboarding.toml |
//! | `POST /admin/onboarding/governance`   | JWT | Write governance preset to governance-policy.toml |
//! | `POST /admin/onboarding/scan-mcp`     | JWT | Scan system MCP config files, return server list |
//! | `POST /admin/onboarding/mcp-connect`  | JWT | Save enabled MCP server list |
//! | `POST /admin/onboarding/scan-skills`  | JWT | Scan `~/.claude/skills/` for SKILL.md files |
//! | `POST /admin/onboarding/import-skills`| JWT | Copy selected skills into `~/.cyberclaw/skills/` |

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use axum::{
    extract::{Extension, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use cyberclaw_core::users::UsersConfig;
use cyberclaw_llm::prelude::{AnthropicClient, ArkClient, GenericOpenAiClient, OpenAiClient};
use cyberclaw_llm::{ChatRequest, LlmClient, Message};

use crate::audit::{AuditEntry, AuditKind, AuditResult};
use crate::error::ApiError;
use crate::middleware::auth::Claims;
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Existing types (Sprint 8 L4)
// ---------------------------------------------------------------------------

/// Response for `GET /admin/onboarding/status`.
#[derive(Debug, Serialize)]
pub struct OnboardingStatus {
    pub needs_onboarding: bool,
    pub onboarded_at: Option<String>,
}

/// Response for `POST /admin/onboarding/complete`.
#[derive(Debug, Serialize)]
pub struct OnboardingCompleteResponse {
    pub ok: bool,
    pub onboarded_at: String,
}

// ---------------------------------------------------------------------------
// New request/response types (Sprint 9)
// ---------------------------------------------------------------------------

/// `POST /admin/onboarding/test-llm` request.
#[derive(Debug, Deserialize)]
pub struct TestLlmRequest {
    pub provider: String,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
}

/// `POST /admin/onboarding/test-llm` response.
#[derive(Debug, Serialize)]
pub struct TestLlmResponse {
    pub ok: bool,
    pub latency_ms: u64,
    pub error: Option<String>,
}

/// `POST /admin/onboarding/llm-config` request.
#[derive(Debug, Deserialize)]
pub struct LlmConfigRequest {
    pub provider: String,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
}

/// `POST /admin/onboarding/llm-config` response.
#[derive(Debug, Serialize)]
pub struct LlmConfigResponse {
    pub ok: bool,
}

/// `POST /admin/onboarding/skill-bundle` request.
#[derive(Debug, Deserialize)]
pub struct SkillBundleRequest {
    pub bundle: SkillBundleChoice,
}

/// Supported bundle presets.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SkillBundleChoice {
    All,
    Developer,
    Minimal,
}

impl SkillBundleChoice {
    fn skill_count(self) -> usize {
        match self {
            Self::All => 42,
            Self::Developer => 18,
            Self::Minimal => 5,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Developer => "developer",
            Self::Minimal => "minimal",
        }
    }
}

/// `POST /admin/onboarding/skill-bundle` response.
#[derive(Debug, Serialize)]
pub struct SkillBundleResponse {
    pub ok: bool,
    pub skill_count: usize,
}

/// `POST /admin/onboarding/governance` request.
#[derive(Debug, Deserialize)]
pub struct GovernanceRequest {
    pub preset: GovernancePreset,
}

/// Governance policy presets.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GovernancePreset {
    Strict,
    Balanced,
    Permissive,
}

impl GovernancePreset {
    fn as_str(self) -> &'static str {
        match self {
            Self::Strict => "strict",
            Self::Balanced => "balanced",
            Self::Permissive => "permissive",
        }
    }

    fn toml_snippet(self) -> &'static str {
        match self {
            Self::Strict => {
                r#"# Governance policy — strict preset
[policy]
preset = "strict"
require_review_for_dangerous = true
max_capability_risk_level = "low"
auto_approve_threshold = 0.0
deny_unknown_capabilities = true
audit_all_executions = true
"#
            }
            Self::Balanced => {
                r#"# Governance policy — balanced preset
[policy]
preset = "balanced"
require_review_for_dangerous = true
max_capability_risk_level = "medium"
auto_approve_threshold = 0.7
deny_unknown_capabilities = false
audit_all_executions = true
"#
            }
            Self::Permissive => {
                r#"# Governance policy — permissive preset
[policy]
preset = "permissive"
require_review_for_dangerous = false
max_capability_risk_level = "high"
auto_approve_threshold = 0.95
deny_unknown_capabilities = false
audit_all_executions = false
"#
            }
        }
    }
}

/// `POST /admin/onboarding/governance` response.
#[derive(Debug, Serialize)]
pub struct GovernanceResponse {
    pub ok: bool,
    pub preset: String,
}

/// A discovered MCP server entry.
#[derive(Debug, Serialize, Clone)]
pub struct McpServerEntry {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
}

/// `POST /admin/onboarding/scan-mcp` response.
#[derive(Debug, Serialize)]
pub struct ScanMcpResponse {
    pub servers: Vec<McpServerEntry>,
}

/// `POST /admin/onboarding/mcp-connect` request.
#[derive(Debug, Deserialize)]
pub struct McpConnectRequest {
    pub enabled_servers: Vec<String>,
}

/// `POST /admin/onboarding/mcp-connect` response.
#[derive(Debug, Serialize)]
pub struct McpConnectResponse {
    pub ok: bool,
    pub enabled_count: usize,
}

/// A discovered local skill.
#[derive(Debug, Serialize)]
pub struct ScannedSkillEntry {
    pub name: String,
    pub path: String,
    pub description: String,
}

/// `POST /admin/onboarding/scan-skills` response.
#[derive(Debug, Serialize)]
pub struct ScanSkillsResponse {
    pub skills: Vec<ScannedSkillEntry>,
}

/// `POST /admin/onboarding/import-skills` request.
#[derive(Debug, Deserialize)]
pub struct ImportSkillsRequest {
    pub paths: Vec<String>,
}

/// `POST /admin/onboarding/import-skills` response.
#[derive(Debug, Serialize)]
pub struct ImportSkillsResponse {
    pub ok: bool,
    pub imported: usize,
    pub errors: Vec<String>,
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the onboarding router (mounted under authenticated lane).
pub fn create_admin_onboarding_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/admin/onboarding/status", get(onboarding_status))
        .route("/admin/onboarding/complete", post(onboarding_complete))
        .route("/admin/onboarding/test-llm", post(onboarding_test_llm))
        .route("/admin/onboarding/llm-config", post(onboarding_llm_config))
        .route(
            "/admin/onboarding/skill-bundle",
            post(onboarding_skill_bundle),
        )
        .route("/admin/onboarding/governance", post(onboarding_governance))
        .route("/admin/onboarding/scan-mcp", post(onboarding_scan_mcp))
        .route(
            "/admin/onboarding/mcp-connect",
            post(onboarding_mcp_connect),
        )
        .route(
            "/admin/onboarding/scan-skills",
            post(onboarding_scan_skills),
        )
        .route(
            "/admin/onboarding/import-skills",
            post(onboarding_import_skills),
        )
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Returns `~/.cyberclaw/` as an absolute path, creating it if absent.
async fn cyberclaw_config_dir() -> Result<PathBuf, ApiError> {
    let home = home_dir()?;
    let dir = home.join(".cyberclaw");
    tokio::fs::create_dir_all(&dir).await.map_err(|e| {
        ApiError::InternalError(format!("failed to create ~/.cyberclaw dir: {}", e))
    })?;
    Ok(dir)
}

/// Returns the user's home directory.
fn home_dir() -> Result<PathBuf, ApiError> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| ApiError::InternalError("HOME env var not set".to_string()))
}

/// Sanitise an API key for logging: show first 4 + last 4 chars only.
fn redact_api_key(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() <= 8 {
        return "*".repeat(chars.len());
    }
    let prefix: String = chars[..4].iter().collect();
    let suffix: String = chars[chars.len() - 4..].iter().collect();
    format!("{}...{}", prefix, suffix)
}

/// Canonicalize `path` and confirm it is under `workspace`.
/// Returns `Err` for path traversal attempts.
fn safe_canonicalize(path: &Path, workspace: &Path) -> Result<PathBuf, String> {
    let canonical = path
        .canonicalize()
        .map_err(|e| format!("cannot resolve path {}: {}", path.display(), e))?;
    if !canonical.starts_with(workspace) {
        return Err(format!(
            "path {} is outside the allowed workspace {}",
            canonical.display(),
            workspace.display()
        ));
    }
    Ok(canonical)
}

/// Recursively copy `src` directory into `dst`, creating `dst` if needed.
/// Sync — called from async context via `tokio::task::spawn_blocking`.
fn copy_dir_sync(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let dest_path = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_sync(&entry.path(), &dest_path)?;
        } else {
            std::fs::copy(entry.path(), &dest_path)?;
        }
    }
    Ok(())
}

/// Build an LLM client from provider name, optional key, optional base_url.
/// Returns `Err(human message)` on invalid configuration.
///
/// Made `pub(crate)` so the MoA admin endpoint can fan out heterogenous
/// proposer clients (one per configured provider) without duplicating the
/// provider-routing match.
pub(crate) fn build_llm_client(
    provider: &str,
    api_key: Option<&str>,
    base_url: Option<&str>,
) -> Result<Box<dyn LlmClient>, String> {
    let key = api_key.unwrap_or("").to_string();
    match provider.to_lowercase().as_str() {
        "openai" => {
            let url = base_url.unwrap_or("https://api.openai.com/v1");
            OpenAiClient::new(key, url)
                .map(|c| Box::new(c) as Box<dyn LlmClient>)
                .map_err(|e| e.to_string())
        }
        "anthropic" => {
            let url = base_url.unwrap_or("https://api.anthropic.com");
            AnthropicClient::new(key, url)
                .map(|c| Box::new(c) as Box<dyn LlmClient>)
                .map_err(|e| e.to_string())
        }
        "ark" | "volcengine" => {
            let url = base_url.unwrap_or("https://ark.cn-beijing.volces.com/api/v3");
            ArkClient::new(key, url)
                .map(|c| Box::new(c) as Box<dyn LlmClient>)
                .map_err(|e| e.to_string())
        }
        _ => {
            // Generic / DeepSeek / Ollama / any OpenAI-compatible
            let url = base_url.unwrap_or("http://localhost:11434/v1");
            GenericOpenAiClient::new(api_key.map(|s| s.to_string()), url)
                .map(|c| Box::new(c) as Box<dyn LlmClient>)
                .map_err(|e| e.to_string())
        }
    }
}

// ---------------------------------------------------------------------------
// Handler: GET /admin/onboarding/status
// ---------------------------------------------------------------------------

/// `GET /admin/onboarding/status` — return whether the caller still needs to
/// complete the admin console onboarding.
pub async fn onboarding_status(
    Extension(claims): Extension<Claims>,
) -> Result<Json<OnboardingStatus>, ApiError> {
    let path = UsersConfig::default_path();
    let cfg = UsersConfig::load_from_path(&path).map_err(|e| {
        warn!(path = %path.display(), error = %e, "onboarding status: users.toml load failed");
        ApiError::InternalError(format!("failed to load users config: {}", e))
    })?;
    let record = cfg.find(&claims.sub).ok_or_else(|| {
        warn!(user_id = %claims.sub, "onboarding status: jwt references unknown operator");
        ApiError::Unauthorized("operator no longer exists".to_string())
    })?;
    Ok(Json(OnboardingStatus {
        needs_onboarding: record.onboarded_at.is_none(),
        onboarded_at: record.onboarded_at.clone(),
    }))
}

// ---------------------------------------------------------------------------
// Handler: POST /admin/onboarding/complete
// ---------------------------------------------------------------------------

/// `POST /admin/onboarding/complete` — mark the caller's onboarding done.
pub async fn onboarding_complete(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
) -> Result<(StatusCode, Json<OnboardingCompleteResponse>), ApiError> {
    let path = UsersConfig::default_path();
    let mut cfg = UsersConfig::load_from_path(&path).map_err(|e| {
        warn!(path = %path.display(), error = %e, "onboarding complete: users.toml load failed");
        ApiError::InternalError(format!("failed to load users config: {}", e))
    })?;
    let mut record = cfg
        .find(&claims.sub)
        .cloned()
        .ok_or_else(|| ApiError::Unauthorized("operator no longer exists".to_string()))?;

    // True idempotency: preserve the original onboarded_at on repeat POSTs.
    // The OnboardingBanner client retries safely if /complete fails after
    // /llm-config succeeded; without this preservation, retries would drift
    // the timestamp and lose audit fidelity ("when did this user first
    // onboard?").
    let now = chrono::Utc::now().to_rfc3339();
    let already_onboarded = record.onboarded_at.is_some();
    if record.onboarded_at.is_none() {
        record.onboarded_at = Some(now.clone());
    }
    let final_ts = record.onboarded_at.clone().unwrap_or_else(|| now.clone());
    cfg.upsert(record);
    cfg.save_to_path(&path).map_err(|e| {
        warn!(path = %path.display(), error = %e, "onboarding complete: users.toml save failed");
        ApiError::InternalError(format!("failed to save users config: {}", e))
    })?;

    if already_onboarded {
        info!(user_id = %claims.sub, onboarded_at = %final_ts, "admin onboarding /complete re-invoked (idempotent no-op on timestamp)");
    } else {
        info!(user_id = %claims.sub, onboarded_at = %final_ts, "admin onboarding marked complete");
    }
    let now = final_ts; // local rename so the rest of the handler keeps the same variable.

    if let Some(sink) = state.audit.as_ref() {
        sink.record(AuditEntry::now(
            claims.sub.as_str().to_string(),
            AuditKind::Mutation,
            "onboarding.complete".to_string(),
            None,
            serde_json::json!({ "onboarded_at": now }),
            AuditResult::Success,
        ))
        .await;
    }

    Ok((
        StatusCode::OK,
        Json(OnboardingCompleteResponse {
            ok: true,
            onboarded_at: now,
        }),
    ))
}

// ---------------------------------------------------------------------------
// Handler: POST /admin/onboarding/test-llm
// ---------------------------------------------------------------------------

/// `POST /admin/onboarding/test-llm` — send a minimal "ping" completion to the
/// configured LLM and return latency.
///
/// Timeout is capped at 9 s to stay under the < 10 s requirement.
pub async fn onboarding_test_llm(
    Extension(_claims): Extension<Claims>,
    Json(req): Json<TestLlmRequest>,
) -> Result<Json<TestLlmResponse>, ApiError> {
    let provider = req.provider.trim().to_string();
    if provider.is_empty() {
        return Err(ApiError::InvalidRequest("provider is required".to_string()));
    }
    if req
        .api_key
        .as_deref()
        .map(|k| k.is_empty())
        .unwrap_or(false)
    {
        return Err(ApiError::InvalidRequest(
            "api_key must not be empty if provided".to_string(),
        ));
    }

    let client = match build_llm_client(&provider, req.api_key.as_deref(), req.base_url.as_deref())
    {
        Ok(c) => c,
        Err(e) => {
            return Ok(Json(TestLlmResponse {
                ok: false,
                latency_ms: 0,
                error: Some(format!("failed to create client: {}", e)),
            }));
        }
    };

    let ping_request = ChatRequest {
        model: default_model_for(&provider),
        messages: vec![Message::user("ping")],
        temperature: Some(0.0),
        max_tokens: Some(5),
        stream: Some(false),
        tools: None,
        tool_choice: None,
        top_p: None,
        extra: std::collections::HashMap::new(),
    };

    let start = Instant::now();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(9),
        client.chat_completion(ping_request),
    )
    .await;

    let latency_ms = start.elapsed().as_millis() as u64;

    match result {
        Ok(Ok(_)) => {
            info!(provider = %provider, latency_ms, "onboarding test-llm success");
            Ok(Json(TestLlmResponse {
                ok: true,
                latency_ms,
                error: None,
            }))
        }
        Ok(Err(e)) => {
            warn!(provider = %provider, error = %e, "onboarding test-llm completion failed");
            Ok(Json(TestLlmResponse {
                ok: false,
                latency_ms,
                error: Some(e.to_string()),
            }))
        }
        Err(_timeout) => {
            warn!(provider = %provider, "onboarding test-llm timed out");
            Ok(Json(TestLlmResponse {
                ok: false,
                latency_ms,
                error: Some("request timed out (> 9s)".to_string()),
            }))
        }
    }
}

/// Pick a sensible default model name per provider for the ping request.
fn default_model_for(provider: &str) -> String {
    match provider.to_lowercase().as_str() {
        "openai" => "gpt-4o-mini".to_string(),
        "anthropic" => "claude-haiku-4-5".to_string(),
        "ark" | "volcengine" => "doubao-lite-4k".to_string(),
        _ => "llama3".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Handler: POST /admin/onboarding/llm-config
// ---------------------------------------------------------------------------

/// `POST /admin/onboarding/llm-config` — persist LLM config to
/// `~/.cyberclaw/config.toml`.  The `api_key` is never logged in plaintext.
pub async fn onboarding_llm_config(
    Extension(claims): Extension<Claims>,
    Json(req): Json<LlmConfigRequest>,
) -> Result<Json<LlmConfigResponse>, ApiError> {
    let provider = req.provider.trim().to_string();
    if provider.is_empty() {
        return Err(ApiError::InvalidRequest("provider is required".to_string()));
    }

    let config_dir = cyberclaw_config_dir().await?;
    let config_path = config_dir.join("config.toml");

    // Use the toml crate's serializer so values containing quotes, newlines, or
    // bracketed section headers cannot inject arbitrary TOML keys. Building the
    // file via `format!()` with raw `{api_key}` interpolation was a TOML
    // injection vector — a payload like `"\n[other]\nfoo = "bar"` would create
    // an attacker-controlled section overriding governance settings.
    #[derive(serde::Serialize)]
    struct LlmConfigToml<'a> {
        provider: &'a str,
        api_key: &'a str,
        base_url: &'a str,
    }
    #[derive(serde::Serialize)]
    struct LlmConfigRoot<'a> {
        llm: LlmConfigToml<'a>,
    }
    let api_key_val = req.api_key.as_deref().unwrap_or("");
    let base_url_val = req.base_url.as_deref().unwrap_or("");
    let toml_content = toml::to_string_pretty(&LlmConfigRoot {
        llm: LlmConfigToml {
            provider: &provider,
            api_key: api_key_val,
            base_url: base_url_val,
        },
    })
    .map_err(|e| ApiError::InternalError(format!("failed to serialize config.toml: {e}")))?;
    let toml_content =
        format!("# CyberClaw LLM configuration — written by onboarding wizard\n{toml_content}");

    tokio::fs::write(&config_path, toml_content)
        .await
        .map_err(|e| {
            warn!(path = %config_path.display(), error = %e, "llm-config: write failed");
            ApiError::InternalError(format!("failed to write config.toml: {}", e))
        })?;

    // Restrict permissions to owner-only — the file contains the LLM API key
    // in plaintext. Without this, the default umask (often 0o022) would leave
    // it world-readable.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) =
            tokio::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o600)).await
        {
            warn!(path = %config_path.display(), error = %e, "llm-config: chmod 0o600 failed");
        }
    }

    let redacted = req
        .api_key
        .as_deref()
        .map(redact_api_key)
        .unwrap_or_else(|| "<none>".to_string());

    info!(
        user_id = %claims.sub,
        provider = %provider,
        api_key  = %redacted,
        path     = %config_path.display(),
        "llm-config saved"
    );

    Ok(Json(LlmConfigResponse { ok: true }))
}

// ---------------------------------------------------------------------------
// Handler: POST /admin/onboarding/skill-bundle
// ---------------------------------------------------------------------------

/// `POST /admin/onboarding/skill-bundle` — write chosen bundle preset to
/// `~/.cyberclaw/onboarding.toml` and return expected skill count.
pub async fn onboarding_skill_bundle(
    Extension(claims): Extension<Claims>,
    Json(req): Json<SkillBundleRequest>,
) -> Result<Json<SkillBundleResponse>, ApiError> {
    let config_dir = cyberclaw_config_dir().await?;
    let path = config_dir.join("onboarding.toml");

    let content = format!(
        "# CyberClaw onboarding configuration\n\
         [onboarding]\n\
         skill_bundle = \"{}\"\n",
        req.bundle.as_str()
    );

    tokio::fs::write(&path, content).await.map_err(|e| {
        warn!(path = %path.display(), error = %e, "skill-bundle: write failed");
        ApiError::InternalError(format!("failed to write onboarding.toml: {}", e))
    })?;

    let skill_count = req.bundle.skill_count();
    info!(
        user_id = %claims.sub,
        bundle = %req.bundle.as_str(),
        skill_count,
        "skill bundle selected"
    );

    Ok(Json(SkillBundleResponse {
        ok: true,
        skill_count,
    }))
}

// ---------------------------------------------------------------------------
// Handler: POST /admin/onboarding/governance
// ---------------------------------------------------------------------------

/// `POST /admin/onboarding/governance` — emit governance policy TOML to
/// `~/.cyberclaw/governance-policy.toml`.
pub async fn onboarding_governance(
    Extension(claims): Extension<Claims>,
    Json(req): Json<GovernanceRequest>,
) -> Result<Json<GovernanceResponse>, ApiError> {
    let config_dir = cyberclaw_config_dir().await?;
    let path = config_dir.join("governance-policy.toml");

    tokio::fs::write(&path, req.preset.toml_snippet())
        .await
        .map_err(|e| {
            warn!(path = %path.display(), error = %e, "governance: write failed");
            ApiError::InternalError(format!("failed to write governance-policy.toml: {}", e))
        })?;

    info!(
        user_id = %claims.sub,
        preset = %req.preset.as_str(),
        "governance policy saved"
    );

    Ok(Json(GovernanceResponse {
        ok: true,
        preset: req.preset.as_str().to_string(),
    }))
}

// ---------------------------------------------------------------------------
// Handler: POST /admin/onboarding/scan-mcp
// ---------------------------------------------------------------------------

/// Known MCP config locations to scan.
const MCP_CONFIG_PATHS: &[&str] = &[
    ".claude-code-config.json",
    ".cursor/mcp.json",
    ".codeium/mcp.json",
];

/// `POST /admin/onboarding/scan-mcp` — look for MCP server definitions in
/// well-known config files under `$HOME`.
pub async fn onboarding_scan_mcp(
    Extension(_claims): Extension<Claims>,
) -> Result<Json<ScanMcpResponse>, ApiError> {
    let home = home_dir()?;
    let mut servers: Vec<McpServerEntry> = Vec::new();

    for rel in MCP_CONFIG_PATHS {
        let path = home.join(rel);
        match tokio::fs::read_to_string(&path).await {
            Ok(content) => {
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&content) {
                    extract_mcp_servers(&parsed, &mut servers);
                }
            }
            Err(_) => {
                // File absent or unreadable — skip silently.
            }
        }
    }

    Ok(Json(ScanMcpResponse { servers }))
}

/// Extract server entries from a parsed MCP config JSON.
/// Handles both `{ "mcpServers": { "name": { "command": ..., "args": [...] } } }`
/// and `{ "servers": [...] }` layouts.
fn extract_mcp_servers(value: &serde_json::Value, out: &mut Vec<McpServerEntry>) {
    // Layout 1: { "mcpServers": { "<name>": { "command": "...", "args": [...] } } }
    if let Some(map) = value.get("mcpServers").and_then(|v| v.as_object()) {
        for (name, cfg) in map {
            let command = cfg
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let args = cfg
                .get("args")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|a| a.as_str())
                        .map(|s| s.to_string())
                        .collect()
                })
                .unwrap_or_default();
            if !command.is_empty() {
                out.push(McpServerEntry {
                    name: name.clone(),
                    command,
                    args,
                });
            }
        }
        return;
    }

    // Layout 2: { "servers": [ { "name": "...", "command": "...", "args": [...] } ] }
    if let Some(arr) = value.get("servers").and_then(|v| v.as_array()) {
        for entry in arr {
            let name = entry
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let command = entry
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let args = entry
                .get("args")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|a| a.as_str())
                        .map(|s| s.to_string())
                        .collect()
                })
                .unwrap_or_default();
            if !command.is_empty() {
                out.push(McpServerEntry {
                    name,
                    command,
                    args,
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Handler: POST /admin/onboarding/mcp-connect
// ---------------------------------------------------------------------------

/// `POST /admin/onboarding/mcp-connect` — persist enabled MCP server list.
/// Wire-up to live MCP is a future sprint; this sprint just saves the selection.
pub async fn onboarding_mcp_connect(
    Extension(claims): Extension<Claims>,
    Json(req): Json<McpConnectRequest>,
) -> Result<Json<McpConnectResponse>, ApiError> {
    let config_dir = cyberclaw_config_dir().await?;
    let path = config_dir.join("mcp-enabled.toml");

    // Validate server names — reject names containing path separators or
    // control characters to prevent log injection.
    for name in &req.enabled_servers {
        if name.contains('/') || name.contains('\\') || name.contains('\n') {
            return Err(ApiError::InvalidRequest(format!(
                "invalid server name: '{}'",
                name
            )));
        }
    }

    let names_toml = req
        .enabled_servers
        .iter()
        .map(|n| format!("  \"{}\",\n", n))
        .collect::<String>();

    let content = format!(
        "# CyberClaw MCP enabled servers\n\
         [mcp]\n\
         enabled_servers = [\n\
         {names_toml}]\n"
    );

    tokio::fs::write(&path, content).await.map_err(|e| {
        warn!(path = %path.display(), error = %e, "mcp-connect: write failed");
        ApiError::InternalError(format!("failed to write mcp-enabled.toml: {}", e))
    })?;

    let enabled_count = req.enabled_servers.len();
    info!(user_id = %claims.sub, enabled_count, "mcp-connect saved");

    Ok(Json(McpConnectResponse {
        ok: true,
        enabled_count,
    }))
}

// ---------------------------------------------------------------------------
// Handler: POST /admin/onboarding/scan-skills
// ---------------------------------------------------------------------------

/// `POST /admin/onboarding/scan-skills` — recursively scan `~/.claude/skills/`
/// for `SKILL.md` files and return discovered skills.
pub async fn onboarding_scan_skills(
    Extension(_claims): Extension<Claims>,
) -> Result<Json<ScanSkillsResponse>, ApiError> {
    let home = home_dir()?;
    let skills_root = home.join(".claude").join("skills");

    // If the directory doesn't exist, return empty list without error.
    if !skills_root.exists() {
        return Ok(Json(ScanSkillsResponse { skills: vec![] }));
    }

    // Canonicalize workspace root for path traversal checks.
    let workspace = skills_root
        .canonicalize()
        .map_err(|e| ApiError::InternalError(format!("cannot canonicalize skills root: {}", e)))?;

    let skills = tokio::task::spawn_blocking(move || scan_skills_sync(&workspace))
        .await
        .map_err(|e| ApiError::InternalError(format!("scan task panicked: {}", e)))??;

    Ok(Json(ScanSkillsResponse { skills }))
}

/// Synchronous recursive scan for SKILL.md files.
fn scan_skills_sync(workspace: &Path) -> Result<Vec<ScannedSkillEntry>, ApiError> {
    let mut skills = Vec::new();
    scan_dir_for_skills(workspace, workspace, &mut skills);
    Ok(skills)
}

fn scan_dir_for_skills(workspace: &Path, dir: &Path, out: &mut Vec<ScannedSkillEntry>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Path traversal guard.
            if let Ok(canonical) = path.canonicalize() {
                if canonical.starts_with(workspace) {
                    scan_dir_for_skills(workspace, &path, out);
                }
            }
        } else if path.file_name().map(|n| n == "SKILL.md").unwrap_or(false) {
            let description = extract_skill_description(&path);
            let name = path
                .parent()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "unknown".to_string());
            out.push(ScannedSkillEntry {
                name,
                path: path.to_string_lossy().to_string(),
                description,
            });
        }
    }
}

/// Extract the first non-heading, non-empty line from a SKILL.md as description.
fn extract_skill_description(path: &Path) -> String {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return String::new(),
    };
    for line in content.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty() && !trimmed.starts_with('#') {
            return trimmed.chars().take(120).collect();
        }
    }
    String::new()
}

// ---------------------------------------------------------------------------
// Handler: POST /admin/onboarding/import-skills
// ---------------------------------------------------------------------------

/// `POST /admin/onboarding/import-skills` — copy selected skill directories
/// (each containing a SKILL.md) into `~/.cyberclaw/skills/<name>/`.
///
/// Only paths under `~/.claude/skills/` are accepted; attempts to reference
/// paths outside that workspace are rejected with `400`.
pub async fn onboarding_import_skills(
    Extension(claims): Extension<Claims>,
    Json(req): Json<ImportSkillsRequest>,
) -> Result<Json<ImportSkillsResponse>, ApiError> {
    if req.paths.is_empty() {
        return Ok(Json(ImportSkillsResponse {
            ok: true,
            imported: 0,
            errors: vec![],
        }));
    }

    let home = home_dir()?;
    let skills_root = home.join(".claude").join("skills");
    let dest_root = home.join(".cyberclaw").join("skills");

    // Ensure destination exists.
    tokio::fs::create_dir_all(&dest_root)
        .await
        .map_err(|e| ApiError::InternalError(format!("failed to create dest skills dir: {}", e)))?;

    let workspace = if skills_root.exists() {
        skills_root.canonicalize().map_err(|e| {
            ApiError::InternalError(format!("cannot canonicalize skills root: {}", e))
        })?
    } else {
        // Skills root doesn't exist — every path will fail the workspace check.
        skills_root.clone()
    };

    let paths = req.paths.clone();
    let dest_root_clone = dest_root.clone();

    let (imported, errors) = tokio::task::spawn_blocking(move || {
        import_skills_sync(&paths, &workspace, &dest_root_clone)
    })
    .await
    .map_err(|e| ApiError::InternalError(format!("import task panicked: {}", e)))?;

    info!(
        user_id = %claims.sub,
        imported,
        errors = errors.len(),
        "skills imported"
    );

    Ok(Json(ImportSkillsResponse {
        ok: errors.is_empty(),
        imported,
        errors,
    }))
}

fn import_skills_sync(
    paths: &[String],
    workspace: &Path,
    dest_root: &Path,
) -> (usize, Vec<String>) {
    let mut imported = 0usize;
    let mut errors: Vec<String> = Vec::new();

    for path_str in paths {
        let src = Path::new(path_str);

        // The input may point to a SKILL.md file itself or its parent directory.
        let skill_dir = if src.file_name().map(|n| n == "SKILL.md").unwrap_or(false) {
            match src.parent() {
                Some(p) => p.to_path_buf(),
                None => {
                    errors.push(format!("invalid path: {}", path_str));
                    continue;
                }
            }
        } else {
            src.to_path_buf()
        };

        // Path traversal check: canonicalize and verify workspace containment.
        let canonical = match safe_canonicalize(&skill_dir, workspace) {
            Ok(c) => c,
            Err(e) => {
                errors.push(e);
                continue;
            }
        };

        // Ensure the directory contains a SKILL.md.
        if !canonical.join("SKILL.md").exists() {
            errors.push(format!("no SKILL.md found in {}", canonical.display()));
            continue;
        }

        let name = canonical
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        let dest = dest_root.join(&name);

        if let Err(e) = copy_dir_sync(&canonical, &dest) {
            errors.push(format!("failed to copy {}: {}", canonical.display(), e));
            continue;
        }

        imported += 1;
    }

    (imported, errors)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::admin::login::tests::{build_test_state, jwt_for, RestoreHome};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::middleware as axum_middleware;
    use axum::Router;
    use serial_test::serial;
    use tower::ServiceExt;

    fn onboarding_app(state: Arc<AppState>) -> Router {
        create_admin_onboarding_router()
            .layer(axum_middleware::from_fn_with_state(
                state.clone(),
                crate::middleware::jwt_auth,
            ))
            .with_state(state)
    }

    async fn post_json(
        app: &Router,
        path: &str,
        body: serde_json::Value,
        token: &str,
    ) -> (StatusCode, serde_json::Value) {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("content-type", "application/json")
                    .header("authorization", format!("Bearer {}", token))
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_else(
            |_| serde_json::json!({ "raw": String::from_utf8_lossy(&bytes).to_string() }),
        );
        (status, json)
    }

    async fn post_no_body(
        app: &Router,
        path: &str,
        token: &str,
    ) -> (StatusCode, serde_json::Value) {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("content-type", "application/json")
                    .header("authorization", format!("Bearer {}", token))
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value =
            serde_json::from_slice(&bytes).unwrap_or_else(|_| serde_json::json!({}));
        (status, json)
    }

    // --- test-llm ---

    #[tokio::test]
    async fn test_llm_rejects_empty_provider() {
        let state = build_test_state();
        let token = jwt_for(&state, "op-test");
        let app = onboarding_app(state);
        let (status, body) = post_json(
            &app,
            "/admin/onboarding/test-llm",
            serde_json::json!({ "provider": "", "api_key": "sk-xxx" }),
            &token,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "body: {:?}", body);
    }

    #[tokio::test]
    async fn test_llm_returns_response_shape_for_unreachable_endpoint() {
        // Using a definitely-unreachable URL should return ok=false with an error,
        // but the response shape must still be valid JSON with the right fields.
        let state = build_test_state();
        let token = jwt_for(&state, "op-test");
        let app = onboarding_app(state);
        let (status, body) = post_json(
            &app,
            "/admin/onboarding/test-llm",
            serde_json::json!({
                "provider": "generic",
                "api_key": "test",
                "base_url": "http://127.0.0.1:19999/v1"
            }),
            &token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.get("ok").is_some(), "missing ok field");
        assert!(body.get("latency_ms").is_some(), "missing latency_ms field");
        // ok should be false because the endpoint is unreachable
        assert_eq!(body["ok"], false);
    }

    #[tokio::test]
    async fn test_llm_rejects_empty_api_key() {
        let state = build_test_state();
        let token = jwt_for(&state, "op-test");
        let app = onboarding_app(state);
        let (status, _body) = post_json(
            &app,
            "/admin/onboarding/test-llm",
            serde_json::json!({ "provider": "openai", "api_key": "" }),
            &token,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    // --- llm-config ---

    #[tokio::test]
    #[serial]
    async fn llm_config_writes_config_toml() {
        let tmp = tempfile::tempdir().unwrap();
        let _restore = RestoreHome::set(tmp.path());

        let state = build_test_state();
        let token = jwt_for(&state, "op-test");
        let app = onboarding_app(state);
        let (status, body) = post_json(
            &app,
            "/admin/onboarding/llm-config",
            serde_json::json!({
                "provider": "openai",
                "api_key": "sk-secret123",
                "base_url": "https://api.openai.com/v1"
            }),
            &token,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body: {:?}", body);
        assert_eq!(body["ok"], true);

        let config_path = tmp.path().join(".cyberclaw/config.toml");
        assert!(config_path.exists(), "config.toml should have been created");
        let content = std::fs::read_to_string(&config_path).unwrap();
        assert!(content.contains("provider = \"openai\""));
        assert!(content.contains("sk-secret123")); // key written to file
        assert!(!content.contains("sk-secret123\n\n")); // sanity
    }

    #[tokio::test]
    async fn llm_config_rejects_empty_provider() {
        let state = build_test_state();
        let token = jwt_for(&state, "op-test");
        let app = onboarding_app(state);
        let (status, _) = post_json(
            &app,
            "/admin/onboarding/llm-config",
            serde_json::json!({ "provider": "" }),
            &token,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    // --- skill-bundle ---

    #[tokio::test]
    #[serial]
    async fn skill_bundle_writes_onboarding_toml_and_returns_count() {
        let tmp = tempfile::tempdir().unwrap();
        let _restore = RestoreHome::set(tmp.path());

        let state = build_test_state();
        let token = jwt_for(&state, "op-test");
        let app = onboarding_app(state);
        let (status, body) = post_json(
            &app,
            "/admin/onboarding/skill-bundle",
            serde_json::json!({ "bundle": "developer" }),
            &token,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body: {:?}", body);
        assert_eq!(body["ok"], true);
        assert_eq!(body["skill_count"], 18);

        let path = tmp.path().join(".cyberclaw/onboarding.toml");
        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("skill_bundle = \"developer\""));
    }

    #[tokio::test]
    async fn skill_bundle_rejects_invalid_bundle_name() {
        let state = build_test_state();
        let token = jwt_for(&state, "op-test");
        let app = onboarding_app(state);
        let (status, _) = post_json(
            &app,
            "/admin/onboarding/skill-bundle",
            serde_json::json!({ "bundle": "nonexistent" }),
            &token,
        )
        .await;
        // Serde should reject this with 422
        assert!(
            status == StatusCode::UNPROCESSABLE_ENTITY || status == StatusCode::BAD_REQUEST,
            "expected 422 or 400, got {}",
            status
        );
    }

    // --- governance ---

    #[tokio::test]
    #[serial]
    async fn governance_writes_policy_file() {
        let tmp = tempfile::tempdir().unwrap();
        let _restore = RestoreHome::set(tmp.path());

        let state = build_test_state();
        let token = jwt_for(&state, "op-test");
        let app = onboarding_app(state);
        let (status, body) = post_json(
            &app,
            "/admin/onboarding/governance",
            serde_json::json!({ "preset": "strict" }),
            &token,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body: {:?}", body);
        assert_eq!(body["ok"], true);
        assert_eq!(body["preset"], "strict");

        let path = tmp.path().join(".cyberclaw/governance-policy.toml");
        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("preset = \"strict\""));
    }

    #[tokio::test]
    async fn governance_rejects_unknown_preset() {
        let state = build_test_state();
        let token = jwt_for(&state, "op-test");
        let app = onboarding_app(state);
        let (status, _) = post_json(
            &app,
            "/admin/onboarding/governance",
            serde_json::json!({ "preset": "ultra-strict" }),
            &token,
        )
        .await;
        assert!(
            status == StatusCode::UNPROCESSABLE_ENTITY || status == StatusCode::BAD_REQUEST,
            "expected 422 or 400, got {}",
            status
        );
    }

    // --- scan-mcp ---

    #[tokio::test]
    #[serial]
    async fn scan_mcp_returns_empty_when_no_config_files() {
        let tmp = tempfile::tempdir().unwrap();
        let _restore = RestoreHome::set(tmp.path());

        let state = build_test_state();
        let token = jwt_for(&state, "op-test");
        let app = onboarding_app(state);
        let (status, body) = post_no_body(&app, "/admin/onboarding/scan-mcp", &token).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["servers"], serde_json::json!([]));
    }

    #[tokio::test]
    #[serial]
    async fn scan_mcp_parses_mcp_servers_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let _restore = RestoreHome::set(tmp.path());

        let config = serde_json::json!({
            "mcpServers": {
                "filesystem": {
                    "command": "npx",
                    "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
                }
            }
        });
        let config_path = tmp.path().join(".claude-code-config.json");
        std::fs::write(&config_path, serde_json::to_string(&config).unwrap()).unwrap();

        let state = build_test_state();
        let token = jwt_for(&state, "op-test");
        let app = onboarding_app(state);
        let (status, body) = post_no_body(&app, "/admin/onboarding/scan-mcp", &token).await;
        assert_eq!(status, StatusCode::OK);
        let servers = body["servers"].as_array().unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0]["name"], "filesystem");
        assert_eq!(servers[0]["command"], "npx");
    }

    // --- mcp-connect ---

    #[tokio::test]
    #[serial]
    async fn mcp_connect_saves_enabled_list() {
        let tmp = tempfile::tempdir().unwrap();
        let _restore = RestoreHome::set(tmp.path());

        let state = build_test_state();
        let token = jwt_for(&state, "op-test");
        let app = onboarding_app(state);
        let (status, body) = post_json(
            &app,
            "/admin/onboarding/mcp-connect",
            serde_json::json!({ "enabled_servers": ["filesystem", "github"] }),
            &token,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body: {:?}", body);
        assert_eq!(body["ok"], true);
        assert_eq!(body["enabled_count"], 2);

        let path = tmp.path().join(".cyberclaw/mcp-enabled.toml");
        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("\"filesystem\""));
        assert!(content.contains("\"github\""));
    }

    #[tokio::test]
    async fn mcp_connect_rejects_path_traversal_in_name() {
        let state = build_test_state();
        let token = jwt_for(&state, "op-test");
        let app = onboarding_app(state);
        let (status, _) = post_json(
            &app,
            "/admin/onboarding/mcp-connect",
            serde_json::json!({ "enabled_servers": ["../evil"] }),
            &token,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    // --- scan-skills ---

    #[tokio::test]
    #[serial]
    async fn scan_skills_returns_empty_when_no_skills_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let _restore = RestoreHome::set(tmp.path());

        let state = build_test_state();
        let token = jwt_for(&state, "op-test");
        let app = onboarding_app(state);
        let (status, body) = post_no_body(&app, "/admin/onboarding/scan-skills", &token).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["skills"], serde_json::json!([]));
    }

    #[tokio::test]
    #[serial]
    async fn scan_skills_finds_skill_md_files() {
        let tmp = tempfile::tempdir().unwrap();
        let _restore = RestoreHome::set(tmp.path());

        let skill_dir = tmp.path().join(".claude/skills/my-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "# My Skill\nDoes something useful.",
        )
        .unwrap();

        let state = build_test_state();
        let token = jwt_for(&state, "op-test");
        let app = onboarding_app(state);
        let (status, body) = post_no_body(&app, "/admin/onboarding/scan-skills", &token).await;
        assert_eq!(status, StatusCode::OK);
        let skills = body["skills"].as_array().unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0]["name"], "my-skill");
        assert!(skills[0]["description"]
            .as_str()
            .unwrap()
            .contains("Does something useful"));
    }

    // --- import-skills ---

    #[tokio::test]
    #[serial]
    async fn import_skills_copies_skill_into_cyberclaw_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let _restore = RestoreHome::set(tmp.path());

        let skill_dir = tmp.path().join(".claude/skills/my-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "# My Skill").unwrap();

        let state = build_test_state();
        let token = jwt_for(&state, "op-test");
        let app = onboarding_app(state);
        let (status, body) = post_json(
            &app,
            "/admin/onboarding/import-skills",
            serde_json::json!({ "paths": [skill_dir.to_str().unwrap()] }),
            &token,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body: {:?}", body);
        assert_eq!(body["ok"], true);
        assert_eq!(body["imported"], 1);

        let dest = tmp.path().join(".cyberclaw/skills/my-skill/SKILL.md");
        assert!(dest.exists(), "SKILL.md should have been copied");
    }

    #[tokio::test]
    #[serial]
    async fn import_skills_rejects_path_outside_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        let _restore = RestoreHome::set(tmp.path());

        // Create a skills dir so workspace can be canonicalized.
        std::fs::create_dir_all(tmp.path().join(".claude/skills")).unwrap();

        // Create a skill outside the workspace.
        let evil_dir = tmp.path().join("evil-skill");
        std::fs::create_dir_all(&evil_dir).unwrap();
        std::fs::write(evil_dir.join("SKILL.md"), "# Evil").unwrap();

        let state = build_test_state();
        let token = jwt_for(&state, "op-test");
        let app = onboarding_app(state);
        let (status, body) = post_json(
            &app,
            "/admin/onboarding/import-skills",
            serde_json::json!({ "paths": [evil_dir.to_str().unwrap()] }),
            &token,
        )
        .await;
        // The response is 200 but with ok=false and errors (partial failure model).
        assert_eq!(status, StatusCode::OK, "body: {:?}", body);
        let errors = body["errors"].as_array().unwrap();
        assert!(!errors.is_empty(), "should have at least one error");
        assert_eq!(body["imported"], 0);
    }

    #[tokio::test]
    async fn import_skills_returns_ok_true_for_empty_paths() {
        let state = build_test_state();
        let token = jwt_for(&state, "op-test");
        let app = onboarding_app(state);
        let (status, body) = post_json(
            &app,
            "/admin/onboarding/import-skills",
            serde_json::json!({ "paths": [] }),
            &token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], true);
        assert_eq!(body["imported"], 0);
    }
}
