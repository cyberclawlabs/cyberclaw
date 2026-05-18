//! Image generation tool — DALL-E / Stability via HTTP.
//!
//! Mirrors Hermes v0.12 `tools/image_generation_tool.py`. Real HTTP call,
//! no placeholder. Output b64-decoded image is written to
//! `<workspace>/generated_images/<uuid>.{png|jpg}`.
//!
//! # Backends
//!
//! 1. **OpenAI Images** (default) — `POST /v1/images/generations` with
//!    `dall-e-3` or `dall-e-2`. Response is `{data:[{b64_json|url, revised_prompt}]}`.
//! 2. **Stability AI** — `POST /v2beta/stable-image/generate/core`. Response is
//!    image bytes directly when `Accept: image/*`.
//!
//! # Risk
//!
//! Medium — generates content + writes to workspace + network call. Costs
//! money per image (DALL-E 3 ~$0.04 each at standard quality). Capability
//! gated by review threshold.

use super::LocalConnector;
use crate::types::CapabilityExecutionRequest;
use base64::{engine::general_purpose, Engine as _};
use serde::{Deserialize, Serialize};
use std::time::Duration;

const DEFAULT_TIMEOUT_MS: u64 = 90_000; // image gen is slow
const DEFAULT_SIZE: &str = "1024x1024";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageGenerateInput {
    pub prompt: String,
    /// `1024x1024` (default), `1024x1792`, `1792x1024` for dall-e-3.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
    /// Override `IMAGE_GEN_PROVIDER` env (`openai` | `stability`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Override model. OpenAI default `dall-e-3`. Stability default `core`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// `standard` | `hd` (DALL-E 3 only). Default `standard`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageGenerateOutput {
    /// Filesystem path the image was written to (workspace-relative).
    pub path: String,
    /// Bytes written.
    pub bytes_written: u64,
    /// Revised prompt that the provider actually used (DALL-E rewrites).
    pub revised_prompt: Option<String>,
    pub provider: String,
    pub model: String,
}

pub async fn generate(
    connector: &LocalConnector,
    request: CapabilityExecutionRequest,
) -> anyhow::Result<serde_json::Value> {
    let input: ImageGenerateInput = serde_json::from_value(request.input)?;
    let provider = input
        .provider
        .clone()
        .or_else(|| std::env::var("IMAGE_GEN_PROVIDER").ok())
        .unwrap_or_else(|| "openai".to_string());

    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(DEFAULT_TIMEOUT_MS))
        .build()?;

    let (image_bytes, revised_prompt, model_used) = match provider.as_str() {
        "openai" => call_openai_images(&client, &input).await?,
        "stability" => call_stability(&client, &input).await?,
        other => anyhow::bail!("Unknown IMAGE_GEN_PROVIDER: '{}'", other),
    };

    // Write to workspace/generated_images/<uuid>.png.
    let dir = connector.workspace().join("generated_images");
    tokio::fs::create_dir_all(&dir).await?;
    let filename = format!("{}.png", uuid::Uuid::new_v4());
    let path = dir.join(&filename);
    tokio::fs::write(&path, &image_bytes).await?;

    Ok(serde_json::to_value(ImageGenerateOutput {
        path: path.to_string_lossy().to_string(),
        bytes_written: image_bytes.len() as u64,
        revised_prompt,
        provider,
        model: model_used,
    })?)
}

async fn call_openai_images(
    client: &reqwest::Client,
    input: &ImageGenerateInput,
) -> anyhow::Result<(Vec<u8>, Option<String>, String)> {
    let api_key = std::env::var("OPENAI_API_KEY")
        .map_err(|_| anyhow::anyhow!("OPENAI_API_KEY required for image_gen.openai"))?;
    let url = std::env::var("OPENAI_BASE_URL")
        .unwrap_or_else(|_| "https://api.openai.com/v1".to_string())
        + "/images/generations";
    let model = input
        .model
        .clone()
        .unwrap_or_else(|| "dall-e-3".to_string());
    let size = input.size.as_deref().unwrap_or(DEFAULT_SIZE);

    let mut body = serde_json::json!({
        "model": model,
        "prompt": input.prompt,
        "n": 1,
        "size": size,
        "response_format": "b64_json",
    });
    if model == "dall-e-3" {
        body["quality"] = serde_json::Value::String(
            input
                .quality
                .clone()
                .unwrap_or_else(|| "standard".to_string()),
        );
    }

    let resp = client
        .post(&url)
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await?;
    let status = resp.status();
    let json: serde_json::Value = resp.json().await?;
    if !status.is_success() {
        anyhow::bail!("OpenAI Images API failed ({}): {}", status, json);
    }
    let first = json
        .get("data")
        .and_then(|d| d.as_array())
        .and_then(|a| a.first())
        .ok_or_else(|| anyhow::anyhow!("OpenAI Images returned empty data array"))?;

    let bytes = if let Some(b64) = first.get("b64_json").and_then(|v| v.as_str()) {
        general_purpose::STANDARD.decode(b64)?
    } else if let Some(url) = first.get("url").and_then(|v| v.as_str()) {
        // Fallback: caller asked for url format.
        let download = reqwest::get(url).await?.bytes().await?;
        download.to_vec()
    } else {
        anyhow::bail!("OpenAI Images response missing b64_json + url");
    };

    let revised = first
        .get("revised_prompt")
        .and_then(|v| v.as_str())
        .map(String::from);
    Ok((bytes, revised, model))
}

