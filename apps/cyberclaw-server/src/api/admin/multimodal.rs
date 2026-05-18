//! Admin Multimodal endpoints — vision, image-gen, audio transcribe/synthesize.
//!
//! | Method | Path                                                | Underlying capability      |
//! |--------|-----------------------------------------------------|----------------------------|
//! | POST   | `/api/v1/admin/multimodal/vision/analyze`           | `local::vision.analyze_image` |
//! | POST   | `/api/v1/admin/multimodal/image/generate`           | `local::image.generate`       |
//! | POST   | `/api/v1/admin/multimodal/audio/transcribe`         | `local::audio.transcribe`     |
//! | POST   | `/api/v1/admin/multimodal/audio/synthesize`         | `local::audio.synthesize`     |
//!
//! Forwards through the production `OrchestratorGateway`
//! (`build_governing_gateway`) so PolicyEngine governs every dispatch —
//! admin REST MUST NOT call `connector.execute()` directly. The
//! underlying capabilities write image / audio artifacts to the
//! connector workspace as files; this layer reads each file back (under a
//! whitelist root) and base64-encodes it for HTTP egress so the admin SPA
//! does not need disk access.

use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use axum::{extract::State, routing::post, Json, Router};
use base64::{engine::general_purpose, Engine as _};
use cyberclaw_core::gateway::{CapabilityRequest, GatewayError};
use cyberclaw_core::identity::{ActorRef, ActorType};
use cyberclaw_core::ids::{ActorId, CapabilityId, ConnectorId, ExecutionId};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::admin::browser::read_file_b64_within;
use crate::api::chat_handler::build_governing_gateway;
use crate::error::ApiError;
use crate::state::AppState;

const LOCAL_CONNECTOR_ID: &str = "local";
const ADMIN_MULTIMODAL_WORKSPACE_ROOT: &str = "/tmp/admin-multimodal";
const ADMIN_AUDIO_TEMP_ROOT_NAME: &str = "cyberclaw-admin-audio";
/// Hard cap on remote image fetch size for vision_analyze. 20 MB matches
/// the OpenAI / Anthropic image-input ceiling.
const VISION_REMOTE_FETCH_MAX_BYTES: u64 = 20 * 1024 * 1024;
const VISION_REMOTE_FETCH_TIMEOUT_SECS: u64 = 10;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn ensure_workspace_root(path: &str) -> Result<(), ApiError> {
    use std::fs::DirBuilder;
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)
            .map_err(|e| ApiError::InternalError(format!("create workspace: {e}")))?;
    }
    #[cfg(not(unix))]
    {
        DirBuilder::new()
            .recursive(true)
            .create(path)
            .map_err(|e| ApiError::InternalError(format!("create workspace: {e}")))?;
    }
    Ok(())
}

async fn dispatch(
    state: &Arc<AppState>,
    capability: &str,
    input: Value,
) -> Result<Value, ApiError> {
    // C-1 fix: governance flows through the production gateway, not a
    // direct connector.execute() call.
    let connector_id = ConnectorId::from_string(LOCAL_CONNECTOR_ID.to_string())
        .map_err(|e| ApiError::InternalError(format!("connector id: {e}")))?;
    if state.connector_registry.get(&connector_id).is_none() {
        return Err(ApiError::NotFound(format!(
            "local connector is not registered (capability '{}' unavailable)",
            capability
        )));
    }

    let capability_id = CapabilityId::from_string(capability.to_string())
        .map_err(|e| ApiError::InvalidInput(format!("capability id: {e}")))?;

    let actor_id = ActorId::from_string("admin".to_string())
        .map_err(|e| ApiError::InternalError(format!("actor id: {e}")))?;
    let actor = ActorRef {
        id: actor_id,
        actor_type: ActorType::Human,
        tenant_id: None,
        home_node_id: None,
        display_name: "admin".to_string(),
    };

    let gateway = build_governing_gateway(state);
    let request = CapabilityRequest {
        execution_id: ExecutionId::new(),
        requested_by: actor,
        capability_id,
        connector_id,
        input,
        reason: format!("admin-multimodal:{}", capability),
    };

    match gateway.execute_capability(request).await {
        Ok(result) => Ok(result.output),
        Err(GatewayError::GovernanceDenied(reason)) => Err(ApiError::Forbidden(reason)),
        Err(GatewayError::ReviewRequired(reason)) => {
            Err(ApiError::Forbidden(format!("review required: {}", reason)))
        }
        Err(GatewayError::CapabilityNotFound(reason)) => Err(ApiError::NotFound(reason)),
        Err(GatewayError::ConnectorError(reason)) => Err(ApiError::InternalError(format!(
            "{} failed: {}",
            capability, reason
        ))),
        Err(GatewayError::Internal(reason)) => Err(ApiError::InternalError(reason)),
    }
}

