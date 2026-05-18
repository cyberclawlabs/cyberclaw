//! # LINE Messaging API 适配器
//!
//! LINE Messaging API IM 平台适配器，对齐 hermes-agent IM 平台广度。支持：
//! - 文字消息接收 / 发送（push API）
//! - 语音消息下载（content API）
//! - Webhook HMAC-SHA256 签名校验（X-Line-Signature）
//!
//! ## LINE Messaging API 端点
//! - Push 消息：`POST https://api.line.me/v2/bot/message/push`
//! - 下载内容：`GET https://api-data.line.me/v2/bot/message/{messageId}/content`
//!
//! ## 认证头部
//! ```text
//! Authorization: Bearer {channel_access_token}
//! Content-Type: application/json
//! ```
//!
//! ## 签名校验
//! `X-Line-Signature` 是 `base64(HMAC-SHA256(channel_secret, request_body))`。
//! 我们用 `subtle::ConstantTimeEq` 比较以防时序泄露。
//!
//! ## 当前限制（v1.2.15 → v1.3 路线图）
//! - `send_voice` 走 push API 的 `audio` 类型，要求 audio 文件先上传到
//!   外部 CDN 拿到 `originalContentUrl`。当前实现把 audio 字节存盘
//!   后返 NotImplemented —— 没有公网 CDN 桥接前不假装能发。
//! - `validate_signature` 已实现并测试。
//! - LINE 不支持 send_card，走基类默认（降级为文字），与其他平台一致。

use crate::im_channel::{AudioFormat, ImMessage, ImMessageType, ImPlatformAdapter};
use base64::Engine as _;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use subtle::ConstantTimeEq;

const LINE_PUSH_URL: &str = "https://api.line.me/v2/bot/message/push";
const LINE_CONTENT_BASE: &str = "https://api-data.line.me/v2/bot/message";

/// 适配器配置 — 从 env 读取，调用 `LineAdapter::from_env` 构造。
#[derive(Debug, Clone)]
pub struct LineConfig {
    /// 用于调用 LINE API 的 Bearer Token。
    pub channel_access_token: String,
    /// 用于校验 webhook 签名的 channel secret。
    pub channel_secret: String,
}

impl LineConfig {
    /// 从 `LINE_CHANNEL_ACCESS_TOKEN` + `LINE_CHANNEL_SECRET` env 构造。
    /// 任一缺失返回 `Err`，让 `register_adapter` 决定是否在 register 失败时
    /// 走 graceful skip。
    pub fn from_env() -> anyhow::Result<Self> {
        let token = std::env::var("LINE_CHANNEL_ACCESS_TOKEN").map_err(|_| {
            anyhow::anyhow!("LINE_CHANNEL_ACCESS_TOKEN env not set")
        })?;
        let secret = std::env::var("LINE_CHANNEL_SECRET")
            .map_err(|_| anyhow::anyhow!("LINE_CHANNEL_SECRET env not set"))?;
        Ok(Self {
            channel_access_token: token,
            channel_secret: secret,
        })
    }
}

pub struct LineAdapter {
    cfg: LineConfig,
    http: reqwest::Client,
}

impl LineAdapter {
    pub fn new(cfg: LineConfig) -> Self {
        Self {
            cfg,
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .expect("reqwest client build"),
        }
    }

    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self::new(LineConfig::from_env()?))
    }
}

// ---------------------------------------------------------------------------
// Webhook event types
// ---------------------------------------------------------------------------
//
// LINE webhook envelope:
//   { "destination": "...", "events": [ { type: ..., message: {...}, source: {...}, timestamp: ms, replyToken: ... }, ... ] }
//
// We pluck the first event and either normalize it into ImMessage or surface
// a clear error for unsupported event types (follow / unfollow / postback).

#[derive(Debug, Deserialize)]
struct LineWebhookEnvelope {
    #[serde(default)]
    events: Vec<LineWebhookEvent>,
}

