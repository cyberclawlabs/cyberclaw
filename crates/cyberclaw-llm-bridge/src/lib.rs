//! CyberClaw LLM Bridge
//!
//! 提供 LLM Tool Calling 到 Capability 执行的桥接层。
//!
//! # 核心功能
//!
//! - **Tool Call 映射**: 将 LLM 的 function call 转换为 Capability 执行请求
//! - **执行器**: 通过 CapabilityDispatcher 执行能力并格式化结果
//! - **错误处理**: 优雅处理执行失败,支持可恢复错误
//!
//! # 示例
//!
//! ```rust,no_run
//! use cyberclaw_llm_bridge::{ToolExecutor, ToolCallMapper};
//! use cyberclaw_connectors::{CapabilityDispatcher, ConnectorRegistry};
//! use std::sync::Arc;
//!
//! # async fn example() -> anyhow::Result<()> {
//! // 创建 registry, dispatcher 和 mapper
//! let registry = Arc::new(ConnectorRegistry::new());
//! let dispatcher = Arc::new(CapabilityDispatcher::new(registry));
//! let mapper = Arc::new(ToolCallMapper::new());
//!
//! // 创建 executor
//! let executor = ToolExecutor::new(dispatcher, mapper);
//!
//! // 执行 tool call (需要 tool_call 和 trace_id)
//! // let result = executor.execute_tool(&tool_call, "trace-123".to_string()).await?;
//! # Ok(())
//! # }
//! ```

pub mod error;
pub mod executor;
pub mod mapper;
pub mod schema_sanitizer;
pub mod standard_mappings;
mod tool_filter;
pub mod types;

pub use error::{BridgeError, BridgeResult};
pub use executor::ToolExecutor;
pub use mapper::ToolCallMapper;
pub use schema_sanitizer::sanitize_tool_schemas;
pub use standard_mappings::{
    get_standard_tool_definitions, get_standard_tool_definitions_filtered,
    register_mcp_tool_mappings, register_standard_mappings,
};
pub use types::{ToolCallMapping, ToolExecutionResult};