// ---------------------------------------------------------------------------
// Vision
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct VisionAnalyzeRequest {
    #[serde(default)]
    pub image_b64: Option<String>,
    #[serde(default)]
    pub image_url: Option<String>,
    pub prompt: String,
    #[serde(default)]
    pub provider: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct VisionAnalyzeResponse {
    pub text: String,
    pub provider: String,
    pub latency_ms: u64,
}

/// Validate that an outbound image URL is safe to fetch from server side.
/// C-3: we MUST NOT pass the URL through unvetted because the underlying
/// capability auto-detects `file://` and arbitrary HTTP, which lets an
/// admin-token holder read local files (`file:///etc/passwd`) or hit
/// internal AWS metadata (`http://169.254.169.254/...`).
///
/// Rules:
///   - URL parses
///   - scheme must be `https` (plain `http` rejected — even loopback)
///   - host must NOT be loopback / private (RFC1918) / link-local /
///     unspecified (0.0.0.0)
fn validate_external_image_url(raw: &str) -> Result<url::Url, ApiError> {
    let parsed = url::Url::parse(raw)
        .map_err(|e| ApiError::InvalidInput(format!("image_url parse: {e}")))?;
    if parsed.scheme() != "https" {
        return Err(ApiError::InvalidInput(format!(
            "image_url must use https scheme, got '{}'",
            parsed.scheme()
        )));
    }
    let host = parsed
        .host()
        .ok_or_else(|| ApiError::InvalidInput("image_url missing host".to_string()))?;
    match host {
        url::Host::Domain(d) => {
            let lower = d.to_ascii_lowercase();
            if lower == "localhost" || lower.ends_with(".localhost") || lower == "metadata" {
                return Err(ApiError::InvalidInput(format!(
                    "image_url host '{}' is not allowed",
                    lower
                )));
            }
        }
        url::Host::Ipv4(ip) => {
            if ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_broadcast()
                || ip.is_documentation()
                || ip.is_multicast()
            {
                return Err(ApiError::InvalidInput(format!(
                    "image_url host {} is in a reserved range",
                    ip
                )));
            }
        }
        url::Host::Ipv6(ip) => {
            if ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() {
                return Err(ApiError::InvalidInput(format!(
                    "image_url host {} is in a reserved range",
                    ip
                )));
            }
            // ULA fc00::/7 + link-local fe80::/10 — reject by segment.
            let segs = ip.segments();
            if (segs[0] & 0xfe00) == 0xfc00 || (segs[0] & 0xffc0) == 0xfe80 {
                return Err(ApiError::InvalidInput(format!(
                    "image_url host {} is in a reserved range",
                    ip
                )));
            }
        }
    }
    Ok(parsed)
}

/// Fetch a remote image as bytes, capped at `VISION_REMOTE_FETCH_MAX_BYTES`.
async fn fetch_remote_image_b64(url: &url::Url) -> Result<String, ApiError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(VISION_REMOTE_FETCH_TIMEOUT_SECS))
        .redirect(reqwest::redirect::Policy::limited(3))
        .no_proxy()
        .build()
        .map_err(|e| ApiError::InternalError(format!("http client: {e}")))?;
    let resp = client
        .get(url.as_str())
        .send()
        .await
        .map_err(|e| ApiError::InvalidInput(format!("image_url fetch failed: {e}")))?
        .error_for_status()
        .map_err(|e| ApiError::InvalidInput(format!("image_url fetch status: {e}")))?;
    if let Some(len) = resp.content_length() {
        if len > VISION_REMOTE_FETCH_MAX_BYTES {
            return Err(ApiError::InvalidInput(format!(
                "image_url body too large: {} bytes (max {})",
                len, VISION_REMOTE_FETCH_MAX_BYTES
            )));
        }
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| ApiError::InvalidInput(format!("image_url body read: {e}")))?;
    if bytes.len() as u64 > VISION_REMOTE_FETCH_MAX_BYTES {
        return Err(ApiError::InvalidInput(format!(
            "image_url body too large: {} bytes (max {})",
            bytes.len(),
            VISION_REMOTE_FETCH_MAX_BYTES
        )));
    }
    Ok(general_purpose::STANDARD.encode(&bytes))
}

