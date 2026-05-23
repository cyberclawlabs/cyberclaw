//! CyberClaw LLM 集成层
//!
//! 提供统一的 LLM 客户端接口，支持多个 LLM 提供商：
//!
//! - **ARK (Volcengine)**: 火山引擎方舟平台（推荐国内用户）
//! - **OpenAI**: OpenAI 官方 API
//! - **Anthropic**: Anthropic Claude API
//! - **Generic**: 通用 OpenAI 兼容接口（DeepSeek、Ollama 等）
//!
//! # 示例
//!
//! ```rust,no_run
//! use cyberclaw_llm::prelude::*;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     // 创建 OpenAI 客户端
//!     let client = OpenAiClient::new(
//!         "sk-xxx".to_string(),
//!         "https://api.openai.com/v1",
//!     )?;
//!
//!     // 发送聊天请求
//!     let request = ChatRequest {
//!         model: "gpt-4".to_string(),
//!         messages: vec![Message::user("Hello, how are you?")],
//!         temperature: Some(0.7),
//!         ..Default::default()
//!     };
//!
//!     let response = client.chat_completion(request).await?;
//!     println!("Response: {}", response.choices[0].message.content);
//!
//!     Ok(())
//! }
//! ```

pub mod client;
pub mod credential_pool;
pub mod embed;
pub mod error;
pub mod failover_reason;
pub mod mixture_of_agents;
pub mod pricing;
pub mod provider_chain;
pub mod providers;
pub mod rate_limit_tracker;
pub mod types;

// Re-export 核心类型
pub use client::LlmClient;
pub use credential_pool::{CredentialPool, CredentialStats, SelectionStrategy};
pub use embed::{EmbedClient, NoopEmbedClient, OpenAiCompatEmbedClient};
pub use error::{LlmError, LlmResult};
pub use mixture_of_agents::{MixtureOfAgents, MoAResult, Proposal};
pub use pricing::{
    estimate_cost, lookup_pricing, CanonicalUsage, CostAccumulator, CostResult, ModelCost,
    PricingEntry,
};
pub use rate_limit_tracker::RateLimitSnapshot;
pub use types::{
    ChatChunk, ChatRequest, ChatResponse, Choice, ChunkChoice, Delta, FunctionCall,
    FunctionDefinition, Message, Role, ToolCall, ToolDefinition, Usage,
};

/// Prelude 模块，包含常用导入
pub mod prelude {
    pub use crate::client::LlmClient;
    pub use crate::error::{LlmError, LlmResult};
    pub use crate::providers::{
        anthropic::AnthropicClient, ark::ArkClient, generic::GenericOpenAiClient,
        openai::OpenAiClient, LlmProvider,
    };
    pub use crate::rate_limit_tracker::RateLimitSnapshot;
    pub use crate::types::{
        ChatChunk, ChatRequest, ChatResponse, Choice, FunctionCall, FunctionDefinition, Message,
        Role, ToolCall, ToolDefinition, Usage,
    };
    // Re-export Stream for implementing LlmClient trait
    pub use futures::stream::Stream;
}
