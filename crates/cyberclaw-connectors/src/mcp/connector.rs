//! MCP Connector implementation

use super::client::McpClient;
use super::transport::create_transport;
use super::types::{McpCapabilityMapping, McpEntityType, McpServerConfig};
use crate::types::{
    CapabilityExecutionRequest, CapabilityExecutionResult, Connector, ExecutionStatus,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use cyberclaw_core::prelude::*;
use serde::Deserialize;
use std::sync::{Arc, RwLock as StdRwLock};
use tokio::sync::RwLock;

const MCP_LIST_TOOLS_CAPABILITY: &str = "mcp.list_tools";
const MCP_CALL_TOOL_CAPABILITY: &str = "mcp.call_tool";
const MCP_LIST_RESOURCES_CAPABILITY: &str = "mcp.list_resources";
const MCP_READ_RESOURCE_CAPABILITY: &str = "mcp.read_resource";
const MCP_LIST_PROMPTS_CAPABILITY: &str = "mcp.list_prompts";
const MCP_GET_PROMPT_CAPABILITY: &str = "mcp.get_prompt";

/// MCP Connector - bridges MCP protocol to CyberClaw Capabilities
#[derive(Debug)]
pub struct McpConnector {
    /// Connector ID
    id: ConnectorId,
    /// MCP client
    client: Arc<RwLock<McpClient>>,
    /// Cached capabilities
    capabilities: Arc<StdRwLock<Vec<CapabilityContract>>>,
    /// Capability mappings
    mappings: Arc<StdRwLock<Vec<McpCapabilityMapping>>>,
    /// Server configuration
    #[allow(dead_code)]
    config: McpServerConfig,
}

impl McpConnector {
    /// Create a new MCP connector
    pub async fn new(config: McpServerConfig) -> Result<Self> {
        // Create transport
        let transport = create_transport(&config.transport)?;

        // Create client
        let client = McpClient::new(transport, config.timeout, config.enable_cache);

        let connector_id = format!("mcp-{}", config.name);
        let connector = Self {
            id: ConnectorId::from_string(connector_id)?,
            client: Arc::new(RwLock::new(client)),
            capabilities: Arc::new(StdRwLock::new(Vec::new())),
            mappings: Arc::new(StdRwLock::new(Vec::new())),
            config,
        };

        // Discover capabilities
        connector.discover_capabilities().await?;

        Ok(connector)
    }

    /// BT-36 test helper — construct a connector around a pre-built
    /// transport (e.g. an in-process mock MCP server) so integration
    /// tests can exercise the full `Connector::execute()` path without
    /// needing a real MCP server reachable over HTTP.
    #[cfg(test)]
    pub async fn new_with_transport_for_test(
        config: McpServerConfig,
        transport: Box<dyn crate::mcp::McpTransport>,
    ) -> Result<Self> {
        let client = McpClient::new(transport, config.timeout, config.enable_cache);
        let connector_id = format!("mcp-{}", config.name);
        let connector = Self {
            id: ConnectorId::from_string(connector_id)?,
            client: Arc::new(RwLock::new(client)),
            capabilities: Arc::new(StdRwLock::new(Vec::new())),
            mappings: Arc::new(StdRwLock::new(Vec::new())),
            config,
        };
        connector.discover_capabilities().await?;
        Ok(connector)
    }

    /// Discover capabilities from MCP server
    async fn discover_capabilities(&self) -> Result<()> {
        let client = self.client.read().await;

        let mut capabilities = Self::builtin_capabilities();
        let mut mappings = Vec::new();

        // Discover tools
        let tools = client
            .list_tools()
            .await
            .context("Failed to list MCP tools")?;

        for tool in &tools {
            let capability_id = format!("mcp.tool.{}", tool.name);

            capabilities.push(CapabilityContract {
                id: capability_id.clone(),
                title: tool.name.clone(),
                description: Some(tool.description.clone()),
                input_schema: serde_json::to_string(&tool.input_schema)?,
                output_schema: "{}".to_string(),
                // 当前仅支持 Native runtime，避免被 Process/Container fail-fast 阻断。
                risk: RiskLevel::Low,
                effects: vec![CapabilityEffect::Execute, CapabilityEffect::Network],
                placement: None,
                timeouts: Default::default(),
            });

            mappings.push(McpCapabilityMapping {
                entity_type: McpEntityType::Tool,
                entity_id: tool.name.clone(),
                capability_id,
            });
        }

        // Discover resources
        let resources = client
            .list_resources()
            .await
            .context("Failed to list MCP resources")?;

        for resource in &resources {
            let capability_id = format!("mcp.resource.{}", sanitize_resource_uri(&resource.uri));

            capabilities.push(CapabilityContract {
                id: capability_id.clone(),
                title: resource.name.clone(),
                description: resource
                    .description
                    .clone()
                    .or_else(|| Some(format!("Read resource: {}", resource.name))),
                input_schema: "{}".to_string(),
                output_schema: "{}".to_string(),
                risk: RiskLevel::Low, // Resources are read-only
                effects: vec![CapabilityEffect::Read, CapabilityEffect::Network],
                placement: None,
                timeouts: Default::default(),
            });

            mappings.push(McpCapabilityMapping {
                entity_type: McpEntityType::Resource,
                entity_id: resource.uri.clone(),
                capability_id,
            });
        }

        // Discover prompts
        let prompts = client
            .list_prompts()
            .await
            .context("Failed to list MCP prompts")?;

        for prompt in &prompts {
            let capability_id = format!("mcp.prompt.{}", prompt.name);

            capabilities.push(CapabilityContract {
                id: capability_id.clone(),
                title: prompt.name.clone(),
                description: Some(prompt.description.clone()),
                input_schema: "{}".to_string(),
                output_schema: "{}".to_string(),
                risk: RiskLevel::Low,
                effects: vec![CapabilityEffect::Read, CapabilityEffect::Network],
                placement: None,
                timeouts: Default::default(),
            });

            mappings.push(McpCapabilityMapping {
                entity_type: McpEntityType::Prompt,
                entity_id: prompt.name.clone(),
                capability_id,
            });
        }

        tracing::info!(
            "Discovered {} MCP capabilities: {} tools, {} resources, {} prompts",
            capabilities.len(),
            tools.len(),
            resources.len(),
            prompts.len()
        );

        // Update cached capabilities and mappings
        {
            let mut capabilities_guard = self
                .capabilities
                .write()
                .map_err(|_| anyhow::anyhow!("MCP capabilities lock poisoned"))?;
            *capabilities_guard = capabilities;
        }
        {
            let mut mappings_guard = self
                .mappings
                .write()
                .map_err(|_| anyhow::anyhow!("MCP mappings lock poisoned"))?;
            *mappings_guard = mappings;
        }

        Ok(())
    }

    fn builtin_capabilities() -> Vec<CapabilityContract> {
        vec![
            CapabilityContract {
                id: MCP_LIST_TOOLS_CAPABILITY.to_string(),
                title: "List MCP Tools".to_string(),
                description: Some("List tools exposed by MCP server".to_string()),
                input_schema: "{}".to_string(),
                output_schema: "{}".to_string(),
                risk: RiskLevel::Low,
                effects: vec![CapabilityEffect::Read, CapabilityEffect::Network],
                placement: None,
                timeouts: Default::default(),
            },
            CapabilityContract {
                id: MCP_CALL_TOOL_CAPABILITY.to_string(),
                title: "Call MCP Tool".to_string(),
                description: Some("Call an MCP tool by name".to_string()),
                input_schema: r#"{"type":"object","required":["tool_name"],"properties":{"tool_name":{"type":"string"},"arguments":{"type":"object"}}}"#.to_string(),
                output_schema: "{}".to_string(),
                risk: RiskLevel::Low,
                effects: vec![CapabilityEffect::Execute, CapabilityEffect::Network],
                placement: None,
                timeouts: Default::default(),
            },
            CapabilityContract {
                id: MCP_LIST_RESOURCES_CAPABILITY.to_string(),
                title: "List MCP Resources".to_string(),
                description: Some("List resources exposed by MCP server".to_string()),
                input_schema: "{}".to_string(),
                output_schema: "{}".to_string(),
                risk: RiskLevel::Low,
                effects: vec![CapabilityEffect::Read, CapabilityEffect::Network],
                placement: None,
                timeouts: Default::default(),
            },
            CapabilityContract {
                id: MCP_READ_RESOURCE_CAPABILITY.to_string(),
                title: "Read MCP Resource".to_string(),
                description: Some("Read MCP resource by URI".to_string()),
                input_schema: r#"{"type":"object","required":["uri"],"properties":{"uri":{"type":"string"}}}"#.to_string(),
                output_schema: "{}".to_string(),
                risk: RiskLevel::Low,
                effects: vec![CapabilityEffect::Read, CapabilityEffect::Network],
                placement: None,
                timeouts: Default::default(),
            },
            CapabilityContract {
                id: MCP_LIST_PROMPTS_CAPABILITY.to_string(),
                title: "List MCP Prompts".to_string(),
                description: Some("List prompts exposed by MCP server".to_string()),
                input_schema: "{}".to_string(),
                output_schema: "{}".to_string(),
                risk: RiskLevel::Low,
                effects: vec![CapabilityEffect::Read, CapabilityEffect::Network],
                placement: None,
                timeouts: Default::default(),
            },
            CapabilityContract {
                id: MCP_GET_PROMPT_CAPABILITY.to_string(),
                title: "Get MCP Prompt".to_string(),
                description: Some("Get an MCP prompt by name".to_string()),
                input_schema: r#"{"type":"object","required":["prompt_name"],"properties":{"prompt_name":{"type":"string"},"arguments":{"type":"object"}}}"#.to_string(),
                output_schema: "{}".to_string(),
                risk: RiskLevel::Low,
                effects: vec![CapabilityEffect::Read, CapabilityEffect::Network],
                placement: None,
                timeouts: Default::default(),
            },
        ]
    }

    /// Find mapping for a capability
    fn find_mapping(&self, capability_id: &str) -> Option<McpCapabilityMapping> {
        let mappings = self.mappings.read().ok()?;
        mappings
            .iter()
            .find(|m| m.capability_id == capability_id)
            .cloned()
    }

    /// Execute an MCP tool
    async fn execute_tool(
        &self,
        tool_name: &str,
        input: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let client = self.client.read().await;
        client.call_tool(tool_name, input).await
    }

    /// Read an MCP resource
    async fn read_resource(&self, resource_uri: &str) -> Result<serde_json::Value> {
        let client = self.client.read().await;
        client.read_resource(resource_uri).await
    }

    /// Get an MCP prompt
    async fn get_prompt(
        &self,
        prompt_name: &str,
        arguments: Option<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let client = self.client.read().await;
        client.get_prompt(prompt_name, arguments).await
    }

    /// Refresh capabilities (re-discover from server)
    pub async fn refresh_capabilities(&self) -> Result<()> {
        self.discover_capabilities().await
    }
}

#[derive(Debug, Deserialize)]
struct CallToolInput {
    tool_name: String,
    #[serde(default)]
    arguments: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct ReadResourceInput {
    uri: String,
}

#[derive(Debug, Deserialize)]
struct GetPromptInput {
    prompt_name: String,
    #[serde(default)]
    arguments: Option<serde_json::Value>,
}

#[async_trait]
impl Connector for McpConnector {
    fn id(&self) -> &ConnectorId {
        &self.id
    }

    fn runtime(&self) -> ConnectorRuntime {
        ConnectorRuntime::Remote
    }

    fn capabilities(&self) -> Vec<CapabilityContract> {
        self.capabilities
            .read()
            .map(|caps| caps.clone())
            .unwrap_or_default()
    }

    async fn execute(
        &self,
        request: CapabilityExecutionRequest,
    ) -> Result<CapabilityExecutionResult> {
        let capability_id = request.capability_id.as_ref();
        let output = match capability_id {
            MCP_LIST_TOOLS_CAPABILITY => {
                let client = self.client.read().await;
                serde_json::to_value(client.list_tools().await?)?
            }
            MCP_CALL_TOOL_CAPABILITY => {
                let input: CallToolInput = serde_json::from_value(request.input.clone())
                    .context("Invalid input for mcp.call_tool")?;
                self.execute_tool(&input.tool_name, input.arguments).await?
            }
            MCP_LIST_RESOURCES_CAPABILITY => {
                let client = self.client.read().await;
                serde_json::to_value(client.list_resources().await?)?
            }
            MCP_READ_RESOURCE_CAPABILITY => {
                let input: ReadResourceInput = serde_json::from_value(request.input.clone())
                    .context("Invalid input for mcp.read_resource")?;
                self.read_resource(&input.uri).await?
            }
            MCP_LIST_PROMPTS_CAPABILITY => {
                let client = self.client.read().await;
                serde_json::to_value(client.list_prompts().await?)?
            }
            MCP_GET_PROMPT_CAPABILITY => {
                let input: GetPromptInput = serde_json::from_value(request.input.clone())
                    .context("Invalid input for mcp.get_prompt")?;
                self.get_prompt(&input.prompt_name, input.arguments).await?
            }
            _ => {
                let mapping = self.find_mapping(capability_id).with_context(|| {
                    format!(
                        "No MCP mapping found for capability: {}",
                        request.capability_id
                    )
                })?;

                match mapping.entity_type {
                    McpEntityType::Tool => {
                        self.execute_tool(&mapping.entity_id, request.input.clone())
                            .await?
                    }
                    McpEntityType::Resource => self.read_resource(&mapping.entity_id).await?,
                    McpEntityType::Prompt => {
                        let arguments = if request.input.is_null() {
                            None
                        } else {
                            Some(request.input.clone())
                        };
                        self.get_prompt(&mapping.entity_id, arguments).await?
                    }
                }
            }
        };

        Ok(CapabilityExecutionResult {
            execution_id: request.execution_id,
            trace_id: request.trace_id,
            connector_id: self.id.clone(),
            capability_id: request.capability_id,
            output,
            status: ExecutionStatus::Success,
            error: None,
            actual_runtime: Some(crate::runtime::RuntimeMode::Native), // MCP calls are remote but execute natively in our process
        })
    }
}

/// Sanitize resource URI to create a valid capability ID
pub fn sanitize_resource_uri(uri: &str) -> String {
    // Handle file:/// (three slashes) specially by replacing with ..
    uri.replace(":///", "..")
        .replace("://", ".")
        .replace(['/', ':'], ".")
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '.' || *c == '_' || *c == '-')
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_resource_uri() {
        assert_eq!(
            sanitize_resource_uri("file:///path/to/file.txt"),
            "file..path.to.file.txt"
        );
        assert_eq!(
            sanitize_resource_uri("http://example.com/api/v1"),
            "http.example.com.api.v1"
        );
        assert_eq!(
            sanitize_resource_uri("custom://resource-name"),
            "custom.resource-name"
        );
    }

    #[test]
    fn test_connector_id_generation() {
        let config = McpServerConfig {
            name: "test-server".to_string(),
            transport: crate::mcp::types::TransportConfig::Http {
                url: "http://localhost:8080".to_string(),
                headers: std::collections::HashMap::new(),
            },
            timeout: std::time::Duration::from_secs(30),
            enable_cache: true,
        };

        // Can't easily test async constructor, but we can test ID format
        let id = ConnectorId::from_string(format!("mcp-{}", config.name)).unwrap();
        assert!(id.to_string().contains("mcp-test-server"));
    }

    #[test]
    fn test_builtin_capabilities_exposed() {
        let capabilities = McpConnector::builtin_capabilities();
        let ids: Vec<String> = capabilities.into_iter().map(|c| c.id).collect();

        assert!(ids.contains(&MCP_LIST_TOOLS_CAPABILITY.to_string()));
        assert!(ids.contains(&MCP_CALL_TOOL_CAPABILITY.to_string()));
        assert!(ids.contains(&MCP_LIST_RESOURCES_CAPABILITY.to_string()));
        assert!(ids.contains(&MCP_READ_RESOURCE_CAPABILITY.to_string()));
        assert!(ids.contains(&MCP_LIST_PROMPTS_CAPABILITY.to_string()));
        assert!(ids.contains(&MCP_GET_PROMPT_CAPABILITY.to_string()));
    }
}