pub async fn vision_analyze(
    State(state): State<Arc<AppState>>,
    Json(req): Json<VisionAnalyzeRequest>,
) -> Result<Json<VisionAnalyzeResponse>, ApiError> {
    if req.prompt.trim().is_empty() {
        return Err(ApiError::InvalidInput(
            "prompt must not be empty".to_string(),
        ));
    }
    // C-3: never forward `image_url` straight to the underlying capability
    // (which auto-detects file://, http loopback, etc.). Either we fetch
    // it from the public Internet under https + a non-private host, or
    // we accept inline base64 that the caller already decoded.
    let image_b64 = match (req.image_url.as_ref(), req.image_b64.as_ref()) {
        (Some(url_raw), _) if !url_raw.is_empty() => {
            let parsed = validate_external_image_url(url_raw)?;
            fetch_remote_image_b64(&parsed).await?
        }
        (_, Some(b64)) if !b64.is_empty() => b64.clone(),
        _ => {
            return Err(ApiError::InvalidInput(
                "either image_url or image_b64 must be provided".to_string(),
            ))
        }
    };
    let provider = req.provider.unwrap_or_else(|| "anthropic".to_string());
    // The underlying capability accepts a single `image` field that
    // auto-detects url/file/b64. We always feed it base64 — that
    // collapses the auto-detect attack surface to a single shape.
    let input = serde_json::json!({
        "image": image_b64,
        "prompt": req.prompt,
        "provider": provider,
    });

    let started = Instant::now();
    let output = dispatch(&state, "vision.analyze_image", input).await?;
    let latency_ms = started.elapsed().as_millis() as u64;

    Ok(Json(VisionAnalyzeResponse {
        text: output
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        provider: output
            .get("provider")
            .and_then(|v| v.as_str())
            .unwrap_or(&provider)
            .to_string(),
        latency_ms,
    }))
}

// ---------------------------------------------------------------------------
// Image generate
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ImageGenerateRequest {
    pub prompt: String,
    #[serde(default)]
    pub size: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ImageGenerateResponse {
    pub b64: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revised_prompt: Option<String>,
}

pub async fn image_generate(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ImageGenerateRequest>,
) -> Result<Json<ImageGenerateResponse>, ApiError> {
    if req.prompt.trim().is_empty() {
        return Err(ApiError::InvalidInput(
            "prompt must not be empty".to_string(),
        ));
    }
    let mut input = serde_json::json!({ "prompt": req.prompt });
    if let Some(size) = req.size {
        input["size"] = Value::String(size);
    }
    if let Some(provider) = req.provider {
        input["provider"] = Value::String(provider);
    }

    let output = dispatch(&state, "image.generate", input).await?;
    let path = output
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::InternalError("image.generate missing path".to_string()))?
        .to_string();
    // H-5: only read back paths that live under our pinned workspace root.
    let b64 = read_file_b64_within(&path, ADMIN_MULTIMODAL_WORKSPACE_ROOT)
        .await?
        .ok_or_else(|| {
            ApiError::InvalidInput(format!(
                "image.generate path '{}' is outside the admin workspace",
                path
            ))
        })?;
    let revised_prompt = output
        .get("revised_prompt")
        .and_then(|v| v.as_str())
        .map(String::from);
    Ok(Json(ImageGenerateResponse {
        b64,
        path,
        revised_prompt,
    }))
}

