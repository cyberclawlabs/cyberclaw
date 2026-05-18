//! # Contract-first Capability Definition
//!
//! Provides a unified `CapabilityDefinition` that auto-derives both LLM tool
//! and MCP tool formats from a single source of truth.

use cyberclaw_core::capability_contract::CapabilityBehaviorContract;
use cyberclaw_llm::types::{FunctionDefinition, ToolDefinition};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Unified capability definition that serves as the single source of truth
/// for generating LLM tool definitions and MCP tool schemas.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CapabilityDefinition {
    /// Unique identifier for this capability.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Description of what this capability does.
    pub description: String,
    /// JSON Schema describing the input parameters.
    pub input_schema: Value,
    /// JSON Schema describing the output format.
    pub output_schema: Value,
    /// Behavioral contract metadata (read-only, destructive, etc.).
    pub behavior: CapabilityBehaviorContract,
    /// ID of the connector that provides this capability.
    pub connector_id: String,
    /// Tags for categorization and discovery.
    pub tags: Vec<String>,
}

impl CapabilityDefinition {
    /// Convert to an LLM `ToolDefinition` for use with chat completion APIs.
    ///
    /// Maps: id -> function name, description -> function description,
    /// input_schema -> parameters.
    pub fn to_llm_tool(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: self.id.clone(),
                description: self.description.clone(),
                parameters: self.input_schema.clone(),
            },
            cache_control: None,
        }
    }

    /// Convert to MCP Tool JSON format.
    ///
    /// Produces the standard MCP tool schema:
    /// `{ "name": ..., "description": ..., "inputSchema": ... }`
    pub fn to_mcp_tool(&self) -> Value {
        serde_json::json!({
            "name": self.id,
            "description": self.description,
            "inputSchema": self.input_schema,
        })
    }
}

/// Trait for connectors that can enumerate their available capabilities.
pub trait ConnectorCapabilityProbe: Send + Sync {
    /// List all capabilities provided by this connector.
    fn list_capabilities(&self) -> Vec<CapabilityDefinition>;

    /// Get a specific capability by ID.
    fn get_capability(&self, id: &str) -> Option<CapabilityDefinition>;
}

/// Fluent builder for constructing `CapabilityDefinition` instances.
#[derive(Debug, Default)]
pub struct CapabilityDefinitionBuilder {
    id: Option<String>,
    name: Option<String>,
    description: Option<String>,
    input_schema: Option<Value>,
    output_schema: Option<Value>,
    behavior: Option<CapabilityBehaviorContract>,
    connector_id: Option<String>,
    tags: Vec<String>,
}

