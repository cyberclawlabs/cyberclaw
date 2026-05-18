// Telegram Bot API 适配器
// 实现 ImPlatformAdapter trait，通过 Telegram Bot API 接入即时通信渠道
// 支持长轮询接收消息、发送文本/语音/图片/文件等

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, Context};
use async_trait::async_trait;
use hmac::{Hmac, Mac};
use reqwest::multipart;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::Sha256;
use tokio::sync::{mpsc, Notify};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

use crate::im_channel::{AudioFormat, ImMessage, ImMessageType, ImPlatformAdapter};

// ─── 配置 ───────────────────────────────────────────────────────────────────

/// Telegram Bot 适配器配置
#[derive(Debug, Clone)]
pub struct TelegramConfig {
    /// Bot Token（格式：{bot_id}:{random_string}）
    pub bot_token: String,
    /// Telegram API 基础 URL，默认 "https://api.telegram.org"
    pub api_base_url: String,
    /// 长轮询超时（秒），默认 30
    pub poll_timeout_secs: u64,
    /// 每次 getUpdates 最大返回条数，默认 100
    pub poll_limit: u32,
}

impl TelegramConfig {
    /// 使用给定 token 创建配置，其余字段使用默认值
    pub fn new(bot_token: impl Into<String>) -> Self {
        Self {
            bot_token: bot_token.into(),
            api_base_url: "https://api.telegram.org".to_string(),
            poll_timeout_secs: 30,
            poll_limit: 100,
        }
    }

    /// 构造带 token 的 Bot API 前缀，例如 "https://api.telegram.org/bot<token>"
    pub fn bot_api_prefix(&self) -> String {
        format!("{}/bot{}", self.api_base_url, self.bot_token)
    }

    /// 构造文件下载基础 URL，例如 "https://api.telegram.org/file/bot<token>"
    pub fn file_api_prefix(&self) -> String {
        format!("{}/file/bot{}", self.api_base_url, self.bot_token)
    }
}

// ─── Telegram API 响应类型 ────────────────────────────────────────────────────

/// getUpdates 返回的单条 Update（保留用于类型化反序列化场景）
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct TgUpdate {
    update_id: i64,
    message: Option<TgMessage>,
}

/// Telegram 消息对象
#[derive(Debug, Deserialize)]
struct TgMessage {
    message_id: i64,
    from: Option<TgUser>,
    chat: TgChat,
    date: i64,
    text: Option<String>,
    voice: Option<TgVoice>,
    photo: Option<Vec<TgPhotoSize>>,
    document: Option<TgDocument>,
    video: Option<TgVideo>,
    sticker: Option<TgSticker>,
}

/// 发送者信息
#[derive(Debug, Deserialize)]
struct TgUser {
    id: i64,
    first_name: Option<String>,
    username: Option<String>,
}

/// 聊天信息
#[derive(Debug, Deserialize)]
struct TgChat {
    id: i64,
}

/// 语音消息
#[derive(Debug, Deserialize)]
struct TgVoice {
    file_id: String,
    duration: Option<f32>,
}

/// 图片尺寸信息（photo 数组中的一项）
#[derive(Debug, Deserialize)]
struct TgPhotoSize {
    file_id: String,
    #[allow(dead_code)]
    width: Option<u32>,
    #[allow(dead_code)]
    height: Option<u32>,
}

/// 文件文档
#[derive(Debug, Deserialize)]
struct TgDocument {
    file_id: String,
    file_name: Option<String>,
}

/// 视频
#[derive(Debug, Deserialize)]
struct TgVideo {
    file_id: String,
}

/// 贴纸
#[derive(Debug, Deserialize)]
struct TgSticker {
    file_id: String,
}

// ─── 适配器主体 ───────────────────────────────────────────────────────────────

/// Telegram Bot API 适配器
pub struct TelegramAdapter {
    /// HTTP 客户端
    client: reqwest::Client,
    /// 适配器配置
    config: TelegramConfig,
    /// 长轮询任务句柄（运行期间持有）
    poll_handle: tokio::sync::Mutex<Option<JoinHandle<()>>>,
    /// 停止轮询信号
    poll_stop: Arc<Notify>,
    /// 当前轮询 offset（已确认到 update_id + 1）
    poll_offset: Arc<AtomicI64>,
}

impl TelegramAdapter {
    /// 创建新的 TelegramAdapter 实例
    pub fn new(config: TelegramConfig) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .use_rustls_tls()
            .build()
            .context("构建 reqwest 客户端失败")?;

