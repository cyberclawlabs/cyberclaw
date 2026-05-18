//! Discord adapter — implements `ImPlatformAdapter` for Discord servers.
//!
//! Mirrors Hermes v0.12 `tools/discord_tool.py` core capabilities. Uses
//! Discord REST API v10 directly with a bot token; no dependency on the
//! Discord Gateway (websocket) so we can ship without `serenity`.
//!
//! # Auth
//!
//! Requires a Discord bot token. Set `DISCORD_BOT_TOKEN` env var or pass
//! via constructor. Bot must be invited to the target server with
//! `bot` + `messages.send` scopes.
//!
//! # Endpoints used
//!
//! - `POST /api/v10/channels/{channel_id}/messages` — send text
//! - `POST /api/v10/channels/{channel_id}/messages` (multipart) — send file
//! - `GET  /api/v10/channels/{channel_id}/messages/{message_id}` — fetch
//! - `GET  /api/v10/users/@me` — token validation
//!
//! # Inbound webhooks
//!
//! Discord interaction webhooks (slash commands) post JSON with an
//! Ed25519 signature over `(timestamp || body)`. We verify via
//! `ed25519-dalek`. For non-interaction events the bot uses the
//! Gateway, which is out of scope for this adapter — caller sets up a
//! Gateway client separately and pushes normalized events through
//! `normalize_inbound`.

use crate::im_channel::{AudioFormat, ImMessage, ImMessageType, ImPlatformAdapter};
use anyhow::Context;
use chrono::Utc;
use std::collections::HashMap;

const DEFAULT_API_BASE: &str = "https://discord.com/api/v10";

/// Discord bot adapter.
pub struct DiscordAdapter {
    bot_token: String,
    api_base: String,
    client: reqwest::Client,
    /// Optional Ed25519 public key for verifying interaction webhook signatures.
    /// Discord requires this for slash commands.
    public_key_hex: Option<String>,
}

impl std::fmt::Debug for DiscordAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiscordAdapter")
            .field("api_base", &self.api_base)
            .field("has_public_key", &self.public_key_hex.is_some())
            .finish()
    }
}

impl DiscordAdapter {
    pub fn new(bot_token: impl Into<String>) -> Self {
        Self {
            bot_token: bot_token.into(),
            api_base: DEFAULT_API_BASE.to_string(),
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("reqwest client"),
            public_key_hex: None,
        }
    }

    pub fn from_env() -> anyhow::Result<Self> {
        let token =
            std::env::var("DISCORD_BOT_TOKEN").context("DISCORD_BOT_TOKEN env var required")?;
        let mut adapter = Self::new(token);
        if let Ok(pk) = std::env::var("DISCORD_PUBLIC_KEY") {
            adapter.public_key_hex = Some(pk);
        }
        Ok(adapter)
    }

    pub fn with_api_base(mut self, base: impl Into<String>) -> Self {
        self.api_base = base.into();
        self
    }

    pub fn with_public_key(mut self, hex_key: impl Into<String>) -> Self {
        self.public_key_hex = Some(hex_key.into());
        self
    }

    fn auth_header_value(&self) -> String {
        format!("Bot {}", self.bot_token)
    }
}

#[async_trait::async_trait]
impl ImPlatformAdapter for DiscordAdapter {
    fn platform_id(&self) -> &str {
        "discord"
    }

    fn supports_voice(&self) -> bool {
        // Discord supports audio attachments; full voice channels are
        // gateway-only and out of scope for this REST adapter.
        true
    }

