//! §4 — authoritative facade declarations for `mcp.*` capabilities.
//!
//! `BuiltinToolRegistry::default_facades` MUST NOT duplicate the `mcp_call`
//! entry; the host binary registers it via `capability_facades()`.

use cyberclaw_core::capability::RiskLevel;
use cyberclaw_core::facade::{CapabilityFacade, FacadeExposure, ToolsetCategory};
use cyberclaw_core::ids::{CapabilityId, ConnectorId};

/// Returns the facade list for the MCP connector.
#[allow(dead_code)]
pub fn capability_facades() -> Vec<(CapabilityFacade, ToolsetCategory)> {
    let connector_id = ConnectorId::from_string("mcp".to_string()).unwrap();
    vec![
        (
            CapabilityFacade {
                name: "mcp_call".to_string(),
                description: "Invoke a tool exposed by an MCP (Model Context Protocol) server. \
                    The server and tool are identified by name; arguments are passed as a JSON object.".to_string(),
                connector_id: connector_id.clone(),
                capability_id: CapabilityId::from_string("tool_call".to_string()).unwrap(),
                risk_level: RiskLevel::Medium,
                effects: vec!["write".to_string()],
                read_only: false,
                destructive: false,
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "server": { "type": "string", "description": "Name of the MCP server to call" },
                        "tool": { "type": "string", "description": "Name of the tool on the MCP server" },
                        "arguments": { "type": "object", "description": "Arguments to pass to the MCP tool" }
                    },
                    "required": ["server", "tool"]
                })),
                exposure: FacadeExposure::LlmDefault,
                workspace_root: None,
            },
            ToolsetCategory::Mcp,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_capability_facades_has_one_entry() {
        let facades = capability_facades();
        assert_eq!(facades.len(), 1);
        let (facade, cat) = &facades[0];
        assert_eq!(facade.name, "mcp_call");
        assert_eq!(facade.capability_id.as_str(), "tool_call");
        assert_eq!(*cat, ToolsetCategory::Mcp);
    }
}