        Ok(Self {
            client,
            config,
            poll_handle: tokio::sync::Mutex::new(None),
            poll_stop: Arc::new(Notify::new()),
            poll_offset: Arc::new(AtomicI64::new(0)),
        })
    }

    // ─── 内部辅助方法 ─────────────────────────────────────────────────────────

    /// 发送 JSON 请求到 Telegram Bot API 端点
    async fn post_json(&self, endpoint: &str, body: Value) -> anyhow::Result<Value> {
        let url = format!("{}/{}", self.config.bot_api_prefix(), endpoint);
        debug!(url = %url, "向 Telegram API 发送请求");

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("请求 Telegram API 端点 {endpoint} 失败"))?;

        let status = resp.status();
        let text = resp.text().await.context("读取 Telegram API 响应体失败")?;

        if !status.is_success() {
            return Err(anyhow!(
                "Telegram API 返回非成功状态 {status}，端点 {endpoint}，响应: {text}"
            ));
        }

        let value: Value = serde_json::from_str(&text)
            .with_context(|| format!("解析 Telegram API 响应 JSON 失败: {text}"))?;

        if !value["ok"].as_bool().unwrap_or(false) {
            let desc = value["description"]
                .as_str()
                .unwrap_or("未知错误")
                .to_string();
            return Err(anyhow!("Telegram API 返回错误: {desc}"));
        }

        Ok(value)
    }

    /// 获取文件的 file_path，然后下载文件字节
    async fn fetch_file_bytes(&self, file_id: &str) -> anyhow::Result<Vec<u8>> {
        // 第一步：getFile 获取 file_path
        let resp = self
            .post_json("getFile", json!({ "file_id": file_id }))
            .await
            .context("getFile 请求失败")?;

        let file_path = resp["result"]["file_path"]
            .as_str()
            .ok_or_else(|| anyhow!("getFile 响应中缺少 file_path，file_id={file_id}"))?
            .to_string();

        // 第二步：下载文件内容
        let download_url = format!("{}/{}", self.config.file_api_prefix(), file_path);
        debug!(url = %download_url, "下载 Telegram 文件");

        let bytes = self
            .client
            .get(&download_url)
            .send()
            .await
            .with_context(|| format!("下载文件失败: {download_url}"))?
            .bytes()
            .await
            .context("读取文件字节失败")?;

        Ok(bytes.to_vec())
    }

    /// 将 TgMessage 解析为 ImMessage（关联函数，不需要 adapter 实例）
    fn parse_tg_message(update_id: i64, msg: TgMessage, raw: Value) -> anyhow::Result<ImMessage> {
        let chat_id = msg.chat.id.to_string();
        let message_id = msg.message_id.to_string();

        // 发送者信息（非 Option，无发送者时用空字符串）
        let sender: String = msg
            .from
            .as_ref()
            .map(|u| {
                u.username
                    .clone()
                    .or_else(|| u.first_name.clone())
                    .unwrap_or_else(|| u.id.to_string())
            })
            .unwrap_or_default();

        // 将 Telegram Unix 时间戳（秒）转换为 chrono::DateTime<Utc>
        let timestamp =
            chrono::DateTime::from_timestamp(msg.date, 0).unwrap_or_else(chrono::Utc::now);

        let mut metadata: HashMap<String, String> = HashMap::new();
        metadata.insert("update_id".to_string(), update_id.to_string());
        metadata.insert("message_id".to_string(), message_id.clone());
        if !sender.is_empty() {
            metadata.insert("sender".to_string(), sender.clone());
        }

        // 根据消息内容类型判断 ImMessageType
        if let Some(voice) = msg.voice {
            // 语音消息
            let duration = voice.duration;
            return Ok(ImMessage {
                id: message_id,
                platform: "telegram".to_string(),
                chat_id,
                sender,
                message_type: ImMessageType::Voice,
                text_content: None,
                voice_media_id: Some(voice.file_id),
                voice_duration_secs: duration,
                timestamp,
                metadata,
                raw,
            });
        }

        if let Some(ref photos) = msg.photo {
            // 图片消息：取最大分辨率（最后一项）
            let file_id = photos.last().map(|p| p.file_id.clone()).unwrap_or_default();
            return Ok(ImMessage {
                id: message_id,
                platform: "telegram".to_string(),
                chat_id,
                sender,
                message_type: ImMessageType::Text,
                text_content: Some(format!("[photo:{file_id}]")),
                voice_media_id: None,
                voice_duration_secs: None,
                timestamp,
                metadata,
                raw,
            });
        }

        if let Some(ref doc) = msg.document {
            // 文件消息
            let file_name = doc
                .file_name
                .clone()
                .unwrap_or_else(|| "unknown".to_string());
            let file_id = &doc.file_id;
            return Ok(ImMessage {
                id: message_id,
                platform: "telegram".to_string(),
                chat_id,
                sender,
                message_type: ImMessageType::Text,
                text_content: Some(format!("[file:{file_name}:{file_id}]")),
                voice_media_id: None,
                voice_duration_secs: None,
                timestamp,
                metadata,
                raw,
            });
        }

        if let Some(ref video) = msg.video {
            // 视频消息
            let file_id = &video.file_id;
            return Ok(ImMessage {
                id: message_id,
                platform: "telegram".to_string(),
                chat_id,
                sender,
                message_type: ImMessageType::Text,
                text_content: Some(format!("[video:{file_id}]")),
                voice_media_id: None,
                voice_duration_secs: None,
                timestamp,
                metadata,
                raw,
            });
        }

        if let Some(ref sticker) = msg.sticker {
            // 贴纸消息
            let file_id = &sticker.file_id;
            return Ok(ImMessage {
                id: message_id,
                platform: "telegram".to_string(),
                chat_id,
                sender,
                message_type: ImMessageType::Text,
                text_content: Some(format!("[sticker:{file_id}]")),
                voice_media_id: None,
                voice_duration_secs: None,
                timestamp,
                metadata,
                raw,
            });
        }

        // 文本消息（包含 /command 指令）
        let text = msg.text.clone().unwrap_or_default();
        let is_command = text.starts_with('/');
        let msg_type = if is_command {
            ImMessageType::Command
        } else {
            ImMessageType::Text
        };

        Ok(ImMessage {
            id: message_id,
            platform: "telegram".to_string(),
            chat_id,
            sender,
            message_type: msg_type,
            text_content: if text.is_empty() { None } else { Some(text) },
            voice_media_id: None,
            voice_duration_secs: None,
            timestamp,
            metadata,
            raw,
        })
    }

    // ─── 公开方法 ─────────────────────────────────────────────────────────────

    /// 启动长轮询，返回接收消息的 channel receiver
    /// 调用后自动在后台任务循环调用 getUpdates
    pub async fn start_polling(&self) -> mpsc::Receiver<ImMessage> {
        let (tx, rx) = mpsc::channel::<ImMessage>(256);

        let client = self.client.clone();
        let config = self.config.clone();
        let stop = self.poll_stop.clone();
        let offset_ref = self.poll_offset.clone();

        let handle = tokio::spawn(async move {
            info!("Telegram 长轮询任务启动");
            loop {
                let offset = offset_ref.load(Ordering::SeqCst);

                let body = json!({
                    "offset": offset,
                    "timeout": config.poll_timeout_secs,
                    "limit": config.poll_limit,
                    "allowed_updates": ["message"],
                });

                let url = format!("{}/getUpdates", config.bot_api_prefix());

                // 使用 select! 同时等待 API 响应和停止信号
                let resp_fut = client
                    .post(&url)
                    .json(&body)
                    .timeout(std::time::Duration::from_secs(
                        config.poll_timeout_secs + 10,
                    ))
                    .send();

                tokio::select! {
                    _ = stop.notified() => {
                        info!("收到停止信号，Telegram 长轮询退出");
                        break;
                    }
                    result = resp_fut => {
                        match result {
                            Err(e) => {
                                warn!("getUpdates 请求失败: {e}，1 秒后重试");
                                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                                continue;
                            }
                            Ok(resp) => {
                                let text = match resp.text().await {
                                    Ok(t) => t,
                                    Err(e) => {
                                        warn!("读取 getUpdates 响应体失败: {e}");
                                        continue;
                                    }
                                };

                                let value: Value = match serde_json::from_str(&text) {
                                    Ok(v) => v,
                                    Err(e) => {
                                        warn!("解析 getUpdates JSON 失败: {e}，原始: {text}");
                                        continue;
                                    }
                                };

                                if !value["ok"].as_bool().unwrap_or(false) {
                                    warn!("getUpdates 返回 ok=false: {}", value["description"].as_str().unwrap_or("未知"));
                                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                                    continue;
                                }

                                // 以原始 Value 数组保留完整数据，避免反序列化后信息丢失
                                let raw_updates: Vec<Value> = match serde_json::from_value(
                                    value["result"].clone(),
                                ) {
                                    Ok(u) => u,
                                    Err(e) => {
                                        warn!("解析 updates 列表失败: {e}");
                                        continue;
                                    }
                                };

                                for raw_update in raw_updates {
                                    let update_id = raw_update["update_id"].as_i64().unwrap_or(0);
                                    let next_offset = update_id + 1;

                                    if raw_update.get("message").is_some() {
                                        let msg_value = raw_update["message"].clone();
                                        let msg: TgMessage = match serde_json::from_value(msg_value) {
                                            Ok(m) => m,
                                            Err(e) => {
                                                warn!("解析 TgMessage 失败，跳过 update_id={update_id}: {e}");
                                                offset_ref.fetch_max(next_offset, Ordering::SeqCst);
                                                continue;
                                            }
                                        };

                                        match TelegramAdapter::parse_tg_message(
                                            update_id,
                                            msg,
                                            raw_update,
                                        ) {
                                            Ok(im_msg) => {
                                                if tx.send(im_msg).await.is_err() {
                                                    info!("消息接收方已关闭，停止长轮询");
                                                    return;
                                                }
                                            }
                                            Err(e) => {
                                                warn!("解析消息失败，跳过 update_id={update_id}: {e}");
                                            }
                                        }
                                    }

                                    // 更新 offset，确保不回退
                                    offset_ref.fetch_max(next_offset, Ordering::SeqCst);
                                }
                            }
                        }
                    }
                }
            }
        });

        *self.poll_handle.lock().await = Some(handle);
        rx
    }

    /// 停止长轮询
    pub async fn stop_polling(&self) {
        info!("发送 Telegram 长轮询停止信号");
        self.poll_stop.notify_waiters();

        let mut guard = self.poll_handle.lock().await;
        if let Some(handle) = guard.take() {
            // 等待任务完成，最多 5 秒
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
        }
    }

    /// 回复特定消息（reply_to_message_id 为可选）
    pub async fn reply_text(
        &self,
        chat_id: i64,
        reply_to_message_id: Option<i64>,
        text: &str,
    ) -> anyhow::Result<i64> {
        let mut body = json!({
            "chat_id": chat_id,
            "text": text,
        });

        if let Some(reply_id) = reply_to_message_id {
            body["reply_to_message_id"] = json!(reply_id);
        }

        let resp = self.post_json("sendMessage", body).await?;
        let message_id = resp["result"]["message_id"]
            .as_i64()
            .ok_or_else(|| anyhow!("sendMessage 响应缺少 message_id"))?;

        debug!("发送回复消息成功，message_id={message_id}");
        Ok(message_id)
    }

    /// 发送图片
    pub async fn send_photo(
        &self,
        chat_id: i64,
        data: Vec<u8>,
        caption: Option<&str>,
    ) -> anyhow::Result<i64> {
        let url = format!("{}/sendPhoto", self.config.bot_api_prefix());

        let mut form = multipart::Form::new()
            .text("chat_id", chat_id.to_string())
            .part("photo", multipart::Part::bytes(data).file_name("photo.jpg"));

        if let Some(cap) = caption {
            form = form.text("caption", cap.to_string());
        }

        let resp = self
            .client
            .post(&url)
            .multipart(form)
            .send()
            .await
            .context("sendPhoto 请求失败")?;

        let text = resp.text().await.context("读取 sendPhoto 响应失败")?;
        let value: Value = serde_json::from_str(&text).context("解析 sendPhoto 响应 JSON 失败")?;

        if !value["ok"].as_bool().unwrap_or(false) {
            return Err(anyhow!(
                "sendPhoto 失败: {}",
                value["description"].as_str().unwrap_or("未知错误")
            ));
        }

        let message_id = value["result"]["message_id"]
            .as_i64()
            .ok_or_else(|| anyhow!("sendPhoto 响应缺少 message_id"))?;

        Ok(message_id)
    }

    /// 发送文件文档
    pub async fn send_document(
        &self,
        chat_id: i64,
        data: Vec<u8>,
        filename: &str,
        caption: Option<&str>,
    ) -> anyhow::Result<i64> {
        let url = format!("{}/sendDocument", self.config.bot_api_prefix());

        let mut form = multipart::Form::new()
            .text("chat_id", chat_id.to_string())
            .part(
                "document",
                multipart::Part::bytes(data).file_name(filename.to_string()),
            );

        if let Some(cap) = caption {
            form = form.text("caption", cap.to_string());
        }

        let resp = self
            .client
            .post(&url)
            .multipart(form)
            .send()
            .await
            .context("sendDocument 请求失败")?;

        let text = resp.text().await.context("读取 sendDocument 响应失败")?;
        let value: Value =
            serde_json::from_str(&text).context("解析 sendDocument 响应 JSON 失败")?;

        if !value["ok"].as_bool().unwrap_or(false) {
            return Err(anyhow!(
                "sendDocument 失败: {}",
                value["description"].as_str().unwrap_or("未知错误")
            ));
        }

        let message_id = value["result"]["message_id"]
            .as_i64()
            .ok_or_else(|| anyhow!("sendDocument 响应缺少 message_id"))?;

        Ok(message_id)
    }

    /// 重置轮询 offset（用于测试或重新消费）
    pub fn reset_poll_offset(&self, offset: i64) {
        self.poll_offset.store(offset, Ordering::SeqCst);
    }

    /// 获取当前轮询 offset
    pub fn current_poll_offset(&self) -> i64 {
        self.poll_offset.load(Ordering::SeqCst)
    }
}

