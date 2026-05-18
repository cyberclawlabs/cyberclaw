//! Vision tools — image understanding via LLM vision endpoints.
//!
//! Mirrors Hermes v0.12 `tools/vision_tools.py`. Two backends:
//!
//! 1. **Anthropic vision** — content block `{type:"image",source:{type:"base64",...}}`
//!    via `messages.create`. Default.
//! 2. **OpenAI vision** — content array `[{type:"image_url",...}]` via
//!    `chat/completions`.
//!
//! Backend chosen via `VISION_PROVIDER` env (`anthropic` | `openai`).
//! Authentication uses the provider's existing env keys (`ANTHROPIC_API_KEY`
//! or `OPENAI_API_KEY`).
//!
//! # Risk
//!
//! Low. Read-only — sends image bytes to the configured vision API and
//! returns text. Image bytes never persist on disk inside this connector.

use super::LocalConnector;
use crate::types::CapabilityExecutionRequest;
use base64::{engine::general_purpose, Engine as _};
use serde::{Deserialize, Serialize};
use std::time::Duration;

const DEFAULT_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_MAX_TOKENS: u32 = 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisionAnalyzeInput {
    /// Either a local file path OR a remote URL OR raw base64 data.
    /// Detection: starts with `http://` / `https://` → URL; starts with `/` → file; otherwise → b64.
    pub image: String,
    /// Free-form prompt asked about the image.
    pub prompt: String,
    /// Optional MIME type hint (auto-detected from path otherwise).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// Override max_tokens for the vision call. Default 1024.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Override `VISION_PROVIDER` env (`anthropic` | `openai`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisionAnalyzeOutput {
    pub text: String,
    pub provider: String,
    pub model: String,
    pub input_tokens: Option<u32>,
    pub output_tokens: Option<u32>,
}

pub async fn analyze_image(
    _connector: &LocalConnector,
    request: CapabilityExecutionRequest,
) -> anyhow::Result<serde_json::Value> {
    let input: VisionAnalyzeInput = serde_json::from_value(request.input)?;
    let provider = input
        .provider
        .clone()
        .or_else(|| std::env::var("VISION_PROVIDER").ok())
        .unwrap_or_else(|| "anthropic".to_string());
    let max_tokens = input.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS);

    // Resolve image to (b64_data, mime_type). URLs go through the
    // provider's URL path; files + b64 are uploaded inline.
    let image_payload = resolve_image(&input.image, input.mime_type.as_deref()).await?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(DEFAULT_TIMEOUT_MS))
        .build()?;

    let output = match provider.as_str() {
        "anthropic" => call_anthropic(&client, &input.prompt, &image_payload, max_tokens).await?,
        "openai" => call_openai(&client, &input.prompt, &image_payload, max_tokens).await?,
        other => anyhow::bail!("Unknown VISION_PROVIDER: '{}'", other),
    };
    Ok(serde_json::to_value(output)?)
}

#[derive(Debug)]
enum ImagePayload {
    Url(String),
    Inline { b64: String, mime: String },
}

async fn resolve_image(image: &str, mime_hint: Option<&str>) -> anyhow::Result<ImagePayload> {
    if image.starts_with("http://") || image.starts_with("https://") {
        return Ok(ImagePayload::Url(image.to_string()));
    }
    if image.starts_with('/') {
        // Local file.
        let bytes = tokio::fs::read(image).await?;
        let mime = mime_hint
            .map(String::from)
            .or_else(|| guess_mime_from_path(image))
            .unwrap_or_else(|| "image/png".to_string());
        return Ok(ImagePayload::Inline {
            b64: general_purpose::STANDARD.encode(&bytes),
            mime,
        });
    }
    // Treat as raw b64.
    let mime = mime_hint.unwrap_or("image/png").to_string();
    Ok(ImagePayload::Inline {
        b64: image.to_string(),
        mime,
    })
}

fn guess_mime_from_path(path: &str) -> Option<String> {
    let lower = path.to_lowercase();
    if lower.ends_with(".png") {
        Some("image/png".into())
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        Some("image/jpeg".into())
    } else if lower.ends_with(".gif") {
        Some("image/gif".into())
    } else if lower.ends_with(".webp") {
        Some("image/webp".into())
    } else {
        None
    }
}

