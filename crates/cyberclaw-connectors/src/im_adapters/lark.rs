//! # 飞书/Lark Bot 适配器
//!
//! 飞书/Lark IM 平台适配器，支持：
//! - 双域名切换（feishu.cn / larksuite.com）
//! - WebSocket 长连接 + Webhook 双传输模式
//! - 完整消息类型：文字、图片、文件、富文本、音频、视频、卡片交互
//! - AES-256-CBC 加密消息解密
//! - 真实 HTTP API 调用（tenant_access_token、发送、下载、回复、更新卡片、表情回应）
//!
//! ## 域名映射
//! - 飞书 (Feishu): `https://open.feishu.cn`
//! - Lark (国际站): `https://open.larksuite.com`
//!
//! ## 传输模式
//! - **Webhook**: 需要公网 IP，飞书向你的服务器推送事件
//! - **WebSocket**: 无需公网 IP，主动连接飞书维持长连接，适合开发和内网部署

use crate::im_channel::{AudioFormat, ImMessage, ImMessageType, ImPlatformAdapter};
use aes::cipher::{BlockDecrypt, KeyInit};
use aes::Aes256;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const FEISHU_BASE_URL: &str = "https://open.feishu.cn";
const LARK_BASE_URL: &str = "https://open.larksuite.com";

// API paths
const PATH_TENANT_TOKEN: &str = "/open-apis/auth/v3/tenant_access_token/internal/";
const PATH_SEND_MESSAGE: &str = "/open-apis/im/v1/messages";
const PATH_UPLOAD_FILE: &str = "/open-apis/im/v1/files";
const PATH_WS_ENDPOINT: &str = "/callback/ws/endpoint";

// ---------------------------------------------------------------------------
// Domain & Transport Configuration
// ---------------------------------------------------------------------------

/// 飞书/Lark 域名选择
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub enum LarkDomain {
    /// 飞书中国站：open.feishu.cn
    #[default]
    Feishu,
    /// Lark 国际站：open.larksuite.com
    Lark,
}

impl LarkDomain {
    /// 获取 API 基础 URL
    pub fn base_url(&self) -> &str {
        match self {
            LarkDomain::Feishu => FEISHU_BASE_URL,
            LarkDomain::Lark => LARK_BASE_URL,
        }
    }

    /// 获取 WebSocket 基础 URL（wss 协议）
    pub fn ws_base_url(&self) -> &str {
        match self {
            LarkDomain::Feishu => "wss://open.feishu.cn",
            LarkDomain::Lark => "wss://open.larksuite.com",
        }
    }
}

