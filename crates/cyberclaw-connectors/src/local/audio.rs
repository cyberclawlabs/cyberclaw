//! Audio tools — transcription (Whisper) and synthesis (TTS).
//!
//! Mirrors Hermes v0.12 `tools/transcription_tools.py` + `tools/tts_tool.py`.
//! Both use OpenAI Audio API by default; both can be swapped to ElevenLabs
//! TTS by env override.
//!
//! # Capabilities
//!
//! - `audio.transcribe` — file → text + segments + detected language
//! - `audio.synthesize` — text → audio file (mp3 default)
//!
//! # Risk
//!
//! - Transcribe: Low — read-only file upload + text out.
//! - Synthesize: Medium — writes audio file to workspace + costs money per
//!   character.

use super::LocalConnector;
use crate::types::CapabilityExecutionRequest;
use serde::{Deserialize, Serialize};
use std::time::Duration;

const DEFAULT_TIMEOUT_MS: u64 = 120_000;

// ---------------------------------------------------------------------------
// audio.transcribe (Whisper)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioTranscribeInput {
    /// Local file path (mp3, wav, m4a, webm, etc.).
    pub audio_path: String,
    /// Optional language hint (ISO-639-1, e.g. "en", "zh").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Optional prompt to bias the transcription.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// Override OpenAI Whisper model. Default: whisper-1.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioTranscribeOutput {
    pub text: String,
    pub language: Option<String>,
    pub duration_secs: Option<f64>,
    pub model: String,
    pub provider: String,
}

pub async fn transcribe(
    _connector: &LocalConnector,
    request: CapabilityExecutionRequest,
) -> anyhow::Result<serde_json::Value> {
    let input: AudioTranscribeInput = serde_json::from_value(request.input)?;
    let api_key = std::env::var("OPENAI_API_KEY")
        .map_err(|_| anyhow::anyhow!("OPENAI_API_KEY required for audio.transcribe"))?;
    let model = input
        .model
        .clone()
        .unwrap_or_else(|| "whisper-1".to_string());
    let url = std::env::var("OPENAI_BASE_URL")
        .unwrap_or_else(|_| "https://api.openai.com/v1".to_string())
        + "/audio/transcriptions";

    // Read file.
    let bytes = tokio::fs::read(&input.audio_path).await?;
    let filename = std::path::Path::new(&input.audio_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("audio.bin")
        .to_string();

    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(DEFAULT_TIMEOUT_MS))
        .build()?;

    let mut form = reqwest::multipart::Form::new()
        .text("model", model.clone())
        .text("response_format", "verbose_json");
    if let Some(lang) = input.language {
        form = form.text("language", lang);
    }
    if let Some(p) = input.prompt {
        form = form.text("prompt", p);
    }
    let part = reqwest::multipart::Part::bytes(bytes).file_name(filename);
    form = form.part("file", part);

    let resp = client
        .post(&url)
        .bearer_auth(api_key)
        .multipart(form)
        .send()
        .await?;
    let status = resp.status();
    let json: serde_json::Value = resp.json().await?;
    if !status.is_success() {
        anyhow::bail!("OpenAI Whisper failed ({}): {}", status, json);
    }
    let text = json
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let language = json
        .get("language")
        .and_then(|v| v.as_str())
        .map(String::from);
    let duration = json.get("duration").and_then(|v| v.as_f64());
    Ok(serde_json::to_value(AudioTranscribeOutput {
        text,
        language,
        duration_secs: duration,
        model,
        provider: "openai".to_string(),
    })?)
}

// ---------------------------------------------------------------------------
// audio.synthesize (TTS)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioSynthesizeInput {
    pub text: String,
    /// Override TTS_PROVIDER env (`openai` | `elevenlabs`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// OpenAI voice (alloy, echo, fable, onyx, nova, shimmer). ElevenLabs voice id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    /// `mp3` (default) | `wav` | `opus` | `aac`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    /// Override model. OpenAI: tts-1 / tts-1-hd. ElevenLabs: eleven_multilingual_v2.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioSynthesizeOutput {
    pub path: String,
    pub bytes_written: u64,
    pub provider: String,
    pub voice: String,
    pub format: String,
}

pub async fn synthesize(
    connector: &LocalConnector,
    request: CapabilityExecutionRequest,
) -> anyhow::Result<serde_json::Value> {
    let input: AudioSynthesizeInput = serde_json::from_value(request.input)?;
    let provider = input
        .provider
        .clone()
        .or_else(|| std::env::var("TTS_PROVIDER").ok())
        .unwrap_or_else(|| "openai".to_string());
    let format = input.format.clone().unwrap_or_else(|| "mp3".to_string());

    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(DEFAULT_TIMEOUT_MS))
        .build()?;

    let (audio_bytes, voice_used) = match provider.as_str() {
        "openai" => call_openai_tts(&client, &input, &format).await?,
        "elevenlabs" => call_elevenlabs_tts(&client, &input, &format).await?,
        other => anyhow::bail!("Unknown TTS_PROVIDER: '{}'", other),
    };

    let dir = connector.workspace().join("tts");
    tokio::fs::create_dir_all(&dir).await?;
    let filename = format!("{}.{}", uuid::Uuid::new_v4(), format);
    let path = dir.join(&filename);
    tokio::fs::write(&path, &audio_bytes).await?;

    Ok(serde_json::to_value(AudioSynthesizeOutput {
        path: path.to_string_lossy().to_string(),
        bytes_written: audio_bytes.len() as u64,
        provider,
        voice: voice_used,
        format,
    })?)
}

