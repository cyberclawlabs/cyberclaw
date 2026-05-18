//! MCP Tool Bridge
//!
//! Bridges MCP server tools to CyberClaw's internal capability model by converting
//! `McpTool` instances into `BridgedTool` projections suitable for LLM consumption.
//!
//! Key design points:
//! - `BridgedTool` is a read-only projection, NOT a first-class platform object
//! - All execution still flows through Connector -> Capability
//! - Server-scoped namespacing prevents tool name collisions across MCP servers

use std::collections::HashMap;

use cyberclaw_core::capability::RiskLevel;

use super::types::{McpServerConfig, McpTool};

/// A tool bridged from an MCP server, ready for LLM consumption.
/// This is a read-only projection — NOT a first-class platform object.
#[derive(Debug, Clone)]
pub struct BridgedTool {
    /// Namespaced name: "server__tool_name"
    pub name: String,
    /// Human-readable description
    pub description: String,
    /// JSON Schema for input parameters
    pub input_schema: serde_json::Value,
    /// Source MCP server name
    pub server_name: String,
    /// Original tool name before namespacing
    pub original_name: String,
    /// Connector identifier (always "mcp")
    pub connector_id: String,
    /// Capability identifier using the governance-aligned canonical format:
    /// `connector:mcp:{server}:{tool}`. This matches the glob patterns in
    /// `cyberclaw-governance::DangerousCapabilityFilter` (e.g.
    /// `connector:*:delete`, `connector:*:deploy`) so that MCP-bridged tools
    /// participate in the same safety net as first-party connector
    /// capabilities. The legacy `mcp.tool.{server}.{tool}` form used before
    /// 2026-04-19 silently bypassed those rules.
    pub capability_id: String,
    /// Classified risk level
    pub risk_level: RiskLevel,
}

impl BridgedTool {
    /// Convert to the core fields needed to construct a `CapabilityFacade`.
    ///
    /// Returns a tuple of `(name, description, input_schema, connector_id, capability_id, risk_level)`.
    /// MCP-specific fields (`server_name`, `original_name`) are intentionally dropped so that
    /// the caller (agent-runtime) can construct a `CapabilityFacade` without depending on this crate.
    pub fn to_capability_facade_fields(
        &self,
    ) -> (String, String, serde_json::Value, String, String, RiskLevel) {
        (
            self.name.clone(),
            self.description.clone(),
            self.input_schema.clone(),
            self.connector_id.clone(),
            self.capability_id.clone(),
            self.risk_level,
        )
    }
}

/// Configuration for the MCP tool bridge.
#[derive(Debug, Clone)]
pub struct BridgeConfig {
    /// Separator between server name and tool name (default: "__")
    pub namespace_separator: String,
    /// Default risk level for tools that don't match any pattern (default: High).
    ///
    /// Unknown MCP tools are treated as high-risk by default because external
    /// MCP servers are untrusted and may expose dangerous capabilities under
    /// innocuous names that evade pattern-based classification.
    pub default_risk: RiskLevel,
    /// Per-tool risk level overrides keyed by original tool name
    pub risk_overrides: HashMap<String, RiskLevel>,
}

impl Default for BridgeConfig {
    fn default() -> Self {
        Self {
            namespace_separator: "__".to_string(),
            default_risk: RiskLevel::High,
            risk_overrides: HashMap::new(),
        }
    }
}

/// Bridge that converts MCP server tools into CyberClaw BridgedTool projections.
#[derive(Debug, Clone)]
pub struct McpToolBridge {
    config: BridgeConfig,
}

/// Errors that can occur during tool bridging.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BridgeError {
    /// The tool's input_schema is not a valid JSON Schema object
    InvalidSchema { tool_name: String, reason: String },
    /// The server name is empty
    EmptyServerName,
    /// The tool name is empty
    EmptyToolName { server_name: String },
    /// The namespace separator is invalid
    InvalidSeparator { reason: String },
    /// A name contains the namespace separator, causing ambiguity
    SeparatorCollision { name: String, separator: String },
}

impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BridgeError::InvalidSchema { tool_name, reason } => {
                write!(f, "invalid schema for tool '{}': {}", tool_name, reason)
            }
            BridgeError::EmptyServerName => write!(f, "server name must not be empty"),
            BridgeError::EmptyToolName { server_name } => {
                write!(f, "tool name must not be empty (server: '{}')", server_name)
            }
            BridgeError::InvalidSeparator { reason } => {
                write!(f, "invalid namespace separator: {}", reason)
            }
            BridgeError::SeparatorCollision { name, separator } => {
                write!(
                    f,
                    "name '{}' contains namespace separator '{}'",
                    name, separator
                )
            }
        }
    }
}

impl std::error::Error for BridgeError {}

/// Patterns used to classify tool risk levels by name.
const CRITICAL_PATTERNS: &[&str] = &["delete", "drop", "destroy", "kill"];
const HIGH_PATTERNS: &[&str] = &["write", "create", "update", "exec", "run", "send"];
const MEDIUM_PATTERNS: &[&str] = &["list", "search", "query"];
const LOW_PATTERNS: &[&str] = &["read", "get", "describe", "help"];

impl McpToolBridge {
    /// Create a new bridge with the given configuration.
    ///
    /// Returns an error if the namespace separator is empty or contains
    /// characters that would cause ambiguity in capability_id parsing.
    pub fn new(config: BridgeConfig) -> Result<Self, BridgeError> {
        if config.namespace_separator.is_empty() {
            return Err(BridgeError::InvalidSeparator {
                reason: "separator must not be empty".to_string(),
            });
        }
        if config.namespace_separator.contains('/')
            || config.namespace_separator.contains('.')
            || config.namespace_separator.contains('\0')
        {
            return Err(BridgeError::InvalidSeparator {
                reason: "separator must not contain '/', '.', or null bytes".to_string(),
            });
        }
        Ok(Self { config })
    }

    /// Create a new bridge with default configuration.
    ///
    /// The default config uses `"__"` as separator, which is always valid.
    pub fn with_defaults() -> Self {
        // SAFETY: default separator "__" passes all validation rules.
        Self::new(BridgeConfig::default()).expect("default BridgeConfig is always valid")
    }

    /// Convert a single MCP tool into a BridgedTool.
    ///
    /// The tool name is namespaced with the server name, and risk level is
    /// classified based on tool name patterns (or overrides).
    pub fn bridge_tool(
        &self,
        tool: &McpTool,
        server_name: &str,
    ) -> Result<BridgedTool, BridgeError> {
        if server_name.is_empty() {
            return Err(BridgeError::EmptyServerName);
        }
        if tool.name.is_empty() {
            return Err(BridgeError::EmptyToolName {
                server_name: server_name.to_string(),
            });
        }
        // Validate that neither server_name nor tool.name contain the separator
        let sep = &self.config.namespace_separator;
        if server_name.contains(sep.as_str()) {
            return Err(BridgeError::SeparatorCollision {
                name: server_name.to_string(),
                separator: sep.clone(),
            });
        }
        if tool.name.contains(sep.as_str()) {
            return Err(BridgeError::SeparatorCollision {
                name: tool.name.clone(),
                separator: sep.clone(),
            });
        }

        validate_schema(&tool.input_schema, &tool.name)?;

        let namespaced = format!(
            "{}{}{}",
            server_name, self.config.namespace_separator, tool.name
        );
        // Canonical capability id format: `connector:mcp:{server}:{tool}`.
        // Aligned with cyberclaw-governance DangerousCapabilityFilter rules
        // like `connector:*:delete` and `connector:*:deploy`.
        let capability_id = format!("connector:mcp:{}:{}", server_name, tool.name);
        let risk_level = self.classify_risk(&tool.name, &tool.description);

        Ok(BridgedTool {
            name: namespaced,
            description: tool.description.clone(),
            input_schema: tool.input_schema.clone(),
            server_name: server_name.to_string(),
            original_name: tool.name.clone(),
            connector_id: "mcp".to_string(),
            capability_id,
            risk_level,
        })
    }