/// 传输模式
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub enum LarkTransportMode {
    /// Webhook 回调（需要公网 IP）
    #[default]
    Webhook,
    /// WebSocket 长连接（无需公网 IP，参考 deer-flow/hermes-agent 实现）
    WebSocket,
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// 飞书/Lark 适配器完整配置
#[derive(Debug, Clone)]
pub struct LarkConfig {
    /// App ID
    pub app_id: String,
    /// App Secret
    pub app_secret: String,
    /// Verification Token（webhook 签名校验）
    pub verification_token: String,
    /// Encrypt Key（消息解密，可选）
    pub encrypt_key: Option<String>,
    /// 域名选择（飞书/Lark）
    pub domain: LarkDomain,
    /// 传输模式（Webhook/WebSocket）
    pub transport: LarkTransportMode,
}

// ---------------------------------------------------------------------------
// Internal Types
// ---------------------------------------------------------------------------

/// 缓存的 access token
struct CachedToken {
    token: String,
    expires_at: chrono::DateTime<chrono::Utc>,
}

/// Lark API 标准响应
#[derive(Debug, Deserialize)]
struct LarkApiResponse<T = serde_json::Value> {
    code: i64,
    #[allow(dead_code)]
    msg: Option<String>,
    data: Option<T>,
}

/// Token 响应
#[derive(Debug, Deserialize)]
struct TokenResponse {
    code: i64,
    tenant_access_token: Option<String>,
    expire: Option<i64>,
}

/// 消息发送响应数据
#[derive(Debug, Deserialize)]
struct SendMessageData {
    #[allow(dead_code)]
    message_id: Option<String>,
}

/// WebSocket 端点响应
#[derive(Debug, Deserialize)]
struct WsEndpointData {
    url: String,
}

// ---------------------------------------------------------------------------
// LarkAdapter
// ---------------------------------------------------------------------------

/// 飞书/Lark Bot 适配器
///
/// 支持飞书中国站和 Lark 国际站，通过 `LarkDomain` 切换域名。
/// 支持 Webhook 和 WebSocket 两种传输模式。
pub struct LarkAdapter {
    /// 适配器配置
    config: LarkConfig,
    /// HTTP 客户端（复用连接池）
    client: reqwest::Client,
    /// Access token 缓存（带过期时间）
    token_cache: Arc<RwLock<Option<CachedToken>>>,
    /// WebSocket 连接任务句柄
    ws_handle: Arc<RwLock<Option<tokio::task::JoinHandle<()>>>>,
    /// WebSocket 停止信号
    ws_stop: Arc<tokio::sync::Notify>,
}

impl LarkAdapter {
    /// 创建新的飞书/Lark 适配器
    pub fn new(config: LarkConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
            token_cache: Arc::new(RwLock::new(None)),
            ws_handle: Arc::new(RwLock::new(None)),
            ws_stop: Arc::new(tokio::sync::Notify::new()),
        }
    }

    /// 兼容旧 API 的构造函数
    pub fn with_credentials(
        app_id: String,
        app_secret: String,
        verification_token: String,
        encrypt_key: Option<String>,
    ) -> Self {
        Self::new(LarkConfig {
            app_id,
            app_secret,
            verification_token,
            encrypt_key,
            domain: LarkDomain::default(),
            transport: LarkTransportMode::default(),
        })
    }

    // -----------------------------------------------------------------------
    // Private: URL helpers
    // -----------------------------------------------------------------------

    /// 构建完整 API URL
    fn api_url(&self, path: &str) -> String {
        format!("{}{}", self.config.domain.base_url(), path)
    }

    // -----------------------------------------------------------------------
    // Private: Token management
    // -----------------------------------------------------------------------

    /// 获取或刷新 tenant_access_token
    ///
    /// 调用飞书/Lark API 获取 token，带本地缓存和过期管理。
    /// 提前 5 分钟刷新以避免使用即将过期的 token。
    async fn ensure_token(&self) -> anyhow::Result<String> {
        // 检查缓存
        {
            let cache = self.token_cache.read().await;
            if let Some(ref cached) = *cache {
                // 提前 5 分钟刷新
                let buffer = chrono::Duration::minutes(5);
                if cached.expires_at - buffer > chrono::Utc::now() {
                    return Ok(cached.token.clone());
                }
            }
        }

        // 请求新 token
        let url = self.api_url(PATH_TENANT_TOKEN);
        let resp = self
            .client
            .post(&url)
            .json(&serde_json::json!({
                "app_id": self.config.app_id,
                "app_secret": self.config.app_secret,
            }))
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("token request failed: {}", e))?;

        let status = resp.status();
        let body: TokenResponse = resp
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("token response parse failed: {}", e))?;

        if body.code != 0 || body.tenant_access_token.is_none() {
            anyhow::bail!(
                "token API error: status={}, code={}, token=None",
                status,
                body.code
            );
        }

        let token = body.tenant_access_token.unwrap();
        let expire_secs = body.expire.unwrap_or(7200);
        let expires_at = chrono::Utc::now() + chrono::Duration::seconds(expire_secs);

        // 更新缓存
        let mut cache = self.token_cache.write().await;
        *cache = Some(CachedToken {
            token: token.clone(),
            expires_at,
        });

        Ok(token)
    }

    // -----------------------------------------------------------------------
    // Private: AES-256-CBC 消息解密
    // -----------------------------------------------------------------------

    /// 解密飞书加密事件
    ///
    /// 算法：
    /// 1. key = SHA-256(encrypt_key)
    /// 2. data = base64_decode(encrypted)
    /// 3. iv = data[0..16]
    /// 4. ciphertext = data[16..]
    /// 5. plaintext = AES-256-CBC-decrypt(key, iv, ciphertext)
    /// 6. PKCS7 去填充
    fn decrypt_event(&self, encrypted: &str) -> anyhow::Result<String> {
        let key_str = self
            .config
            .encrypt_key
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("encrypt_key not configured"))?;

        // key = SHA-256(encrypt_key)
        let key_hash = Sha256::digest(key_str.as_bytes());

        // base64 decode
        let data = base64::engine::general_purpose::STANDARD
            .decode(encrypted)
            .map_err(|e| anyhow::anyhow!("base64 decode failed: {}", e))?;

        if data.len() < 16 {
            anyhow::bail!("encrypted data too short (< 16 bytes)");
        }
        if (data.len() - 16) % 16 != 0 {
            anyhow::bail!("ciphertext length not aligned to 16-byte blocks");
        }

        let iv = &data[..16];
        let ciphertext = &data[16..];

        // AES-256-CBC decrypt (手动实现 CBC 模式，避免额外依赖 cbc crate 的 padding 模块)
        let cipher = Aes256::new(aes::cipher::generic_array::GenericArray::from_slice(
            &key_hash,
        ));
        let mut plaintext = Vec::with_capacity(ciphertext.len());
        let mut prev_block = [0u8; 16];
        prev_block.copy_from_slice(iv);

        for chunk in ciphertext.chunks(16) {
            let mut block = aes::cipher::generic_array::GenericArray::clone_from_slice(chunk);
            cipher.decrypt_block(&mut block);
            // CBC: XOR with previous ciphertext block
            for i in 0..16 {
                plaintext.push(block[i] ^ prev_block[i]);
            }
            prev_block.copy_from_slice(chunk);
        }

        // PKCS7 unpadding
        if let Some(&pad_len) = plaintext.last() {
            let pad_len = pad_len as usize;
            if pad_len > 0
                && pad_len <= 16
                && plaintext.len() >= pad_len
                && plaintext[plaintext.len() - pad_len..]
                    .iter()
                    .all(|&b| b == pad_len as u8)
            {
                plaintext.truncate(plaintext.len() - pad_len);
            }
        }

        String::from_utf8(plaintext)
            .map_err(|e| anyhow::anyhow!("decrypted data is not UTF-8: {}", e))
    }

    /// 尝试解密 webhook 载荷（如果有 encrypt 字段且配置了 encrypt_key）
    fn maybe_decrypt_payload(&self, raw: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        if let Some(encrypted) = raw.get("encrypt").and_then(|v| v.as_str()) {
            if self.config.encrypt_key.is_some() {
                let decrypted_str = self.decrypt_event(encrypted)?;
                let decrypted: serde_json::Value = serde_json::from_str(&decrypted_str)?;
                return Ok(decrypted);
            }
        }
        Ok(raw.clone())
    }

    // -----------------------------------------------------------------------
    // Private: Message parsing helpers
    // -----------------------------------------------------------------------

    /// 解析消息内容 JSON 字符串
    fn parse_content_json(content_str: &str) -> serde_json::Value {
        serde_json::from_str(content_str).unwrap_or_default()
    }

    /// 从事件载荷中提取标准化消息
    fn extract_message_fields(
        event: &serde_json::Value,
    ) -> (String, String, String, String, serde_json::Value) {
        let message = event.get("message").unwrap_or(event);

        let message_id = message
            .get("message_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let chat_id = message
            .get("chat_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let sender_id = event
            .get("sender")
            .and_then(|s| s.get("sender_id"))
            .and_then(|s| s.get("open_id"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let msg_type_str = message
            .get("message_type")
            .and_then(|v| v.as_str())
            .unwrap_or("text")
            .to_string();

        let content_str = message
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("{}");
        let content = Self::parse_content_json(content_str);

        (message_id, chat_id, sender_id, msg_type_str, content)
    }

    // -----------------------------------------------------------------------
    // Public: Extended API (beyond ImPlatformAdapter trait)
    // -----------------------------------------------------------------------

    /// 回复指定消息（在消息线程中回复）
    pub async fn reply_text(&self, message_id: &str, text: &str) -> anyhow::Result<String> {
        let token = self.ensure_token().await?;
        let url = format!("{}/{}/reply", self.api_url(PATH_SEND_MESSAGE), message_id);
        let content = serde_json::json!({"text": text}).to_string();

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&token)
            .json(&serde_json::json!({
                "msg_type": "text",
                "content": content,
            }))
            .send()
            .await?;

        let body: LarkApiResponse<SendMessageData> = resp.json().await?;
        if body.code != 0 {
            anyhow::bail!("reply failed: code={}, msg={:?}", body.code, body.msg);
        }

        Ok(body.data.and_then(|d| d.message_id).unwrap_or_default())
    }

    /// 更新已发送的卡片消息（running card 模式，参考 deer-flow 实现）
    pub async fn update_card(
        &self,
        message_id: &str,
        card: serde_json::Value,
    ) -> anyhow::Result<()> {
        let token = self.ensure_token().await?;
        let url = format!("{}/{}", self.api_url(PATH_SEND_MESSAGE), message_id);
        let content = serde_json::to_string(&card)?;

        let resp = self
            .client
            .patch(&url)
            .bearer_auth(&token)
            .json(&serde_json::json!({
                "content": content,
            }))
            .send()
            .await?;

        let body: LarkApiResponse = resp.json().await?;
        if body.code != 0 {
            anyhow::bail!("update card failed: code={}, msg={:?}", body.code, body.msg);
        }
        Ok(())
    }

    /// 添加表情回应（emoji reaction）
    pub async fn add_reaction(&self, message_id: &str, emoji_type: &str) -> anyhow::Result<()> {
        let token = self.ensure_token().await?;
        let url = format!(
            "{}/{}/reactions",
            self.api_url(PATH_SEND_MESSAGE),
            message_id
        );

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&token)
            .json(&serde_json::json!({
                "reaction_type": {
                    "emoji_type": emoji_type,
                }
            }))
            .send()
            .await?;

        let body: LarkApiResponse = resp.json().await?;
        if body.code != 0 {
            anyhow::bail!(
                "add reaction failed: code={}, msg={:?}",
                body.code,
                body.msg
            );
        }
        Ok(())
    }

    /// 上传图片，返回 image_key
    pub async fn upload_image(&self, data: &[u8], file_name: &str) -> anyhow::Result<String> {
        let token = self.ensure_token().await?;
        let url = self.api_url("/open-apis/im/v1/images");

        let part = reqwest::multipart::Part::bytes(data.to_vec())
            .file_name(file_name.to_string())
            .mime_str("application/octet-stream")?;

        let form = reqwest::multipart::Form::new()
            .text("image_type", "message")
            .part("image", part);

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&token)
            .multipart(form)
            .send()
            .await?;

        let body: LarkApiResponse = resp.json().await?;
        if body.code != 0 {
            anyhow::bail!(
                "upload image failed: code={}, msg={:?}",
                body.code,
                body.msg
            );
        }

        body.data
            .and_then(|d| {
                d.get("image_key")
                    .and_then(|v| v.as_str())
                    .map(String::from)
            })
            .ok_or_else(|| anyhow::anyhow!("upload image: missing image_key"))
    }

    /// 上传文件，返回 file_key
    pub async fn upload_file(
        &self,
        data: &[u8],
        file_name: &str,
        file_type: &str,
    ) -> anyhow::Result<String> {
        let token = self.ensure_token().await?;
        let url = self.api_url(PATH_UPLOAD_FILE);

        let part = reqwest::multipart::Part::bytes(data.to_vec())
            .file_name(file_name.to_string())
            .mime_str("application/octet-stream")?;

        let form = reqwest::multipart::Form::new()
            .text("file_type", file_type.to_string())
            .text("file_name", file_name.to_string())
            .part("file", part);

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&token)
            .multipart(form)
            .send()
            .await?;

        let body: LarkApiResponse = resp.json().await?;
        if body.code != 0 {
            anyhow::bail!("upload file failed: code={}, msg={:?}", body.code, body.msg);
        }

        body.data
            .and_then(|d| d.get("file_key").and_then(|v| v.as_str()).map(String::from))
            .ok_or_else(|| anyhow::anyhow!("upload file: missing file_key"))
    }

    // -----------------------------------------------------------------------
    // Public: WebSocket long connection (参考 deer-flow FeishuChannel 实现)
    // -----------------------------------------------------------------------

    /// 启动 WebSocket 长连接，返回事件接收器
    ///
    /// 无需公网 IP。连接到飞书/Lark WebSocket 端点，接收实时事件推送。
    /// 内置自动重连和心跳保活。
    ///
    /// # 使用方式
    /// ```ignore
    /// let mut rx = adapter.start_ws().await?;
    /// while let Some(event) = rx.recv().await {
    ///     let msg = adapter.normalize_inbound(event).await?;
    ///     // 处理消息...
    /// }
    /// ```
    pub async fn start_ws(&self) -> anyhow::Result<tokio::sync::mpsc::Receiver<serde_json::Value>> {
        // 获取 WebSocket 连接 URL
        let token = self.ensure_token().await?;
        let ws_api_url = self.api_url(PATH_WS_ENDPOINT);

        let resp = self
            .client
            .post(&ws_api_url)
            .bearer_auth(&token)
            .send()
            .await?;

        let body: LarkApiResponse<WsEndpointData> = resp.json().await?;
        if body.code != 0 {
            anyhow::bail!(
                "WS endpoint request failed: code={}, msg={:?}",
                body.code,
                body.msg
            );
        }

        let ws_url = body
            .data
            .map(|d| d.url)
            .ok_or_else(|| anyhow::anyhow!("WS endpoint: missing url"))?;

        let (tx, rx) = tokio::sync::mpsc::channel(256);
        let stop = self.ws_stop.clone();

        let handle = tokio::spawn(Self::ws_event_loop(ws_url, tx, stop));

        let mut ws_handle = self.ws_handle.write().await;
        *ws_handle = Some(handle);

        Ok(rx)
    }

    /// 停止 WebSocket 连接
    pub async fn stop_ws(&self) {
        self.ws_stop.notify_one();
        let mut handle = self.ws_handle.write().await;
        if let Some(h) = handle.take() {
            h.abort();
        }
    }

    /// WebSocket 事件循环（内部）
    ///
    /// 参考 deer-flow 的 `_run_ws()` 实现：在独立任务中运行，
    /// 处理心跳 ping/pong 和事件分发。
    async fn ws_event_loop(
        url: String,
        tx: tokio::sync::mpsc::Sender<serde_json::Value>,
        stop: Arc<tokio::sync::Notify>,
    ) {
        use futures::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message as WsMessage;

        loop {
            // 连接 WebSocket
            let ws_stream = match tokio_tungstenite::connect_async(&url).await {
                Ok((stream, _)) => stream,
                Err(e) => {
                    tracing::error!("Lark WS connect failed: {}, retrying in 5s", e);
                    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                    continue;
                }
            };

            tracing::info!("Lark WebSocket connected");
            let (mut write, mut read) = ws_stream.split();

            loop {
                tokio::select! {
                    // 停止信号
                    _ = stop.notified() => {
                        tracing::info!("Lark WS stop signal received");
                        let _ = write.send(WsMessage::Close(None)).await;
                        return;
                    }

                    // 接收消息
                    msg = read.next() => {
                        match msg {
                            Some(Ok(WsMessage::Text(text))) => {
                                // 尝试解析为 JSON
                                match serde_json::from_str::<serde_json::Value>(&text) {
                                    Ok(value) => {
                                        // 处理心跳 ping
                                        if value.get("type").and_then(|v| v.as_str()) == Some("ping") {
                                            let pong = serde_json::json!({"type": "pong"});
                                            let _ = write.send(WsMessage::Text(pong.to_string())).await;
                                            continue;
                                        }

                                        // 转发事件
                                        if tx.send(value).await.is_err() {
                                            tracing::warn!("Lark WS event receiver dropped");
                                            return;
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!("Lark WS non-JSON message: {}", e);
                                    }
                                }
                            }
                            Some(Ok(WsMessage::Ping(data))) => {
                                let _ = write.send(WsMessage::Pong(data)).await;
                            }
                            Some(Ok(WsMessage::Close(_))) => {
                                tracing::info!("Lark WS server closed connection");
                                break; // 跳出内层循环，触发重连
                            }
                            Some(Err(e)) => {
                                tracing::error!("Lark WS read error: {}", e);
                                break; // 重连
                            }
                            None => {
                                tracing::info!("Lark WS stream ended");
                                break; // 重连
                            }
                            _ => {} // Binary, Frame 等忽略
                        }
                    }
                }
            }

            // 自动重连前等待
            tracing::info!("Lark WS reconnecting in 3s...");
            tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
        }
    }
}

