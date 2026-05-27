//! cyberclaw-agent-runtime: Minimal agent runtime for the CyberClaw platform.

pub mod agentic_loop;
pub mod builtin_tools;
pub mod builtin_tools_todo;
pub mod chat_verification_gate;
pub mod clarify;
pub mod config;
pub mod constitution;
pub mod context_compressor;
pub mod deferred_registry;
pub mod dsml_parser;
pub mod emergence_kit;
pub mod error;
pub mod loop_delegate;
pub mod loop_governor;
pub mod memory_integration;
pub mod mock;
pub mod prompt_assembler;
pub mod runtime;
pub mod session_search_injector;
pub mod skill_binder;
pub mod streaming;
pub mod sub_agent;
pub mod tool_description;
pub mod tool_result_budget;
pub mod tool_result_pipeline;
pub mod types;
pub mod verify;

pub use clarify::ClarifyCoordinator;
pub use config::{AgentConfig, RuntimeConfig, ServiceConfig};
pub use error::{AgentRuntimeError, AgentRuntimeResult};
pub use mock::MockAgentRuntime;
pub use runtime::MinimalAgentRuntime;
pub use tool_description::CapabilityFacade;
pub use tool_result_budget::{BudgetConfig, BudgetedResult, ToolResultBudget};
pub use types::{AgentRequest, AgentResponse};

pub use loop_delegate::{
    AutopilotDelegate, DelegateDecision, InteractiveDelegate, LoopDelegate, NoOpDelegate,
};
pub use memory_integration::{MemoryIntegration, MemoryScope, MemorySnapshot};
pub use skill_binder::{SkillBinder, SkillBinding, SkillInfo, SkillProvider, SkillToolDescriptor};
pub use sub_agent::{AgentHandle, AgentStatus, SpawnPolicy, SubAgentError, SubAgentOrchestrator};

pub use context_compressor::{
    CompressedResult, CompressionConfig, CompressionStage, ContextCompressionError,
    ContextCompressor, ContextSummarizer, DeterministicSummarizer, LlmContextSummarizer,
    MemoryLevel, COMPRESSION_SYSTEM_PROMPT,
};

pub use loop_governor::{AgenticLoopGovernor, CostTracker, GovernorConfig, LoopCtx, LoopDecision};

pub use verify::{
    CodeBlockVerifier, JsonStructureVerifier, OutputVerifier, RegexAssertVerifier,
    ToolFactVerifier, VerificationDirective, VerifierChain, VerifyCtx,
};

pub use agentic_loop::{ToolOutcomeEntry, ToolStatus};

pub use streaming::{
    ChannelStreamSink, StreamAdapter, StreamError, StreamEvent, StreamReceiver, StreamSink,
    StreamSummary,
};

pub use cyberclaw_core::validation::{Validate, ValidationError, ValidationResult};

use async_trait::async_trait;
use cyberclaw_core::ids::AgentId;

/// Core trait for all agent runtime implementations.
#[async_trait]
pub trait AgentRuntime: Send + Sync {
    /// Execute an agent request and return a response.
    async fn execute(&self, request: AgentRequest) -> AgentRuntimeResult<AgentResponse>;

    /// Load agent configuration by agent ID.
    async fn load_config(&self, agent_id: &AgentId) -> AgentRuntimeResult<AgentConfig>;
}
