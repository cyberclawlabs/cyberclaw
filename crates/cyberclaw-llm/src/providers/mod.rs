//! LLM 提供商实现

pub mod anthropic;
pub mod ark;
pub mod generic;
pub mod openai;
pub mod router;

use crate::client::LlmClient;
use crate::error::{LlmError, LlmResult};
use std::sync::Arc;

/// LLM 提供商类型
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderType {
    /// 火山引擎方舟
    Ark,
    /// OpenAI
    OpenAi,
    /// Anthropic Claude
    Anthropic,
    /// 通用 OpenAI 兼容（DeepSeek、Ollama 等）
    Generic,
}

/// LLM 提供商管理器
///
/// 负责创建和管理不同的 LLM 客户端实例
pub struct LlmProvider {
    client: Arc<dyn LlmClient>,
    provider_type: ProviderType,
}

impl LlmProvider {
    /// 从环境变量创建提供商
    ///
    /// 根据 `LLM_PROVIDER` 环境变量选择提供商：
    /// - `volcengine` / `ark` → ARK Client
    /// - `openai` → OpenAI Client
    /// - `anthropic` / `claude` → Anthropic Client
    /// - `generic` → Generic OpenAI-compatible Client
    pub fn from_env() -> LlmResult<Self> {
        let provider_name = std::env::var("LLM_PROVIDER")
            .unwrap_or_else(|_| "ark".to_string())
            .to_lowercase();

        match provider_name.as_str() {
            "volcengine" | "ark" => Self::ark_from_env(),
            "openai" => Self::openai_from_env(),
            "anthropic" | "claude" => Self::anthropic_from_env(),
            "generic" => Self::generic_from_env(),
            _ => Err(LlmError::UnknownProvider(provider_name)),
        }
    }

    /// 从环境变量创建 ARK 客户端
    ///
    /// 需要的环境变量：
    /// - `ARK_API_KEY`: API 密钥
    /// - `ARK_BASE_URL`: API 基础 URL（可选，默认 https://ark.cn-beijing.volces.com/api/v3）
    /// - `ARK_MODEL`: 默认模型（可选）
    pub fn ark_from_env() -> LlmResult<Self> {
        let api_key = std::env::var("ARK_API_KEY")
            .map_err(|_| LlmError::ConfigError("ARK_API_KEY not set".to_string()))?;

        let base_url = std::env::var("ARK_BASE_URL")
            .unwrap_or_else(|_| "https://ark.cn-beijing.volces.com/api/v3".to_string());

        let client = ark::ArkClient::new(api_key, &base_url)?;

        Ok(Self {
            client: Arc::new(client),
            provider_type: ProviderType::Ark,
        })
    }

    /// 从环境变量创建 OpenAI 客户端
    ///
    /// 需要的环境变量：
    /// - `OPENAI_API_KEY`: API 密钥
    /// - `OPENAI_BASE_URL`: API 基础 URL（可选，默认 https://api.openai.com/v1）
    pub fn openai_from_env() -> LlmResult<Self> {
        let api_key = std::env::var("OPENAI_API_KEY")
            .map_err(|_| LlmError::ConfigError("OPENAI_API_KEY not set".to_string()))?;

        let base_url = std::env::var("OPENAI_BASE_URL")
            .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());

        let client = openai::OpenAiClient::new(api_key, &base_url)?;

        Ok(Self {
            client: Arc::new(client),
            provider_type: ProviderType::OpenAi,
        })
    }

    /// 从环境变量创建 Anthropic 客户端
    ///
    /// 需要的环境变量：
    /// - `ANTHROPIC_API_KEY`: API 密钥
    /// - `ANTHROPIC_BASE_URL`: API 基础 URL（可选，默认 https://api.anthropic.com）
    pub fn anthropic_from_env() -> LlmResult<Self> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .map_err(|_| LlmError::ConfigError("ANTHROPIC_API_KEY not set".to_string()))?;

        let base_url = std::env::var("ANTHROPIC_BASE_URL")
            .unwrap_or_else(|_| "https://api.anthropic.com".to_string());

        let client = anthropic::AnthropicClient::new(api_key, &base_url)?;

        Ok(Self {
            client: Arc::new(client),
            provider_type: ProviderType::Anthropic,
        })
    }

    /// 从环境变量创建通用 OpenAI 兼容客户端
    ///
    /// 需要的环境变量：
    /// - `GENERIC_API_KEY`: API 密钥（可选）
    /// - `GENERIC_BASE_URL`: API 基础 URL
    pub fn generic_from_env() -> LlmResult<Self> {
        let api_key = std::env::var("GENERIC_API_KEY").ok();

        let base_url = std::env::var("GENERIC_BASE_URL")
            .map_err(|_| LlmError::ConfigError("GENERIC_BASE_URL not set".to_string()))?;

        let client = generic::GenericOpenAiClient::new(api_key, &base_url)?;

        Ok(Self {
            client: Arc::new(client),
            provider_type: ProviderType::Generic,
        })
    }

    /// 获取客户端实例
    pub fn client(&self) -> Arc<dyn LlmClient> {
        self.client.clone()
    }

    /// 获取提供商类型
    pub fn provider_type(&self) -> &ProviderType {
        &self.provider_type
    }

    /// 使用自定义客户端创建提供商（主要用于测试）
    ///
    /// # Warning
    /// 此方法主要用于测试目的。生产环境应使用 `from_env()` 或其他工厂方法。
    pub fn with_client(client: Arc<dyn LlmClient>, provider_type: ProviderType) -> Self {
        Self {
            client,
            provider_type,
        }
    }
}