// ---------------------------------------------------------------------------
// ImPlatformAdapter 实现
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
impl ImPlatformAdapter for LarkAdapter {
    fn platform_id(&self) -> &str {
        match self.config.domain {
            LarkDomain::Feishu => "feishu",
            LarkDomain::Lark => "lark",
        }
    }

    fn supports_voice(&self) -> bool {
        true
    }

    async fn normalize_inbound(&self, raw: serde_json::Value) -> anyhow::Result<ImMessage> {
        // 如果有加密，先解密
        let payload = self.maybe_decrypt_payload(&raw)?;

        // 飞书事件结构：
        // header.event_type = "im.message.receive_v1"
        // event.message.message_type = "text" | "audio" | "image" | "file" | "post" | ...
        // event.message.content = JSON string
        let event = payload.get("event").unwrap_or(&payload);
        let (message_id, chat_id, sender_id, msg_type_str, content) =
            Self::extract_message_fields(event);

        let (message_type, text_content, voice_media_id) = match msg_type_str.as_str() {
            "text" => {
                let text = content
                    .get("text")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let is_command = text.as_ref().map(|t| t.starts_with('/')).unwrap_or(false);
                let msg_type = if is_command {
                    ImMessageType::Command
                } else {
                    ImMessageType::Text
                };
                (msg_type, text, None)
            }
            "audio" => {
                // 语音消息：voice_media_id = "message_id:file_key" 以支持下载
                let file_key = content
                    .get("file_key")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let media_id = file_key.map(|fk| format!("{}:{}", message_id, fk));
                (ImMessageType::Voice, None, media_id)
            }
            "image" => {
                // 图片消息：存储 image_key 到 metadata
                let image_key = content
                    .get("image_key")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let text = Some(format!("[image:{}]", image_key));
                (ImMessageType::Text, text, None)
            }
            "file" => {
                // 文件消息
                let file_key = content
                    .get("file_key")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let file_name = content
                    .get("file_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let text = Some(format!("[file:{}:{}]", file_name, file_key));
                (ImMessageType::Text, text, None)
            }
            "post" => {
                // 富文本消息：提取纯文本内容
                let text = Self::extract_post_text(&content);
                (ImMessageType::Text, Some(text), None)
            }
            "interactive" => {
                // 卡片交互回调
                (ImMessageType::Interactive, None, None)
            }
            "video" => {
                let file_key = content
                    .get("file_key")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let text = Some(format!("[video:{}]", file_key));
                (ImMessageType::Text, text, None)
            }
            "sticker" => {
                let file_key = content
                    .get("file_key")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let text = Some(format!("[sticker:{}]", file_key));
                (ImMessageType::Text, text, None)
            }
            _ => (ImMessageType::Text, None, None),
        };

        // 构建元数据
        let mut metadata = HashMap::new();
        metadata.insert("message_type_raw".to_string(), msg_type_str);
        if let Some(chat_type) = event
            .get("message")
            .and_then(|m| m.get("chat_type"))
            .and_then(|v| v.as_str())
        {
            metadata.insert("chat_type".to_string(), chat_type.to_string());
        }

        Ok(ImMessage {
            id: message_id,
            platform: self.platform_id().to_string(),
            chat_id,
            sender: sender_id,
            message_type,
            text_content,
            voice_media_id,
            voice_duration_secs: None,
            timestamp: chrono::Utc::now(),
            metadata,
            raw,
        })
    }

    async fn send_text(&self, chat_id: &str, text: &str) -> anyhow::Result<()> {
        let token = self.ensure_token().await?;
        let url = format!(
            "{}?receive_id_type=chat_id",
            self.api_url(PATH_SEND_MESSAGE)
        );
        let content = serde_json::json!({"text": text}).to_string();

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&token)
            .json(&serde_json::json!({
                "receive_id": chat_id,
                "msg_type": "text",
                "content": content,
            }))
            .send()
            .await?;

        let body: LarkApiResponse = resp.json().await?;
        if body.code != 0 {
            anyhow::bail!("send_text failed: code={}, msg={:?}", body.code, body.msg);
        }
        Ok(())
    }