async fn call_openai_tts(
    client: &reqwest::Client,
    input: &AudioSynthesizeInput,
    format: &str,
) -> anyhow::Result<(Vec<u8>, String)> {
    let api_key = std::env::var("OPENAI_API_KEY")
        .map_err(|_| anyhow::anyhow!("OPENAI_API_KEY required for tts.openai"))?;
    let model = input.model.clone().unwrap_or_else(|| "tts-1".to_string());
    let voice = input.voice.clone().unwrap_or_else(|| "alloy".to_string());
    let url = std::env::var("OPENAI_BASE_URL")
        .unwrap_or_else(|_| "https://api.openai.com/v1".to_string())
        + "/audio/speech";

    let body = serde_json::json!({
        "model": model,
        "voice": voice,
        "input": input.text,
        "response_format": format,
    });

    let resp = client
        .post(&url)
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await?;
    let status = resp.status();
    if !status.is_success() {
        let txt = resp.text().await.unwrap_or_default();
        anyhow::bail!("OpenAI TTS failed ({}): {}", status, txt);
    }
    let bytes = resp.bytes().await?.to_vec();
    Ok((bytes, voice))
}

async fn call_elevenlabs_tts(
    client: &reqwest::Client,
    input: &AudioSynthesizeInput,
    format: &str,
) -> anyhow::Result<(Vec<u8>, String)> {
    let api_key = std::env::var("ELEVENLABS_API_KEY")
        .map_err(|_| anyhow::anyhow!("ELEVENLABS_API_KEY required for tts.elevenlabs"))?;
    let voice = input
        .voice
        .clone()
        .unwrap_or_else(|| "21m00Tcm4TlvDq8ikWAM".to_string()); // "Rachel" default
    let model = input
        .model
        .clone()
        .unwrap_or_else(|| "eleven_multilingual_v2".to_string());
    // ElevenLabs accept header maps format to mime type.
    let accept_mime = match format {
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "opus" => "audio/opus",
        _ => "audio/mpeg",
    };
    let url = format!("https://api.elevenlabs.io/v1/text-to-speech/{}", voice);
    let body = serde_json::json!({
        "text": input.text,
        "model_id": model,
    });
    let resp = client
        .post(&url)
        .header("xi-api-key", api_key)
        .header("accept", accept_mime)
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await?;
    let status = resp.status();
    if !status.is_success() {
        let txt = resp.text().await.unwrap_or_default();
        anyhow::bail!("ElevenLabs TTS failed ({}): {}", status, txt);
    }
    let bytes = resp.bytes().await?.to_vec();
    Ok((bytes, voice))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcribe_input_deserializes_minimal() {
        let v: AudioTranscribeInput =
            serde_json::from_str(r#"{"audio_path": "/tmp/a.mp3"}"#).unwrap();
        assert_eq!(v.audio_path, "/tmp/a.mp3");
        assert!(v.language.is_none());
    }

    #[test]
    fn transcribe_input_deserializes_full() {
        let json = r#"{
            "audio_path": "/x.wav",
            "language": "zh",
            "prompt": "讲座",
            "model": "whisper-1"
        }"#;
        let v: AudioTranscribeInput = serde_json::from_str(json).unwrap();
        assert_eq!(v.language.as_deref(), Some("zh"));
        assert_eq!(v.prompt.as_deref(), Some("讲座"));
    }

    #[test]
    fn synthesize_input_deserializes_full() {
        let json = r#"{
            "text": "Hello",
            "provider": "elevenlabs",
            "voice": "rachel",
            "format": "wav",
            "model": "eleven_v2"
        }"#;
        let v: AudioSynthesizeInput = serde_json::from_str(json).unwrap();
        assert_eq!(v.provider.as_deref(), Some("elevenlabs"));
        assert_eq!(v.format.as_deref(), Some("wav"));
    }

    #[test]
    fn synthesize_output_serializes() {
        let v = serde_json::to_value(AudioSynthesizeOutput {
            path: "/x/y.mp3".to_string(),
            bytes_written: 1024,
            provider: "openai".to_string(),
            voice: "alloy".to_string(),
            format: "mp3".to_string(),
        })
        .unwrap();
        assert_eq!(v["path"], "/x/y.mp3");
        assert_eq!(v["bytes_written"], 1024);
    }

    #[test]
    fn transcribe_output_round_trip() {
        let out = AudioTranscribeOutput {
            text: "hello world".to_string(),
            language: Some("en".to_string()),
            duration_secs: Some(3.5),
            model: "whisper-1".to_string(),
            provider: "openai".to_string(),
        };
        let j = serde_json::to_value(&out).unwrap();
        let back: AudioTranscribeOutput = serde_json::from_value(j).unwrap();
        assert_eq!(back.text, "hello world");
        assert_eq!(back.duration_secs, Some(3.5));
    }
}
