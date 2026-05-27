//! `cyberclaw chat` — 终端 REPL，与 CyberClaw agent 对话。
//!
//! # 认证
//! JWT 读取优先级：`CYBERCLAW_TOKEN` 环境变量 > `~/.cyberclaw/cli-token` 文件。
//! 若两者均不存在，提示用户输入凭证并调用 `POST /admin/login` 获取 JWT，
//! 结果以 0600 权限写入 `~/.cyberclaw/cli-token`。
//!
//! # SSE 解析
//! 使用 `reqwest` bytes stream 手动解析 `data: <json>\n\n` 格式，避免引入
//! eventsource crate。支持以下帧类型：
//! - `{"choices":[{"delta":{"content":"..."}}]}` — token
//! - `{"type":"clarify","clarify":{...}}` — 澄清请求
//! - `{"type":"clarify_resolved",...}` — 澄清已解答（静默）
//! - `[DONE]` — 流结束

use anyhow::{Context, Result};
use clap::Args;
use cyberclaw_core::i18n::t;
use cyberclaw_llm::LlmFailoverReason;
use futures::StreamExt;
use reqwest::Client;
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::PathBuf;

use super::chat_tui;

// ---------------------------------------------------------------------------
// CLI args
// ---------------------------------------------------------------------------

/// `cyberclaw chat` 参数
#[derive(Debug, Args)]
pub struct ChatArgs {
    /// 指定 Agent ID（默认使用服务器配置的默认 agent）
    #[arg(long)]
    pub agent: Option<String>,

    /// 恢复已有会话（传入 conversation ID）（已废弃，用 --conversation）
    #[arg(long)]
    pub resume: Option<String>,

    /// 显式指定 conversation ID 恢复
    #[arg(long)]
    pub conversation: Option<String>,

    /// 强制新建会话（不 resume 上次）
    #[arg(long)]
    pub new: bool,

    /// LLM model 覆盖
    #[arg(long)]
    pub model: Option<String>,

    /// Server URL（默认：$CYBERCLAW_SERVER 或 http://127.0.0.1:38090）
    #[arg(long)]
    pub server: Option<String>,
}

// ---------------------------------------------------------------------------
// SSE frame types
// ---------------------------------------------------------------------------

/// Rate-limit snapshot from a server SSE `rate_limit` frame.
#[derive(Debug, Clone)]
pub struct SseRateLimit {
    pub provider: String,
    pub requests_limit: Option<u64>,
    pub requests_remaining: Option<u64>,
    pub tokens_limit: Option<u64>,
    pub tokens_remaining: Option<u64>,
    pub requests_reset_secs: Option<f64>,
    pub tokens_reset_secs: Option<f64>,
}

/// Token usage snapshot from a server SSE `usage` frame.
#[derive(Debug, Clone)]
pub struct SseUsage {
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
}

/// SSE 帧解析结果
#[derive(Debug)]
pub enum SseFrame {
    /// LLM token 片段
    Token(String),
    /// 澄清请求
    Clarify(ClarifyPayload),
    /// 澄清已解答（静默继续）
    ClarifyResolved,
    /// Rate-limit snapshot from the server.
    RateLimit(SseRateLimit),
    /// Token usage snapshot from the server (for cost estimation).
    Usage(SseUsage),
    /// A tool call entered the governance approval queue.
    /// BUG-CB-03: emitted by the server when a capability dispatch is
    /// governance-denied so the TUI can overlay a notice instead of
    /// showing "Thinking…" for 60-90 s while the approval timeout elapses.
    ApprovalPending {
        tool: String,
        reason: Option<String>,
    },
    /// v1.3 WP-3: tool dispatch began. TUI renders an inline spinner.
    ToolStart {
        tool: String,
        args: serde_json::Value,
    },
    /// v1.3 WP-3: tool dispatch completed. TUI replaces the spinner.
    ToolComplete {
        tool: String,
        ok: bool,
        preview: String,
        duration_ms: u64,
    },
    /// v1.3 WP-3: governance approval was granted.
    ApprovalGranted { tool: String },
    /// v1.3 WP-3: governance approval was denied.
    ApprovalDenied {
        tool: String,
        reason: Option<String>,
    },
    /// v1.3 WP-3: application-level keep-alive frame.
    Heartbeat { elapsed_secs: u64 },
    /// Server-side terminal error. v1.3 WP-4 Change 1: when `reason` carries a
    /// typed `LlmFailoverReason`, the CLI maps it through
    /// [`friendly_error_message`] for a stable hint instead of regex-scanning
    /// the message body. `message` + `kind` mirror the legacy `error.message` /
    /// `error.type` payload for clients that don't (yet) match on `reason`.
    ErrorMsg {
        message: String,
        kind: String,
        reason: Option<LlmFailoverReason>,
    },
    /// 流结束
    Done,
    /// 未知帧（跳过）
    Unknown,
}

/// 澄清请求负载（从 SSE JSON 反序列化）
#[derive(Debug, Clone, Deserialize)]
pub struct ClarifyPayload {
    pub id: String,
    #[allow(dead_code)]
    // Reserved for future use — per-conversation CLI state
    pub conversation_id: String,
    pub questions: Vec<ClarifyQuestion>,
}

/// 澄清问题
#[derive(Debug, Clone, Deserialize)]
pub struct ClarifyQuestion {
    pub question: String,
    pub options: Vec<ClarifyOption>,
    #[serde(default)]
    #[allow(dead_code)]
    // Reserved for future use — per-conversation CLI state
    pub multi_select: bool,
}

/// 澄清选项
#[derive(Debug, Clone, Deserialize)]
pub struct ClarifyOption {
    pub label: String,
    pub description: String,
}

/// POST /admin/login 请求体
#[derive(Debug, Serialize)]
struct LoginRequest {
    user_id: String,
    password: String,
}

/// POST /admin/login 响应体
#[derive(Debug, Deserialize)]
struct LoginResponse {
    token: String,
}

/// POST /api/v1/chat/completions 请求体（v1 — 保留备用，不再主动构造）
#[allow(dead_code)]
#[derive(Debug, Serialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<ChatMessage>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    conversation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_id: Option<String>,
}

/// POST /v2/agent/chat/completions 请求体（v1.3 WP-1）
///
/// 单次用户消息；历史由服务端持有，不再由客户端累积。
#[derive(Debug, Serialize)]
struct AgentChatRequestV2 {
    /// 单条用户消息（当前轮次）
    message: String,
    /// 模型覆盖（可选）
    model: String,
    /// 流式响应
    stream: bool,
    /// 已有会话 ID（None = 新建）
    #[serde(skip_serializing_if = "Option::is_none")]
    conversation_id: Option<String>,
    /// Agent ID（可选）
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_id: Option<String>,
}

/// 消息条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

/// POST /api/v1/chat/clarify/:id/respond 请求体
#[derive(Debug, Serialize)]
struct SubmitClarifyRequest {
    answer: ClarifyAnswer,
    conversation_id: Option<String>,
}

/// 澄清回答
#[derive(Debug, Serialize)]
struct ClarifyAnswer {
    answers: BTreeMap<String, String>,
}

/// POST /api/v1/chat/conversations/:id/compress 响应体（部分）
#[derive(Debug, Deserialize)]
struct CompressResponse {
    original_count: usize,
    compressed_count: usize,
}

// ---------------------------------------------------------------------------
// JWT 加载 / 保存
// ---------------------------------------------------------------------------

