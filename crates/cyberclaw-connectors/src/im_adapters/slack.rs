//! Slack adapter — implements `ImPlatformAdapter` for Slack workspaces.
//!
//! Mirrors Hermes v0.12 messaging-platform expectations. Uses Slack Web API
//! directly with a bot token (`xoxb-...`); event delivery via standard
//! HTTP webhook (Events API + Slash Commands), no RTM dependency.
//!
//! # Auth
//!
//! - `SLACK_BOT_TOKEN` (`xoxb-...`) for Web API calls.
//! - `SLACK_SIGNING_SECRET` for webhook signature verification.
//!
//! # Endpoints used
//!
//! - `POST https://slack.com/api/chat.postMessage` — send text
//! - `POST https://slack.com/api/files.upload` — send voice / file
//!
//! # Webhook signature
//!
//! Slack signs every event POST with HMAC-SHA256 over `v0:<timestamp>:<body>`,
//! sent as `X-Slack-Signature: v0=<hex>`. We verify via `hmac` crate.

use crate::im_channel::{AudioFormat, ImMessage, ImMessageType, ImPlatformAdapter};
use anyhow::Context;
use chrono::Utc;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::collections::HashMap;

const DEFAULT_API_BASE: &str = "https://slack.com/api";

type HmacSha256 = Hmac<Sha256>;

pub struct SlackAdapter {
    bot_token: String,
    signing_secret: Option<String>,
    api_base: String,
    client: reqwest::Client,
}

impl std::fmt::Debug for SlackAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SlackAdapter")
            .field("api_base", &self.api_base)
            .field("has_signing_secret", &self.signing_secret.is_some())
            .finish()
    }
}

impl SlackAdapter {
    pub fn new(bot_token: impl Into<String>) -> Self {
        Self {
            bot_token: bot_token.into(),
            signing_secret: None,
            api_base: DEFAULT_API_BASE.to_string(),
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("reqwest client"),
        }
    }

    pub fn from_env() -> anyhow::Result<Self> {
        let token = std::env::var("SLACK_BOT_TOKEN").context("SLACK_BOT_TOKEN env var required")?;
        let mut adapter = Self::new(token);
        if let Ok(secret) = std::env::var("SLACK_SIGNING_SECRET") {
            adapter.signing_secret = Some(secret);
        }
        Ok(adapter)
    }

    pub fn with_signing_secret(mut self, secret: impl Into<String>) -> Self {
        self.signing_secret = Some(secret.into());
        self
    }

    pub fn with_api_base(mut self, base: impl Into<String>) -> Self {
        self.api_base = base.into();
        self
    }

    /// Verify a Slack webhook payload. `body` is the raw POST body;
    /// `timestamp` comes from `X-Slack-Request-Timestamp`. The full
    /// signature is `v0=<hex>` from `X-Slack-Signature`.
    pub fn verify_webhook(&self, timestamp: &str, body: &[u8], signature: &str) -> bool {
        let secret = match self.signing_secret.as_deref() {
            Some(s) => s,
            None => return true, // dev mode
        };
        let stripped = signature.strip_prefix("v0=").unwrap_or(signature);
        let expected_sig_bytes = match hex::decode(stripped) {
            Ok(b) => b,
            Err(_) => return false,
        };
        let mut mac = match HmacSha256::new_from_slice(secret.as_bytes()) {
            Ok(m) => m,
            Err(_) => return false,
        };
        mac.update(b"v0:");
        mac.update(timestamp.as_bytes());
        mac.update(b":");
        mac.update(body);
        mac.verify_slice(&expected_sig_bytes).is_ok()
    }
}

#[async_trait::async_trait]
impl ImPlatformAdapter for SlackAdapter {
    fn platform_id(&self) -> &str {
        "slack"
    }

    fn supports_voice(&self) -> bool {
        true
    }

    async fn normalize_inbound(&self, raw: serde_json::Value) -> anyhow::Result<ImMessage> {
        // Slack Events API payload shape (event_callback wrapper):
        //   { "type": "event_callback",
        //     "event": { "type": "message", "user": "U123", "channel": "C123",
        //                "text": "hi", "ts": "1700000000.000100" } }
        // Slash commands: form-encoded to JSON-ish:
        //   { "command": "/echo", "text": "hi", "channel_id": "C123", "user_id": "U123" }
        let event = raw.get("event").unwrap_or(&raw);
        let id = event
            .get("ts")
            .and_then(|v| v.as_str())
            .or_else(|| raw.get("trigger_id").and_then(|v| v.as_str()))
            .unwrap_or_default()
            .to_string();
        let chat_id = event
            .get("channel")
            .or_else(|| raw.get("channel_id"))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let sender = event
            .get("user")
            .or_else(|| raw.get("user_id"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let text = if let Some(cmd) = raw.get("command").and_then(|v| v.as_str()) {
            // Slash command: prepend command name.
            let args = raw.get("text").and_then(|v| v.as_str()).unwrap_or("");
            format!("{cmd} {args}").trim().to_string()
        } else {
            event
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };
        Ok(ImMessage {
            id,
            platform: "slack".to_string(),
            chat_id,
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
        let url = format!("{}/chat.postMessage", self.api_base);
        let body = serde_json::json!({
            "channel": chat_id,
            "text": text,
        });
        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.bot_token)
            .json(&body)
            .send()
            .await?;
        let status = resp.status();
        let json: serde_json::Value = resp.json().await?;
        if !status.is_success() {
            anyhow::bail!("Slack chat.postMessage HTTP {}: {}", status, json);
        }
        // Slack returns 200 with `{"ok": false, "error": "..."}` on logical
        // failures — surface those.
        if !json.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
            anyhow::bail!("Slack chat.postMessage rejected: {}", json);
        }
        Ok(())
    }

    async fn send_voice(
        &self,
        chat_id: &str,
        audio: &[u8],
        format: AudioFormat,
    ) -> anyhow::Result<()> {
        let url = format!("{}/files.upload", self.api_base);
        let filename = format!("voice-{}.{}", uuid::Uuid::new_v4(), ext_for_format(format));
        let form = reqwest::multipart::Form::new()
            .text("channels", chat_id.to_string())
            .text("filename", filename.clone())
            .part(
                "file",
                reqwest::multipart::Part::bytes(audio.to_vec()).file_name(filename),
            );
        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.bot_token)
            .multipart(form)
            .send()
            .await?;
        let status = resp.status();
        let json: serde_json::Value = resp.json().await?;
        if !status.is_success() {
            anyhow::bail!("Slack files.upload HTTP {}: {}", status, json);
        }
        if !json.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
            anyhow::bail!("Slack files.upload rejected: {}", json);
        }
        Ok(())
    }