    async fn normalize_inbound(&self, raw: serde_json::Value) -> anyhow::Result<ImMessage> {
        // Discord interaction webhook payload shape:
        // { "type": 2, "data": { "name": "cmd", "options": [...] },
        //   "channel_id": "...", "member": { "user": { "id": "..." } },
        //   "id": "...", "token": "..." }
        // OR a regular message via Gateway-pushed JSON.

        let kind = raw.get("type").and_then(|v| v.as_u64()).unwrap_or(0);
        let id = raw
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let channel_id = raw
            .get("channel_id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();

        let sender = raw
            .pointer("/member/user/id")
            .or_else(|| raw.pointer("/author/id"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        let text = if kind == 2 {
            // Interaction (slash command). Concatenate command + options.
            let cmd = raw
                .pointer("/data/name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let mut buf = format!("/{cmd}");
            if let Some(opts) = raw.pointer("/data/options").and_then(|v| v.as_array()) {
                for opt in opts {
                    if let (Some(name), Some(val)) =
                        (opt.get("name").and_then(|v| v.as_str()), opt.get("value"))
                    {
                        buf.push_str(&format!(" {name}={val}"));
                    }
                }
            }
            buf
        } else {
            raw.get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };

        Ok(ImMessage {
            id,
            platform: "discord".to_string(),
            chat_id: channel_id,
            sender,
            message_type: ImMessageType::Text,
            text_content: Some(text),
            voice_media_id: None,
            voice_duration_secs: None,
            timestamp: Utc::now(),
            metadata: HashMap::new(),
            raw,
        })
    }

    async fn send_text(&self, chat_id: &str, text: &str) -> anyhow::Result<()> {
        let url = format!("{}/channels/{}/messages", self.api_base, chat_id);
        let body = serde_json::json!({ "content": text });
        let resp = self
            .client
            .post(&url)
            .header("Authorization", self.auth_header_value())
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let err_body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Discord send_text failed ({}): {}", status, err_body);
        }
        Ok(())
    }

    async fn send_voice(
        &self,
        chat_id: &str,
        audio: &[u8],
        format: AudioFormat,
    ) -> anyhow::Result<()> {
        let url = format!("{}/channels/{}/messages", self.api_base, chat_id);
        let ext = match format {
            AudioFormat::Mp3 => "mp3",
            AudioFormat::Wav => "wav",
            AudioFormat::Ogg => "ogg",
            AudioFormat::Pcm => "pcm",
            AudioFormat::Opus => "opus",
            _ => "bin",
        };
        let filename = format!("voice-{}.{}", uuid::Uuid::new_v4(), ext);

        let payload = serde_json::json!({
            "attachments": [{
                "id": 0,
                "description": "voice message",
                "filename": filename
            }]
        });

        let form = reqwest::multipart::Form::new()
            .text("payload_json", payload.to_string())
            .part(
                "files[0]",
                reqwest::multipart::Part::bytes(audio.to_vec())
                    .file_name(filename)
                    .mime_str(mime_for_format(format))?,
            );
        let resp = self
            .client
            .post(&url)
            .header("Authorization", self.auth_header_value())
            .multipart(form)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let err_body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Discord send_voice failed ({}): {}", status, err_body);
        }
        Ok(())
    }

    async fn download_voice(&self, media_id: &str) -> anyhow::Result<Vec<u8>> {
        // Discord media URLs come pre-signed on `attachment.url`. Caller
        // passes the URL as media_id directly; we do a plain GET.
        let url = if media_id.starts_with("http") {
            media_id.to_string()
        } else {
            format!("{}/{}", self.api_base, media_id)
        };
        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("Discord download_voice failed ({}): {}", resp.status(), url);
        }
        Ok(resp.bytes().await?.to_vec())
    }

    fn validate_signature(&self, payload: &[u8], signature: &str) -> bool {
        // Discord signs `(X-Signature-Timestamp || raw_body)` with Ed25519.
        // Caller must concatenate timestamp + body before passing to us.
        // For minimal viable: accept if no public key configured (dev mode),
        // verify with ed25519-dalek when configured.
        let pk_hex = match self.public_key_hex.as_deref() {
            Some(s) => s,
            None => return true, // dev mode
        };
        let pk_bytes = match hex::decode(pk_hex) {
            Ok(b) => b,
            Err(_) => return false,
        };
        let sig_bytes = match hex::decode(signature) {
            Ok(b) => b,
            Err(_) => return false,
        };
        if pk_bytes.len() != 32 || sig_bytes.len() != 64 {
            return false;
        }
        let pk_arr: [u8; 32] = match pk_bytes.try_into() {
            Ok(a) => a,
            Err(_) => return false,
        };
        let sig_arr: [u8; 64] = match sig_bytes.try_into() {
            Ok(a) => a,
            Err(_) => return false,
        };
        let pk = match ed25519_dalek::VerifyingKey::from_bytes(&pk_arr) {
            Ok(k) => k,
            Err(_) => return false,
        };
        let sig = ed25519_dalek::Signature::from_bytes(&sig_arr);
        pk.verify_strict(payload, &sig).is_ok()
    }
}

fn mime_for_format(f: AudioFormat) -> &'static str {
    match f {
        AudioFormat::Mp3 => "audio/mpeg",
        AudioFormat::Wav => "audio/wav",
        AudioFormat::Ogg => "audio/ogg",
        AudioFormat::Pcm => "audio/L16",
        AudioFormat::Opus => "audio/opus",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_header_uses_bot_prefix() {
        let a = DiscordAdapter::new("sometoken");
        assert_eq!(a.auth_header_value(), "Bot sometoken");
    }

    #[test]
    fn platform_id_is_discord() {
        let a = DiscordAdapter::new("t");
        assert_eq!(a.platform_id(), "discord");
        assert!(a.supports_voice());
        assert!(!a.supports_realtime_audio());
    }

    #[tokio::test]
    async fn normalize_inbound_text_message() {
        let a = DiscordAdapter::new("t");
        let raw = serde_json::json!({
            "id": "msg123",
            "channel_id": "chan456",
            "author": { "id": "user789" },
            "content": "hello world",
            "type": 0
        });
        let msg = a.normalize_inbound(raw.clone()).await.unwrap();
        assert_eq!(msg.platform, "discord");
        assert_eq!(msg.id, "msg123");
        assert_eq!(msg.chat_id, "chan456");
        assert_eq!(msg.sender, "user789");
        assert_eq!(msg.text_content.as_deref(), Some("hello world"));
        assert_eq!(msg.message_type, ImMessageType::Text);
    }

    #[tokio::test]
    async fn normalize_inbound_slash_command_serializes_options() {
        let a = DiscordAdapter::new("t");
        let raw = serde_json::json!({
            "id": "iact1",
            "channel_id": "c",
            "type": 2,
            "member": { "user": { "id": "u" } },
            "data": {
                "name": "echo",
                "options": [
                    {"name": "msg", "value": "hi"},
                    {"name": "n", "value": 3}
                ]
            }
        });
        let msg = a.normalize_inbound(raw).await.unwrap();
        let text = msg.text_content.unwrap();
        assert!(text.starts_with("/echo"));
        assert!(text.contains("msg=\"hi\""));
        assert!(text.contains("n=3"));
    }

    #[test]
    fn validate_signature_accepts_when_no_public_key_configured() {
        let a = DiscordAdapter::new("t");
        // Dev mode: no public key → accept anything.
        assert!(a.validate_signature(b"body", "deadbeef"));
    }

    #[test]
    fn validate_signature_rejects_invalid_hex_when_key_configured() {
        let a = DiscordAdapter::new("t").with_public_key("zz".repeat(32));
        assert!(!a.validate_signature(b"body", "00".repeat(64).as_str()));
    }

    #[test]
    fn validate_signature_rejects_wrong_length_signature() {
        let a = DiscordAdapter::new("t").with_public_key("aa".repeat(32));
        // Valid hex but wrong byte length.
        assert!(!a.validate_signature(b"body", "ff"));
    }
}