    async fn send_card(&self, chat_id: &str, card: serde_json::Value) -> anyhow::Result<()> {
        let token = self.ensure_token().await?;
        let url = format!(
            "{}?receive_id_type=chat_id",
            self.api_url(PATH_SEND_MESSAGE)
        );
        let content = serde_json::to_string(&card)?;

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&token)
            .json(&serde_json::json!({
                "receive_id": chat_id,
                "msg_type": "interactive",
                "content": content,
            }))
            .send()
            .await?;

        let body: LarkApiResponse = resp.json().await?;
        if body.code != 0 {
            anyhow::bail!("send_card failed: code={}, msg={:?}", body.code, body.msg);
        }
        Ok(())
    }

    async fn send_voice(
        &self,
        chat_id: &str,
        audio: &[u8],
        _format: AudioFormat,
    ) -> anyhow::Result<()> {
        // 1. 先上传音频文件
        let file_key = self.upload_file(audio, "voice.opus", "opus").await?;

        // 2. 发送语音消息
        let token = self.ensure_token().await?;
        let url = format!(
            "{}?receive_id_type=chat_id",
            self.api_url(PATH_SEND_MESSAGE)
        );
        let content = serde_json::json!({"file_key": file_key}).to_string();

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&token)
            .json(&serde_json::json!({
                "receive_id": chat_id,
                "msg_type": "audio",
                "content": content,
            }))
            .send()
            .await?;

        let body: LarkApiResponse = resp.json().await?;
        if body.code != 0 {
            anyhow::bail!("send_voice failed: code={}, msg={:?}", body.code, body.msg);
        }
        Ok(())
    }

    async fn download_voice(&self, media_id: &str) -> anyhow::Result<Vec<u8>> {
        let token = self.ensure_token().await?;

        // media_id 格式: "message_id:file_key"
        let parts: Vec<&str> = media_id.splitn(2, ':').collect();
        if parts.len() != 2 {
            anyhow::bail!(
                "invalid voice media_id format (expected 'message_id:file_key'): {}",
                media_id
            );
        }
        let (message_id, file_key) = (parts[0], parts[1]);

        let url = format!(
            "{}/{}/resources/{}?type=file",
            self.api_url(PATH_SEND_MESSAGE),
            message_id,
            file_key
        );

        let resp = self.client.get(&url).bearer_auth(&token).send().await?;

        if !resp.status().is_success() {
            anyhow::bail!("download_voice failed: status={}", resp.status());
        }

        let bytes = resp.bytes().await?;
        Ok(bytes.to_vec())
    }

    fn validate_signature(&self, payload: &[u8], signature: &str) -> bool {
        // 飞书签名校验：SHA256(timestamp + nonce + encrypt_key + body)
        // signature 格式预期：timestamp:nonce:sha256hex
        // 对于简化场景：SHA256(encrypt_key_or_verification_token + body)
        let key = self
            .config
            .encrypt_key
            .as_deref()
            .unwrap_or(&self.config.verification_token);

        let mut hasher = Sha256::new();
        hasher.update(key.as_bytes());
        hasher.update(payload);
        let computed = hex::encode(hasher.finalize());

        // 常量时间比较（防止时序攻击）
        use subtle::ConstantTimeEq;
        computed.as_bytes().ct_eq(signature.as_bytes()).into()
    }
}

