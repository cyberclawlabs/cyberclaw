//! CyberClaw Connectors - Capability execution layer
//!
//! This crate provides the connector infrastructure for executing capabilities
//! in various runtime environments (native, remote, process, container).

pub mod acp_runtime;
pub mod acp_transport;
pub mod agent_hook_bridge;
pub mod browser;
pub mod builtin;
pub mod cluster;
pub mod contract;
pub mod database_connector;
pub mod dispatch_interceptor;
pub mod dispatcher;
pub mod error_sanitizer;
pub mod github_connector;
pub mod handoff;
pub mod http;
pub mod im_adapters;
pub mod im_channel;
pub mod local;
pub mod mcp;
pub mod message_gateway;
pub mod openviking;
pub mod registry;
pub mod retrieval;
pub mod rl_training;
pub mod runtime;
pub mod sandbox;
pub mod slack_connector;
pub mod toml_filter;
pub mod trajectory_compressor;
pub mod types;
pub mod voice_processing;

#[cfg(test)]
mod tests;

pub use acp_runtime::{
    AcpConfig, AcpRuntimeConnector, AcpSession, ArtifactType, ExternalRuntime, PermissionProfile,
    SecretRef, SessionArtifact, SessionEvent, SessionEventType, SessionState, TransportBackend,
};
pub use acp_transport::{AcpTransport, MockTransport, SpawnConfig};
pub use agent_hook_bridge::{
    CommandCategory, CommandClassification, CommandRiskLevel, HookAction, HookBridge,
    HookBridgeError, HookFormat, RewriteRegistry, RewriteRule,
};
pub use browser::{
    BrowserActionOutput, BrowserClickInput, BrowserConnector, BrowserConnectorConfig,
    BrowserDialogHandleInput, BrowserEvaluateInput, BrowserEvaluateOutput, BrowserFillInput,
    BrowserNavigateInput, BrowserNavigateOutput, BrowserScreenshotInput, BrowserScreenshotOutput,
};
pub use cluster::{ClusterAwareConnector, ClusterContext, RoutingDecision, RoutingStrategy};
pub use contract::{CapabilityDefinition, CapabilityDefinitionBuilder, ConnectorCapabilityProbe};
pub use database_connector::{
    DatabaseConnector, DatabasePool, DatabaseType, DbExecuteInput, DbExecuteOutput, DbMigrateInput,
    DbMigrateOutput, DbMigration, DbQueryInput, DbQueryOutput, DbTransactionInput,
    DbTransactionOutput,
};
pub use dispatch_interceptor::{
    DispatchCtx, DispatchInterceptor, SandboxInjectionInterceptor, TruncationMetadataInterceptor,
    WallClockInterceptor,
};
pub use dispatcher::CapabilityDispatcher;
pub use github_connector::{Authenticator, Credentials, GitHubAuth, GitHubConnector, RateLimiter};
pub use handoff::{HandoffConnector, HandoffSink, NoopReviewQueueSink, ReviewQueueSink};
pub use http::{
    AuthStrategy, HttpConnector, HttpConnectorConfig, HttpConnectorResponse, HttpEndpoint,
};
pub use im_adapters::LarkAdapter;
pub use im_channel::{
    AudioFormat, ImChannelConfig, ImChannelConnector, ImMessage, ImMessageType, ImPlatformAdapter,
    ReplyMode, SessionBinding,
};
pub use local::lsp::{LspConnector, LspConnectorConfig};
pub use local::vision::{VisionAnalyzeInput, VisionAnalyzeOutput};
pub use local::LocalConnector;
pub use mcp::{
    BridgeConfig, BridgedTool, HttpTransport, McpClient, McpConnector, McpPrompt, McpRequest,
    McpResource, McpResponse, McpServerConfig, McpTool, McpToolBridge, McpTransport,
    StdioTransport, TransportConfig,
};
pub use message_gateway::{
    GenericWebhookAdapter, MessageGatewayConnector, MessageRouter, NormalizedMessage,
    PlatformAdapter,
};
pub use openviking::types::OvSearchResult;
pub use openviking::{OpenVikingConfig, OpenVikingConnector, OvRetrievalDepth};
pub use registry::ConnectorRegistry;
pub use retrieval::{
    Document, InMemoryRetrievalBackend, RetrievalBackend, RetrievalConfig, RetrievalConnector,
    SearchResult,
};
pub use rl_training::{
    DeployWeightsInput, DeployWeightsOutput, ExecutionTrace, ExportTracesInput, ExportTracesOutput,
    InMemoryTraceStore, RlTrainingConnector, TraceExporter, TraceFilter, TraceOutcome, TraceStep,
    WeightDeployment,
};
pub use runtime::{
    ProcessConfig, ProcessExecutor, ProcessResult, ProcessRuntime, RuntimeMode,
    RuntimeSelectionStrategy, RuntimeSelector, RuntimeSelectorConfig,
};
pub use slack_connector::{
    CreateChannelInput, CreateChannelOutput, ReactEmojiInput, ReactEmojiOutput, SendMessageInput,
    SendMessageOutput, SlackConnector, UploadFileInput, UploadFileOutput,
};
pub use toml_filter::{
    CompiledFilter, FilterConfig, FilterDef, FilterEngine, FilterError, FilterResult,
    FilterTestDef, MatchOutputRule, ReplaceRule, TestResult,
};
pub use types::{
    CapabilityExecutionRequest, CapabilityExecutionResult, CmdExecInput, CmdExecOutput, Connector,
    ExecutionStatus, FsEditInput, FsEditOutput, FsPatchApplyInput, FsPatchApplyOutput, FsReadInput,
    FsReadOutput, FsWriteInput, FsWriteOutput, OsvScanInput, OsvScanOutput, OsvVulnerability,
    SearchGlobInput, SearchGlobOutput, SearchGrepInput, SearchGrepOutput, WebFetchInput,
    WebFetchOutput, WebSearchInput, WebSearchOutput, WebSearchResult,
};
pub use voice_processing::{
    ClassificationContext, ClassificationRule, ClassificationRuleType, IntentClassifier,
    MockSttBackend, MockTtsBackend, RuleBasedClassifier, SttBackend, SummarizerConfig, Transcript,
    TtsBackend, UserIntent, VoiceConfig, VoiceProcessingConnector, VoiceSafeSummarizer,
    VoiceSummary,
};

/// Initialize the connector runtime with default connectors
pub fn init_default_connectors(workspace: std::path::PathBuf) -> anyhow::Result<()> {
    let registry = ConnectorRegistry::global();

    // Register LocalConnector
    let local_connector = LocalConnector::new(workspace);
    let connector: std::sync::Arc<dyn Connector> = std::sync::Arc::new(local_connector);
    registry.register(connector)?;

    Ok(())
}