    /// Convert all tools from an MCP server config in one call.
    ///
    /// Returns successfully bridged tools and any errors encountered.
    pub fn bridge_server_tools(
        &self,
        server: &McpServerConfig,
        tools: &[McpTool],
    ) -> BridgeBatchResult {
        let mut bridged = Vec::with_capacity(tools.len());
        let mut errors = Vec::new();

        for tool in tools {
            match self.bridge_tool(tool, &server.name) {
                Ok(bt) => bridged.push(bt),
                Err(e) => errors.push(e),
            }
        }

        BridgeBatchResult { bridged, errors }
    }

    /// Classify risk level for a tool based on its name **and** description.
    ///
    /// Both fields are scanned so that a malicious MCP server cannot hide
    /// destructive intent behind an innocuous tool name.
    ///
    /// Priority: overrides > critical > high > medium > low > default
    fn classify_risk(&self, tool_name: &str, description: &str) -> RiskLevel {
        if let Some(level) = self.config.risk_overrides.get(tool_name) {
            return *level;
        }

        let lower_name = tool_name.to_lowercase();
        let lower_desc = description.to_lowercase();

        // Scan both name and description for risk signals
        let matches = |patterns: &[&str]| {
            patterns
                .iter()
                .any(|p| lower_name.contains(p) || lower_desc.contains(p))
        };

        if matches(CRITICAL_PATTERNS) {
            return RiskLevel::Critical;
        }
        if matches(HIGH_PATTERNS) {
            return RiskLevel::High;
        }
        if matches(MEDIUM_PATTERNS) {
            return RiskLevel::Medium;
        }
        if matches(LOW_PATTERNS) {
            return RiskLevel::Low;
        }

        self.config.default_risk
    }
}

/// Result of a batch bridging operation.
#[derive(Debug)]
pub struct BridgeBatchResult {
    /// Successfully bridged tools
    pub bridged: Vec<BridgedTool>,
    /// Errors for tools that could not be bridged
    pub errors: Vec<BridgeError>,
}