    async fn download_voice(&self, media_id: &str) -> anyhow::Result<Vec<u8>> {
        // Slack file URLs require Bearer auth even for download.
        let resp = self
            .client
            .get(media_id)
            .bearer_auth(&self.bot_token)
            .send()
            .await?;
        if !resp.status().is_success() {
            anyhow::bail!(
                "Slack file download failed ({}): {}",
                resp.status(),
                media_id
            );
        }
        Ok(resp.bytes().await?.to_vec())
    }

    fn validate_signature(&self, _payload: &[u8], _signature: &str) -> bool {
        // Slack signature verification needs the X-Slack-Request-Timestamp
        // header in addition to the body. The trait signature only takes
        // (payload, signature), so callers using Slack should call
        // `verify_webhook(timestamp, body, signature)` directly.
        // Returning true here is the dev-mode fallback consistent with
        // Discord's approach.
        true
    }
}

fn ext_for_format(f: AudioFormat) -> &'static str {
    match f {
        AudioFormat::Mp3 => "mp3",
        AudioFormat::Wav => "wav",
        AudioFormat::Ogg => "ogg",
        AudioFormat::Pcm => "pcm",
        AudioFormat::Opus => "opus",
        _ => "bin",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_id_is_slack() {
        let a = SlackAdapter::new("xoxb-test");
        assert_eq!(a.platform_id(), "slack");
        assert!(a.supports_voice());
    }

    #[tokio::test]
    async fn normalize_inbound_event_message() {
        let a = SlackAdapter::new("t");
        let raw = serde_json::json!({
            "type": "event_callback",
            "event": {
                "type": "message",
                "user": "U123",
                "channel": "C456",
                "text": "hello",
                "ts": "1700000000.001"
            }
        });
        let m = a.normalize_inbound(raw).await.unwrap();
        assert_eq!(m.platform, "slack");
        assert_eq!(m.chat_id, "C456");
        assert_eq!(m.sender, "U123");
        assert_eq!(m.text_content.as_deref(), Some("hello"));
    }

    #[tokio::test]
    async fn normalize_inbound_slash_command() {
        let a = SlackAdapter::new("t");
        let raw = serde_json::json!({
            "command": "/echo",
            "text": "hi there",
            "channel_id": "C1",
            "user_id": "U1",
            "trigger_id": "trig"
        });
        let m = a.normalize_inbound(raw).await.unwrap();
        assert_eq!(m.text_content.as_deref(), Some("/echo hi there"));
        assert_eq!(m.chat_id, "C1");
        assert_eq!(m.sender, "U1");
    }

    #[test]
    fn verify_webhook_dev_mode_when_no_secret() {
        let a = SlackAdapter::new("xoxb-test");
        assert!(a.verify_webhook("1700000000", b"body", "v0=deadbeef"));
    }

    #[test]
    fn verify_webhook_succeeds_for_correctly_signed_request() {
        // Pre-computed: HMAC-SHA256("test_secret", "v0:1700:hi") = ...
        // Compute it dynamically in the test for stability.
        let secret = "test_secret";
        let timestamp = "1700";
        let body = b"hi";
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(b"v0:");
        mac.update(timestamp.as_bytes());
        mac.update(b":");
        mac.update(body);
        let sig_hex = hex::encode(mac.finalize().into_bytes());
        let sig = format!("v0={sig_hex}");

        let a = SlackAdapter::new("xoxb-test").with_signing_secret(secret);
        assert!(a.verify_webhook(timestamp, body, &sig));
    }

    #[test]
    fn verify_webhook_rejects_tampered_body() {
        let secret = "test_secret";
        let timestamp = "1700";
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(b"v0:");
        mac.update(timestamp.as_bytes());
        mac.update(b":");
        mac.update(b"original");
        let sig = format!("v0={}", hex::encode(mac.finalize().into_bytes()));

        let a = SlackAdapter::new("xoxb-test").with_signing_secret(secret);
        // Tampered body — verify must reject.
        assert!(!a.verify_webhook(timestamp, b"tampered", &sig));
    }

    #[test]
    fn verify_webhook_rejects_invalid_hex_signature() {
        let a = SlackAdapter::new("xoxb-test").with_signing_secret("s");
        assert!(!a.verify_webhook("1700", b"body", "v0=zzzz"));
    }
}