async fn call_anthropic(
    client: &reqwest::Client,
    prompt: &str,
    img: &ImagePayload,
    max_tokens: u32,
) -> anyhow::Result<VisionAnalyzeOutput> {
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| anyhow::anyhow!("ANTHROPIC_API_KEY required for vision.anthropic"))?;
    let model = std::env::var("VISION_ANTHROPIC_MODEL")
        .unwrap_or_else(|_| "claude-3-5-sonnet-20241022".to_string());
    let url = std::env::var("ANTHROPIC_BASE_URL")
        .unwrap_or_else(|_| "https://api.anthropic.com".to_string())
        + "/v1/messages";

    let image_block = match img {
        ImagePayload::Inline { b64, mime } => serde_json::json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": mime,
                "data": b64
            }
        }),
        ImagePayload::Url(u) => serde_json::json!({
            "type": "image",
            "source": { "type": "url", "url": u }
        }),
    };

    let body = serde_json::json!({
        "model": model,
        "max_tokens": max_tokens,
        "messages": [{
            "role": "user",
            "content": [
                image_block,
                {"type": "text", "text": prompt}
            ]
        }]
    });

    let resp = client
        .post(&url)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await?;
    let status = resp.status();
    let resp_json: serde_json::Value = resp.json().await?;
    if !status.is_success() {
        anyhow::bail!("Anthropic vision API failed ({}): {}", status, resp_json);
    }

    let text = resp_json
        .get("content")
        .and_then(|c| c.as_array())
        .and_then(|arr| {
            arr.iter()
                .find(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
        })
        .and_then(|b| b.get("text").and_then(|t| t.as_str()))
        .unwrap_or_default()
        .to_string();
    let usage = resp_json.get("usage");
    Ok(VisionAnalyzeOutput {
        text,
        provider: "anthropic".to_string(),
        model: resp_json
            .get("model")
            .and_then(|m| m.as_str())
            .unwrap_or(&model)
            .to_string(),
        input_tokens: usage
            .and_then(|u| u.get("input_tokens"))
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        output_tokens: usage
            .and_then(|u| u.get("output_tokens"))
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
    })
}

async fn call_openai(
    client: &reqwest::Client,
    prompt: &str,
    img: &ImagePayload,
    max_tokens: u32,
) -> anyhow::Result<VisionAnalyzeOutput> {
    let api_key = std::env::var("OPENAI_API_KEY")
        .map_err(|_| anyhow::anyhow!("OPENAI_API_KEY required for vision.openai"))?;
    let model = std::env::var("VISION_OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".to_string());
    let url = std::env::var("OPENAI_BASE_URL")
        .unwrap_or_else(|_| "https://api.openai.com/v1".to_string())
        + "/chat/completions";

    let image_url_value = match img {
        ImagePayload::Url(u) => u.clone(),
        ImagePayload::Inline { b64, mime } => format!("data:{};base64,{}", mime, b64),
    };

    let body = serde_json::json!({
        "model": model,
        "max_tokens": max_tokens,
        "messages": [{
            "role": "user",
            "content": [
                {"type": "text", "text": prompt},
                {"type": "image_url", "image_url": {"url": image_url_value}}
            ]
        }]
    });

    let resp = client
        .post(&url)
        .bearer_auth(api_key)
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await?;
    let status = resp.status();
    let resp_json: serde_json::Value = resp.json().await?;
    if !status.is_success() {
        anyhow::bail!("OpenAI vision API failed ({}): {}", status, resp_json);
    }
    let text = resp_json
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or_default()
        .to_string();
    let usage = resp_json.get("usage");
    Ok(VisionAnalyzeOutput {
        text,
        provider: "openai".to_string(),
        model: resp_json
            .get("model")
            .and_then(|m| m.as_str())
            .unwrap_or(&model)
            .to_string(),
        input_tokens: usage
            .and_then(|u| u.get("prompt_tokens"))
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        output_tokens: usage
            .and_then(|u| u.get("completion_tokens"))
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guess_mime_from_path_known_extensions() {
        assert_eq!(guess_mime_from_path("/x/y.png"), Some("image/png".into()));
        assert_eq!(guess_mime_from_path("/X.JPEG"), Some("image/jpeg".into()));
        assert_eq!(guess_mime_from_path("a.gif"), Some("image/gif".into()));
        assert_eq!(guess_mime_from_path("z.WEBP"), Some("image/webp".into()));
        assert_eq!(guess_mime_from_path("no_ext"), None);
    }

    #[tokio::test]
    async fn resolve_image_url_passthrough() {
        let p = resolve_image("https://example.com/x.png", None)
            .await
            .unwrap();
        match p {
            ImagePayload::Url(u) => assert_eq!(u, "https://example.com/x.png"),
            _ => panic!("expected Url variant"),
        }
    }

    #[tokio::test]
    async fn resolve_image_file_loads_bytes_and_guesses_mime() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.png");
        std::fs::write(&path, b"fake-png-bytes").unwrap();
        let p = resolve_image(path.to_str().unwrap(), None).await.unwrap();
        match p {
            ImagePayload::Inline { b64, mime } => {
                assert_eq!(mime, "image/png");
                assert!(!b64.is_empty());
                let decoded = general_purpose::STANDARD.decode(&b64).unwrap();
                assert_eq!(decoded, b"fake-png-bytes");
            }
            _ => panic!("expected Inline"),
        }
    }

    #[tokio::test]
    async fn resolve_image_b64_passthrough_with_mime_hint() {
        let raw_b64 = "Zm9vYmFy"; // "foobar"
        let p = resolve_image(raw_b64, Some("image/jpeg")).await.unwrap();
        match p {
            ImagePayload::Inline { b64, mime } => {
                assert_eq!(b64, raw_b64);
                assert_eq!(mime, "image/jpeg");
            }
            _ => panic!("expected Inline"),
        }
    }
}