async fn call_stability(
    client: &reqwest::Client,
    input: &ImageGenerateInput,
) -> anyhow::Result<(Vec<u8>, Option<String>, String)> {
    let api_key = std::env::var("STABILITY_API_KEY")
        .map_err(|_| anyhow::anyhow!("STABILITY_API_KEY required for image_gen.stability"))?;
    let model = input.model.clone().unwrap_or_else(|| "core".to_string());
    let url = format!(
        "https://api.stability.ai/v2beta/stable-image/generate/{}",
        model
    );

    let form = reqwest::multipart::Form::new()
        .text("prompt", input.prompt.clone())
        .text("output_format", "png")
        .text("aspect_ratio", aspect_ratio_for_size(input.size.as_deref()));

    let resp = client
        .post(&url)
        .bearer_auth(api_key)
        .header("accept", "image/*")
        .multipart(form)
        .send()
        .await?;
    let status = resp.status();
    if !status.is_success() {
        let txt = resp.text().await.unwrap_or_default();
        anyhow::bail!("Stability AI failed ({}): {}", status, txt);
    }
    let bytes = resp.bytes().await?.to_vec();
    Ok((bytes, None, model))
}

fn aspect_ratio_for_size(size: Option<&str>) -> String {
    match size.unwrap_or(DEFAULT_SIZE) {
        "1024x1024" => "1:1",
        "1024x1792" => "9:16",
        "1792x1024" => "16:9",
        "1024x576" => "16:9",
        "576x1024" => "9:16",
        _ => "1:1",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aspect_ratio_for_size_known_values() {
        assert_eq!(aspect_ratio_for_size(Some("1024x1024")), "1:1");
        assert_eq!(aspect_ratio_for_size(Some("1024x1792")), "9:16");
        assert_eq!(aspect_ratio_for_size(Some("1792x1024")), "16:9");
        assert_eq!(aspect_ratio_for_size(None), "1:1");
        assert_eq!(aspect_ratio_for_size(Some("weird")), "1:1");
    }

    #[test]
    fn input_deserializes_minimal() {
        let json = r#"{"prompt": "a red cat"}"#;
        let v: ImageGenerateInput = serde_json::from_str(json).expect("parse");
        assert_eq!(v.prompt, "a red cat");
        assert!(v.size.is_none());
        assert!(v.provider.is_none());
    }

    #[test]
    fn input_deserializes_full() {
        let json = r#"{
            "prompt": "X",
            "size": "1024x1792",
            "provider": "stability",
            "model": "ultra",
            "quality": "hd"
        }"#;
        let v: ImageGenerateInput = serde_json::from_str(json).expect("parse");
        assert_eq!(v.size.as_deref(), Some("1024x1792"));
        assert_eq!(v.provider.as_deref(), Some("stability"));
        assert_eq!(v.model.as_deref(), Some("ultra"));
        assert_eq!(v.quality.as_deref(), Some("hd"));
    }

    #[test]
    fn output_serializes_with_optional_revised_prompt() {
        let out = ImageGenerateOutput {
            path: "/tmp/x.png".to_string(),
            bytes_written: 100,
            revised_prompt: Some("a beautiful red cat".to_string()),
            provider: "openai".to_string(),
            model: "dall-e-3".to_string(),
        };
        let v = serde_json::to_value(&out).unwrap();
        assert_eq!(v["path"], "/tmp/x.png");
        assert_eq!(v["bytes_written"], 100);
        assert!(v["revised_prompt"].as_str().is_some());
    }
}