impl BridgeBatchResult {
    /// Returns true if all tools were bridged successfully.
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Validate that the input schema is a JSON object (JSON Schema must be an object).
fn validate_schema(schema: &serde_json::Value, tool_name: &str) -> Result<(), BridgeError> {
    if !schema.is_object() {
        return Err(BridgeError::InvalidSchema {
            tool_name: tool_name.to_string(),
            reason: "input_schema must be a JSON object".to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn make_tool(name: &str, desc: &str) -> McpTool {
        McpTool {
            name: name.to_string(),
            description: desc.to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "arg1": { "type": "string" }
                }
            }),
        }
    }

    fn make_server(name: &str) -> McpServerConfig {
        McpServerConfig {
            name: name.to_string(),
            transport: super::super::types::TransportConfig::Http {
                url: "http://localhost:8080".to_string(),
                headers: HashMap::new(),
            },
            timeout: Duration::from_secs(30),
            enable_cache: true,
        }
    }

    #[test]
    fn test_basic_bridge() {
        let bridge = McpToolBridge::with_defaults();
        let tool = make_tool("get_user", "Get a user by ID");
        let bt = bridge.bridge_tool(&tool, "github").unwrap();

        assert_eq!(bt.name, "github__get_user");
        assert_eq!(bt.original_name, "get_user");
        assert_eq!(bt.server_name, "github");
        assert_eq!(bt.connector_id, "mcp");
        assert_eq!(bt.capability_id, "connector:mcp:github:get_user");
        assert_eq!(bt.description, "Get a user by ID");
        assert_eq!(bt.risk_level, RiskLevel::Low);
    }

    #[test]
    fn test_namespace_separator_custom() {
        let bridge = McpToolBridge::new(BridgeConfig {
            namespace_separator: "::".to_string(),
            ..Default::default()
        })
        .unwrap();
        let tool = make_tool("read_file", "Read a file");
        let bt = bridge.bridge_tool(&tool, "fs").unwrap();

        assert_eq!(bt.name, "fs::read_file");
    }

    #[test]
    fn test_risk_critical() {
        let bridge = McpToolBridge::with_defaults();

        for name in &[
            "delete_repo",
            "drop_table",
            "destroy_instance",
            "kill_process",
        ] {
            let tool = make_tool(name, "test");
            let bt = bridge.bridge_tool(&tool, "srv").unwrap();
            assert_eq!(
                bt.risk_level,
                RiskLevel::Critical,
                "expected Critical for {}",
                name
            );
        }
    }

    #[test]
    fn test_risk_high() {
        let bridge = McpToolBridge::with_defaults();

        for name in &[
            "write_file",
            "create_issue",
            "update_config",
            "exec_command",
            "run_job",
            "send_message",
        ] {
            let tool = make_tool(name, "test");
            let bt = bridge.bridge_tool(&tool, "srv").unwrap();
            assert_eq!(bt.risk_level, RiskLevel::High, "expected High for {}", name);
        }
    }

    #[test]
    fn test_risk_medium() {
        let bridge = McpToolBridge::with_defaults();

        for name in &["list_repos", "search_issues", "query_logs"] {
            let tool = make_tool(name, "test");
            let bt = bridge.bridge_tool(&tool, "srv").unwrap();
            assert_eq!(
                bt.risk_level,
                RiskLevel::Medium,
                "expected Medium for {}",
                name
            );
        }
    }

    #[test]
    fn test_risk_low_and_default() {
        let bridge = McpToolBridge::with_defaults();

        // Explicit low patterns
        for name in &["read_file", "get_status", "describe_table", "help_menu"] {
            let tool = make_tool(name, "test");
            let bt = bridge.bridge_tool(&tool, "srv").unwrap();
            assert_eq!(bt.risk_level, RiskLevel::Low, "expected Low for {}", name);
        }

        // Unknown pattern falls to default (High — unknown MCP tools are untrusted)
        let tool = make_tool("ping", "test");
        let bt = bridge.bridge_tool(&tool, "srv").unwrap();
        assert_eq!(bt.risk_level, RiskLevel::High);
    }

    #[test]
    fn test_risk_override() {
        let mut overrides = HashMap::new();
        overrides.insert("safe_delete".to_string(), RiskLevel::Low);

        let bridge = McpToolBridge::new(BridgeConfig {
            risk_overrides: overrides,
            ..Default::default()
        })
        .unwrap();

        // Without override, "delete" would be Critical
        let tool = make_tool("safe_delete", "Soft delete");
        let bt = bridge.bridge_tool(&tool, "srv").unwrap();
        assert_eq!(bt.risk_level, RiskLevel::Low);
    }

    #[test]
    fn test_invalid_schema_rejected() {
        let bridge = McpToolBridge::with_defaults();
        let tool = McpTool {
            name: "bad_tool".to_string(),
            description: "bad".to_string(),
            input_schema: serde_json::json!("not an object"),
        };

        let err = bridge.bridge_tool(&tool, "srv").unwrap_err();
        assert_eq!(
            err,
            BridgeError::InvalidSchema {
                tool_name: "bad_tool".to_string(),
                reason: "input_schema must be a JSON object".to_string(),
            }
        );
    }

    #[test]
    fn test_empty_server_name_rejected() {
        let bridge = McpToolBridge::with_defaults();
        let tool = make_tool("get_user", "test");

        let err = bridge.bridge_tool(&tool, "").unwrap_err();
        assert_eq!(err, BridgeError::EmptyServerName);
    }

    #[test]
    fn test_empty_tool_name_rejected() {
        let bridge = McpToolBridge::with_defaults();
        let tool = make_tool("", "test");

        let err = bridge.bridge_tool(&tool, "srv").unwrap_err();
        assert_eq!(
            err,
            BridgeError::EmptyToolName {
                server_name: "srv".to_string(),
            }
        );
    }

    #[test]
    fn test_batch_bridge() {
        let bridge = McpToolBridge::with_defaults();
        let server = make_server("github");
        let tools = vec![
            make_tool("get_user", "Get user"),
            make_tool("create_issue", "Create issue"),
            McpTool {
                name: "bad".to_string(),
                description: "bad".to_string(),
                input_schema: serde_json::json!(42),
            },
        ];

        let result = bridge.bridge_server_tools(&server, &tools);

        assert_eq!(result.bridged.len(), 2);
        assert_eq!(result.errors.len(), 1);
        assert!(!result.is_ok());

        assert_eq!(result.bridged[0].name, "github__get_user");
        assert_eq!(result.bridged[1].name, "github__create_issue");
    }

    #[test]
    fn test_batch_all_success() {
        let bridge = McpToolBridge::with_defaults();
        let server = make_server("slack");
        let tools = vec![
            make_tool("send_message", "Send a message"),
            make_tool("list_channels", "List channels"),
        ];

        let result = bridge.bridge_server_tools(&server, &tools);

        assert!(result.is_ok());
        assert_eq!(result.bridged.len(), 2);
        assert_eq!(result.bridged[0].risk_level, RiskLevel::High);
        assert_eq!(result.bridged[1].risk_level, RiskLevel::Medium);
    }

    #[test]
    fn test_case_insensitive_classification() {
        let bridge = McpToolBridge::with_defaults();

        let tool = make_tool("DELETE_ALL", "Delete everything");
        let bt = bridge.bridge_tool(&tool, "srv").unwrap();
        assert_eq!(bt.risk_level, RiskLevel::Critical);

        let tool = make_tool("RunBuild", "Run a build");
        let bt = bridge.bridge_tool(&tool, "srv").unwrap();
        assert_eq!(bt.risk_level, RiskLevel::High);
    }

    #[test]
    fn test_critical_takes_priority_over_high() {
        let bridge = McpToolBridge::with_defaults();

        // "delete" (Critical) should win over "run" (High)
        let tool = make_tool("run_delete_job", "Run a delete job");
        let bt = bridge.bridge_tool(&tool, "srv").unwrap();
        assert_eq!(bt.risk_level, RiskLevel::Critical);
    }

    #[test]
    fn test_bridged_tool_to_facade_fields() {
        let bridge = McpToolBridge::with_defaults();
        let tool = make_tool("get_user", "Get a user by ID");
        let bt = bridge.bridge_tool(&tool, "github").unwrap();

        let (name, description, input_schema, connector_id, capability_id, risk_level) =
            bt.to_capability_facade_fields();

        assert_eq!(name, "github__get_user");
        assert_eq!(description, "Get a user by ID");
        assert_eq!(input_schema, tool.input_schema);
        assert_eq!(connector_id, "mcp");
        assert_eq!(capability_id, "connector:mcp:github:get_user");
        assert_eq!(risk_level, RiskLevel::Low);
    }

    #[test]
    fn test_bridged_tool_facade_drops_mcp_fields() {
        let bridge = McpToolBridge::with_defaults();
        let tool = make_tool("list_repos", "List repositories");
        let bt = bridge.bridge_tool(&tool, "github").unwrap();

        let fields = bt.to_capability_facade_fields();

        // The tuple has exactly 6 elements — server_name and original_name are absent.
        // Verify the returned name is the namespaced form, not the original.
        let (name, _desc, _schema, _connector, _cap, _risk) = fields;
        assert_eq!(name, "github__list_repos");
        // original_name ("list_repos") and server_name ("github") are not exposed.
        assert_ne!(name, bt.original_name);
        assert_ne!(name, bt.server_name);
    }
}