// ─── ImPlatformAdapter 实现 ───────────────────────────────────────────────────

#[async_trait]
impl ImPlatformAdapter for TelegramAdapter {
    fn platform_id(&self) -> &str {
        "telegram"
    }

    fn supports_voice(&self) -> bool {
        true
    }

    fn supports_realtime_audio(&self) -> bool {
        false
    }

    /// 将 Telegram Update 原始 JSON 解析为统一 ImMessage
    async fn normalize_inbound(&self, raw: Value) -> anyhow::Result<ImMessage> {
        // 接受完整 Update 对象或裸 Message 对象
        let (update_id, msg_value) = if raw.get("update_id").is_some() {
            let update_id = raw["update_id"].as_i64().unwrap_or(0);
            let msg_value = raw["message"].clone();
            (update_id, msg_value)
        } else if raw.get("message_id").is_some() {
            // 裸 Message 对象
            (0i64, raw.clone())
        } else {
            return Err(anyhow!("无法识别的 Telegram 入站消息格式"));
        };

        let msg: TgMessage =
            serde_json::from_value(msg_value.clone()).context("解析 TgMessage 失败")?;

        Self::parse_tg_message(update_id, msg, raw)
    }

    /// 向指定 chat_id 发送文本消息
    async fn send_text(&self, chat_id: &str, text: &str) -> anyhow::Result<()> {
        let chat_id_num: i64 = chat_id
            .parse()
            .with_context(|| format!("chat_id 不是有效整数: {chat_id}"))?;

        self.post_json(
            "sendMessage",
            json!({
                "chat_id": chat_id_num,
                "text": text,
            }),
        )
        .await?;

        debug!("发送文本消息成功，chat_id={chat_id}");
        Ok(())
    }