impl CapabilityDefinitionBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the capability ID.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set the capability name.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Set the capability description.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set the input JSON Schema.
    pub fn input_schema(mut self, schema: Value) -> Self {
        self.input_schema = Some(schema);
        self
    }

    /// Set the output JSON Schema.
    pub fn output_schema(mut self, schema: Value) -> Self {
        self.output_schema = Some(schema);
        self
    }

    /// Set the behavior contract.
    pub fn behavior(mut self, behavior: CapabilityBehaviorContract) -> Self {
        self.behavior = Some(behavior);
        self
    }

    /// Set the connector ID.
    pub fn connector_id(mut self, connector_id: impl Into<String>) -> Self {
        self.connector_id = Some(connector_id.into());
        self
    }

    /// Add a single tag.
    pub fn tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    /// Add multiple tags.
    pub fn tags(mut self, tags: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.tags.extend(tags.into_iter().map(|t| t.into()));
        self
    }

    /// Mark this capability as read-only (convenience shorthand).
    pub fn read_only(mut self) -> Self {
        let mut b = self.behavior.take().unwrap_or_default();
        b.is_read_only = true;
        b.is_concurrency_safe = true;
        self.behavior = Some(b);
        self
    }

    /// Mark this capability as destructive (convenience shorthand).
    pub fn destructive(mut self) -> Self {
        let mut b = self.behavior.take().unwrap_or_default();
        b.is_destructive = true;
        self.behavior = Some(b);
        self
    }

    /// Build the `CapabilityDefinition`.
    ///
    /// Returns `None` if required fields (`id`, `name`, `description`,
    /// `connector_id`) are missing.
    pub fn build(self) -> Option<CapabilityDefinition> {
        Some(CapabilityDefinition {
            id: self.id?,
            name: self.name?,
            description: self.description?,
            input_schema: self.input_schema.unwrap_or(serde_json::json!({})),
            output_schema: self.output_schema.unwrap_or(serde_json::json!({})),
            behavior: self.behavior.unwrap_or_default(),
            connector_id: self.connector_id?,
            tags: self.tags,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_input_schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File path" }
            },
            "required": ["path"]
        })
    }

    fn sample_definition() -> CapabilityDefinition {
        CapabilityDefinitionBuilder::new()
            .id("fs_read")
            .name("File Read")
            .description("Read a file from disk")
            .input_schema(sample_input_schema())
            .output_schema(json!({ "type": "object" }))
            .connector_id("local")
            .read_only()
            .tag("filesystem")
            .tag("read")
            .build()
            .unwrap()
    }

    // --- Builder tests ---

    #[test]
    fn builder_creates_valid_definition() {
        let def = sample_definition();
        assert_eq!(def.id, "fs_read");
        assert_eq!(def.name, "File Read");
        assert_eq!(def.connector_id, "local");
        assert_eq!(def.tags, vec!["filesystem", "read"]);
    }

    #[test]
    fn builder_returns_none_when_id_missing() {
        let result = CapabilityDefinitionBuilder::new()
            .name("Test")
            .description("desc")
            .connector_id("c")
            .build();
        assert!(result.is_none());
    }

    #[test]
    fn builder_returns_none_when_name_missing() {
        let result = CapabilityDefinitionBuilder::new()
            .id("test")
            .description("desc")
            .connector_id("c")
            .build();
        assert!(result.is_none());
    }

    #[test]
    fn builder_returns_none_when_connector_id_missing() {
        let result = CapabilityDefinitionBuilder::new()
            .id("test")
            .name("Test")
            .description("desc")
            .build();
        assert!(result.is_none());
    }

    #[test]
    fn builder_defaults_schemas_to_empty_object() {
        let def = CapabilityDefinitionBuilder::new()
            .id("x")
            .name("X")
            .description("x")
            .connector_id("c")
            .build()
            .unwrap();
        assert_eq!(def.input_schema, json!({}));
        assert_eq!(def.output_schema, json!({}));
    }

    #[test]
    fn builder_read_only_sets_behavior() {
        let def = sample_definition();
        assert!(def.behavior.is_read_only);
        assert!(def.behavior.is_concurrency_safe);
        assert!(!def.behavior.is_destructive);
    }

    #[test]
    fn builder_destructive_sets_behavior() {
        let def = CapabilityDefinitionBuilder::new()
            .id("fs_delete")
            .name("Delete")
            .description("Delete a file")
            .connector_id("local")
            .destructive()
            .build()
            .unwrap();
        assert!(def.behavior.is_destructive);
        assert!(!def.behavior.is_read_only);
    }

    #[test]
    fn builder_tags_batch() {
        let def = CapabilityDefinitionBuilder::new()
            .id("x")
            .name("X")
            .description("x")
            .connector_id("c")
            .tags(vec!["a", "b", "c"])
            .build()
            .unwrap();
        assert_eq!(def.tags, vec!["a", "b", "c"]);
    }

    // --- to_llm_tool tests ---

    #[test]
    fn to_llm_tool_maps_fields_correctly() {
        let def = sample_definition();
        let tool = def.to_llm_tool();
        assert_eq!(tool.tool_type, "function");
        assert_eq!(tool.function.name, "fs_read");
        assert_eq!(tool.function.description, "Read a file from disk");
        assert_eq!(tool.function.parameters, sample_input_schema());
    }

    #[test]
    fn to_llm_tool_with_empty_schema() {
        let def = CapabilityDefinitionBuilder::new()
            .id("noop")
            .name("No-op")
            .description("Does nothing")
            .connector_id("c")
            .build()
            .unwrap();
        let tool = def.to_llm_tool();
        assert_eq!(tool.function.parameters, json!({}));
    }

    // --- to_mcp_tool tests ---

    #[test]
    fn to_mcp_tool_produces_standard_format() {
        let def = sample_definition();
        let mcp = def.to_mcp_tool();
        assert_eq!(mcp["name"], "fs_read");
        assert_eq!(mcp["description"], "Read a file from disk");
        assert_eq!(mcp["inputSchema"], sample_input_schema());
    }

    #[test]
    fn to_mcp_tool_with_empty_schema() {
        let def = CapabilityDefinitionBuilder::new()
            .id("ping")
            .name("Ping")
            .description("Health check")
            .connector_id("c")
            .build()
            .unwrap();
        let mcp = def.to_mcp_tool();
        assert_eq!(mcp["inputSchema"], json!({}));
    }

    // --- Serde roundtrip ---

    #[test]
    fn serde_roundtrip() {
        let def = sample_definition();
        let json_str = serde_json::to_string(&def).unwrap();
        let restored: CapabilityDefinition = serde_json::from_str(&json_str).unwrap();
        assert_eq!(def, restored);
    }

    // --- Probe trait mock test ---

    struct MockProbe {
        capabilities: Vec<CapabilityDefinition>,
    }

    impl ConnectorCapabilityProbe for MockProbe {
        fn list_capabilities(&self) -> Vec<CapabilityDefinition> {
            self.capabilities.clone()
        }

        fn get_capability(&self, id: &str) -> Option<CapabilityDefinition> {
            self.capabilities.iter().find(|c| c.id == id).cloned()
        }
    }

    #[test]
    fn probe_list_and_get() {
        let probe = MockProbe {
            capabilities: vec![sample_definition()],
        };
        let list = probe.list_capabilities();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "fs_read");

        let found = probe.get_capability("fs_read");
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "File Read");

        let missing = probe.get_capability("nonexistent");
        assert!(missing.is_none());
    }
}
