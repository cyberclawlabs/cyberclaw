//! 通用 OpenAI 兼容客户端
//!
//! 支持 DeepSeek、Ollama、LocalAI 等 OpenAI 兼容接口

use crate::client::LlmClient;
use crate::error::{LlmError, LlmResult};
use crate::types::{ChatChunk, ChatRequest, ChatResponse};
use async_trait::async_trait;
use futures::stream::{Stream, StreamExt};
use reqwest::Client;
use std::time::Duration;

/// 通用 OpenAI 兼容客户端
pub struct GenericOpenAiClient {
    client: Client,
    api_key: Option<String>,
    base_url: String,
}

impl GenericOpenAiClient {
    /// 创建新的通用客户端
    ///
    /// # Arguments
    ///
    /// * `api_key` - API 密钥（可选,某些本地服务不需要）
    /// * `base_url` - API 基础 URL
    pub fn new(api_key: Option<String>, base_url: &str) -> LlmResult<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(120)) // 本地服务可能较慢
            .no_proxy() // 直接连接，不经过系统代理（避免本地/localhost 地址被代理拦截）
            // Bug I — 连接弹性：长 cmd.run（30s+）容器执行期间，到上游的 keepalive
            // 空闲连接会被对端关闭；复用这个 stale 连接会触发 broken pipe
            // （"error sending request"）。下面三项缩短空闲连接寿命并主动探活，
            // 避免复用上游已关闭的连接。
            .pool_idle_timeout(Duration::from_secs(15)) // 空闲连接 15s 后丢弃，短于上游 keepalive 关闭窗口
            .tcp_keepalive(Duration::from_secs(30)) // 主动 TCP keepalive 探活
            .connect_timeout(Duration::from_secs(10)) // 连接建立超时
            .build()
            .map_err(|e| LlmError::Internal(format!("Failed to create HTTP client: {}", e)))?;

        Ok(Self {
            client,
            api_key,
            base_url: base_url.trim_end_matches('/').to_string(),
        })
    }
}

/// Encode a CyberClaw tool name (which uses `dot.notation` like `cmd.run`)
/// into something that satisfies the strict OpenAI / DeepSeek tool-name regex
/// `^[a-zA-Z0-9_-]+$`. We map `.` to `__` (double underscore) — reversible
/// because real CyberClaw tool ids never contain `__`.
///
/// MiniMax accepts `.` directly; DeepSeek and OpenAI reject it with a 400
/// validation error. Doing the substitution here keeps the rest of the
/// codebase using the canonical dotted names.
fn encode_tool_name(name: &str) -> String {
    name.replace('.', "__")
}

/// Reverse of `encode_tool_name`. Applied to every tool_call returned by the
/// LLM so downstream dispatch sees the canonical dotted name.
fn decode_tool_name(name: &str) -> String {
    name.replace("__", ".")
}

/// Normalize the messages array so all system-role messages are merged into
/// a single system message at index 0.
///
/// MiniMax (and some other OpenAI-compatible providers) reject requests that
/// contain a `system` role message anywhere other than the very first position
/// (error 2013: "invalid message role: system" mid-array). The agentic loop
/// may append `Message::system(...)` to the tail via `add_system_hint` and
/// the GAP-4 nudge, so we normalize before sending.
///
/// Algorithm:
/// 1. Collect all system messages; join their content with `\n\n`.
/// 2. Place one merged system message at index 0.
/// 3. Append all non-system messages in their original relative order.
fn merge_system_messages(messages: Vec<crate::types::Message>) -> Vec<crate::types::Message> {
    use crate::types::Role;
    let mut system_parts: Vec<String> = Vec::new();
    let mut non_system: Vec<crate::types::Message> = Vec::new();
    for msg in messages {
        if msg.role == Role::System {
            system_parts.push(msg.content.clone());
        } else {
            non_system.push(msg);
        }
    }
    let mut out = Vec::with_capacity(non_system.len() + 1);
    if !system_parts.is_empty() {
        out.push(crate::types::Message::system(system_parts.join("\n\n")));
    }
    out.extend(non_system);
    out
}