/// v1.3 WP-4 Change 3 — typed lookup table mapping every
/// [`LlmFailoverReason`] variant to a Chinese-language hint for the user.
///
/// Returning `&'static str` keeps the lookup zero-allocation. Callers that
/// need to splice the original message body in still use
/// [`friendly_token_error`] (HTTP-status path) or render the hint alongside
/// the raw `message` from the SSE error frame.
///
/// This replaces the legacy `friendly_token_error` string-match logic
/// (which only recognised `"401"` / `"InvalidSignature"` / `"ExpiredSignature"`)
/// with a stable enum match driven by the server's classifier in
/// `cyberclaw_llm::classify_llm_error`. New `LlmFailoverReason` variants
/// added in the future will fall through to the catch-all hint, never
/// silently mis-classifying.
pub fn friendly_error_message(reason: LlmFailoverReason) -> &'static str {
    match reason {
        LlmFailoverReason::AuthInvalid | LlmFailoverReason::AuthExpired => {
            "JWT 过期或签名无效。\n\
             修复：rm ~/.cyberclaw/cli-token && cyberclaw chat   # 重新交互式登录\n\
             或：cyberclaw chat --new                            # 强制重新登录路径"
        }
        LlmFailoverReason::Billing => {
            "Provider 余额不足。检查 API key billing 状态，或配置 credential pool fallback。"
        }
        LlmFailoverReason::QuotaExceeded => {
            "组织配额已用完。换一组 API key，或等待配额重置后重试。"
        }
        LlmFailoverReason::RateLimit => {
            "命中 provider rate limit。退避后重试（通常 60s），或切换 credential pool。"
        }
        LlmFailoverReason::ContextOverflow => {
            "上下文超过模型窗口。运行 /compress 压缩会话历史，或换支持更长上下文的 model。"
        }
        LlmFailoverReason::ImageTooLarge => {
            "图片附件过大。压缩后重试（≤ 5 MB），或拆成多张较小图片。"
        }
        LlmFailoverReason::ModelNotFound => {
            "Model 名称无效或当前 provider 不支持。检查 /model 当前值，或运行 cyberclaw doctor 查看可用 model。"
        }
        LlmFailoverReason::PermissionDenied => {
            "权限不足。检查 API key 是否绑定到该 model / endpoint。"
        }
        LlmFailoverReason::ServiceUnavailable | LlmFailoverReason::InternalError => {
            "Provider 服务暂时不可用。退避后重试，或切换 provider chain 中的备用项。"
        }
        LlmFailoverReason::Timeout => {
            "Provider 请求超时。退避后重试，或缩短输入长度。"
        }
        LlmFailoverReason::ContentFilter => {
            "请求被 content/safety filter 拒绝。改写 prompt 后重试。"
        }
        LlmFailoverReason::ThinkingSignature => {
            "Anthropic thinking-block signature 校验失败。清理上下文后重试。"
        }
        LlmFailoverReason::BadRequest => {
            "Provider 拒绝请求（400）。检查 model 参数、message 结构是否合规。"
        }
        LlmFailoverReason::Unknown => {
            "服务端错误，请重试或查看服务日志（cyberclaw doctor）。"
        }
    }
}

/// v1.3 WP-3 Step 6: produce a typed, glyph-prefixed tag for [`SseFrame::ErrorMsg`]
/// so the TUI renders categorised error blocks instead of an opaque
/// `[Error: ...]` line.
///
/// Mapping mirrors spec section E:
///
/// - Billing → `[💳 Billing]`
/// - RateLimit → `[⏳ Rate Limited]`
/// - AuthInvalid / AuthExpired → `[🔒 Auth Error]`
/// - ContextOverflow → `[📏 Context Too Long]`
/// - Timeout → `[⏱ Timeout]`
/// - Any other LLM reason → `[Error: <reason>]`
/// - No reason → `[Error: <kind>]` (falls back to the legacy `kind` string)
pub fn typed_error_prefix(reason: Option<LlmFailoverReason>, kind: &str) -> String {
    match reason {
        Some(LlmFailoverReason::Billing) => "[\u{1f4b3} Billing]".to_string(),
        Some(LlmFailoverReason::RateLimit) => "[\u{23f3} Rate Limited]".to_string(),
        Some(LlmFailoverReason::AuthInvalid | LlmFailoverReason::AuthExpired) => {
            "[\u{1f512} Auth Error]".to_string()
        }
        Some(LlmFailoverReason::ContextOverflow) => "[\u{1f4cf} Context Too Long]".to_string(),
        Some(LlmFailoverReason::Timeout) => "[\u{23f1} Timeout]".to_string(),
        Some(r) => format!("[Error: {}]", r.wire_name()),
        None => format!("[Error: {kind}]"),
    }
}

/// 把 401 InvalidSignature / ExpiredSignature 等错误转成对用户友好的多行提示。
/// 返回 None = 不是可识别的 token 错误（让调用方走默认错误路径）。
///
/// v1.3 WP-4 Change 3 — first tries the typed
/// [`cyberclaw_llm::classify_llm_error`] classifier so the hint is driven by
/// the same `LlmFailoverReason` enum the server uses, and only falls back to
/// the legacy string-match (`"InvalidSignature"` / `"ExpiredSignature"` /
/// `"Invalid token"`) when the classifier returns the generic auth bucket
/// without context. This means every HTTP-status code the server emits gets a
/// stable Chinese hint, not just 401-with-three-known-substrings.
fn friendly_token_error(status: reqwest::StatusCode, body: &str) -> Option<String> {
    let reason = cyberclaw_llm::classify_llm_error(Some(status.as_u16()), body);
    let first_line = body.lines().next().unwrap_or(body);

    match reason {
        LlmFailoverReason::AuthInvalid | LlmFailoverReason::AuthExpired => Some(format!(
            "Token 被拒绝 ({status} {first_line})\n\n\
             常见原因：\n  · 服务端 JWT_SECRET 重启后变了（你本地的 cli-token 签名作废）\n  · token TTL 过期\n\n\
             {hint}",
            hint = friendly_error_message(reason),
        )),
        // For other classified reasons, surface the typed hint so the user
        // sees a friendly message even for the non-auth paths (rate limit,
        // billing, context overflow, …) the legacy string-match dropped.
        LlmFailoverReason::Billing
        | LlmFailoverReason::QuotaExceeded
        | LlmFailoverReason::RateLimit
        | LlmFailoverReason::ContextOverflow
        | LlmFailoverReason::ModelNotFound
        | LlmFailoverReason::PermissionDenied
        | LlmFailoverReason::ServiceUnavailable
        | LlmFailoverReason::Timeout
        | LlmFailoverReason::ContentFilter => Some(format!(
            "请求被拒绝 ({status} {first_line})\n\n{}",
            friendly_error_message(reason),
        )),
        // Unknown / BadRequest / InternalError / ImageTooLarge /
        // ThinkingSignature: let the default error path render the raw body.
        _ => None,
    }
}

fn cyberclaw_config_dir() -> Result<PathBuf> {
    let home = dirs_next();
    Ok(home.join(".cyberclaw"))
}

/// 返回 home 目录（仅 Unix）
fn dirs_next() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

fn token_file_path() -> Result<PathBuf> {
    Ok(cyberclaw_config_dir()?.join("cli-token"))
}

