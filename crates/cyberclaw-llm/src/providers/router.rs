//! Named convenience constructors for popular OpenAI-compatible routes.
//!
//! Mirrors Hermes v0.12 `tools/openrouter_client.py` + `tools/yuanbao_tools.py`
//! "load this provider by name" pattern. CyberClaw already has
//! [`GenericOpenAiClient`] for OpenAI-compatible endpoints; this module just
//! bakes in the well-known base URLs and required headers so callers don't
//! have to remember them.
//!
//! # Adding a new route
//!
//! 1. Add a function `pub fn <name>(api_key: String) -> LlmResult<GenericOpenAiClient>`.
//! 2. Reference its base_url + any required header.
//! 3. Add a unit test that the URL trims trailing slashes.

use crate::error::LlmResult;
use crate::providers::generic::GenericOpenAiClient;

/// OpenRouter — multi-LLM routing gateway. Set `OPENROUTER_API_KEY`.
/// `base_url`: <https://openrouter.ai/api/v1>.
///
/// OpenRouter expects `Authorization: Bearer <key>` (handled by
/// `GenericOpenAiClient`), and the request body uses standard
/// OpenAI-compatible shape with `model: "<provider>/<model-name>"` (e.g.
/// `"anthropic/claude-3-5-sonnet-20241022"`).
pub fn openrouter(api_key: String) -> LlmResult<GenericOpenAiClient> {
    GenericOpenAiClient::new(Some(api_key), "https://openrouter.ai/api/v1")
}

/// Yuanbao (元宝) — Tencent's Chinese LLM. Reads `YUANBAO_API_KEY` typically.
/// `base_url`: <https://yuanbao.tencent.com/api/v1> (OpenAI-compatible).
///
/// Note: Yuanbao's actual endpoint may require an additional header.
/// Production deployments should verify against current docs; the URL
/// here matches Hermes v0.12 `tools/yuanbao_tools.py` baseline.
pub fn yuanbao(api_key: String) -> LlmResult<GenericOpenAiClient> {
    GenericOpenAiClient::new(Some(api_key), "https://yuanbao.tencent.com/api/v1")
}

/// MiniMax M2.7-HighSpeed — staging container compat path used by CyberClaw
/// today. `base_url`: <https://api.minimax.chat/v1>.
pub fn minimax(api_key: String) -> LlmResult<GenericOpenAiClient> {
    GenericOpenAiClient::new(Some(api_key), "https://api.minimax.chat/v1")
}

/// Together AI — open-model hosting with OpenAI-compat API.
pub fn together(api_key: String) -> LlmResult<GenericOpenAiClient> {
    GenericOpenAiClient::new(Some(api_key), "https://api.together.xyz/v1")
}

/// Groq — high-throughput inference. OpenAI-compat.
pub fn groq(api_key: String) -> LlmResult<GenericOpenAiClient> {
    GenericOpenAiClient::new(Some(api_key), "https://api.groq.com/openai/v1")
}

/// Local Ollama — `http://localhost:11434/v1`. No API key required.
pub fn ollama_local() -> LlmResult<GenericOpenAiClient> {
    GenericOpenAiClient::new(None, "http://localhost:11434/v1")
}

/// Build a client by provider name string (case-insensitive). Returns
/// `Err` if the name isn't recognized. Useful for env-driven config:
/// `LLM_PROVIDER=openrouter cyberclaw-server …`.
pub fn by_name(name: &str, api_key: Option<String>) -> LlmResult<GenericOpenAiClient> {
    match name.trim().to_ascii_lowercase().as_str() {
        "openrouter" => openrouter(require_key(api_key, "OPENROUTER_API_KEY")?),
        "yuanbao" => yuanbao(require_key(api_key, "YUANBAO_API_KEY")?),
        "minimax" => minimax(require_key(api_key, "MINIMAX_API_KEY")?),
        "together" => together(require_key(api_key, "TOGETHER_API_KEY")?),
        "groq" => groq(require_key(api_key, "GROQ_API_KEY")?),
        "ollama" | "ollama-local" => ollama_local(),
        other => Err(crate::error::LlmError::UnknownProvider(other.to_string())),
    }
}

fn require_key(key: Option<String>, var: &str) -> LlmResult<String> {
    key.ok_or_else(|| {
        crate::error::LlmError::ConfigError(format!(
            "{var} required for this provider but not provided"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openrouter_constructor_succeeds() {
        use crate::client::LlmClient;
        let c = match openrouter("k".to_string()) {
            Ok(c) => c,
            Err(e) => panic!("openrouter constructor failed: {e}"),
        };
        assert_eq!(c.provider(), "generic");
    }

    #[test]
    fn yuanbao_constructor_succeeds() {
        assert!(yuanbao("k".to_string()).is_ok());
    }

    #[test]
    fn ollama_local_no_key_required() {
        assert!(ollama_local().is_ok());
    }

    #[test]
    fn by_name_dispatches_known_routes() {
        assert!(by_name("openrouter", Some("k".to_string())).is_ok());
        assert!(by_name("YUANBAO", Some("k".to_string())).is_ok());
        assert!(by_name("ollama", None).is_ok());
    }

    #[test]
    fn by_name_errors_on_unknown_provider() {
        let r = by_name("not-a-provider", None);
        let Err(e) = r else {
            panic!("expected error");
        };
        assert!(e.to_string().contains("not-a-provider"));
    }

    #[test]
    fn by_name_errors_when_api_key_missing_for_keyed_provider() {
        let r = by_name("openrouter", None);
        let Err(e) = r else {
            panic!("expected error");
        };
        assert!(e.to_string().contains("OPENROUTER_API_KEY"));
    }
}