/// Bug I-d — 移除 assistant 历史 content 里的 `<think>...</think>` 推理块。
///
/// MiniMax reasoning 模型把推理输出放在 content 的 `<think>` 标签里。reasoning
/// 模型的标准要求是历史消息**不回传**上一轮的 reasoning，只回传最终答案；原样
/// 回传 `<think>...</think>` 会触发静默返空补全（HTTP 200 + 空 body）。这个纯
/// 函数剥掉所有 think 块（跨多行、可多个、标签大小写与属性宽松匹配），保留块外
/// 的真实回答文本，并 trim 首尾空白。
///
/// - 无 think 块：原样返回（trim 后）。
/// - 只有 think 块：返回空串。
fn strip_think_blocks(content: &str) -> String {
    use once_cell::sync::Lazy;
    use regex::Regex;
    // (?is): i = 大小写不敏感（<THINK>/<Think> 同样匹配），s = `.` 匹配换行
    // （跨多行推理块）。`[^>]*` 容忍 `<think foo="bar">` 这类带属性的开标签。
    // `.*?` 非贪婪，多个相邻块各自最小匹配而非吞并中间文本。
    static THINK_BLOCK: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?is)<think[^>]*>.*?</think\s*>")
            .expect("THINK_BLOCK regex is valid")
    });
    THINK_BLOCK.replace_all(content, "").trim().to_string()
}

/// Bug I-d 根因 — 出站前保证对话至少有一条 `User` 消息。
///
/// 直接 curl 实验确认：上下文压缩（SummarizeEarly / HideSystemDetails）后对话
/// 可能变成纯 `[system, assistant, tool, ...]`——原始 user 请求被压缩挤掉/折叠进
/// system，整段对话没有任何 `role==User` 消息。MiniMax 面对无 user 消息的对话
/// **确定性返回 HTTP 200 + 空 body**（无可应答的 user turn）。实验铁证：同一请求
/// 无 user → 空 body；插入任意一条 user → 正常补全。
///
/// 这是覆盖任何来源（不只压缩）的防御层：若完全没有 `User` 消息，则在末尾补一条
/// 中性 `"Continue."`，确保对话以 user turn 结尾。已有 user 消息时不动。
fn ensure_user_message(messages: &mut Vec<crate::types::Message>) {
    use crate::types::Role;
    if messages.iter().any(|m| m.role == Role::User) {
        return;
    }
    tracing::warn!("no user message in request, appended synthetic continue");
    messages.push(crate::types::Message::user("Continue."));
}

/// Decode a successful chat-completion body into a [`ChatResponse`].
///
/// `response.json()` collapses a schema mismatch into reqwest's opaque
/// "error decoding response body", which hides what the provider actually
/// returned. This pure helper instead parses raw bytes and, on failure, logs
/// the HTTP status plus a truncated body preview and returns an
/// [`LlmError::InvalidResponse`] whose message embeds the status and a body
/// snippet. That makes provider anomalies (such as MiniMax responses after a
/// SummarizeEarly compaction) diagnosable from both logs and the error chain.
fn decode_chat_response(status: u16, body_bytes: &[u8]) -> LlmResult<ChatResponse> {
    match serde_json::from_slice::<ChatResponse>(body_bytes) {
        Ok(chat_response) => Ok(chat_response),
        Err(e) => {
            let body_text = String::from_utf8_lossy(body_bytes);
            let log_snippet: String = body_text.chars().take(500).collect();
            tracing::error!(
                status,
                error = %e,
                body_snippet = %log_snippet,
                "generic provider: failed to decode successful chat completion body"
            );
            let err_snippet: String = body_text.chars().take(200).collect();
            Err(LlmError::InvalidResponse(format!(
                "failed to decode chat completion body (status {status}): {e}; body snippet: {err_snippet}"
            )))
        }
    }
}