/// 从环境变量或文件加载 JWT。返回 None 表示需要登录。
fn load_token() -> Option<String> {
    if let Ok(t) = std::env::var("CYBERCLAW_TOKEN") {
        if !t.is_empty() {
            return Some(t);
        }
    }
    let path = token_file_path().ok()?;
    std::fs::read_to_string(&path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 将 JWT 写入 ~/.cyberclaw/cli-token，权限 0600（Unix only）
fn save_token(token: &str) -> Result<()> {
    use std::fs;

    let dir = cyberclaw_config_dir()?;
    fs::create_dir_all(&dir).with_context(|| format!("创建目录 {}", dir.display()))?;

    let path = token_file_path()?;
    fs::write(&path, token).with_context(|| format!("写入 {}", path.display()))?;

    // 设置 0600 权限
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("设置权限 {}", path.display()))?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// 登录
// ---------------------------------------------------------------------------

async fn login_and_save(client: &Client, server: &str) -> Result<String> {
    print!("user_id: ");
    io::stdout().flush()?;
    let mut user_id = String::new();
    io::stdin().read_line(&mut user_id)?;
    let user_id = user_id.trim().to_string();

    print!("password: ");
    io::stdout().flush()?;
    let mut password = String::new();
    io::stdin().read_line(&mut password)?;
    let password = password.trim().to_string();

    let url = format!("{}/admin/login", server);
    let resp = client
        .post(&url)
        .json(&LoginRequest { user_id, password })
        .send()
        .await
        .with_context(|| format!("POST {}", url))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("登录失败 ({}): {}", status, body);
    }

    let login: LoginResponse = resp.json().await.context("解析登录响应失败")?;

    save_token(&login.token)?;
    println!("Token 已保存到 ~/.cyberclaw/cli-token");

    Ok(login.token)
}

// ---------------------------------------------------------------------------
// SSE 帧解析
// ---------------------------------------------------------------------------

/// 解析单个 SSE data 行（已去掉 "data: " 前缀）
///
/// v1.3 WP-3: first try the typed `cyberclaw_wire::Frame` envelope. If that
/// fails (legacy server, missing `v` field, or wire crate decode error), fall
/// back to the pre-WP-3 manual JSON parser so we stay compatible with
/// in-flight servers during rollout.
pub fn parse_sse_data(data: &str) -> SseFrame {
    let data = data.trim();
    if data == "[DONE]" {
        return SseFrame::Done;
    }

    // — Path 1: typed wire::Frame envelope (preferred on v1.3+ servers) —
    match cyberclaw_wire::Frame::from_sse_data(data) {
        Ok(frame) => return wire_frame_to_sse_frame(frame),
        Err(cyberclaw_wire::DecodeError::VersionMismatch { sender, supported }) => {
            return SseFrame::ErrorMsg {
                message: format!(
                    "Server protocol version {sender} not supported (CLI supports v{supported}). Please update CLI."
                ),
                kind: "version_mismatch".to_string(),
                reason: None,
            };
        }
        Err(_) => {
            // Fall through to the legacy parser for backward compat.
        }
    }

    // — Path 2: legacy pre-WP-3 ad-hoc JSON envelope —
    parse_legacy_sse_data(data)
}

/// Translate a [`cyberclaw_wire::Frame`] into the CLI's internal [`SseFrame`].
fn wire_frame_to_sse_frame(frame: cyberclaw_wire::Frame) -> SseFrame {
    use cyberclaw_wire::Frame as Wf;
    match frame {
        Wf::Token { content } => SseFrame::Token(content),
        Wf::ToolStart { tool, args } => SseFrame::ToolStart { tool, args },
        Wf::ToolProgress { .. } => SseFrame::Unknown, // not rendered in v1.3
        Wf::ToolComplete {
            tool,
            ok,
            preview,
            duration_ms,
        } => SseFrame::ToolComplete {
            tool,
            ok,
            preview,
            duration_ms,
        },
        Wf::ApprovalPending { tool, reason } => SseFrame::ApprovalPending { tool, reason },
        Wf::ApprovalGranted { tool } => SseFrame::ApprovalGranted { tool },
        Wf::ApprovalDenied { tool, reason } => SseFrame::ApprovalDenied { tool, reason },
        Wf::Error { message, kind } => {
            let (kind_str, reason) = wire_error_kind_to_legacy(&kind);
            SseFrame::ErrorMsg {
                message,
                kind: kind_str,
                reason,
            }
        }
        Wf::Usage {
            model,
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
        } => SseFrame::Usage(SseUsage {
            model,
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
        }),
        Wf::RateLimit {
            provider,
            requests_limit,
            requests_remaining,
            tokens_limit,
            tokens_remaining,
            requests_reset_secs,
            tokens_reset_secs,
        } => SseFrame::RateLimit(SseRateLimit {
            provider,
            requests_limit,
            requests_remaining,
            tokens_limit,
            tokens_remaining,
            requests_reset_secs,
            tokens_reset_secs,
        }),
        Wf::Heartbeat { elapsed_secs } => SseFrame::Heartbeat { elapsed_secs },
        Wf::Done => SseFrame::Done,
    }
}

/// Convert a typed [`cyberclaw_wire::ErrorKind`] back to the legacy
/// `(kind: String, reason: Option<LlmFailoverReason>)` pair the existing
/// TUI render path expects.
fn wire_error_kind_to_legacy(
    kind: &cyberclaw_wire::ErrorKind,
) -> (String, Option<LlmFailoverReason>) {
    use cyberclaw_wire::ErrorKind as Wk;
    let llm = match kind {
        Wk::Billing => Some(LlmFailoverReason::Billing),
        Wk::RateLimit => Some(LlmFailoverReason::RateLimit),
        Wk::ContextOverflow => Some(LlmFailoverReason::ContextOverflow),
        Wk::ImageTooLarge => Some(LlmFailoverReason::ImageTooLarge),
        Wk::ModelNotFound => Some(LlmFailoverReason::ModelNotFound),
        Wk::AuthInvalid => Some(LlmFailoverReason::AuthInvalid),
        Wk::AuthExpired => Some(LlmFailoverReason::AuthExpired),
        Wk::PermissionDenied => Some(LlmFailoverReason::PermissionDenied),
        Wk::QuotaExceeded => Some(LlmFailoverReason::QuotaExceeded),
        Wk::ServiceUnavailable => Some(LlmFailoverReason::ServiceUnavailable),
        Wk::Timeout => Some(LlmFailoverReason::Timeout),
        Wk::InternalError => Some(LlmFailoverReason::InternalError),
        Wk::BadRequest => Some(LlmFailoverReason::BadRequest),
        Wk::ContentFilter => Some(LlmFailoverReason::ContentFilter),
        Wk::ThinkingSignature => Some(LlmFailoverReason::ThinkingSignature),
        Wk::InvalidRequest | Wk::GovernanceDenied | Wk::Unknown => None,
    };
    let kind_str = match kind {
        Wk::InvalidRequest => "invalid_request".to_string(),
        Wk::GovernanceDenied => "governance_denied".to_string(),
        Wk::Unknown => "error".to_string(),
        other => {
            // For LLM-mirrored kinds, surface the snake_case wire name so the
            // TUI's existing string matching still works (e.g. `lower.contains("401")`).
            serde_json::to_value(other)
                .ok()
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_else(|| "error".to_string())
        }
    };
    (kind_str, llm)
}

/// Legacy pre-WP-3 ad-hoc JSON parser. Kept so a new CLI can still talk to an
/// older server emitting `{"choices":[{"delta":{"content":"…"}}]}` and the
/// `{"type":"clarify",…}` / `{"type":"usage",…}` style frames.
fn parse_legacy_sse_data(data: &str) -> SseFrame {
    let v: Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(_) => return SseFrame::Unknown,
    };

    // 检查 type 字段
    match v.get("type").and_then(|t| t.as_str()) {
        Some("clarify") => {
            if let Some(clarify_val) = v.get("clarify") {
                match serde_json::from_value::<ClarifyPayload>(clarify_val.clone()) {
                    Ok(payload) => return SseFrame::Clarify(payload),
                    Err(_) => return SseFrame::Unknown,
                }
            }
            SseFrame::Unknown
        }
        Some("clarify_resolved") => SseFrame::ClarifyResolved,
        Some("rate_limit") => {
            if let Some(rl) = v.get("rate_limit") {
                let provider = rl
                    .get("provider")
                    .and_then(|p| p.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                return SseFrame::RateLimit(SseRateLimit {
                    provider,
                    requests_limit: rl.get("requests_limit").and_then(|v| v.as_u64()),
                    requests_remaining: rl.get("requests_remaining").and_then(|v| v.as_u64()),
                    tokens_limit: rl.get("tokens_limit").and_then(|v| v.as_u64()),
                    tokens_remaining: rl.get("tokens_remaining").and_then(|v| v.as_u64()),
                    requests_reset_secs: rl.get("requests_reset_secs").and_then(|v| v.as_f64()),
                    tokens_reset_secs: rl.get("tokens_reset_secs").and_then(|v| v.as_f64()),
                });
            }
            SseFrame::Unknown
        }
        Some("usage") => {
            if let Some(u) = v.get("usage") {
                let model = u
                    .get("model")
                    .and_then(|m| m.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                return SseFrame::Usage(SseUsage {
                    model,
                    input_tokens: u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                    output_tokens: u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                    cache_read_tokens: u
                        .get("cache_read_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                    cache_write_tokens: u
                        .get("cache_write_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                });
            }
            SseFrame::Unknown
        }
        // BUG-CB-03: governance approval-pending notification.
        Some("approval_pending") => {
            if let Some(ap) = v.get("approval_pending") {
                let tool = ap
                    .get("tool")
                    .and_then(|t| t.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                let reason = ap.get("reason").and_then(|r| r.as_str()).map(String::from);
                return SseFrame::ApprovalPending { tool, reason };
            }
            SseFrame::Unknown
        }
        _ => {
            // v1.3 WP-4 Change 1: typed error frame
            // {"error":{"message":"…","type":"…","reason":"auth_invalid"}}
            if let Some(err_obj) = v.get("error") {
                let message = err_obj
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("")
                    .to_string();
                let kind = err_obj
                    .get("type")
                    .and_then(|t| t.as_str())
                    .unwrap_or("error")
                    .to_string();
                let reason = err_obj
                    .get("reason")
                    .and_then(|r| r.as_str())
                    .and_then(|s| {
                        serde_json::from_value::<LlmFailoverReason>(serde_json::Value::String(
                            s.to_string(),
                        ))
                        .ok()
                    });
                return SseFrame::ErrorMsg {
                    message,
                    kind,
                    reason,
                };
            }
            // token 帧：{"choices":[{"delta":{"content":"..."}}]}
            if let Some(content) = v
                .get("choices")
                .and_then(|c| c.get(0))
                .and_then(|c| c.get("delta"))
                .and_then(|d| d.get("content"))
                .and_then(|c| c.as_str())
            {
                SseFrame::Token(content.to_string())
            } else {
                SseFrame::Unknown
            }
        }
    }
}

/// 从 SSE 响应字节流解析帧序列，逐帧调用回调。
/// 回调返回 `true` 继续，`false` 提前结束。
async fn stream_sse<F>(response: reqwest::Response, mut on_frame: F) -> Result<()>
where
    F: FnMut(SseFrame) -> bool,
{
    let mut byte_stream = response.bytes_stream();
    let mut buf = String::new();
    // BUG-CB-07: carry partial multi-byte sequences (e.g. CJK / emoji) across
    // chunk boundaries.  reqwest splits on TCP packet boundaries, not UTF-8
    // char boundaries, so a 2/3/4-byte codepoint can straddle two chunks.
    let mut residual: Vec<u8> = Vec::new();

    while let Some(chunk) = byte_stream.next().await {
        let chunk = chunk.context("读取 SSE 字节流失败")?;
        residual.extend_from_slice(&chunk);

        // Decode only the valid UTF-8 prefix; keep the trailing incomplete
        // byte sequence in `residual` for the next iteration.
        let valid_text = match std::str::from_utf8(&residual) {
            Ok(s) => {
                let owned = s.to_string();
                residual.clear();
                owned
            }
            Err(e) => {
                let valid_up_to = e.valid_up_to();
                // SAFETY: valid_up_to is always a valid char boundary by
                // definition of Utf8Error.
                let s = std::str::from_utf8(&residual[..valid_up_to])
                    .expect("valid_up_to guarantees valid UTF-8")
                    .to_string();
                residual = residual[valid_up_to..].to_vec();
                s
            }
        };
        buf.push_str(&valid_text);

        // 按 "\n\n" 分割事件块
        while let Some(pos) = buf.find("\n\n") {
            let event_block = buf[..pos].to_string();
            buf = buf[pos + 2..].to_string();

            for line in event_block.lines() {
                let data = if let Some(d) = line.strip_prefix("data: ") {
                    d
                } else {
                    continue;
                };

                let frame = parse_sse_data(data);
                let is_done = matches!(frame, SseFrame::Done);
                if !on_frame(frame) || is_done {
                    return Ok(());
                }
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// 会话操作
// ---------------------------------------------------------------------------

/// 向 agent 发送消息，流式打印 token，处理 clarify。
///
/// v1.3 WP-1: 改走 `/v2/agent/chat/completions`。
/// - 只发当前轮次的 `message`，不再累积历史到客户端。
/// - 返回 `(assistant_text, Option<new_conv_id>)`；`new_conv_id` 在服务端
///   新建会话时非 None（由 `X-Conversation-Id` 响应头携带）。
/// - HTTP 404 表示会话已过期，调用方负责重置 conv_id 并提示用户。
#[allow(clippy::too_many_arguments)]
async fn send_message(
    client: &Client,
    server: &str,
    token: &str,
    conv_id: Option<&str>,
    agent_id: Option<&str>,
    model: &str,
    message: &str,
    ctrl_c_flag: &std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<(String, Option<String>)> {
    // v1.3 WP-1: use server-side session endpoint.
    let url = format!("{}/v2/agent/chat/completions", server);
    let body = AgentChatRequestV2 {
        message: message.to_string(),
        model: model.to_string(),
        stream: true,
        conversation_id: conv_id.map(|s| s.to_string()),
        agent_id: agent_id.map(|s| s.to_string()),
    };

    let resp = client
        .post(&url)
        .bearer_auth(token)
        .header("Accept", "text/event-stream")
        .json(&body)
        .send()
        .await
        .with_context(|| format!("POST {}", url))?;

    if !resp.status().is_success() {
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            // 会话已过期（服务端驱逐）
            anyhow::bail!("SESSION_EXPIRED");
        }
        let body_text = resp.text().await.unwrap_or_default();
        anyhow::bail!("chat/completions 失败 ({}): {}", status, body_text);
    }

    // 从响应头中读取服务端分配的 conversation_id。
    let new_conv_id = resp
        .headers()
        .get("x-conversation-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let mut assistant_content = String::new();
    let client_clone = client.clone();
    let server_owned = server.to_string();
    let token_owned = token.to_string();
    let conv_id_owned = new_conv_id.as_deref().or(conv_id).unwrap_or("").to_string();
    let ctrl_c = ctrl_c_flag.clone();

    stream_sse(resp, |frame| {
        if ctrl_c.load(std::sync::atomic::Ordering::Relaxed) {
            println!("\n[已取消]");
            return false;
        }
        match frame {
            SseFrame::Token(chunk) => {
                print!("{}", chunk);
                let _ = io::stdout().flush();
                assistant_content.push_str(&chunk);
                true
            }
            SseFrame::Clarify(payload) => {
                // 打印换行 + 澄清 UI
                println!();
                handle_clarify_sync(
                    &client_clone,
                    &server_owned,
                    &token_owned,
                    &conv_id_owned,
                    &payload,
                );
                true
            }
            SseFrame::ClarifyResolved => true,
            // Rate limit info is silently ignored in the legacy REPL path;
            // it is consumed by the TUI via TokenEvent::RateLimit instead.
            SseFrame::RateLimit(_) => true,
            // Usage info is silently ignored in the legacy REPL path;
            // cost tracking is only active in the TUI (TokenEvent::Usage).
            SseFrame::Usage(_) => true,
            // BUG-CB-03: print an inline notice in the legacy REPL.
            SseFrame::ApprovalPending { tool, reason } => {
                match reason.as_deref() {
                    Some(r) => println!(
                        "\n[!] Awaiting approval for {} ({}) — check /approvals",
                        tool, r
                    ),
                    None => println!("\n[!] Awaiting approval for {} — check /approvals", tool),
                }
                let _ = io::stdout().flush();
                true
            }
            // v1.3 WP-4 Change 3: print the typed-reason friendly hint
            // alongside the raw message so the REPL user gets actionable
            // text instead of a silently-dropped frame.
            SseFrame::ErrorMsg {
                message,
                kind,
                reason,
            } => {
                match reason {
                    Some(r) => println!(
                        "\n[!] Error ({kind}): {message}\n    {}",
                        friendly_error_message(r),
                    ),
                    None => println!("\n[!] Error ({kind}): {message}"),
                }
                let _ = io::stdout().flush();
                true
            }
            // v1.3 WP-3 new frames — REPL prints a concise inline status.
            SseFrame::ToolStart { tool, .. } => {
                print!("\n[tool: {tool} …]\n");
                let _ = io::stdout().flush();
                true
            }
            SseFrame::ToolComplete {
                tool,
                ok,
                duration_ms,
                ..
            } => {
                let mark = if ok { '\u{2713}' } else { '\u{2717}' };
                print!("\n[tool: {tool} {mark} {duration_ms}ms]\n");
                let _ = io::stdout().flush();
                true
            }
            SseFrame::ApprovalGranted { tool } => {
                println!("\n[\u{2713} approved: {tool}]");
                let _ = io::stdout().flush();
                true
            }
            SseFrame::ApprovalDenied { tool, reason } => {
                match reason.as_deref() {
                    Some(r) => println!("\n[\u{2717} denied: {tool}] — {r}"),
                    None => println!("\n[\u{2717} denied: {tool}]"),
                }
                let _ = io::stdout().flush();
                true
            }
            // Heartbeats are silent in the REPL (TUI uses them for the status bar).
            SseFrame::Heartbeat { .. } => true,
            SseFrame::Done => false,
            SseFrame::Unknown => true,
        }
    })
    .await?;

    println!(); // 换行（token 流不带换行）
    Ok((assistant_content, new_conv_id))
}

/// 同步（阻塞 tokio task）处理 clarify：渲染选项 + 读取用户输入 + POST respond
fn handle_clarify_sync(
    client: &Client,
    server: &str,
    token: &str,
    conv_id: &str,
    payload: &ClarifyPayload,
) {
    for question in &payload.questions {
        println!("\n[?] {}", question.question);
        for (i, opt) in question.options.iter().enumerate() {
            println!("  [{}] {} — {}", i + 1, opt.label, opt.description);
        }
        println!("  [o] Other (freeform)");
        print!("> ");
        let _ = io::stdout().flush();

        let mut line = String::new();
        if io::stdin().read_line(&mut line).is_err() {
            return;
        }
        let input = line.trim().to_string();

        let answer_text = if input.eq_ignore_ascii_case("o") {
            print!("请输入自定义回答: ");
            let _ = io::stdout().flush();
            let mut freeform = String::new();
            let _ = io::stdin().read_line(&mut freeform);
            freeform.trim().to_string()
        } else if let Ok(idx) = input.parse::<usize>() {
            if idx >= 1 && idx <= question.options.len() {
                question.options[idx - 1].label.clone()
            } else {
                println!("[!] 无效选项，跳过");
                continue;
            }
        } else if !input.is_empty() {
            // 直接输入 label 文字也接受
            input.clone()
        } else {
            println!("[!] 无输入，跳过");
            continue;
        };

        // 提交回答（在 tokio 阻塞任务中同步执行）
        let mut answers = BTreeMap::new();
        answers.insert(question.question.clone(), answer_text);

        let req_body = SubmitClarifyRequest {
            answer: ClarifyAnswer { answers },
            conversation_id: Some(conv_id.to_string()),
        };

        let url = format!("{}/api/v1/chat/clarify/{}/respond", server, payload.id);
        // 使用 tokio::task::block_in_place 在 async 上下文中同步发送
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                client
                    .post(&url)
                    .bearer_auth(token)
                    .json(&req_body)
                    .send()
                    .await
            })
        });

        match result {
            Ok(r) if r.status().is_success() => {
                println!("[clarify 已提交]");
            }
            Ok(r) => {
                println!("[!] clarify respond 失败: {}", r.status());
            }
            Err(e) => {
                println!("[!] clarify respond 请求失败: {}", e);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Slash 命令处理
// ---------------------------------------------------------------------------

pub enum SlashResult {
    /// 继续 REPL
    Continue,
    /// 退出 REPL
    Quit,
}

pub async fn handle_slash(
    cmd: &str,
    client: &Client,
    server: &str,
    token: &str,
    conv_id: &str,
    current_model: &mut String,
) -> Result<SlashResult> {
    let parts: Vec<&str> = cmd.trim().splitn(2, ' ').collect();
    match parts[0] {
        "/help" | "/h" => {
            println!("{}", t("slash.help"));
            println!("  /help           — 显示帮助");
            println!("  /compress       — 压缩会话历史");
            println!("  /save <path>    — 导出会话为 markdown");
            println!("  /model <name>   — 切换 LLM model");
            println!("  /quit | /exit   — 退出");
        }
        "/compress" => {
            let url = format!("{}/api/v1/chat/conversations/{}/compress", server, conv_id);
            let resp = client
                .post(&url)
                .bearer_auth(token)
                .send()
                .await
                .with_context(|| format!("POST {}", url))?;

            if resp.status().is_success() {
                let cr: CompressResponse = resp.json().await.unwrap_or(CompressResponse {
                    original_count: 0,
                    compressed_count: 0,
                });
                println!(
                    "[compress] {} 条消息压缩为 {} 条",
                    cr.original_count, cr.compressed_count
                );
            } else {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                println!("[!] compress 失败 ({}): {}", status, body);
            }
        }
        "/save" => {
            let path = parts.get(1).copied().unwrap_or("conversation.md");
            save_conversation_as_markdown(client, server, token, conv_id, path).await?;
        }
        "/model" => {
            if let Some(name) = parts.get(1).copied() {
                *current_model = name.to_string();
                println!("[model] 已切换至: {}", current_model);
            } else {
                println!("[model] 当前 model: {}", current_model);
            }
        }
        "/quit" | "/exit" | "/q" => {
            return Ok(SlashResult::Quit);
        }
        _ => {
            println!("[!] {}", t("error.unknown_command").replace("{}", parts[0]));
        }
    }
    Ok(SlashResult::Continue)
}

/// GET conversation 转为 markdown 并写文件
async fn save_conversation_as_markdown(
    client: &Client,
    server: &str,
    token: &str,
    conv_id: &str,
    path: &str,
) -> Result<()> {
    let url = format!("{}/api/v1/chat/conversations/{}", server, conv_id);
    let resp = client
        .get(&url)
        .bearer_auth(token)
        .send()
        .await
        .with_context(|| format!("GET {}", url))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if let Some(msg) = friendly_token_error(status, &body) {
            anyhow::bail!(msg);
        }
        anyhow::bail!("GET conversation 失败 ({}): {}", status, body);
    }

    #[derive(Deserialize)]
    struct ConvResp {
        title: String,
        #[serde(default)]
        messages: Vec<ChatMessage>,
    }

    let conv: ConvResp = resp.json().await.context("解析 conversation 失败")?;
    let mut md = format!("# {}\n\n", conv.title);
    for msg in &conv.messages {
        md.push_str(&format!("## {}\n\n{}\n\n", msg.role, msg.content));
    }

    std::fs::write(path, &md).with_context(|| format!("写入 {}", path))?;
    println!("[save] 已导出到 {}", path);
    Ok(())
}

// ---------------------------------------------------------------------------
// TUI send_message — token 通过 mpsc channel 发给 TUI 主循环
// ---------------------------------------------------------------------------

/// 异步发送消息，把 SSE token/done/error/clarify 事件发送到 mpsc channel。
/// 在独立 tokio task 中运行，不直接操作 stdout。
///
/// v1.3 WP-1: 改走 `/v2/agent/chat/completions`（单条 message，服务端持历史）。
/// 成功响应后通过 `TokenEvent::ConvId` 把新 conversation_id 发给主循环。
/// HTTP 404 表示会话已过期，通过 `TokenEvent::Error("SESSION_EXPIRED")` 通知。
#[allow(clippy::too_many_arguments)]
pub async fn send_message_tui(
    client: &Client,
    server: &str,
    token: &str,
    conv_id: Option<&str>,
    agent_id: Option<&str>,
    model: &str,
    message: &str,
    tx: tokio::sync::mpsc::Sender<chat_tui::TokenEvent>,
) {
    // v1.3 WP-1: use server-side session endpoint.
    let url = format!("{}/v2/agent/chat/completions", server);
    let body = AgentChatRequestV2 {
        message: message.to_string(),
        model: model.to_string(),
        stream: true,
        conversation_id: conv_id.map(|s| s.to_string()),
        agent_id: agent_id.map(|s| s.to_string()),
    };

    let resp = match client
        .post(&url)
        .bearer_auth(token)
        .header("Accept", "text/event-stream")
        .json(&body)
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => {
            let status = r.status();
            if status == reqwest::StatusCode::NOT_FOUND {
                let _ = tx
                    .send(chat_tui::TokenEvent::Error("SESSION_EXPIRED".to_string()))
                    .await;
            } else {
                let body_text = r.text().await.unwrap_or_default();
                let _ = tx
                    .send(chat_tui::TokenEvent::Error(format!(
                        "HTTP {} — {}",
                        status, body_text
                    )))
                    .await;
            }
            return;
        }
        Err(e) => {
            let _ = tx
                .send(chat_tui::TokenEvent::Error(format!("请求失败: {}", e)))
                .await;
            return;
        }
    };

    // 从响应头中读取服务端分配的 conversation_id，回传给主循环。
    if let Some(new_id) = resp
        .headers()
        .get("x-conversation-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
    {
        let _ = tx.try_send(chat_tui::TokenEvent::ConvId(new_id));
    }

    let tx_stream = tx.clone();
    let result = stream_sse(resp, |frame| {
        let tok = tx_stream.clone();
        match frame {
            SseFrame::Token(chunk) => {
                let _ = tok.try_send(chat_tui::TokenEvent::Token(chunk));
                true
            }
            SseFrame::Clarify(payload) => {
                let _ = tok.try_send(chat_tui::TokenEvent::Clarify(payload));
                true
            }
            SseFrame::ClarifyResolved => true,
            SseFrame::RateLimit(rl) => {
                let _ = tok.try_send(chat_tui::TokenEvent::RateLimit(chat_tui::RateLimitInfo {
                    provider: rl.provider,
                    requests_limit: rl.requests_limit,
                    requests_remaining: rl.requests_remaining,
                    tokens_limit: rl.tokens_limit,
                    tokens_remaining: rl.tokens_remaining,
                    requests_reset_secs: rl.requests_reset_secs,
                    tokens_reset_secs: rl.tokens_reset_secs,
                }));
                true
            }
            SseFrame::Usage(u) => {
                let _ = tok.try_send(chat_tui::TokenEvent::Usage(u));
                true
            }
            // BUG-CB-03: governance approval-pending notice.
            // Augment the spinner with an inline token so the user sees
            // "⏳ Awaiting approval for {tool} — check /approvals" without
            // replacing the assistant message being built.
            SseFrame::ApprovalPending { tool, reason } => {
                let notice = match reason.as_deref() {
                    Some(r) => format!(
                        "\n\u{23f3} Awaiting approval for {} ({}) — check /approvals\n",
                        tool, r
                    ),
                    None => format!(
                        "\n\u{23f3} Awaiting approval for {} — check /approvals\n",
                        tool
                    ),
                };
                let _ = tok.try_send(chat_tui::TokenEvent::Token(notice));
                true
            }
            // v1.3 WP-4 Change 3 + WP-3 Step 6: route SSE error frames through
            // the TUI's error channel with a typed visual prefix and the
            // friendly hint appended so the user gets actionable text.
            SseFrame::ErrorMsg {
                message,
                kind,
                reason,
            } => {
                let prefix = typed_error_prefix(reason, &kind);
                let composed = match reason {
                    Some(r) => format!("{prefix} {message}\n  {}", friendly_error_message(r),),
                    None => format!("{prefix} {message}"),
                };
                let _ = tok.try_send(chat_tui::TokenEvent::Error(composed));
                true
            }
            // v1.3 WP-3: typed tool / approval / heartbeat frames — forwarded
            // as TUI events so Steps 5+6 can render them inline.
            SseFrame::ToolStart { tool, args } => {
                let _ = tok.try_send(chat_tui::TokenEvent::ToolStart { tool, args });
                true
            }
            SseFrame::ToolComplete {
                tool,
                ok,
                preview,
                duration_ms,
            } => {
                let _ = tok.try_send(chat_tui::TokenEvent::ToolComplete {
                    tool,
                    ok,
                    preview,
                    duration_ms,
                });
                true
            }
            SseFrame::ApprovalGranted { tool } => {
                let _ = tok.try_send(chat_tui::TokenEvent::ApprovalGranted { tool });
                true
            }
            SseFrame::ApprovalDenied { tool, reason } => {
                let _ = tok.try_send(chat_tui::TokenEvent::ApprovalDenied { tool, reason });
                true
            }
            SseFrame::Heartbeat { elapsed_secs } => {
                let _ = tok.try_send(chat_tui::TokenEvent::Heartbeat { elapsed_secs });
                true
            }
            SseFrame::Done => false,
            SseFrame::Unknown => true,
        }
    })
    .await;

    match result {
        Ok(_) => {
            let _ = tx.send(chat_tui::TokenEvent::Done).await;
        }
        Err(e) => {
            let _ = tx
                .send(chat_tui::TokenEvent::Error(format!("SSE 错误: {}", e)))
                .await;
        }
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// 运行 `cyberclaw chat` REPL
pub async fn run(args: ChatArgs) -> Result<()> {
    let server = args
        .server
        .or_else(|| std::env::var("CYBERCLAW_SERVER").ok())
        .unwrap_or_else(|| "http://127.0.0.1:38090".to_string());
    let server = server.trim_end_matches('/').to_string();

    // `.no_proxy()` to bypass system HTTP_PROXY (otherwise Clash/Surge
    // intercept 127.0.0.1 local server traffic with TLS proxy 403).
    let client = Client::builder()
        .no_proxy()
        .build()
        .context("构建 HTTP client 失败")?;

    // 1. 获取 JWT
    let token = if let Some(t) = load_token() {
        t
    } else {
        println!("未找到 JWT，请登录 (server: {}):", server);
        login_and_save(&client, &server).await?
    };

    // 2. 解析/恢复 conversation
    // 优先级: --new > --conversation > --resume (旧参数) > ~/.cyberclaw/last-conversation > None
    // --new 始终胜出：若同时传了 --conversation/--resume，发出 warn 并忽略它们
    // BUG-R10-01: 不再通过 v1 endpoint 预建 conversation（v1 返回 conv_<hex> 格式 ID，
    // v2 用 Uuid::parse_str() 拒收该前缀）。第一条消息由 v2 自动创建会话，
    // UUID 从 X-Conversation-Id 响应头读取。
    let conv_id: Option<String> = {
        let explicit_id = if args.new {
            if args.conversation.is_some() || args.resume.is_some() {
                eprintln!("[warn] --new overrides --conversation/--resume; starting fresh");
            }
            None
        } else {
            args.conversation.or(args.resume)
        };
        if let Some(id) = explicit_id {
            // 验证存在
            let url = format!("{}/api/v1/chat/conversations/{}", server, id);
            let resp = client
                .get(&url)
                .bearer_auth(&token)
                .send()
                .await
                .with_context(|| format!("GET {}", url))?;
            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                if let Some(msg) = friendly_token_error(status, &body) {
                    anyhow::bail!(msg);
                }
                anyhow::bail!("conversation {} 不存在或无权限 ({})", id, status);
            }
            Some(id)
        } else if args.new {
            // --new 强制新建：不恢复上次，让 v2 endpoint 自动创建
            None
        } else if let Some(last_id) = chat_tui::load_last_conv_id() {
            // 尝试 resume 上次会话（验证仍存在）
            let url = format!("{}/api/v1/chat/conversations/{}", server, last_id);
            let resp = client.get(&url).bearer_auth(&token).send().await;
            match resp {
                Ok(r) if r.status().is_success() => Some(last_id),
                _ => {
                    // 上次 conv 已不存在，让 v2 自动创建
                    None
                }
            }
        } else {
            // 无任何缓存，让 v2 自动创建
            None
        }
    };

    // Model resolution priority (2026-05-19 real-business test discovery):
    //   1. --model CLI flag (operator explicit choice)
    //   2. GET /api/v2/status → default_model (live server truth, survives env rotation)
    //   3. ~/.cyberclaw/models.json `current_default` (local cache, may be stale)
    //   4. Hardcoded "gpt-4" as last-resort
    //
    // Before this change the order was [flag → local cache → hardcoded].
    // After admin rotates ~/.cyberclaw/llm.env (e.g. DeepSeek → MiniMax)
    // the local cache stays stale → chat-tui shipped "deepseek-chat" to a
    // MiniMax-backed server, which forwarded it verbatim and got
    // `unknown model 'deepseek-chat' (2013)` 400. Querying the live
    // server fixes the root cause.
    let current_model = match args.model {
        Some(m) => m,
        None => resolve_default_model(&client, &server).await,
    };
    let agent_id = args.agent;

    // 保存本次 conv_id（只在已有 UUID 时保存；新会话由 v2 响应头提供后再存）
    if let Some(ref id) = conv_id {
        chat_tui::save_last_conv_id(id);
    }

    // 进入 ratatui TUI
    chat_tui::run_tui(client, server, token, conv_id, agent_id, current_model).await
}

/// Resolve the default LLM model name when --model is not provided.
///
/// Tries live server (`/api/v2/status` default_model) first because that
/// is the authoritative source of "what the backend can serve right now".
/// Falls back to the local catalog cache (~/.cyberclaw/models.json
/// `current_default`) if server is unreachable. Falls back to "gpt-4"
/// only when neither is available.
async fn resolve_default_model(client: &Client, server: &str) -> String {
    // 1. Try server.
    #[derive(serde::Deserialize)]
    struct StatusResp {
        default_model: String,
    }
    let token = load_token().unwrap_or_default();
    let mut req = client
        .get(format!("{}/api/v2/status", server))
        .timeout(std::time::Duration::from_secs(3));
    if !token.is_empty() {
        req = req.bearer_auth(&token);
    }
    if let Ok(resp) = req.send().await {
        if resp.status().is_success() {
            if let Ok(s) = resp.json::<StatusResp>().await {
                if !s.default_model.is_empty() {
                    return s.default_model;
                }
            }
        }
    }

    // 2. Fall back to local catalog cache.
    let path = std::env::var("HOME").ok().map(|h| {
        std::path::PathBuf::from(h)
            .join(".cyberclaw")
            .join("models.json")
    });
    if let Some(p) = path {
        if let Ok(body) = std::fs::read_to_string(&p) {
            #[derive(serde::Deserialize)]
            struct CatalogResp {
                current_default: String,
            }
            if let Ok(c) = serde_json::from_str::<CatalogResp>(&body) {
                if !c.current_default.is_empty() {
                    tracing::info!(
                        model = %c.current_default,
                        "server status unreachable; using ~/.cyberclaw/models.json cache"
                    );
                    return c.current_default;
                }
            }
        }
    }

    // 3. Last-resort. We log at warn so operators see the failure mode
    // (better than silently shipping "gpt-4" against a non-OpenAI backend).
    let fallback = "gpt-4".to_string();
    tracing::warn!(
        model = %fallback,
        "no --model + server /api/v2/status unreachable + models.json missing; using hardcoded fallback"
    );
    fallback
}

/// 旧的 rustyline REPL（保留供 fallback / 测试）
#[allow(dead_code)]
async fn run_repl_legacy(
    client: Client,
    server: String,
    token: String,
    conv_id: String,
    agent_id: Option<String>,
    mut current_model: String,
) -> Result<()> {
    println!("cyberclaw chat — conversation: {}", conv_id);
    println!(
        "model: {}  |  /help 查看命令  |  Ctrl+D 退出",
        current_model
    );
    println!();

    let ctrl_c_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let flag = ctrl_c_flag.clone();
        tokio::spawn(async move {
            loop {
                if tokio::signal::ctrl_c().await.is_ok() {
                    flag.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            }
        });
    }

    let mut rl = DefaultEditor::new().context("初始化 readline 失败")?;
    let history_path = cyberclaw_config_dir()
        .map(|d| d.join("chat_history"))
        .unwrap_or_else(|_| PathBuf::from("/tmp/cyberclaw_chat_history"));
    let _ = rl.load_history(&history_path);

    let mut first_message = true;
    // v1.3 WP-1: 历史由服务端持有，客户端只需跟踪当前 conv_id。
    let mut current_conv_id = conv_id.clone();

    loop {
        ctrl_c_flag.store(false, std::sync::atomic::Ordering::Relaxed);

        let readline = rl.readline("> ");
        match readline {
            Ok(line) => {
                let input = line.trim().to_string();
                if input.is_empty() {
                    continue;
                }
                let _ = rl.add_history_entry(&input);

                if input.starts_with('/') {
                    match handle_slash(
                        &input,
                        &client,
                        &server,
                        &token,
                        &conv_id,
                        &mut current_model,
                    )
                    .await?
                    {
                        SlashResult::Quit => break,
                        SlashResult::Continue => continue,
                    }
                }

                if first_message {
                    first_message = false;
                    let title: String = input.chars().take(40).collect();
                    let patch_url = format!("{}/api/v1/chat/conversations/{}", server, conv_id);
                    let _ = client
                        .patch(&patch_url)
                        .bearer_auth(&token)
                        .json(&serde_json::json!({ "title": title }))
                        .send()
                        .await;
                }

                // v1.3 WP-1: 只发当前用户消息，不再累积历史。
                match send_message(
                    &client,
                    &server,
                    &token,
                    Some(current_conv_id.as_str()),
                    agent_id.as_deref(),
                    &current_model,
                    &input,
                    &ctrl_c_flag,
                )
                .await
                {
                    Ok((_reply, new_id)) => {
                        if let Some(id) = new_id {
                            current_conv_id = id;
                        }
                    }
                    Err(e) => {
                        let msg = e.to_string();
                        if msg.contains("SESSION_EXPIRED") {
                            eprintln!("[Session expired] 会话已超时，下次消息将新建会话。");
                        } else {
                            eprintln!("[!] 请求失败: {:#}", e);
                        }
                    }
                }
            }
            Err(ReadlineError::Eof) => {
                println!("\n再见！");
                break;
            }
            Err(ReadlineError::Interrupted) => {
                println!("(Ctrl+C — 输入 /quit 退出)");
                continue;
            }
            Err(e) => {
                eprintln!("[!] readline 错误: {}", e);
                break;
            }
        }
    }

    let _ = rl.save_history(&history_path);
    Ok(())
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_sse_frame_token() {
        let data = r#"{"choices":[{"delta":{"content":"Hello"}}]}"#;
        let frame = parse_sse_data(data);
        match frame {
            SseFrame::Token(s) => assert_eq!(s, "Hello"),
            other => panic!("expected Token, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_sse_frame_done() {
        let frame = parse_sse_data("[DONE]");
        assert!(matches!(frame, SseFrame::Done));
    }

    #[test]
    fn test_parse_sse_frame_clarify() {
        let data = r#"{
            "type": "clarify",
            "clarify": {
                "id": "clr-001",
                "conversation_id": "conv-abc",
                "questions": [
                    {
                        "question": "Which env?",
                        "options": [
                            {"label": "staging-a", "description": "Primary"},
                            {"label": "staging-b", "description": "Secondary"}
                        ],
                        "multi_select": false
                    }
                ]
            }
        }"#;
        let frame = parse_sse_data(data);
        match frame {
            SseFrame::Clarify(p) => {
                assert_eq!(p.id, "clr-001");
                assert_eq!(p.questions.len(), 1);
                assert_eq!(p.questions[0].options.len(), 2);
                assert_eq!(p.questions[0].options[0].label, "staging-a");
            }
            other => panic!("expected Clarify, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_sse_frame_mixed() {
        // 模拟混合流：token → clarify → token → done
        let frames = [
            r#"{"choices":[{"delta":{"content":"Hi "}}]}"#,
            r#"{"type":"clarify","clarify":{"id":"c1","conversation_id":"v1","questions":[{"question":"Q?","options":[{"label":"A","description":"desc A"},{"label":"B","description":"desc B"}],"multi_select":false}]}}"#,
            r#"{"choices":[{"delta":{"content":"there"}}]}"#,
            "[DONE]",
        ];

        let parsed: Vec<SseFrame> = frames.iter().map(|d| parse_sse_data(d)).collect();

        assert!(matches!(&parsed[0], SseFrame::Token(s) if s == "Hi "));
        assert!(matches!(&parsed[1], SseFrame::Clarify(_)));
        assert!(matches!(&parsed[2], SseFrame::Token(s) if s == "there"));
        assert!(matches!(&parsed[3], SseFrame::Done));
    }

    #[test]
    fn test_parse_sse_frame_clarify_resolved() {
        let data = r#"{"type":"clarify_resolved","clarify_id":"c1","answer":{"answers":{}}}"#;
        let frame = parse_sse_data(data);
        assert!(matches!(frame, SseFrame::ClarifyResolved));
    }

    #[test]
    fn test_parse_sse_frame_unknown() {
        let frame = parse_sse_data("not json at all");
        assert!(matches!(frame, SseFrame::Unknown));
    }

    // BUG-CB-03 tests

    #[test]
    fn test_parse_approval_pending_frame_serializes_correctly() {
        let data = r#"{"type":"approval_pending","approval_pending":{"tool":"fs.write","reason":"Path outside workspace"}}"#;
        let frame = parse_sse_data(data);
        match frame {
            SseFrame::ApprovalPending { tool, reason } => {
                assert_eq!(tool, "fs.write");
                assert_eq!(reason.as_deref(), Some("Path outside workspace"));
            }
            other => panic!("expected ApprovalPending, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_unknown_sse_frame_type_does_not_crash() {
        // Forward compat: an unknown frame type must return Unknown, not panic.
        let data = r#"{"type":"some_future_frame_type_v99","data":{"foo":"bar"}}"#;
        let frame = parse_sse_data(data);
        assert!(
            matches!(frame, SseFrame::Unknown),
            "Unknown SSE frame types must be silently skipped"
        );
    }

    // v1.3 WP-4 Change 3 tests — typed reason → friendly hint lookup.

    #[test]
    fn test_friendly_error_message_auth_invalid_mentions_rm_token() {
        let hint = friendly_error_message(LlmFailoverReason::AuthInvalid);
        assert!(
            hint.contains("rm ~/.cyberclaw/cli-token"),
            "AuthInvalid hint must instruct user to remove the stale token; got: {hint}"
        );
    }

    #[test]
    fn test_friendly_error_message_billing_mentions_credentials() {
        let hint = friendly_error_message(LlmFailoverReason::Billing);
        assert!(
            hint.contains("余额") || hint.contains("billing"),
            "Billing hint must mention balance/billing; got: {hint}"
        );
    }

    #[test]
    fn test_friendly_error_message_context_overflow_mentions_compress() {
        let hint = friendly_error_message(LlmFailoverReason::ContextOverflow);
        assert!(
            hint.contains("/compress"),
            "ContextOverflow hint must mention /compress; got: {hint}"
        );
    }

    #[test]
    fn test_friendly_error_message_covers_every_variant() {
        // Smoke test — every variant must produce a non-empty &'static str so
        // a future enum extension can't silently fall through to a placeholder.
        for reason in [
            LlmFailoverReason::Billing,
            LlmFailoverReason::RateLimit,
            LlmFailoverReason::ContextOverflow,
            LlmFailoverReason::ImageTooLarge,
            LlmFailoverReason::ModelNotFound,
            LlmFailoverReason::AuthInvalid,
            LlmFailoverReason::AuthExpired,
            LlmFailoverReason::PermissionDenied,
            LlmFailoverReason::QuotaExceeded,
            LlmFailoverReason::ServiceUnavailable,
            LlmFailoverReason::Timeout,
            LlmFailoverReason::InternalError,
            LlmFailoverReason::BadRequest,
            LlmFailoverReason::ContentFilter,
            LlmFailoverReason::ThinkingSignature,
            LlmFailoverReason::Unknown,
        ] {
            assert!(
                !friendly_error_message(reason).is_empty(),
                "reason {reason:?} must produce a non-empty hint"
            );
        }
    }

    #[test]
    fn test_parse_sse_error_frame_with_typed_reason() {
        let data =
            r#"{"error":{"message":"jwt expired","type":"llm_error","reason":"auth_expired"}}"#;
        let frame = parse_sse_data(data);
        match frame {
            SseFrame::ErrorMsg {
                message,
                kind,
                reason,
            } => {
                assert_eq!(message, "jwt expired");
                assert_eq!(kind, "llm_error");
                assert_eq!(reason, Some(LlmFailoverReason::AuthExpired));
            }
            other => panic!("expected ErrorMsg, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_sse_error_frame_without_reason_is_legacy_compatible() {
        // Pre-WP-4 servers don't emit `reason` — the frame must still
        // deserialise with `reason = None`.
        let data = r#"{"error":{"message":"oops","type":"internal"}}"#;
        let frame = parse_sse_data(data);
        match frame {
            SseFrame::ErrorMsg {
                message,
                kind,
                reason,
            } => {
                assert_eq!(message, "oops");
                assert_eq!(kind, "internal");
                assert_eq!(reason, None);
            }
            other => panic!("expected ErrorMsg, got {other:?}"),
        }
    }

    // BUG-CB-07: residual buffer handles split multi-byte chars across chunks.
    //
    // We test the core decode logic directly (without a live HTTP response).
    // The scenario: a 3-byte UTF-8 CJK character is split so that the first
    // two bytes arrive in chunk A and the third byte arrives in chunk B.
    // The fix must yield the complete character, not an error.
    #[test]
    fn test_stream_sse_handles_split_multi_byte_chars() {
        // "你" = 0xE4 0xBD 0xA0 (3 bytes)
        // "好" = 0xE5 0xA5 0xBD (3 bytes)
        let full = "你好";
        let bytes = full.as_bytes();
        assert_eq!(bytes.len(), 6);

        // Split after 2nd byte — first chunk is incomplete UTF-8.
        let chunk_a = &bytes[..2]; // 0xE4 0xBD — incomplete "你"
        let chunk_b = &bytes[2..]; // 0xA0 0xE5 0xA5 0xBD — rest of "你" + "好"

        let mut residual: Vec<u8> = Vec::new();
        let mut assembled = String::new();

        for chunk in [chunk_a, chunk_b] {
            residual.extend_from_slice(chunk);
            match std::str::from_utf8(&residual) {
                Ok(s) => {
                    assembled.push_str(s);
                    residual.clear();
                }
                Err(e) => {
                    let valid_up_to = e.valid_up_to();
                    assembled.push_str(
                        std::str::from_utf8(&residual[..valid_up_to])
                            .expect("valid_up_to guarantees valid UTF-8"),
                    );
                    residual = residual[valid_up_to..].to_vec();
                }
            }
        }

        // After processing both chunks the residual must be empty and the
        // assembled text must equal the original string.
        assert!(
            residual.is_empty(),
            "residual should be empty after all chunks"
        );
        assert_eq!(
            assembled, full,
            "assembled text must equal original CJK string"
        );
    }

    // ─── v1.3 WP-3 — typed wire envelope tests ──────────────────────────

    #[test]
    fn test_parse_wire_frame_token() {
        let envelope = r#"{"v":1,"type":"token","data":{"content":"Hello"}}"#;
        let frame = parse_sse_data(envelope);
        match frame {
            SseFrame::Token(s) => assert_eq!(s, "Hello"),
            other => panic!("expected Token from wire envelope, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_wire_frame_tool_start() {
        let envelope =
            r#"{"v":1,"type":"tool_start","data":{"tool":"fs.read","args":{"path":"/tmp/x"}}}"#;
        match parse_sse_data(envelope) {
            SseFrame::ToolStart { tool, args } => {
                assert_eq!(tool, "fs.read");
                assert_eq!(args["path"], "/tmp/x");
            }
            other => panic!("expected ToolStart, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_wire_frame_tool_complete() {
        let envelope = r#"{"v":1,"type":"tool_complete","data":{"tool":"fs.read","ok":true,"preview":"ok","duration_ms":42}}"#;
        match parse_sse_data(envelope) {
            SseFrame::ToolComplete {
                tool,
                ok,
                preview,
                duration_ms,
            } => {
                assert_eq!(tool, "fs.read");
                assert!(ok);
                assert_eq!(preview, "ok");
                assert_eq!(duration_ms, 42);
            }
            other => panic!("expected ToolComplete, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_wire_frame_error_typed_kind() {
        let envelope =
            r#"{"v":1,"type":"error","data":{"message":"jwt expired","kind":"auth_expired"}}"#;
        match parse_sse_data(envelope) {
            SseFrame::ErrorMsg {
                message,
                kind: _kind,
                reason,
            } => {
                assert_eq!(message, "jwt expired");
                assert_eq!(reason, Some(LlmFailoverReason::AuthExpired));
            }
            other => panic!("expected ErrorMsg, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_wire_frame_heartbeat() {
        let envelope = r#"{"v":1,"type":"heartbeat","data":{"elapsed_secs":15}}"#;
        match parse_sse_data(envelope) {
            SseFrame::Heartbeat { elapsed_secs } => assert_eq!(elapsed_secs, 15),
            other => panic!("expected Heartbeat, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_wire_frame_version_mismatch_yields_error_with_hint() {
        // A v=2 envelope from a future server must produce a friendly error
        // frame instead of crashing or falling through to Unknown.
        let envelope = r#"{"v":2,"type":"token","data":{"content":"hi"}}"#;
        match parse_sse_data(envelope) {
            SseFrame::ErrorMsg { message, kind, .. } => {
                assert!(
                    message.contains("v2") || message.contains("update"),
                    "version mismatch message should mention version / update; got: {message}"
                );
                assert_eq!(kind, "version_mismatch");
            }
            other => panic!("expected ErrorMsg version_mismatch, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_legacy_token_still_works() {
        // Legacy frame format (no `v` field) must still parse via the
        // fallback path so a new CLI keeps talking to a pre-WP-3 server.
        let legacy = r#"{"choices":[{"delta":{"content":"legacy"}}]}"#;
        match parse_sse_data(legacy) {
            SseFrame::Token(s) => assert_eq!(s, "legacy"),
            other => panic!("expected Token from legacy parser, got {other:?}"),
        }
    }

    // ─── v1.3 WP-3 Step 6 — typed error prefix tests ───────────────────

    #[test]
    fn test_typed_error_prefix_billing() {
        let prefix = typed_error_prefix(Some(LlmFailoverReason::Billing), "llm_error");
        assert!(prefix.contains("Billing"), "got: {prefix}");
        assert!(prefix.starts_with('['));
    }

    #[test]
    fn test_typed_error_prefix_auth_invalid() {
        let prefix = typed_error_prefix(Some(LlmFailoverReason::AuthInvalid), "llm_error");
        assert!(prefix.contains("Auth Error"), "got: {prefix}");
    }

    #[test]
    fn test_typed_error_prefix_context_overflow() {
        let prefix = typed_error_prefix(Some(LlmFailoverReason::ContextOverflow), "llm_error");
        assert!(prefix.contains("Context"), "got: {prefix}");
    }

    #[test]
    fn test_typed_error_prefix_no_reason_falls_back_to_kind() {
        let prefix = typed_error_prefix(None, "invalid_request");
        assert_eq!(prefix, "[Error: invalid_request]");
    }
}