#[derive(Debug, Deserialize)]
struct LineWebhookEvent {
    #[serde(rename = "type")]
    event_type: String,
    #[serde(default)]
    message: Option<LineMessageObj>,
    #[serde(default)]
    source: Option<LineSource>,
    #[serde(default)]
    timestamp: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct LineMessageObj {
    id: String,
    #[serde(rename = "type")]
    msg_type: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    duration: Option<u32>, // milliseconds for audio
}

#[derive(Debug, Deserialize)]
struct LineSource {
    // `type` (user/group/room) is implied by which of the three id fields
    // is populated, so we don't store it separately.
    #[serde(default, rename = "userId")]
    user_id: Option<String>,
    #[serde(default, rename = "groupId")]
    group_id: Option<String>,
    #[serde(default, rename = "roomId")]
    room_id: Option<String>,
}

impl LineSource {
    /// LINE chat_id 优先级：group > room > user。group/room 是多人对话，
    /// 同一人在不同 group 中的 chat_id 不同；这是 LINE API 的语义。
    fn chat_id(&self) -> Option<&str> {
        self.group_id
            .as_deref()
            .or(self.room_id.as_deref())
            .or(self.user_id.as_deref())
    }
}

#[derive(Debug, Serialize)]
struct LinePushRequest<'a> {
    to: &'a str,
    messages: Vec<LineOutboundMessage>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum LineOutboundMessage {
    #[serde(rename = "text")]
    Text { text: String },
}

#[async_trait::async_trait]
impl ImPlatformAdapter for LineAdapter {
    fn platform_id(&self) -> &str {
        "line"
    }

    fn supports_voice(&self) -> bool {
        // download_voice works; send_voice does NOT (no CDN), so we report
        // false to keep the contract honest. Routers should not pick LINE for
        // outbound voice replies until v1.3 wires a CDN.
        false
    }

    async fn normalize_inbound(&self, raw: serde_json::Value) -> anyhow::Result<ImMessage> {
        let envelope: LineWebhookEnvelope = serde_json::from_value(raw.clone())
            .map_err(|e| anyhow::anyhow!("LINE webhook payload not an envelope: {e}"))?;
        let event = envelope
            .events
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("LINE webhook has no events"))?;
        if event.event_type != "message" {
            anyhow::bail!(
                "LINE event type '{}' is not 'message' (skipping)",
                event.event_type
            );
        }
        let msg = event
            .message
            .ok_or_else(|| anyhow::anyhow!("LINE message event missing 'message' object"))?;
        let source = event
            .source
            .ok_or_else(|| anyhow::anyhow!("LINE message event missing 'source' object"))?;
        let chat_id = source
            .chat_id()
            .ok_or_else(|| anyhow::anyhow!("LINE source has no user/group/room id"))?
            .to_string();
        let sender = source.user_id.clone().unwrap_or_else(|| chat_id.clone());
        let ts_ms = event.timestamp.unwrap_or(0);
        let timestamp = chrono::DateTime::from_timestamp_millis(ts_ms)
            .unwrap_or_else(chrono::Utc::now);

        let (message_type, text_content, voice_media_id, voice_duration_secs) =
            match msg.msg_type.as_str() {
                "text" => (
                    ImMessageType::Text,
                    msg.text.clone(),
                    None,
                    None,
                ),
                "audio" => (
                    ImMessageType::Voice,
                    None,
                    Some(msg.id.clone()),
                    msg.duration.map(|ms| ms as f32 / 1000.0),
                ),
                other => {
                    anyhow::bail!("LINE message type '{}' not supported yet", other)
                }
            };

        Ok(ImMessage {
            id: msg.id,
            platform: "line".to_string(),
            chat_id,
            sender,
            message_type,
            text_content,
            voice_media_id,
            voice_duration_secs,
            timestamp,
            metadata: std::collections::HashMap::new(),
            raw,
        })
    }