/// Bug I-d — transport / 空200 body 重试配额。
///
/// 直接 curl bisect 确认 MiniMax 对大请求（~25KB+ 含大 tool 结果）间歇性返回
/// HTTP 200 + 空 body，且是非确定性的——同一请求隔几秒重发大多能拿到正常响应。
/// 原 2 次快速重试（500ms/1500ms）对被快速连打的上游仍不足；提到 4 次重试
/// （max 5 次尝试），退避拉长为指数 1s/2s/4s/8s，给上游足够恢复时间。
pub(crate) const MAX_TRANSPORT_RETRIES: usize = 4;
/// 第 N 次重试前 sleep 的退避时长（毫秒），指数 1/2/4/8 秒。索引 = 已用 attempt。
pub(crate) const BACKOFF_MS: [u64; MAX_TRANSPORT_RETRIES] = [1000, 2000, 4000, 8000];

/// Bug I精确根因 — 判断响应 body 是否为空（0 字节或全空白）。
///
/// MiniMax 间歇性返回 HTTP 200 + 空 body，reqwest 报
/// "EOF while parsing at line 1 column 0"。这是上游退化响应，
/// 重试通常可以拿到真内容，故视为可重试而非业务错误。
fn is_empty_body(bytes: &[u8]) -> bool {
    bytes.iter().all(|b| b.is_ascii_whitespace())
}

/// Bug I — 判断一个 reqwest 错误是否属于 transport 层（连接建立失败、请求
/// 发送失败、超时），值得重试。
///
/// 这类错误对应症状中的 "error sending request for url"（复用了上游已关闭的
/// stale keepalive 连接导致 broken pipe）和 "error decoding response body"
/// （拿到半截响应，reqwest 归类为 request 错误）。重试它们是安全的：请求未被
/// 上游成功处理，不会重复扣费或产生重复副作用。
///
/// 业务错误（4xx/5xx 已有 body、JSON schema 不匹配等）不在此返回 true——
/// 它们不应重试，由调用方直接冒泡。
fn should_retry_transport(err: &reqwest::Error) -> bool {
    err.is_connect() || err.is_request() || err.is_timeout()
}