// ---------------------------------------------------------------------------
// Audio transcribe
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct AudioTranscribeRequest {
    /// Base64-encoded audio body (the only accepted upload path — admin
    /// REST never reads server-side filesystem paths under operator
    /// control). C-2.
    #[serde(default)]
    pub audio_b64: Option<String>,
    /// MIME type hint (informational; underlying Whisper accepts everything).
    #[serde(default)]
    pub mime: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AudioSegment {
    pub start: f32,
    pub end: f32,
    pub text: String,
}

#[derive(Debug, Serialize)]
pub struct AudioTranscribeResponse {
    pub text: String,
    pub segments: Vec<AudioSegment>,
    pub language: String,
}

pub async fn audio_transcribe(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AudioTranscribeRequest>,
) -> Result<Json<AudioTranscribeResponse>, ApiError> {
    // C-2 fix: ONLY accept inline audio_b64. The previous version
    // accepted `audio_path` and forwarded it straight to the Whisper
    // capability — admin JWT could read /etc/shadow, ~/.ssh/id_rsa,
    // etc. via that path.
    let Some(b64) = req.audio_b64.as_ref().filter(|s| !s.is_empty()) else {
        return Err(ApiError::InvalidInput(
            "audio_b64 must be provided".to_string(),
        ));
    };

    let bytes = general_purpose::STANDARD
        .decode(b64.as_bytes())
        .map_err(|e| ApiError::InvalidInput(format!("audio_b64 decode: {e}")))?;
    let ext = req.mime.as_deref().map(audio_ext_for_mime).unwrap_or("bin");
    let dir = std::env::temp_dir().join(ADMIN_AUDIO_TEMP_ROOT_NAME);
    ensure_workspace_root(&dir.to_string_lossy())?;
    let p = dir.join(format!("{}.{}", uuid::Uuid::new_v4(), ext));
    tokio::fs::write(&p, &bytes)
        .await
        .map_err(|e| ApiError::InternalError(format!("write audio: {e}")))?;
    let path = p.to_string_lossy().to_string();

    let mut input = serde_json::json!({ "audio_path": path });
    if let Some(lang) = req.language {
        input["language"] = Value::String(lang);
    }

    let output = dispatch(&state, "audio.transcribe", input).await?;
    Ok(Json(AudioTranscribeResponse {
        text: output
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        // Segments are not currently returned by the OpenAI verbose-json
        // path that the LocalConnector wraps. Surface an empty array so the
        // contract shape is stable.
        segments: Vec::new(),
        language: output
            .get("language")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
    }))
}

fn audio_ext_for_mime(mime: &str) -> &'static str {
    match mime {
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/wav" | "audio/x-wav" => "wav",
        "audio/webm" => "webm",
        "audio/ogg" => "ogg",
        "audio/flac" => "flac",
        "audio/m4a" | "audio/mp4" => "m4a",
        _ => "bin",
    }
}

