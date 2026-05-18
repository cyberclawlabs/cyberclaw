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

/// SSE 帧解析结果
#[derive(Debug)]
pub enum SseFrame {
    /// LLM token 片段
    Token(String),
    /// 澄清请求
    Clarify(ClarifyPayload),
    /// 澄清已解答（静默继续）
    ClarifyResolved,
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

/// POST /api/v1/chat/conversations 请求体
#[derive(Debug, Serialize)]
struct CreateConversationRequest {
    title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_id: Option<String>,
}

/// POST /api/v1/chat/conversations 响应体（部分）
#[derive(Debug, Deserialize)]
struct CreateConversationResponse {
    id: String,
}

/// POST /api/v1/chat/completions 请求体
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

/// 把 401 InvalidSignature / ExpiredSignature 错误转成对用户友好的多行提示。
/// 返回 None = 不是可识别的 token 错误（让调用方走默认错误路径）。
fn friendly_token_error(status: reqwest::StatusCode, body: &str) -> Option<String> {
    if status != reqwest::StatusCode::UNAUTHORIZED {
        return None;
    }
    if !(body.contains("InvalidSignature")
        || body.contains("ExpiredSignature")
        || body.contains("Invalid token"))
    {
        return None;
    }
    Some(format!(
        "Token 被拒绝 (401 {})\n\n\
         常见原因：\n  · 服务端 JWT_SECRET 重启后变了（你本地的 cli-token 签名作废）\n  · token TTL 过期\n\n\
         修复：\n  rm ~/.cyberclaw/cli-token && cyberclaw chat   # 重新交互式登录\n  或：cyberclaw chat --new                       # 强制重新登录路径",
        body.lines().next().unwrap_or(body)
    ))
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
pub fn parse_sse_data(data: &str) -> SseFrame {
    let data = data.trim();
    if data == "[DONE]" {
        return SseFrame::Done;
    }

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
        _ => {
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

    while let Some(chunk) = byte_stream.next().await {
        let chunk = chunk.context("读取 SSE 字节流失败")?;
        let text = std::str::from_utf8(&chunk).context("SSE 字节非 UTF-8")?;
        buf.push_str(text);

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

/// 创建新 conversation，返回 ID
async fn create_conversation(
    client: &Client,
    server: &str,
    token: &str,
    title: &str,
    agent_id: Option<&str>,
) -> Result<String> {
    let url = format!("{}/api/v1/chat/conversations", server);
    let body = CreateConversationRequest {
        title: title.to_string(),
        agent_id: agent_id.map(|s| s.to_string()),
    };

    let resp = client
        .post(&url)
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .with_context(|| format!("POST {}", url))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if let Some(msg) = friendly_token_error(status, &body) {
            anyhow::bail!(msg);
        }
        anyhow::bail!("创建 conversation 失败 ({}): {}", status, body);
    }

    let created: CreateConversationResponse =
        resp.json().await.context("解析 conversation 响应失败")?;
    Ok(created.id)
}

/// 向 agent 发送消息，流式打印 token，处理 clarify
#[allow(clippy::too_many_arguments)]
async fn send_message(
    client: &Client,
    server: &str,
    token: &str,
    conv_id: &str,
    agent_id: Option<&str>,
    model: &str,
    messages: &[ChatMessage],
    ctrl_c_flag: &std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<String> {
    // 2026-05-17 — moved from /v1/chat/completions to /v1/agent/chat/completions
    // for parity with chat_conversations.rs WebUI path. Both endpoints have
    // silent-abandon enforcement (v1.2.4); /v1/agent/chat/completions
    // additionally has the full 41-tool palette + skill_search inline intercept
    // wired through DefaultAgenticLoop.
    let url = format!("{}/v1/agent/chat/completions", server);
    let body = ChatCompletionRequest {
        model: model.to_string(),
        messages: messages.to_vec(),
        stream: true,
        conversation_id: Some(conv_id.to_string()),
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
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("chat/completions 失败 ({}): {}", status, body);
    }

    let mut assistant_content = String::new();
    let client_clone = client.clone();
    let server_owned = server.to_string();
    let token_owned = token.to_string();
    let conv_id_owned = conv_id.to_string();
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
            SseFrame::Done => false,
            SseFrame::Unknown => true,
        }
    })
    .await?;

    println!(); // 换行（token 流不带换行）
    Ok(assistant_content)
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
            println!("Slash 命令:");
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
            println!("[!] 未知命令: {}。输入 /help 查看命令列表", parts[0]);
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
#[allow(clippy::too_many_arguments)]
pub async fn send_message_tui(
    client: &Client,
    server: &str,
    token: &str,
    conv_id: &str,
    agent_id: Option<&str>,
    model: &str,
    messages: &[ChatMessage],
    tx: tokio::sync::mpsc::Sender<chat_tui::TokenEvent>,
) {
    // 2026-05-17 — moved from /v1/chat/completions to /v1/agent/chat/completions
    // for parity with chat_conversations.rs WebUI path. Both endpoints have
    // silent-abandon enforcement (v1.2.4); /v1/agent/chat/completions
    // additionally has the full 41-tool palette + skill_search inline intercept
    // wired through DefaultAgenticLoop.
    let url = format!("{}/v1/agent/chat/completions", server);
    let body = ChatCompletionRequest {
        model: model.to_string(),
        messages: messages.to_vec(),
        stream: true,
        conversation_id: Some(conv_id.to_string()),
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
            let body_text = r.text().await.unwrap_or_default();
            let _ = tx
                .send(chat_tui::TokenEvent::Error(format!(
                    "HTTP {} — {}",
                    status, body_text
                )))
                .await;
            return;
        }
        Err(e) => {
            let _ = tx
                .send(chat_tui::TokenEvent::Error(format!("请求失败: {}", e)))
                .await;
            return;
        }
    };

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

    // 2. 解析/创建 conversation
    // 优先级: --conversation > --resume (旧参数) > ~/.cyberclaw/last-conversation > 新建
    let conv_id = {
        let explicit_id = args.conversation.or(args.resume);
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
            id
        } else if !args.new {
            // 尝试 resume 上次
            if let Some(last_id) = chat_tui::load_last_conv_id() {
                let url = format!("{}/api/v1/chat/conversations/{}", server, last_id);
                let resp = client.get(&url).bearer_auth(&token).send().await;
                match resp {
                    Ok(r) if r.status().is_success() => last_id,
                    _ => {
                        // 上次 conv 已不存在，新建
                        create_conversation(
                            &client,
                            &server,
                            &token,
                            "New Chat",
                            args.agent.as_deref(),
                        )
                        .await?
                    }
                }
            } else {
                create_conversation(&client, &server, &token, "New Chat", args.agent.as_deref())
                    .await?
            }
        } else {
            // --new 强制新建
            create_conversation(&client, &server, &token, "New Chat", args.agent.as_deref()).await?
        }
    };

    // S17 CLI --model default — 优先 --model flag，否则**直接读本地** ~/.cyberclaw/models.json
    // （CLI 自治：catalog 是本地状态，不依赖 server 在线即可工作）。
    let current_model = args.model.unwrap_or_else(|| {
        let path = std::env::var("HOME")
            .ok()
            .map(|h| std::path::PathBuf::from(h).join(".cyberclaw").join("models.json"));
        if let Some(p) = path {
            if let Ok(body) = std::fs::read_to_string(&p) {
                #[derive(serde::Deserialize)]
                struct CatalogResp {
                    current_default: String,
                }
                if let Ok(c) = serde_json::from_str::<CatalogResp>(&body) {
                    return c.current_default;
                }
            }
        }
        let fallback = "deepseek-chat".to_string();
        tracing::warn!(model = %fallback, "no --model + models.json missing; using fallback");
        fallback
    });
    let agent_id = args.agent;

    // 保存本次 conv_id
    chat_tui::save_last_conv_id(&conv_id);

    // 进入 ratatui TUI
    chat_tui::run_tui(client, server, token, conv_id, agent_id, current_model).await
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

    let mut messages: Vec<ChatMessage> = Vec::new();
    let mut first_message = true;

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

                messages.push(ChatMessage {
                    role: "user".to_string(),
                    content: input.clone(),
                });

                match send_message(
                    &client,
                    &server,
                    &token,
                    &conv_id,
                    agent_id.as_deref(),
                    &current_model,
                    &messages,
                    &ctrl_c_flag,
                )
                .await
                {
                    Ok(reply) => {
                        if !reply.is_empty() {
                            messages.push(ChatMessage {
                                role: "assistant".to_string(),
                                content: reply,
                            });
                        }
                    }
                    Err(e) => {
                        eprintln!("[!] 请求失败: {:#}", e);
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
}