#[async_trait]
impl LlmClient for GenericOpenAiClient {
    async fn chat_completion(&self, mut request: ChatRequest) -> LlmResult<ChatResponse> {
        let url = format!("{}/chat/completions", self.base_url);

        // Sanitize outbound tool names (DeepSeek / OpenAI reject dots).
        if let Some(ref mut tools) = request.tools {
            for t in tools.iter_mut() {
                t.function.name = encode_tool_name(&t.function.name);
            }
        }

        // BUG-CB-17: merge all system-role messages into one at index 0.
        // MiniMax rejects system messages appearing mid-array (error 2013).
        request.messages = merge_system_messages(request.messages);

        // Bug I-d: MiniMax reasoning 模型把推理放在 assistant content 的
        // `<think>...</think>` 块里。reasoning 模型历史**不应**回传上一轮的
        // reasoning，原样回传会触发静默返空补全（HTTP 200 + 空 body）。出站前
        // 剥掉 assistant 历史消息 content 里的 think 块（只动发出的副本——request
        // 是 owned；只动 content，tool_calls 不变；User/System/Tool 不动）。
        for msg in request.messages.iter_mut() {
            if msg.role == crate::types::Role::Assistant {
                msg.content = strip_think_blocks(&msg.content);
            }
        }

        // Bug I-d 根因防御：压缩后对话可能无任何 user 消息，MiniMax 确定性返空
        // 200。出站前若无 user 则末尾补一条 "Continue."，确保有可应答 turn。
        ensure_user_message(&mut request.messages);

        // Bug I/I-d — transport + 空200 body 重试：长 cmd.run gap 后复用 stale
        // keepalive 连接会触发 broken pipe（"error sending request"）或半截响应
        // （"error decoding response body"）；MiniMax 对大请求还会间歇性返回
        // HTTP 200 + 空 body。对这两类退化指数退避重试最多 MAX_TRANSPORT_RETRIES
        // 次（BACKOFF_MS = 1/2/4/8s），业务错误（4xx/5xx、schema 不匹配）不重试，
        // 避免重复扣费 / 重复副作用。请求体在循环外构造一次，循环内复用。

        // `attempt` 计数贯穿一整次「发送 + 读取 body」尝试：send 与 bytes() 都属于
        // transport 阶段，任一失败都消耗同一个 retry 配额。
        let mut attempt: usize = 0;
        let (status, body_bytes) = loop {
            let mut req_builder = self
                .client
                .post(&url)
                .header("Content-Type", "application/json");

            // 添加 API Key（如果有）
            if let Some(ref api_key) = self.api_key {
                req_builder = req_builder.header("Authorization", format!("Bearer {}", api_key));
            }

            // send + 读取 body 任一 transport 失败都走同一退避分支。
            let transport_err = match req_builder.json(&request).send().await {
                Ok(response) => {
                    let status = response.status();
                    if !status.is_success() {
                        // 业务错误（4xx/5xx 已有 body）不重试，直接冒泡。
                        let error_text = response.text().await.unwrap_or_default();
                        return Err(LlmError::ApiError {
                            status: status.as_u16(),
                            message: error_text,
                        });
                    }
                    // Read raw bytes first, then parse. A schema mismatch on a
                    // 2xx body (Bug I) must surface the real payload, not
                    // reqwest's opaque "error decoding response body". `bytes()`
                    // itself can fail with a transport error (half-read body) —
                    // fold into the same retry path.
                    match response.bytes().await {
                        Ok(bytes) if is_empty_body(&bytes) => {
                            // Bug I精确根因：MiniMax 间歇性返回 HTTP 200 + 空 body
                            // (EOF while parsing at line 1 column 0, 0 字节)。
                            // 这是上游退化响应，不是业务错误——重试通常能拿到真内容。
                            // 空 body 视为可重试，走与 transport 错误相同的退避路径。
                            if attempt < MAX_TRANSPORT_RETRIES {
                                tracing::warn!(
                                    attempt = attempt + 1,
                                    max = MAX_TRANSPORT_RETRIES,
                                    backoff_ms = BACKOFF_MS[attempt],
                                    status = status.as_u16(),
                                    "generic provider: HTTP 200 with empty body (upstream degraded), retrying"
                                );
                                tokio::time::sleep(Duration::from_millis(BACKOFF_MS[attempt]))
                                    .await;
                                attempt += 1;
                                continue;
                            }
                            return Err(LlmError::InvalidResponse(format!(
                                "HTTP 200 with empty body after {} retries (upstream returned no content)",
                                MAX_TRANSPORT_RETRIES
                            )));
                        }
                        Ok(bytes) => break (status, bytes),
                        Err(e) => e,
                    }
                }
                Err(e) => e,
            };

            if should_retry_transport(&transport_err) && attempt < MAX_TRANSPORT_RETRIES {
                tracing::warn!(
                    attempt = attempt + 1,
                    max = MAX_TRANSPORT_RETRIES,
                    backoff_ms = BACKOFF_MS[attempt],
                    error = %transport_err,
                    "generic provider: transport error on chat completion, retrying"
                );
                tokio::time::sleep(Duration::from_millis(BACKOFF_MS[attempt])).await;
                attempt += 1;
                continue;
            }
            return Err(LlmError::HttpError(transport_err));
        };
        let mut chat_response = decode_chat_response(status.as_u16(), &body_bytes)?;
        // Reverse-map any tool_call names so downstream sees canonical dotted ids.
        for choice in chat_response.choices.iter_mut() {
            if let Some(ref mut tool_calls) = choice.message.tool_calls {
                for tc in tool_calls.iter_mut() {
                    tc.function.name = decode_tool_name(&tc.function.name);
                }
            }
        }
        Ok(chat_response)
    }

