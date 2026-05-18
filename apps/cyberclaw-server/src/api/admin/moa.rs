//! Admin Mixture-of-Agents endpoints.
//!
//! | Method | Path                          | Purpose                          |
//! |--------|-------------------------------|----------------------------------|
//! | GET    | `/api/v1/admin/moa/config`    | Read persisted MoA config        |
//! | PUT    | `/api/v1/admin/moa/config`    | Persist MoA config               |
//! | POST   | `/api/v1/admin/moa/test`      | Run a one-shot MoA prompt        |
//!
//! `MixtureOfAgents` lives in `cyberclaw-llm`. The `/test` endpoint
//! constructs N proposer `LlmClient`s on-demand from the persisted config
//! via `onboarding::build_llm_client`, fans the prompt out sequentially,
//! and pipes the results through the configured aggregator. Per-proposer
//! API keys come from the canonical env var for that provider
//! (`ANTHROPIC_API_KEY` / `OPENAI_API_KEY` / `ARK_API_KEY` / `LLM_API_KEY`).
//! Sequential rather than parallel because admin UI requests are low-rate
//! and parallel fan-out would hit rate-limits sooner on shared keys.

use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    extract::{Extension, State},
    routing::{get, post, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::audit::{AuditEntry, AuditKind, AuditResult};
use crate::error::ApiError;
use crate::middleware::auth::{require_admin, Claims};
use crate::state::AppState;

fn moa_config_path() -> PathBuf {
    std::env::var_os("CYBERCLAW_MOA_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(|h| PathBuf::from(h).join(".cyberclaw").join("moa.toml"))
                .unwrap_or_else(|| PathBuf::from(".cyberclaw").join("moa.toml"))
        })
}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoAProposer {
    pub model: String,
    pub provider: String,
    #[serde(default = "default_weight")]
    pub weight: f32,
}

fn default_weight() -> f32 {
    1.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoAAggregator {
    pub model: String,
    pub provider: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoAConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub proposers: Vec<MoAProposer>,
    #[serde(default = "default_aggregator")]
    pub aggregator: MoAAggregator,
}

fn default_aggregator() -> MoAAggregator {
    MoAAggregator {
        model: "claude-3-5-sonnet-20241022".to_string(),
        provider: "anthropic".to_string(),
    }
}

impl Default for MoAConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            proposers: Vec::new(),
            aggregator: default_aggregator(),
        }
    }
}

fn load_config_from_disk() -> MoAConfig {
    let path = moa_config_path();
    if !path.exists() {
        return MoAConfig::default();
    }
    match std::fs::read_to_string(&path) {
        Ok(s) => toml::from_str(&s).unwrap_or_else(|err| {
            tracing::warn!(path = %path.display(), %err, "moa config parse failed; returning default");
            MoAConfig::default()
        }),
        Err(err) => {
            tracing::warn!(path = %path.display(), %err, "moa config read failed; returning default");
            MoAConfig::default()
        }
    }
}

fn save_config_to_disk(cfg: &MoAConfig) -> Result<(), ApiError> {
    let path = moa_config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| ApiError::InternalError(format!("create_dir_all: {e}")))?;
    }
    let body = toml::to_string_pretty(cfg)
        .map_err(|e| ApiError::InternalError(format!("serialize moa.toml: {e}")))?;
    std::fs::write(&path, body)
        .map_err(|e| ApiError::InternalError(format!("write moa.toml: {e}")))?;
    // chmod 600 — moa.toml contains provider routing config
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

// In-process cache so consecutive PUT/GET calls don't hammer disk.
static CONFIG_CACHE: tokio::sync::OnceCell<Mutex<MoAConfig>> = tokio::sync::OnceCell::const_new();

async fn cached_config() -> &'static Mutex<MoAConfig> {
    CONFIG_CACHE
        .get_or_init(|| async { Mutex::new(load_config_from_disk()) })
        .await
}