// ---------------------------------------------------------------------------
// 富文本解析辅助
// ---------------------------------------------------------------------------

impl LarkAdapter {
    /// 从飞书富文本（post）中提取纯文本
    ///
    /// 富文本结构：
    /// ```json
    /// {
    ///   "zh_cn": {
    ///     "title": "标题",
    ///     "content": [[{"tag":"text","text":"内容"}, {"tag":"at","user_id":"ou_xxx"}]]
    ///   }
    /// }
    /// ```
    fn extract_post_text(content: &serde_json::Value) -> String {
        let mut texts = Vec::new();

        // 尝试多语言 key：zh_cn, en_us, ja_jp 等
        let post = content
            .get("zh_cn")
            .or_else(|| content.get("en_us"))
            .or_else(|| content.get("ja_jp"))
            .unwrap_or(content);

        if let Some(title) = post.get("title").and_then(|v| v.as_str()) {
            if !title.is_empty() {
                texts.push(title.to_string());
            }
        }

        if let Some(paragraphs) = post.get("content").and_then(|v| v.as_array()) {
            for paragraph in paragraphs {
                if let Some(elements) = paragraph.as_array() {
                    for element in elements {
                        let tag = element.get("tag").and_then(|v| v.as_str()).unwrap_or("");
                        match tag {
                            "text" => {
                                if let Some(text) = element.get("text").and_then(|v| v.as_str()) {
                                    texts.push(text.to_string());
                                }
                            }
                            "a" => {
                                if let Some(text) = element.get("text").and_then(|v| v.as_str()) {
                                    texts.push(text.to_string());
                                }
                            }
                            "at" => {
                                // @mention
                                if let Some(name) =
                                    element.get("user_name").and_then(|v| v.as_str())
                                {
                                    texts.push(format!("@{}", name));
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        texts.join(" ")
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config() -> LarkConfig {
        LarkConfig {
            app_id: "app_id_test".to_string(),
            app_secret: "app_secret_test".to_string(),
            verification_token: "verify_token_test".to_string(),
            encrypt_key: Some("encrypt_key_test".to_string()),
            domain: LarkDomain::Feishu,
            transport: LarkTransportMode::Webhook,
        }
    }

    fn make_adapter() -> LarkAdapter {
        LarkAdapter::new(make_config())
    }

    fn make_lark_adapter() -> LarkAdapter {
        LarkAdapter::new(LarkConfig {
            domain: LarkDomain::Lark,
            ..make_config()
        })
    }

    // -- Domain tests --

    #[test]
    fn test_feishu_domain_url() {
        let domain = LarkDomain::Feishu;
        assert_eq!(domain.base_url(), "https://open.feishu.cn");
        assert_eq!(domain.ws_base_url(), "wss://open.feishu.cn");
    }

    #[test]
    fn test_lark_domain_url() {
        let domain = LarkDomain::Lark;
        assert_eq!(domain.base_url(), "https://open.larksuite.com");
        assert_eq!(domain.ws_base_url(), "wss://open.larksuite.com");
    }

    #[test]
    fn test_domain_default() {
        assert_eq!(LarkDomain::default(), LarkDomain::Feishu);
    }

    #[test]
    fn test_domain_serde() {
        let domain = LarkDomain::Lark;
        let json = serde_json::to_string(&domain).unwrap();
        let deserialized: LarkDomain = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, LarkDomain::Lark);
    }

    // -- Transport tests --

    #[test]
    fn test_transport_default() {
        assert_eq!(LarkTransportMode::default(), LarkTransportMode::Webhook);
    }

    #[test]
    fn test_transport_serde() {
        let mode = LarkTransportMode::WebSocket;
        let json = serde_json::to_string(&mode).unwrap();
        let deserialized: LarkTransportMode = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, LarkTransportMode::WebSocket);
    }

    // -- Platform ID tests --

    #[test]
    fn test_platform_id_feishu() {
        let adapter = make_adapter();
        assert_eq!(adapter.platform_id(), "feishu");
        assert!(adapter.supports_voice());
        assert!(!adapter.supports_realtime_audio());
    }

    #[test]
    fn test_platform_id_lark() {
        let adapter = make_lark_adapter();
        assert_eq!(adapter.platform_id(), "lark");
    }

    // -- API URL tests --

    #[test]
    fn test_api_url_feishu() {
        let adapter = make_adapter();
        assert_eq!(
            adapter.api_url(PATH_TENANT_TOKEN),
            "https://open.feishu.cn/open-apis/auth/v3/tenant_access_token/internal/"
        );
    }

    #[test]
    fn test_api_url_lark() {
        let adapter = make_lark_adapter();
        assert_eq!(
            adapter.api_url(PATH_SEND_MESSAGE),
            "https://open.larksuite.com/open-apis/im/v1/messages"
        );
    }

    // -- AES decryption tests --

    #[test]
    fn test_decrypt_event_round_trip() {
        let adapter = make_adapter();
        let plaintext = r#"{"event":{"message":{"text":"hello"}}}"#;

        // 手动加密以验证解密
        let key_hash = Sha256::digest(b"encrypt_key_test");
        let iv: [u8; 16] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];

        // PKCS7 padding
        let mut padded = plaintext.as_bytes().to_vec();
        let pad_len = 16 - (padded.len() % 16);
        padded.extend(std::iter::repeat_n(pad_len as u8, pad_len));

        // AES-256-CBC encrypt
        let cipher = Aes256::new(aes::cipher::generic_array::GenericArray::from_slice(
            &key_hash,
        ));
        let mut encrypted = Vec::new();
        let mut prev_block = iv;
        for chunk in padded.chunks(16) {
            let mut block = [0u8; 16];
            for i in 0..16 {
                block[i] = chunk[i] ^ prev_block[i];
            }
            let mut ga = aes::cipher::generic_array::GenericArray::clone_from_slice(&block);
            aes::cipher::BlockEncrypt::encrypt_block(&cipher, &mut ga);
            encrypted.extend_from_slice(&ga);
            prev_block.copy_from_slice(&ga);
        }

        // 组合 IV + ciphertext 并 base64 编码
        let mut combined = iv.to_vec();
        combined.extend_from_slice(&encrypted);
        let encoded = base64::engine::general_purpose::STANDARD.encode(&combined);

        // 解密验证
        let decrypted = adapter.decrypt_event(&encoded).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_decrypt_no_key() {
        let adapter = LarkAdapter::new(LarkConfig {
            encrypt_key: None,
            ..make_config()
        });
        let result = adapter.decrypt_event("some_data");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("encrypt_key not configured"));
    }

    #[test]
    fn test_decrypt_short_data() {
        let adapter = make_adapter();
        // base64 of less than 16 bytes
        let short = base64::engine::general_purpose::STANDARD.encode([1, 2, 3]);
        let result = adapter.decrypt_event(&short);
        assert!(result.is_err());
    }

    // -- maybe_decrypt_payload tests --

    #[test]
    fn test_maybe_decrypt_plaintext() {
        let adapter = make_adapter();
        let payload = serde_json::json!({"event": {"message": {"text": "hello"}}});
        let result = adapter.maybe_decrypt_payload(&payload).unwrap();
        assert_eq!(result, payload);
    }

    // -- Normalize inbound tests --

    #[tokio::test]
    async fn test_normalize_text_message() {
        let adapter = make_adapter();
        let raw = serde_json::json!({
            "event": {
                "message": {
                    "message_id": "msg-lark-001",
                    "chat_id": "oc_chat_001",
                    "message_type": "text",
                    "content": "{\"text\":\"hello world\"}"
                },
                "sender": {
                    "sender_id": {
                        "open_id": "ou_user_001"
                    }
                }
            }
        });
        let msg = adapter.normalize_inbound(raw).await.unwrap();
        assert_eq!(msg.id, "msg-lark-001");
        assert_eq!(msg.platform, "feishu");
        assert_eq!(msg.chat_id, "oc_chat_001");
        assert_eq!(msg.sender, "ou_user_001");
        assert_eq!(msg.message_type, ImMessageType::Text);
        assert_eq!(msg.text_content.as_deref(), Some("hello world"));
    }

    #[tokio::test]
    async fn test_normalize_command_message() {
        let adapter = make_adapter();
        let raw = serde_json::json!({
            "event": {
                "message": {
                    "message_id": "msg-lark-002",
                    "chat_id": "oc_chat_001",
                    "message_type": "text",
                    "content": "{\"text\":\"/status\"}"
                },
                "sender": {
                    "sender_id": { "open_id": "ou_user_001" }
                }
            }
        });
        let msg = adapter.normalize_inbound(raw).await.unwrap();
        assert_eq!(msg.message_type, ImMessageType::Command);
        assert_eq!(msg.text_content.as_deref(), Some("/status"));
    }

    #[tokio::test]
    async fn test_normalize_voice_message() {
        let adapter = make_adapter();
        let raw = serde_json::json!({
            "event": {
                "message": {
                    "message_id": "msg-lark-003",
                    "chat_id": "oc_chat_001",
                    "message_type": "audio",
                    "content": "{\"file_key\":\"file_xxx\"}"
                },
                "sender": {
                    "sender_id": { "open_id": "ou_user_001" }
                }
            }
        });
        let msg = adapter.normalize_inbound(raw).await.unwrap();
        assert_eq!(msg.message_type, ImMessageType::Voice);
        // media_id = "message_id:file_key"
        assert_eq!(msg.voice_media_id.as_deref(), Some("msg-lark-003:file_xxx"));
        assert!(msg.text_content.is_none());
    }

    #[tokio::test]
    async fn test_normalize_image_message() {
        let adapter = make_adapter();
        let raw = serde_json::json!({
            "event": {
                "message": {
                    "message_id": "msg-lark-004",
                    "chat_id": "oc_chat_001",
                    "message_type": "image",
                    "content": "{\"image_key\":\"img_v2_xxx\"}"
                },
                "sender": {
                    "sender_id": { "open_id": "ou_user_001" }
                }
            }
        });
        let msg = adapter.normalize_inbound(raw).await.unwrap();
        assert_eq!(msg.message_type, ImMessageType::Text);
        assert_eq!(msg.text_content.as_deref(), Some("[image:img_v2_xxx]"));
    }

    #[tokio::test]
    async fn test_normalize_file_message() {
        let adapter = make_adapter();
        let raw = serde_json::json!({
            "event": {
                "message": {
                    "message_id": "msg-lark-005",
                    "chat_id": "oc_chat_001",
                    "message_type": "file",
                    "content": "{\"file_key\":\"file_yyy\",\"file_name\":\"report.pdf\"}"
                },
                "sender": {
                    "sender_id": { "open_id": "ou_user_001" }
                }
            }
        });
        let msg = adapter.normalize_inbound(raw).await.unwrap();
        assert_eq!(msg.message_type, ImMessageType::Text);
        assert_eq!(
            msg.text_content.as_deref(),
            Some("[file:report.pdf:file_yyy]")
        );
    }

    #[tokio::test]
    async fn test_normalize_post_message() {
        let adapter = make_adapter();
        let raw = serde_json::json!({
            "event": {
                "message": {
                    "message_id": "msg-lark-006",
                    "chat_id": "oc_chat_001",
                    "message_type": "post",
                    "content": "{\"zh_cn\":{\"title\":\"周报\",\"content\":[[{\"tag\":\"text\",\"text\":\"本周完成了\"},{\"tag\":\"text\",\"text\":\"三个任务\"}]]}}"
                },
                "sender": {
                    "sender_id": { "open_id": "ou_user_001" }
                }
            }
        });
        let msg = adapter.normalize_inbound(raw).await.unwrap();
        assert_eq!(msg.message_type, ImMessageType::Text);
        let text = msg.text_content.unwrap();
        assert!(text.contains("周报"));
        assert!(text.contains("本周完成了"));
        assert!(text.contains("三个任务"));
    }

    #[tokio::test]
    async fn test_normalize_interactive_message() {
        let adapter = make_adapter();
        let raw = serde_json::json!({
            "event": {
                "message": {
                    "message_id": "msg-lark-007",
                    "chat_id": "oc_chat_001",
                    "message_type": "interactive",
                    "content": "{}"
                },
                "sender": {
                    "sender_id": { "open_id": "ou_user_001" }
                }
            }
        });
        let msg = adapter.normalize_inbound(raw).await.unwrap();
        assert_eq!(msg.message_type, ImMessageType::Interactive);
    }

    #[tokio::test]
    async fn test_normalize_video_message() {
        let adapter = make_adapter();
        let raw = serde_json::json!({
            "event": {
                "message": {
                    "message_id": "msg-lark-008",
                    "chat_id": "oc_chat_001",
                    "message_type": "video",
                    "content": "{\"file_key\":\"vid_xxx\"}"
                },
                "sender": {
                    "sender_id": { "open_id": "ou_user_001" }
                }
            }
        });
        let msg = adapter.normalize_inbound(raw).await.unwrap();
        assert_eq!(msg.text_content.as_deref(), Some("[video:vid_xxx]"));
    }

    #[tokio::test]
    async fn test_normalize_lark_domain() {
        let adapter = make_lark_adapter();
        let raw = serde_json::json!({
            "event": {
                "message": {
                    "message_id": "msg-lark-int",
                    "chat_id": "oc_chat_int",
                    "message_type": "text",
                    "content": "{\"text\":\"hello from lark\"}"
                },
                "sender": {
                    "sender_id": { "open_id": "ou_intl_001" }
                }
            }
        });
        let msg = adapter.normalize_inbound(raw).await.unwrap();
        assert_eq!(msg.platform, "lark");
        assert_eq!(msg.text_content.as_deref(), Some("hello from lark"));
    }

    // -- Signature validation tests --

    #[test]
    fn test_signature_validation() {
        let adapter = make_adapter();
        let payload = b"test payload data";

        // 计算预期签名
        let mut hasher = Sha256::new();
        hasher.update(b"encrypt_key_test");
        hasher.update(payload);
        let expected_sig = hex::encode(hasher.finalize());

        assert!(adapter.validate_signature(payload, &expected_sig));
        assert!(!adapter.validate_signature(payload, "invalid_signature_hex"));
    }

    #[test]
    fn test_signature_without_encrypt_key() {
        let adapter = LarkAdapter::new(LarkConfig {
            encrypt_key: None,
            ..make_config()
        });
        let payload = b"test payload";

        // 使用 verification_token 作为 key
        let mut hasher = Sha256::new();
        hasher.update(b"verify_token_test");
        hasher.update(payload);
        let sig = hex::encode(hasher.finalize());

        assert!(adapter.validate_signature(payload, &sig));
    }

    // -- Token cache tests --

    #[tokio::test]
    async fn test_token_cache_reuse() {
        let adapter = make_adapter();
        // 手动写入 token 缓存
        {
            let mut cache = adapter.token_cache.write().await;
            *cache = Some(CachedToken {
                token: "cached_token_123".to_string(),
                expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
            });
        }
        // 应该直接返回缓存 token，不发 HTTP
        let token = adapter.ensure_token().await.unwrap();
        assert_eq!(token, "cached_token_123");
    }

    #[tokio::test]
    async fn test_token_cache_expired() {
        let adapter = make_adapter();
        // 写入已过期的 token
        {
            let mut cache = adapter.token_cache.write().await;
            *cache = Some(CachedToken {
                token: "old_token".to_string(),
                expires_at: chrono::Utc::now() - chrono::Duration::hours(1),
            });
        }
        // 过期 token 应该触发刷新（会因无真实服务而失败）
        let result = adapter.ensure_token().await;
        assert!(result.is_err());
    }

    // -- with_credentials 兼容测试 --

    #[test]
    fn test_with_credentials_compat() {
        let adapter = LarkAdapter::with_credentials(
            "app1".to_string(),
            "secret1".to_string(),
            "verify1".to_string(),
            None,
        );
        assert_eq!(adapter.config.app_id, "app1");
        assert_eq!(adapter.config.domain, LarkDomain::Feishu);
        assert_eq!(adapter.config.transport, LarkTransportMode::Webhook);
    }

    // -- Extract post text tests --

    #[test]
    fn test_extract_post_text_with_title() {
        let content = serde_json::json!({
            "zh_cn": {
                "title": "测试标题",
                "content": [[
                    {"tag": "text", "text": "段落内容"},
                    {"tag": "a", "text": "链接文字"},
                    {"tag": "at", "user_name": "张三"}
                ]]
            }
        });
        let text = LarkAdapter::extract_post_text(&content);
        assert!(text.contains("测试标题"));
        assert!(text.contains("段落内容"));
        assert!(text.contains("链接文字"));
        assert!(text.contains("@张三"));
    }

    #[test]
    fn test_extract_post_text_en() {
        let content = serde_json::json!({
            "en_us": {
                "title": "Weekly Report",
                "content": [[
                    {"tag": "text", "text": "Completed 3 tasks"}
                ]]
            }
        });
        let text = LarkAdapter::extract_post_text(&content);
        assert!(text.contains("Weekly Report"));
        assert!(text.contains("Completed 3 tasks"));
    }

    // -- Metadata in normalized message --

    #[tokio::test]
    async fn test_metadata_includes_raw_type() {
        let adapter = make_adapter();
        let raw = serde_json::json!({
            "event": {
                "message": {
                    "message_id": "msg-meta-1",
                    "chat_id": "oc_chat_001",
                    "message_type": "sticker",
                    "content": "{\"file_key\":\"sticker_xxx\"}",
                    "chat_type": "group"
                },
                "sender": {
                    "sender_id": { "open_id": "ou_user_001" }
                }
            }
        });
        let msg = adapter.normalize_inbound(raw).await.unwrap();
        assert_eq!(msg.metadata.get("message_type_raw").unwrap(), "sticker");
        assert_eq!(msg.metadata.get("chat_type").unwrap(), "group");
    }
}