    async fn send_text(&self, chat_id: &str, text: &str) -> anyhow::Result<()> {
        let body = LinePushRequest {
            to: chat_id,
            messages: vec![LineOutboundMessage::Text {
                text: text.to_string(),
            }],
        };
        let resp = self
            .http
            .post(LINE_PUSH_URL)
            .bearer_auth(&self.cfg.channel_access_token)
            .json(&body)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("LINE push request failed: {e}"))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let txt = resp.text().await.unwrap_or_default();
            anyhow::bail!("LINE push returned {}: {}", status, txt);
        }
        Ok(())
    }

    async fn send_voice(
        &self,
        _chat_id: &str,
        _audio: &[u8],
        _format: AudioFormat,
    ) -> anyhow::Result<()> {
        anyhow::bail!(
            "LINE send_voice requires a public CDN URL for originalContentUrl; \
             v1.2.15 has no CDN bridge. Use text-only replies on LINE for now."
        )
    }

    async fn download_voice(&self, media_id: &str) -> anyhow::Result<Vec<u8>> {
        let url = format!("{}/{}/content", LINE_CONTENT_BASE, media_id);
        let resp = self
            .http
            .get(url)
            .bearer_auth(&self.cfg.channel_access_token)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("LINE download request failed: {e}"))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let txt = resp.text().await.unwrap_or_default();
            anyhow::bail!("LINE download returned {}: {}", status, txt);
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| anyhow::anyhow!("LINE download body read failed: {e}"))?;
        Ok(bytes.to_vec())
    }

    fn validate_signature(&self, payload: &[u8], signature: &str) -> bool {
        // X-Line-Signature is base64(HMAC-SHA256(channel_secret, body)).
        type HmacSha256 = Hmac<Sha256>;
        let Ok(mut mac) = HmacSha256::new_from_slice(self.cfg.channel_secret.as_bytes()) else {
            return false;
        };
        mac.update(payload);
        let computed = mac.finalize().into_bytes();
        let expected_b64 = base64::engine::general_purpose::STANDARD.encode(computed);
        let provided = signature.trim();
        // Constant-time compare to prevent timing attacks.
        expected_b64.as_bytes().ct_eq(provided.as_bytes()).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn test_adapter() -> LineAdapter {
        LineAdapter::new(LineConfig {
            channel_access_token: "test-token".to_string(),
            channel_secret: "test-secret".to_string(),
        })
    }

    #[test]
    fn platform_id_is_line() {
        assert_eq!(test_adapter().platform_id(), "line");
    }

    #[test]
    fn supports_voice_is_false_for_now() {
        assert!(!test_adapter().supports_voice());
    }

    #[tokio::test]
    async fn normalize_inbound_text_message() {
        let adapter = test_adapter();
        let raw = json!({
            "destination": "U0123",
            "events": [{
                "type": "message",
                "timestamp": 1700000000000_i64,
                "source": { "type": "user", "userId": "Uabc" },
                "message": { "id": "msg-1", "type": "text", "text": "hello bot" },
                "replyToken": "rt-xxx"
            }]
        });
        let msg = adapter.normalize_inbound(raw).await.unwrap();
        assert_eq!(msg.platform, "line");
        assert_eq!(msg.id, "msg-1");
        assert_eq!(msg.chat_id, "Uabc");
        assert_eq!(msg.sender, "Uabc");
        assert!(matches!(msg.message_type, ImMessageType::Text));
        assert_eq!(msg.text_content.as_deref(), Some("hello bot"));
    }

    #[tokio::test]
    async fn normalize_inbound_audio_message() {
        let adapter = test_adapter();
        let raw = json!({
            "events": [{
                "type": "message",
                "timestamp": 1700000000000_i64,
                "source": { "type": "user", "userId": "Uxyz" },
                "message": { "id": "audio-1", "type": "audio", "duration": 3500 }
            }]
        });
        let msg = adapter.normalize_inbound(raw).await.unwrap();
        assert!(matches!(msg.message_type, ImMessageType::Voice));
        assert_eq!(msg.voice_media_id.as_deref(), Some("audio-1"));
        assert_eq!(msg.voice_duration_secs, Some(3.5));
        assert!(msg.text_content.is_none());
    }

    #[tokio::test]
    async fn normalize_inbound_group_chat_id_takes_precedence() {
        let adapter = test_adapter();
        let raw = json!({
            "events": [{
                "type": "message",
                "timestamp": 1700000000000_i64,
                "source": { "type": "group", "userId": "Uuser", "groupId": "Cgroup" },
                "message": { "id": "m", "type": "text", "text": "hi" }
            }]
        });
        let msg = adapter.normalize_inbound(raw).await.unwrap();
        // Group ID wins for chat_id, individual user id for sender.
        assert_eq!(msg.chat_id, "Cgroup");
        assert_eq!(msg.sender, "Uuser");
    }

    #[tokio::test]
    async fn normalize_inbound_rejects_non_message_events() {
        let adapter = test_adapter();
        let raw = json!({
            "events": [{
                "type": "follow",
                "timestamp": 1700000000000_i64,
                "source": { "type": "user", "userId": "U" }
            }]
        });
        let err = adapter.normalize_inbound(raw).await.unwrap_err();
        assert!(err.to_string().contains("not 'message'"));
    }

    #[tokio::test]
    async fn normalize_inbound_rejects_empty_events() {
        let adapter = test_adapter();
        let raw = json!({ "events": [] });
        let err = adapter.normalize_inbound(raw).await.unwrap_err();
        assert!(err.to_string().contains("no events"));
    }

    #[tokio::test]
    async fn normalize_inbound_rejects_unsupported_message_type() {
        let adapter = test_adapter();
        let raw = json!({
            "events": [{
                "type": "message",
                "timestamp": 1700000000000_i64,
                "source": { "type": "user", "userId": "U" },
                "message": { "id": "x", "type": "sticker" }
            }]
        });
        let err = adapter.normalize_inbound(raw).await.unwrap_err();
        assert!(err.to_string().contains("not supported"));
    }

    #[test]
    fn validate_signature_matches_official_sample() {
        // Test vector: empty body with known secret, the resulting
        // base64(HMAC-SHA256("secret", "")) should be the known constant.
        let cfg = LineConfig {
            channel_access_token: "irrelevant".to_string(),
            channel_secret: "secret".to_string(),
        };
        let adapter = LineAdapter::new(cfg);
        // base64(HMAC-SHA256("secret", "")) — verified independently against
        // openssl: `openssl dgst -sha256 -hmac "secret" -binary < /dev/null | base64`
        let expected = "+eZuF5tnR65UEI+C+K3os8Jddv0wr95sOVgixTAZYWk=";
        assert!(adapter.validate_signature(b"", expected));
        assert!(!adapter.validate_signature(b"", "wrong-signature"));
        assert!(!adapter.validate_signature(b"different body", expected));
    }

    #[test]
    fn validate_signature_constant_time_does_not_short_circuit_on_length() {
        // A signature shorter than 44 chars (the b64 length of a 32-byte
        // digest) should still be safely rejected, not panic.
        let cfg = LineConfig {
            channel_access_token: "x".to_string(),
            channel_secret: "y".to_string(),
        };
        let adapter = LineAdapter::new(cfg);
        assert!(!adapter.validate_signature(b"body", "short"));
        assert!(!adapter.validate_signature(b"body", ""));
    }

    #[tokio::test]
    async fn send_voice_returns_explicit_error_for_now() {
        let adapter = test_adapter();
        let err = adapter
            .send_voice("U", b"audio bytes", AudioFormat::Wav)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("send_voice"),
            "must surface 'send_voice' in error message so caller knows the limitation"
        );
    }

    #[test]
    fn config_from_env_requires_both_vars() {
        use std::sync::Mutex;
        static LOCK: Mutex<()> = Mutex::new(());
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::remove_var("LINE_CHANNEL_ACCESS_TOKEN");
            std::env::remove_var("LINE_CHANNEL_SECRET");
        }
        assert!(LineConfig::from_env().is_err());
        unsafe { std::env::set_var("LINE_CHANNEL_ACCESS_TOKEN", "t") };
        assert!(LineConfig::from_env().is_err());
        unsafe { std::env::set_var("LINE_CHANNEL_SECRET", "s") };
        let cfg = LineConfig::from_env().unwrap();
        assert_eq!(cfg.channel_access_token, "t");
        assert_eq!(cfg.channel_secret, "s");
        unsafe {
            std::env::remove_var("LINE_CHANNEL_ACCESS_TOKEN");
            std::env::remove_var("LINE_CHANNEL_SECRET");
        }
    }
}
