//! MCP (Model Context Protocol) Connector
//!
//! Implements the Model Context Protocol for external tool and resource integration.
//!
//! ## Architecture
//!
//! ```text
//! ┌────────────────────┐
//! │  MCP Connector     │
//! │                    │
//! │  ┌──────────────┐  │
//! │  │ JSON-RPC     │  │
//! │  │ Client       │  │
//! │  └──────────────┘  │
//! │         │          │
//! │  ┌──────▼──────┐   │
//! │  │ Transport   │   │
//! │  │ stdio/HTTP  │   │
//! │  └─────────────┘   │
//! └────────────────────┘
//!         │
//!         ▼
//! ┌────────────────────┐
//! │  MCP Server        │
//! │  (External)        │
//! └────────────────────┘
//! ```
//!
//! ## Features
//!
//! - JSON-RPC 2.0 protocol implementation
//! - Stdio and HTTP transport support
//! - Dynamic capability discovery
//! - Tool and Resource mapping to Capabilities
//! - Response caching
//! - Error handling and retry logic

mod client;
mod connector;
pub mod facades;
mod protocol;
pub mod tool_bridge;
mod transport;
mod types;

#[cfg(test)]
mod tests;

pub use client::McpClient;
pub use connector::{sanitize_resource_uri, McpConnector};
pub use protocol::{McpError, McpRequest, McpResponse, RequestId};
pub use tool_bridge::{BridgeConfig, BridgedTool, McpToolBridge};
pub use transport::{create_transport, HttpTransport, McpTransport, StdioTransport};
pub use types::{
    McpCache, McpCapabilityMapping, McpEntityType, McpPrompt, McpResource, McpServerConfig,
    McpTool, TransportConfig,
};