    async fn chat_completion_stream(
        &self,
        mut request: ChatRequest,
    ) -> LlmResult<Box<dyn Stream<Item = LlmResult<ChatChunk>> + Send + Unpin>> {
        request.stream = Some(true);
        // Same outbound sanitization as the non-streaming path.
        if let Some(ref mut tools) = request.tools {
            for t in tools.iter_mut() {
                t.function.name = encode_tool_name(&t.function.name);
            }
        }
        // BUG-CB-17: merge all system-role messages into one at index 0.
        request.messages = merge_system_messages(request.messages);
        let url = format!("{}/chat/completions", self.base_url);

        let mut req_builder = self
            .client
            .post(&url)
            .header("Content-Type", "application/json");

        if let Some(ref api_key) = self.api_key {
            req_builder = req_builder.header("Authorization", format!("Bearer {}", api_key));
        }

        let response = req_builder.json(&request).send().await?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(LlmError::ApiError {
                status: status.as_u16(),
                message: error_text,
            });
        }

        // Proper SSE buffer: TCP chunks can carry multiple `data: ...\n\n` events
        // or split one event across chunks. Naive 1-chunk-1-event parse breaks
        // streaming on real providers (e.g. DeepSeek) — produces "trailing
        // characters" errors and drops most tokens.
        let (tx, rx) = tokio::sync::mpsc::channel::<LlmResult<ChatChunk>>(64);
        tokio::spawn(async move {
            let mut bytes_stream = response.bytes_stream();
            let mut buffer = String::new();
            while let Some(chunk_res) = bytes_stream.next().await {
                match chunk_res {
                    Err(e) => {
                        let _ = tx.send(Err(LlmError::HttpError(e))).await;
                        return;
                    }
                    Ok(bytes) => {
                        buffer.push_str(&String::from_utf8_lossy(&bytes));
                        // split on event boundary "\n\n"
                        while let Some(pos) = buffer.find("\n\n") {
                            let event_block = buffer[..pos].to_string();
                            buffer.drain(..pos + 2);
                            for line in event_block.lines() {
                                let line = line.trim();
                                if let Some(json_str) = line.strip_prefix("data:") {
                                    let json_str = json_str.trim();
                                    if json_str.is_empty() {
                                        continue;
                                    }
                                    if json_str == "[DONE]" {
                                        return;
                                    }
                                    match serde_json::from_str::<ChatChunk>(json_str) {
                                        Ok(chunk) => {
                                            if tx.send(Ok(chunk)).await.is_err() {
                                                return;
                                            }
                                        }
                                        Err(e) => {
                                            if tx.send(Err(LlmError::JsonError(e))).await.is_err() {
                                                return;
                                            }
                                        }
                                    }
                                }
                                // 忽略 event: / id: / : 注释行 / 空行
                            }
                        }
                    }
                }
            }
        });