// ---------------------------------------------------------------------------
// Audio synthesize
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct AudioSynthesizeRequest {
    pub text: String,
    #[serde(default)]
    pub voice: Option<String>,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AudioSynthesizeResponse {
    pub b64: String,
    pub format: String,
    pub duration_ms: u64,
}

pub async fn audio_synthesize(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AudioSynthesizeRequest>,
) -> Result<Json<AudioSynthesizeResponse>, ApiError> {
    if req.text.trim().is_empty() {
        return Err(ApiError::InvalidInput("text must not be empty".to_string()));
    }
    let mut input = serde_json::json!({ "text": req.text });
    if let Some(v) = req.voice {
        input["voice"] = Value::String(v);
    }
    if let Some(f) = req.format.clone() {
        input["format"] = Value::String(f);
    }
    if let Some(p) = req.provider {
        input["provider"] = Value::String(p);
    }

    let started = Instant::now();
    let output = dispatch(&state, "audio.synthesize", input).await?;
    let duration_ms = started.elapsed().as_millis() as u64;

    let path = output
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::InternalError("audio.synthesize missing path".to_string()))?
        .to_string();
    // H-5: read-back limited to the admin workspace root.
    let b64 = read_file_b64_within(&path, ADMIN_MULTIMODAL_WORKSPACE_ROOT)
        .await?
        .ok_or_else(|| {
            ApiError::InvalidInput(format!(
                "audio.synthesize path '{}' is outside the admin workspace",
                path
            ))
        })?;
    let format = output
        .get("format")
        .and_then(|v| v.as_str())
        .map(String::from)
        .or(req.format)
        .unwrap_or_else(|| "mp3".to_string());

    Ok(Json(AudioSynthesizeResponse {
        b64,
        format,
        // Note: real audio duration is not measured server-side. We surface
        // wall-clock dispatch latency as a best-effort proxy so the contract
        // shape (`duration_ms: u64`) stays populated.
        duration_ms,
    }))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn create_admin_multimodal_router() -> Router<Arc<AppState>> {
    // H-9: lock down workspace permission bits eagerly.
    let _ = ensure_workspace_root(ADMIN_MULTIMODAL_WORKSPACE_ROOT);
    let audio_dir = std::env::temp_dir().join(ADMIN_AUDIO_TEMP_ROOT_NAME);
    let _ = ensure_workspace_root(&audio_dir.to_string_lossy());
    Router::new()
        .route(
            "/api/v1/admin/multimodal/vision/analyze",
            post(vision_analyze),
        )
        .route(
            "/api/v1/admin/multimodal/image/generate",
            post(image_generate),
        )
        .route(
            "/api/v1/admin/multimodal/audio/transcribe",
            post(audio_transcribe),
        )
        .route(
            "/api/v1/admin/multimodal/audio/synthesize",
            post(audio_synthesize),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn vision_rejects_missing_image() {
        let state = crate::api::test_helpers::build_test_state();
        let res = vision_analyze(
            State(state),
            Json(VisionAnalyzeRequest {
                image_b64: None,
                image_url: None,
                prompt: "describe".to_string(),
                provider: None,
            }),
        )
        .await;
        assert!(matches!(res, Err(ApiError::InvalidInput(_))));
    }

    #[tokio::test]
    async fn audio_synthesize_rejects_empty_text() {
        let state = crate::api::test_helpers::build_test_state();
        let res = audio_synthesize(
            State(state),
            Json(AudioSynthesizeRequest {
                text: "".to_string(),
                voice: None,
                format: None,
                provider: None,
            }),
        )
        .await;
        assert!(matches!(res, Err(ApiError::InvalidInput(_))));
    }

    #[tokio::test]
    async fn image_generate_returns_404_when_local_connector_missing() {
        let state = crate::api::test_helpers::build_test_state();
        let res = image_generate(
            State(state),
            Json(ImageGenerateRequest {
                prompt: "a red cat".to_string(),
                size: None,
                provider: None,
            }),
        )
        .await;
        match res {
            Err(ApiError::NotFound(_)) => {}
            other => panic!("expected NotFound, got {:?}", other),
        }
    }

    #[test]
    fn audio_ext_for_mime_known() {
        assert_eq!(audio_ext_for_mime("audio/mpeg"), "mp3");
        assert_eq!(audio_ext_for_mime("audio/wav"), "wav");
        assert_eq!(audio_ext_for_mime("audio/anything"), "bin");
    }

    /// C-3 regression: file:// URLs must be flat-out rejected.
    #[test]
    fn vision_url_rejects_file_scheme() {
        let res = validate_external_image_url("file:///etc/passwd");
        assert!(matches!(res, Err(ApiError::InvalidInput(_))));
    }

    /// C-3 regression: AWS / GCP / Azure metadata IPs MUST NOT be fetchable.
    #[test]
    fn vision_url_rejects_metadata_ip() {
        let res = validate_external_image_url("https://169.254.169.254/latest/meta-data/");
        assert!(matches!(res, Err(ApiError::InvalidInput(_))));
    }

    /// C-3 regression: loopback and RFC1918 are not the public Internet.
    #[test]
    fn vision_url_rejects_loopback_and_private() {
        assert!(matches!(
            validate_external_image_url("https://127.0.0.1/cat.png"),
            Err(ApiError::InvalidInput(_))
        ));
        assert!(matches!(
            validate_external_image_url("https://10.0.0.5/cat.png"),
            Err(ApiError::InvalidInput(_))
        ));
        assert!(matches!(
            validate_external_image_url("https://192.168.1.1/cat.png"),
            Err(ApiError::InvalidInput(_))
        ));
        assert!(matches!(
            validate_external_image_url("https://localhost/cat.png"),
            Err(ApiError::InvalidInput(_))
        ));
    }

    /// C-3 regression: plain http (even to public IPs) is rejected — the
    /// underlying capability would happily auto-detect and we don't want
    /// admin-token holders downgrading the channel.
    #[test]
    fn vision_url_rejects_plain_http() {
        let res = validate_external_image_url("http://example.com/cat.png");
        assert!(matches!(res, Err(ApiError::InvalidInput(_))));
    }

    /// Public https URL passes (the actual fetch is not exercised here).
    #[test]
    fn vision_url_accepts_public_https() {
        let res = validate_external_image_url("https://example.com/cat.png");
        assert!(res.is_ok());
    }

    /// C-2 regression: AudioTranscribeRequest must not accept a server
    /// filesystem path. We assert by construction — the field no longer
    /// exists in the type, so attempting to deserialize a payload with
    /// `audio_path` and no `audio_b64` returns InvalidInput.
    #[tokio::test]
    async fn audio_transcribe_rejects_bare_audio_path() {
        let state = crate::api::test_helpers::build_test_state();
        let payload = serde_json::from_value::<AudioTranscribeRequest>(serde_json::json!({
            "audio_path": "/etc/shadow",
        }))
        .expect("deserialise (extra fields ignored)");
        let res = audio_transcribe(State(state), Json(payload)).await;
        assert!(
            matches!(res, Err(ApiError::InvalidInput(_))),
            "audio_path-only request must be rejected; got {:?}",
            res
        );
    }
}