// ---------------------------------------------------------------------------
// GET /config
// ---------------------------------------------------------------------------

pub async fn get_config(State(_state): State<Arc<AppState>>) -> Result<Json<MoAConfig>, ApiError> {
    let cfg = cached_config().await.lock().await.clone();
    Ok(Json(cfg))
}

// ---------------------------------------------------------------------------
// PUT /config
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct OkResponse {
    pub ok: bool,
}

pub async fn put_config(
    State(state): State<Arc<AppState>>,
    claims: Option<Extension<Claims>>,
    Json(req): Json<MoAConfig>,
) -> Result<Json<OkResponse>, ApiError> {
    if let Some(Extension(ref c)) = claims {
        if let Err(err) = require_admin(c).await {
            if let Some(sink) = state.audit.as_ref() {
                sink.record(AuditEntry::now(
                    c.sub.as_str().to_string(),
                    AuditKind::Auth,
                    "moa.config.update:role_denied".to_string(),
                    None,
                    serde_json::json!({ "reason": "role_denied" }),
                    AuditResult::Failure {
                        reason: "admin role required".to_string(),
                    },
                ))
                .await;
            }
            return Err(err);
        }
    }
    if req
        .proposers
        .iter()
        .any(|p| p.model.is_empty() || p.provider.is_empty())
    {
        return Err(ApiError::InvalidInput(
            "every proposer must declare model + provider".to_string(),
        ));
    }
    save_config_to_disk(&req)?;
    let summary = serde_json::json!({
        "enabled": req.enabled,
        "aggregator_model": req.aggregator.model,
        "proposer_count": req.proposers.len(),
    });
    *cached_config().await.lock().await = req;

    if let Some(sink) = state.audit.as_ref() {
        let actor = claims
            .as_ref()
            .map(|Extension(c)| c.sub.as_str().to_string())
            .unwrap_or_else(|| "system".to_string());
        sink.record(AuditEntry::now(
            actor,
            AuditKind::Mutation,
            "moa.config.update".to_string(),
            None,
            summary,
            AuditResult::Success,
        ))
        .await;
    }

    Ok(Json(OkResponse { ok: true }))
}

// ---------------------------------------------------------------------------
// POST /test
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct TestRequest {
    pub prompt: String,
}

#[derive(Debug, Serialize)]
pub struct ProposerOutput {
    pub model: String,
    pub text: String,
}

#[derive(Debug, Serialize)]
pub struct TestResponse {
    pub proposer_outputs: Vec<ProposerOutput>,
    pub aggregated: String,
    pub latency_ms: u64,
}

/// Return the conventional env-var name for `provider`'s API key.
/// `LLM_API_KEY` is the catch-all for generic / OpenAI-compatible endpoints.
fn env_key_for_provider(provider: &str) -> Option<String> {
    match provider.to_lowercase().as_str() {
        "openai" => std::env::var("OPENAI_API_KEY").ok(),
        "anthropic" => std::env::var("ANTHROPIC_API_KEY").ok(),
        "ark" | "volcengine" => std::env::var("ARK_API_KEY").ok(),
        _ => std::env::var("LLM_API_KEY")
            .ok()
            .or_else(|| std::env::var("OPENAI_API_KEY").ok()),
    }
}

/// Return the operator-configured base URL for `provider`, when one is set.
///
/// Critical for `provider = "generic"` because the underlying
/// `GenericOpenAiClient` falls back to `http://localhost:11434/v1` (Ollama)
/// when no base URL is passed — which silently misroutes DeepSeek / MiniMax /
/// any other OpenAI-compatible endpoint and fails with a fast connection error.
/// We honor `LLM_BASE_URL` (the same env the primary server LLM client reads
/// on boot) so MoA proposers reach the same endpoint operators already use.
fn env_base_url_for_provider(provider: &str) -> Option<String> {
    match provider.to_lowercase().as_str() {
        "openai" => std::env::var("OPENAI_BASE_URL").ok(),
        "anthropic" => std::env::var("ANTHROPIC_BASE_URL").ok(),
        "ark" | "volcengine" => std::env::var("ARK_BASE_URL").ok(),
        _ => std::env::var("LLM_BASE_URL").ok(),
    }
}