    /// 发送卡片消息（Telegram 不支持 Lark 风格卡片，降级为文本）
    async fn send_card(&self, chat_id: &str, card: Value) -> anyhow::Result<()> {
        // 尝试从卡片中提取可读文本，否则序列化 JSON
        let text = if let Some(title) = card["header"]["title"]["content"].as_str() {
            title.to_string()
        } else if let Some(t) = card["title"].as_str() {
            t.to_string()
        } else {
            serde_json::to_string_pretty(&card).unwrap_or_else(|_| "（卡片消息）".to_string())
        };

        warn!("Telegram 不支持原生卡片，降级为文本发送，chat_id={chat_id}");
        self.send_text(chat_id, &text).await
    }

    /// 发送语音消息（OGG Opus 格式）
    async fn send_voice(
        &self,
        chat_id: &str,
        audio: &[u8],
        _format: AudioFormat,
    ) -> anyhow::Result<()> {
        let chat_id_num: i64 = chat_id
            .parse()
            .with_context(|| format!("chat_id 不是有效整数: {chat_id}"))?;

        let url = format!("{}/sendVoice", self.config.bot_api_prefix());

        let form = multipart::Form::new()
            .text("chat_id", chat_id_num.to_string())
            .part(
                "voice",
                multipart::Part::bytes(audio.to_vec()).file_name("voice.ogg"),
            );

        let resp = self
            .client
            .post(&url)
            .multipart(form)
            .send()
            .await
            .context("sendVoice 请求失败")?;

        let text_body = resp.text().await.context("读取 sendVoice 响应失败")?;
        let value: Value =
            serde_json::from_str(&text_body).context("解析 sendVoice 响应 JSON 失败")?;

        if !value["ok"].as_bool().unwrap_or(false) {
            return Err(anyhow!(
                "sendVoice 失败: {}",
                value["description"].as_str().unwrap_or("未知错误")
            ));
        }

        debug!("发送语音消息成功，chat_id={chat_id}");
        Ok(())
    }