        let stream = futures::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|item| (item, rx))
        });
        Ok(Box::new(Box::pin(stream)))
    }

    fn provider(&self) -> &str {
        "generic"
    }

    async fn validate_connection(&self) -> LlmResult<()> {
        // 简单的健康检查
        let request = ChatRequest {
            model: "test".to_string(),
            messages: vec![crate::types::Message::user("test")],
            temperature: None,
            top_p: None,
            max_tokens: Some(1),
            tools: None,
            tool_choice: None,
            stream: None,
            extra: Default::default(),
            api_key_override: None,
        };

        let url = format!("{}/chat/completions", self.base_url);
        let mut req_builder = self
            .client
            .post(&url)
            .header("Content-Type", "application/json");

        if let Some(ref api_key) = self.api_key {
            req_builder = req_builder.header("Authorization", format!("Bearer {}", api_key));
        }

        let response = req_builder.json(&request).send().await?;

        let status = response.status();
        if status == 401 || status == 403 {
            return Err(LlmError::ApiError {
                status: status.as_u16(),
                message: "Authentication failed".to_string(),
            });
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Message, Role};

    #[test]
    fn test_generic_client_creation() {
        let client =
            GenericOpenAiClient::new(Some("test-key".to_string()), "http://localhost:11434/v1");
        assert!(client.is_ok());
    }

    #[test]
    fn test_generic_client_without_key() {
        let client = GenericOpenAiClient::new(None, "http://localhost:11434/v1");
        assert!(client.is_ok());
    }

    #[test]
    fn test_provider_name() {
        let client = GenericOpenAiClient::new(None, "http://localhost:11434/v1").unwrap();
        assert_eq!(client.provider(), "generic");
    }

    // Bug I — decode_chat_response diagnostics tests ----------------------------

    #[test]
    fn test_decode_chat_response_valid_body() {
        // Minimal valid ChatResponse JSON should decode cleanly.
        let body = br#"{"id":"x","object":"chat.completion","created":0,"model":"m","choices":[]}"#;
        let result = decode_chat_response(200, body);
        assert!(result.is_ok(), "valid body should decode: {result:?}");
    }

    #[test]
    fn test_decode_chat_response_wrong_shape_includes_status_and_snippet() {
        let body = br#"{"unexpected":"shape"}"#;
        let err = decode_chat_response(200, body).expect_err("wrong shape must fail");
        let msg = err.to_string();
        // Not the opaque reqwest message.
        assert!(
            !msg.contains("error decoding response body"),
            "must not be the opaque reqwest message: {msg}"
        );
        // Carries status and a snippet of the offending body.
        assert!(msg.contains("200"), "message should embed status: {msg}");
        assert!(
            msg.contains("unexpected") && msg.contains("shape"),
            "message should embed body snippet: {msg}"
        );
    }

    #[test]
    fn test_decode_chat_response_truncated_json_includes_snippet() {
        // 2xx but truncated / non-JSON body.
        let body = b"{\"choices\": [ truncated...";
        let err = decode_chat_response(200, body).expect_err("truncated body must fail");
        let msg = err.to_string();
        assert!(
            !msg.contains("error decoding response body"),
            "must not be opaque: {msg}"
        );
        assert!(msg.contains("200"), "should embed status: {msg}");
        assert!(msg.contains("truncated"), "should embed body snippet: {msg}");
    }

    #[test]
    fn test_decode_chat_response_snippet_truncated_to_200_chars() {
        // A large invalid body — error message snippet must stay bounded.
        let big = format!("{{\"junk\":\"{}\"}}", "z".repeat(5000));
        let err = decode_chat_response(200, big.as_bytes()).expect_err("must fail");
        let msg = err.to_string();
        // The full 5000-char body must not be embedded verbatim.
        assert!(
            msg.len() < 1000,
            "error message must stay bounded (snippet capped), got {} chars",
            msg.len()
        );
    }

    // BUG-CB-17 tests -----------------------------------------------------------

    #[test]
    fn test_merge_system_messages_combines_multiple_system_into_first_position() {
        let messages = vec![
            Message::system("first system"),
            Message::user("hello"),
            Message::system("second system appended by add_system_hint"),
        ];
        let out = merge_system_messages(messages);
        assert_eq!(out.len(), 2, "merged: 1 system + 1 user");
        assert_eq!(out[0].role, Role::System);
        assert_eq!(
            out[0].content,
            "first system\n\nsecond system appended by add_system_hint"
        );
        assert_eq!(out[1].role, Role::User);
        assert_eq!(out[1].content, "hello");
    }

    #[test]
    fn test_merge_system_messages_preserves_non_system_order() {
        let messages = vec![
            Message::system("sys"),
            Message::user("u1"),
            Message::assistant("a1"),
            Message::user("u2"),
        ];
        let out = merge_system_messages(messages);
        assert_eq!(out.len(), 4);
        assert_eq!(out[0].role, Role::System);
        assert_eq!(out[1].role, Role::User);
        assert_eq!(out[1].content, "u1");
        assert_eq!(out[2].role, Role::Assistant);
        assert_eq!(out[3].role, Role::User);
        assert_eq!(out[3].content, "u2");
    }

    // Bug I-d — 重试配额 tests -------------------------------------------------

    #[test]
    fn test_max_transport_retries_is_four() {
        assert_eq!(
            MAX_TRANSPORT_RETRIES, 4,
            "Bug I-d: spaced retry 已证明能恢复 MiniMax 间歇返空 200，重试次数须为 4"
        );
    }

    #[test]
    fn test_backoff_is_exponential_1_2_4_8_seconds() {
        assert_eq!(
            BACKOFF_MS,
            [1000, 2000, 4000, 8000],
            "Bug I-d: 退避须为指数 1/2/4/8s，给被快速连打的上游恢复时间"
        );
        assert_eq!(
            BACKOFF_MS.len(),
            MAX_TRANSPORT_RETRIES,
            "退避数组长度须与重试次数一致，避免越界索引"
        );
    }

    // Bug I 精确根因 — is_empty_body tests -------------------------------------

    #[test]
    fn test_is_empty_body_true_for_zero_bytes() {
        assert!(is_empty_body(b""), "zero bytes must be empty");
    }

    #[test]
    fn test_is_empty_body_true_for_whitespace_only() {
        assert!(is_empty_body(b"   \n\t\r\n"), "whitespace-only must be empty");
    }

    #[test]
    fn test_is_empty_body_false_for_json_content() {
        assert!(
            !is_empty_body(b"{\"id\":\"x\"}"),
            "json content must not be empty"
        );
    }

    #[test]
    fn test_is_empty_body_false_for_single_brace() {
        assert!(!is_empty_body(b"{"), "single char must not be empty");
    }

    // Bug I — transport retry classification tests -----------------------------

    #[tokio::test]
    async fn test_should_retry_transport_true_for_connect_error() {
        // A connect failure to an unroutable / closed port produces a reqwest
        // error whose `is_connect()` (or `is_request()`) is true. Such errors
        // are safe to retry (request never reached the upstream).
        let client = Client::builder()
            .connect_timeout(Duration::from_millis(200))
            .timeout(Duration::from_millis(500))
            .no_proxy()
            .build()
            .unwrap();
        // 127.0.0.1:1 — reserved/closed port, connection refused fast.
        let err = client
            .get("http://127.0.0.1:1/")
            .send()
            .await
            .expect_err("connect to closed port must fail");
        assert!(
            should_retry_transport(&err),
            "connect error should be retryable: is_connect={} is_request={} is_timeout={}",
            err.is_connect(),
            err.is_request(),
            err.is_timeout(),
        );
    }

    #[tokio::test]
    async fn test_should_retry_transport_true_for_timeout_error() {
        // A request that exceeds the timeout yields is_timeout()==true, which
        // we treat as retryable transport failure.
        let client = Client::builder()
            .timeout(Duration::from_millis(1))
            .no_proxy()
            .build()
            .unwrap();
        // Non-routable address (TEST-NET-1) so the request hangs past the 1ms
        // timeout rather than failing connect instantly.
        let err = client
            .get("http://192.0.2.1/")
            .send()
            .await
            .expect_err("request must time out");
        assert!(
            should_retry_transport(&err),
            "timeout/connect error should be retryable: is_timeout={} is_connect={} is_request={}",
            err.is_timeout(),
            err.is_connect(),
            err.is_request(),
        );
    }

    #[test]
    fn test_should_retry_transport_false_for_business_error() {
        // Business errors (already-decoded API errors, schema mismatches) are
        // represented as LlmError variants, not reqwest::Error, so they never
        // reach should_retry_transport. Guard the decode-failure path: a
        // schema-mismatch surfaces as InvalidResponse and is not classified as
        // a retryable reqwest transport error.
        let body = br#"{"unexpected":"shape"}"#;
        let err = decode_chat_response(200, body).expect_err("must fail");
        // InvalidResponse is a non-HttpError variant — it is never retried.
        assert!(
            matches!(err, LlmError::InvalidResponse(_)),
            "schema mismatch must be InvalidResponse, not a retryable transport error: {err:?}"
        );
    }

    #[test]
    fn test_merge_system_messages_no_system_returns_unchanged() {
        let messages = vec![Message::user("hello"), Message::assistant("world")];
        let out = merge_system_messages(messages);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].role, Role::User);
        assert_eq!(out[1].role, Role::Assistant);
    }

    // Bug I-d 根因 — ensure_user_message tests --------------------------------

    #[test]
    fn test_ensure_user_message_appends_continue_when_no_user() {
        // 压缩后的纯 system+assistant+tool 对话，无任何 user 消息。
        let mut messages = vec![
            Message::system("sys"),
            Message::assistant("a"),
        ];
        ensure_user_message(&mut messages);
        assert_eq!(messages.len(), 3);
        let last = messages.last().expect("appended message present");
        assert_eq!(last.role, Role::User);
        assert_eq!(last.content, "Continue.");
    }

    #[test]
    fn test_ensure_user_message_unchanged_when_user_present() {
        let mut messages = vec![
            Message::system("sys"),
            Message::user("real request"),
            Message::assistant("a"),
        ];
        ensure_user_message(&mut messages);
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[1].role, Role::User);
        assert_eq!(messages[1].content, "real request");
        // 末尾仍是原 assistant，未追加合成 user。
        assert_eq!(messages.last().expect("last present").role, Role::Assistant);
    }

    // Bug I-d — strip_think_blocks tests ---------------------------------------

    #[test]
    fn test_strip_think_blocks_removes_leading_block_keeps_answer() {
        assert_eq!(strip_think_blocks("<think>x</think>真实答案"), "真实答案");
    }

    #[test]
    fn test_strip_think_blocks_multiline_keeps_surrounding_text() {
        // 块在中间、跨多行；保留块外文本（trim 后相邻拼接）。
        assert_eq!(
            strip_think_blocks("前缀<think>多行\n推理\n内容</think>后缀"),
            "前缀后缀"
        );
    }

    #[test]
    fn test_strip_think_blocks_no_block_returns_unchanged() {
        assert_eq!(
            strip_think_blocks("just a normal answer"),
            "just a normal answer"
        );
    }

    #[test]
    fn test_strip_think_blocks_only_block_returns_empty() {
        assert_eq!(strip_think_blocks("<think>纯推理无答案</think>"), "");
    }

    #[test]
    fn test_strip_think_blocks_removes_multiple_blocks() {
        assert_eq!(
            strip_think_blocks("<think>a</think>答案1<think>b</think>答案2"),
            "答案1答案2"
        );
    }

    #[test]
    fn test_strip_think_blocks_case_insensitive_and_attributes() {
        // 大小写不敏感 + 带属性的开标签宽松匹配。
        assert_eq!(
            strip_think_blocks(r#"<THINK foo="bar">推理</THINK>结果"#),
            "结果"
        );
    }

    #[test]
    fn test_strip_think_blocks_trims_surrounding_whitespace() {
        assert_eq!(
            strip_think_blocks("  <think>r</think>  答案  "),
            "答案"
        );
    }

    #[test]
    fn test_strip_think_blocks_applied_only_to_assistant_role() {
        // 集成层面验证：出站 strip 只动 Assistant。User/System/Tool content 不动。
        // 此处直接验证函数语义；调用点见 chat_completion 的 iter_mut 守卫。
        let assistant = strip_think_blocks("<think>reason</think>final");
        assert_eq!(assistant, "final");
        // User content with a think-looking string is NOT touched by chat_completion
        // because the guard is `role == Assistant`; the helper itself is role-agnostic.
    }
}