pub async fn test_moa(
    State(_state): State<Arc<AppState>>,
    Json(req): Json<TestRequest>,
) -> Result<Json<TestResponse>, ApiError> {
    if req.prompt.trim().is_empty() {
        return Err(ApiError::InvalidInput(
            "prompt must not be empty".to_string(),
        ));
    }

    let cfg = load_config_from_disk();
    if !cfg.enabled {
        return Err(ApiError::InvalidInput(
            "MoA is disabled in config — set enabled=true and save before running /test".to_string(),
        ));
    }
    if cfg.proposers.is_empty() {
        return Err(ApiError::InvalidInput(
            "no proposers configured — add at least one before running /test".to_string(),
        ));
    }

    let start = std::time::Instant::now();
    let mut proposer_outputs: Vec<ProposerOutput> = Vec::with_capacity(cfg.proposers.len());

    // Fan out sequentially. Errors do NOT abort the whole call — the failing
    // proposer's slot records the error text in its `text` field so the
    // operator can debug which provider key is missing or wrong.
    for p in &cfg.proposers {
        let key = env_key_for_provider(&p.provider);
        let base = env_base_url_for_provider(&p.provider);
        let client = match crate::api::admin::onboarding::build_llm_client(
            &p.provider,
            key.as_deref(),
            base.as_deref(),
        ) {
            Ok(c) => c,
            Err(e) => {
                proposer_outputs.push(ProposerOutput {
                    model: format!("{}/{}", p.provider, p.model),
                    text: format!("[client build error] {}", e),
                });
                continue;
            }
        };

        let request = cyberclaw_llm::types::ChatRequest {
            model: p.model.clone(),
            messages: vec![cyberclaw_llm::types::Message::user(req.prompt.clone())],
            ..Default::default()
        };
        match client.chat_completion(request).await {
            Ok(resp) => {
                let content = resp
                    .choices
                    .first()
                    .map(|c| c.message.content.clone())
                    .unwrap_or_default();
                proposer_outputs.push(ProposerOutput {
                    model: format!("{}/{}", p.provider, p.model),
                    text: content,
                });
            }
            Err(e) => {
                proposer_outputs.push(ProposerOutput {
                    model: format!("{}/{}", p.provider, p.model),
                    text: format!("[proposer error] {}", e),
                });
            }
        }
    }

    // Build aggregator. If aggregator client construction fails OR all
    // proposers errored, return InternalError — there's nothing meaningful
    // to synthesize.
    let agg_key = env_key_for_provider(&cfg.aggregator.provider);
    let agg_base = env_base_url_for_provider(&cfg.aggregator.provider);
    let agg_client = crate::api::admin::onboarding::build_llm_client(
        &cfg.aggregator.provider,
        agg_key.as_deref(),
        agg_base.as_deref(),
    )
    .map_err(|e| {
        ApiError::InternalError(format!(
            "aggregator client build failed for provider '{}': {}",
            cfg.aggregator.provider, e
        ))
    })?;

    let all_failed = proposer_outputs.iter().all(|p| {
        p.text.starts_with("[client build error]") || p.text.starts_with("[proposer error]")
    });
    if all_failed {
        return Err(ApiError::InternalError(format!(
            "all {} proposers failed; nothing to aggregate. Check API keys / endpoints. First error: {}",
            proposer_outputs.len(),
            proposer_outputs
                .first()
                .map(|p| p.text.as_str())
                .unwrap_or("(none)")
        )));
    }

    let proposals_text = proposer_outputs
        .iter()
        .enumerate()
        .map(|(i, p)| format!("=== Proposal {} ({}) ===\n{}", i + 1, p.model, p.text))
        .collect::<Vec<_>>()
        .join("\n\n");
    let agg_user = format!(
        "User question:\n{}\n\nProposals from {} models:\n{}\n\nSynthesize a single answer.",
        req.prompt,
        proposer_outputs.len(),
        proposals_text
    );
    let agg_system = "You are an expert aggregator. Synthesize a final answer from the candidate proposals: identify agreements, flag disagreements, do not pick a single proposal verbatim, do not invent claims no proposal made.";

    // CONSTITUTION-BYPASS-OK: admin-only MoA test endpoint
    // (/api/v1/admin/moa/test) uses a deliberately narrow aggregator-role
    // prompt that already encodes anti-fabrication ("do not invent claims
    // no proposal made"). This is operator-facing meta-aggregation, not a
    // user-facing chat path — prepending the full constitution would
    // conflict with the aggregator's specific "synthesize from candidates"
    // contract. See workbench.rs for the same diagnostic-mode pattern.
    let agg_request = cyberclaw_llm::types::ChatRequest {
        model: cfg.aggregator.model.clone(),
        messages: vec![
            cyberclaw_llm::types::Message::system(agg_system),
            cyberclaw_llm::types::Message::user(agg_user),
        ],
        ..Default::default()
    };
    let agg_resp = agg_client.chat_completion(agg_request).await.map_err(|e| {
        ApiError::InternalError(format!(
            "aggregator call failed (provider={}, model={}): {}",
            cfg.aggregator.provider, cfg.aggregator.model, e
        ))
    })?;
    let aggregated = agg_resp
        .choices
        .first()
        .map(|c| c.message.content.clone())
        .unwrap_or_default();

    Ok(Json(TestResponse {
        proposer_outputs,
        aggregated,
        latency_ms: start.elapsed().as_millis() as u64,
    }))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn create_admin_moa_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/admin/moa/config", get(get_config))
        .route("/api/v1/admin/moa/config", put(put_config))
        .route("/api/v1/admin/moa/test", post(test_moa))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[tokio::test]
    #[serial]
    async fn put_then_get_round_trips() {
        // Point persistence at a temp dir to avoid touching the host's $HOME.
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg_path = tmp.path().join("moa.toml");
        // SAFETY: tests do not currently parallelize this env var; cargo test
        // runs each #[tokio::test] on a shared multi-threaded runtime so we
        // accept the small race window.
        unsafe {
            std::env::set_var("CYBERCLAW_MOA_CONFIG", &cfg_path);
        }

        let state = crate::api::test_helpers::build_test_state();
        let new_cfg = MoAConfig {
            enabled: true,
            proposers: vec![
                MoAProposer {
                    model: "gpt-4o".to_string(),
                    provider: "openai".to_string(),
                    weight: 1.0,
                },
                MoAProposer {
                    model: "claude-3-5-sonnet-20241022".to_string(),
                    provider: "anthropic".to_string(),
                    weight: 1.0,
                },
            ],
            aggregator: MoAAggregator {
                model: "claude-3-5-sonnet-20241022".to_string(),
                provider: "anthropic".to_string(),
            },
        };
        let resp = put_config(State(state.clone()), None, Json(new_cfg.clone()))
            .await
            .unwrap();
        assert!(resp.0.ok);

        let got = get_config(State(state)).await.unwrap();
        assert_eq!(got.0.enabled, new_cfg.enabled);
        assert_eq!(got.0.proposers.len(), 2);
        unsafe {
            std::env::remove_var("CYBERCLAW_MOA_CONFIG");
        }
    }

    #[tokio::test]
    #[serial]
    async fn test_endpoint_rejects_when_disabled() {
        // Point at a temp config dir so we don't pick up the operator's real
        // moa.toml. Default config has enabled=false, so /test should return
        // InvalidInput, not InternalError.
        let tmp = tempfile::TempDir::new().unwrap();
        unsafe {
            std::env::set_var("CYBERCLAW_MOA_CONFIG", tmp.path().join("moa.toml"));
        }
        let state = crate::api::test_helpers::build_test_state();
        let res = test_moa(
            State(state),
            Json(TestRequest {
                prompt: "hi".to_string(),
            }),
        )
        .await;
        unsafe {
            std::env::remove_var("CYBERCLAW_MOA_CONFIG");
        }
        match res {
            Err(ApiError::InvalidInput(msg)) => {
                assert!(
                    msg.contains("MoA is disabled"),
                    "expected disabled message, got: {msg}"
                );
            }
            other => panic!("expected InvalidInput(disabled), got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_endpoint_rejects_empty_prompt() {
        let state = crate::api::test_helpers::build_test_state();
        let res = test_moa(
            State(state),
            Json(TestRequest {
                prompt: "   ".to_string(),
            }),
        )
        .await;
        assert!(matches!(res, Err(ApiError::InvalidInput(_))));
    }

    #[tokio::test]
    #[serial]
    async fn test_endpoint_rejects_enabled_but_no_proposers() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg_path = tmp.path().join("moa.toml");
        unsafe {
            std::env::set_var("CYBERCLAW_MOA_CONFIG", &cfg_path);
        }
        // Hand-write a config with enabled=true but no proposers.
        let body = toml::to_string_pretty(&MoAConfig {
            enabled: true,
            proposers: vec![],
            aggregator: default_aggregator(),
        })
        .unwrap();
        std::fs::create_dir_all(cfg_path.parent().unwrap()).unwrap();
        std::fs::write(&cfg_path, body).unwrap();

        let state = crate::api::test_helpers::build_test_state();
        let res = test_moa(
            State(state),
            Json(TestRequest {
                prompt: "hi".to_string(),
            }),
        )
        .await;
        unsafe {
            std::env::remove_var("CYBERCLAW_MOA_CONFIG");
        }
        match res {
            Err(ApiError::InvalidInput(msg)) => {
                assert!(
                    msg.contains("no proposers"),
                    "expected no-proposers message, got: {msg}"
                );
            }
            other => panic!("expected InvalidInput(no proposers), got {:?}", other),
        }
    }

    #[tokio::test]
    #[serial]
    async fn env_key_for_provider_routes_known_providers() {
        // unsafe: tests share env; clear before each assertion to avoid leakage.
        unsafe {
            std::env::set_var("ANTHROPIC_API_KEY", "anth");
            std::env::set_var("OPENAI_API_KEY", "oai");
            std::env::set_var("ARK_API_KEY", "ark");
            std::env::set_var("LLM_API_KEY", "generic");
        }
        assert_eq!(env_key_for_provider("anthropic"), Some("anth".to_string()));
        assert_eq!(env_key_for_provider("openai"), Some("oai".to_string()));
        assert_eq!(env_key_for_provider("ark"), Some("ark".to_string()));
        assert_eq!(env_key_for_provider("volcengine"), Some("ark".to_string()));
        assert_eq!(env_key_for_provider("deepseek"), Some("generic".to_string()));
        unsafe {
            std::env::remove_var("ANTHROPIC_API_KEY");
            std::env::remove_var("OPENAI_API_KEY");
            std::env::remove_var("ARK_API_KEY");
            std::env::remove_var("LLM_API_KEY");
        }
    }

    #[tokio::test]
    async fn put_rejects_empty_proposer_fields() {
        let state = crate::api::test_helpers::build_test_state();
        let bad = MoAConfig {
            enabled: true,
            proposers: vec![MoAProposer {
                model: "".to_string(),
                provider: "".to_string(),
                weight: 1.0,
            }],
            aggregator: default_aggregator(),
        };
        let res = put_config(State(state), None, Json(bad)).await;
        assert!(matches!(res, Err(ApiError::InvalidInput(_))));
    }
}