    /// 下载语音文件：先 getFile 获取 file_path，再下载字节
    async fn download_voice(&self, media_id: &str) -> anyhow::Result<Vec<u8>> {
        debug!("下载 Telegram 语音文件，file_id={media_id}");
        self.fetch_file_bytes(media_id).await
    }

    /// 验证 Telegram 请求签名
    /// Telegram 使用 Bot Token 作为 HMAC-SHA256 密钥对 payload 签名
    /// signature 参数应为十六进制字符串
    fn validate_signature(&self, payload: &[u8], signature: &str) -> bool {
        type HmacSha256 = Hmac<Sha256>;

        let token_bytes = self.config.bot_token.as_bytes();
        let mut mac = match HmacSha256::new_from_slice(token_bytes) {
            Ok(m) => m,
            Err(e) => {
                error!("创建 HMAC 实例失败: {e}");
                return false;
            }
        };

        mac.update(payload);

        let expected = hex::encode(mac.finalize().into_bytes());

        // 常量时间比较，防止时序攻击
        use subtle::ConstantTimeEq;
        expected.as_bytes().ct_eq(signature.as_bytes()).into()
    }
}

// ─── 单元测试 ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 创建测试用适配器（不会实际发起网络请求）
    fn make_adapter() -> TelegramAdapter {
        let config = TelegramConfig::new("123456:ABCDEFGHIJKLMNOPabcdefghijklmnop");
        TelegramAdapter::new(config).expect("创建适配器失败")
    }

    // ── 配置测试 ──────────────────────────────────────────────────────────────

    #[test]
    fn test_config_defaults() {
        let cfg = TelegramConfig::new("my_token");
        assert_eq!(cfg.bot_token, "my_token");
        assert_eq!(cfg.api_base_url, "https://api.telegram.org");
        assert_eq!(cfg.poll_timeout_secs, 30);
        assert_eq!(cfg.poll_limit, 100);
    }

    #[test]
    fn test_bot_api_prefix() {
        let cfg = TelegramConfig::new("123:token");
        assert_eq!(
            cfg.bot_api_prefix(),
            "https://api.telegram.org/bot123:token"
        );
    }

    #[test]
    fn test_file_api_prefix() {
        let cfg = TelegramConfig::new("123:token");
        assert_eq!(
            cfg.file_api_prefix(),
            "https://api.telegram.org/file/bot123:token"
        );
    }

    #[test]
    fn test_custom_api_base_url() {
        let mut cfg = TelegramConfig::new("tok");
        cfg.api_base_url = "https://custom.example.com".to_string();
        assert_eq!(cfg.bot_api_prefix(), "https://custom.example.com/bottok");
    }

    // ── 平台标识测试 ───────────────────────────────────────────────────────────

    #[test]
    fn test_platform_id() {
        let adapter = make_adapter();
        assert_eq!(adapter.platform_id(), "telegram");
    }

    #[test]
    fn test_supports_voice() {
        let adapter = make_adapter();
        assert!(adapter.supports_voice());
    }

    #[test]
    fn test_not_supports_realtime_audio() {
        let adapter = make_adapter();
        assert!(!adapter.supports_realtime_audio());
    }

    // ── normalize_inbound 消息解析测试 ────────────────────────────────────────

    #[tokio::test]
    async fn test_normalize_text_message() {
        let adapter = make_adapter();
        let raw = json!({
            "update_id": 100,
            "message": {
                "message_id": 1,
                "from": {"id": 42, "first_name": "Alice"},
                "chat": {"id": 789, "type": "private"},
                "date": 1700000000i64,
                "text": "hello world"
            }
        });

        let msg = adapter.normalize_inbound(raw).await.unwrap();
        assert_eq!(msg.platform, "telegram");
        assert_eq!(msg.chat_id, "789");
        assert_eq!(msg.message_type, ImMessageType::Text);
        assert_eq!(msg.text_content.as_deref(), Some("hello world"));
        assert_eq!(msg.voice_media_id, None);
    }

    #[tokio::test]
    async fn test_normalize_command_message() {
        let adapter = make_adapter();
        let raw = json!({
            "update_id": 101,
            "message": {
                "message_id": 2,
                "from": {"id": 42, "first_name": "Alice"},
                "chat": {"id": 789, "type": "private"},
                "date": 1700000001i64,
                "text": "/start"
            }
        });

        let msg = adapter.normalize_inbound(raw).await.unwrap();
        assert_eq!(msg.message_type, ImMessageType::Command);
        assert_eq!(msg.text_content.as_deref(), Some("/start"));
    }

    #[tokio::test]
    async fn test_normalize_voice_message() {
        let adapter = make_adapter();
        let raw = json!({
            "update_id": 102,
            "message": {
                "message_id": 3,
                "from": {"id": 42, "first_name": "Alice"},
                "chat": {"id": 789, "type": "private"},
                "date": 1700000002i64,
                "voice": {
                    "file_id": "voice_file_abc",
                    "duration": 7
                }
            }
        });

        let msg = adapter.normalize_inbound(raw).await.unwrap();
        assert_eq!(msg.message_type, ImMessageType::Voice);
        assert_eq!(msg.voice_media_id.as_deref(), Some("voice_file_abc"));
        assert_eq!(msg.voice_duration_secs, Some(7.0_f32));
        assert_eq!(msg.text_content, None);
    }

    #[tokio::test]
    async fn test_normalize_photo_message() {
        let adapter = make_adapter();
        let raw = json!({
            "update_id": 103,
            "message": {
                "message_id": 4,
                "from": {"id": 42, "first_name": "Alice"},
                "chat": {"id": 789, "type": "private"},
                "date": 1700000003i64,
                "photo": [
                    {"file_id": "small_photo", "width": 90, "height": 90},
                    {"file_id": "large_photo", "width": 800, "height": 600}
                ]
            }
        });

        let msg = adapter.normalize_inbound(raw).await.unwrap();
        assert_eq!(msg.message_type, ImMessageType::Text);
        // 取最大分辨率（最后一项）
        assert_eq!(msg.text_content.as_deref(), Some("[photo:large_photo]"));
    }

    #[tokio::test]
    async fn test_normalize_document_message() {
        let adapter = make_adapter();
        let raw = json!({
            "update_id": 104,
            "message": {
                "message_id": 5,
                "from": {"id": 42, "first_name": "Alice"},
                "chat": {"id": 789, "type": "private"},
                "date": 1700000004i64,
                "document": {
                    "file_id": "doc_file_xyz",
                    "file_name": "report.pdf"
                }
            }
        });

        let msg = adapter.normalize_inbound(raw).await.unwrap();
        assert_eq!(msg.message_type, ImMessageType::Text);
        assert_eq!(
            msg.text_content.as_deref(),
            Some("[file:report.pdf:doc_file_xyz]")
        );
    }

    #[tokio::test]
    async fn test_normalize_video_message() {
        let adapter = make_adapter();
        let raw = json!({
            "update_id": 105,
            "message": {
                "message_id": 6,
                "from": {"id": 42, "first_name": "Alice"},
                "chat": {"id": 789, "type": "private"},
                "date": 1700000005i64,
                "video": {"file_id": "video_file_123"}
            }
        });

        let msg = adapter.normalize_inbound(raw).await.unwrap();
        assert_eq!(msg.message_type, ImMessageType::Text);
        assert_eq!(msg.text_content.as_deref(), Some("[video:video_file_123]"));
    }

    #[tokio::test]
    async fn test_normalize_sticker_message() {
        let adapter = make_adapter();
        let raw = json!({
            "update_id": 106,
            "message": {
                "message_id": 7,
                "from": {"id": 42, "first_name": "Alice"},
                "chat": {"id": 789, "type": "private"},
                "date": 1700000006i64,
                "sticker": {"file_id": "sticker_file_456"}
            }
        });

        let msg = adapter.normalize_inbound(raw).await.unwrap();
        assert_eq!(msg.message_type, ImMessageType::Text);
        assert_eq!(
            msg.text_content.as_deref(),
            Some("[sticker:sticker_file_456]")
        );
    }

    #[tokio::test]
    async fn test_normalize_bare_message_object() {
        // 允许直接传入裸 Message 对象（不包含 update_id 外层）
        let adapter = make_adapter();
        let raw = json!({
            "message_id": 8,
            "from": {"id": 99, "username": "bob"},
            "chat": {"id": 111, "type": "group"},
            "date": 1700000007i64,
            "text": "bare message"
        });

        let msg = adapter.normalize_inbound(raw).await.unwrap();
        assert_eq!(msg.chat_id, "111");
        assert_eq!(msg.sender, "bob");
        assert_eq!(msg.text_content.as_deref(), Some("bare message"));
    }

    #[tokio::test]
    async fn test_normalize_sender_fallback_to_first_name() {
        let adapter = make_adapter();
        let raw = json!({
            "update_id": 107,
            "message": {
                "message_id": 9,
                "from": {"id": 55, "first_name": "Charlie"},
                "chat": {"id": 222, "type": "private"},
                "date": 1700000008i64,
                "text": "hi"
            }
        });

        let msg = adapter.normalize_inbound(raw).await.unwrap();
        assert_eq!(msg.sender, "Charlie");
    }

    // ── API URL 构造测试 ───────────────────────────────────────────────────────

    #[test]
    fn test_api_url_construction() {
        let cfg = TelegramConfig::new("TOKEN");
        assert!(cfg
            .bot_api_prefix()
            .starts_with("https://api.telegram.org/botTOKEN"));
        assert!(cfg
            .file_api_prefix()
            .starts_with("https://api.telegram.org/file/botTOKEN"));
    }

    // ── 签名验证测试 ───────────────────────────────────────────────────────────

    #[test]
    fn test_validate_signature_correct() {
        use hmac::{Hmac, Mac};
        use sha2::Sha256;

        let adapter = make_adapter();
        let payload = b"test_payload_data";
        let token = adapter.config.bot_token.as_bytes();

        type HmacSha256 = Hmac<Sha256>;
        let mut mac = HmacSha256::new_from_slice(token).unwrap();
        mac.update(payload);
        let expected_sig = hex::encode(mac.finalize().into_bytes());

        assert!(adapter.validate_signature(payload, &expected_sig));
    }

    #[test]
    fn test_validate_signature_wrong() {
        let adapter = make_adapter();
        let payload = b"test_payload_data";
        assert!(!adapter.validate_signature(payload, "wrong_signature_hex"));
    }

    #[test]
    fn test_validate_signature_empty_payload() {
        use hmac::{Hmac, Mac};
        use sha2::Sha256;

        let adapter = make_adapter();
        let payload = b"";
        let token = adapter.config.bot_token.as_bytes();

        type HmacSha256 = Hmac<Sha256>;
        let mut mac = HmacSha256::new_from_slice(token).unwrap();
        mac.update(payload);
        let sig = hex::encode(mac.finalize().into_bytes());

        assert!(adapter.validate_signature(payload, &sig));
    }

    // ── 轮询 offset 跟踪测试 ──────────────────────────────────────────────────

    #[test]
    fn test_poll_offset_initial_zero() {
        let adapter = make_adapter();
        assert_eq!(adapter.current_poll_offset(), 0);
    }

    #[test]
    fn test_poll_offset_reset() {
        let adapter = make_adapter();
        adapter.reset_poll_offset(500);
        assert_eq!(adapter.current_poll_offset(), 500);
    }

    #[test]
    fn test_poll_offset_fetch_max() {
        let adapter = make_adapter();
        adapter.poll_offset.fetch_max(100, Ordering::SeqCst);
        assert_eq!(adapter.current_poll_offset(), 100);
        // 不能回退
        adapter.poll_offset.fetch_max(50, Ordering::SeqCst);
        assert_eq!(adapter.current_poll_offset(), 100);
        // 可以前进
        adapter.poll_offset.fetch_max(200, Ordering::SeqCst);
        assert_eq!(adapter.current_poll_offset(), 200);
    }

    // ── 元数据字段测试 ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_metadata_contains_update_id_and_message_id() {
        let adapter = make_adapter();
        let raw = json!({
            "update_id": 999,
            "message": {
                "message_id": 42,
                "chat": {"id": 123, "type": "private"},
                "date": 1700000000i64,
                "text": "meta test"
            }
        });

        let msg = adapter.normalize_inbound(raw).await.unwrap();
        assert_eq!(
            msg.metadata.get("update_id").map(|s| s.as_str()),
            Some("999")
        );
        assert_eq!(
            msg.metadata.get("message_id").map(|s| s.as_str()),
            Some("42")
        );
    }

    #[tokio::test]
    async fn test_normalize_invalid_json_returns_error() {
        let adapter = make_adapter();
        // 没有 update_id 也没有 message_id 字段
        let raw = json!({"foo": "bar"});
        let result = adapter.normalize_inbound(raw).await;
        assert!(result.is_err());
    }
}
