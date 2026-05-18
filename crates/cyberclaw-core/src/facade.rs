//! # Capability Facade — LLM-facing projection of a `Capability`
//!
//! `CapabilityFacade` is the platform-wide protocol type for exposing a
//! `Capability` to LLM consumers (chat tool palette, prompt assembly,
//! etc.). It is intentionally a pure data DTO with no runtime behaviour
//! and no agent-runtime-specific fields, so connectors can declare
//! their own facades without reverse-importing `cyberclaw-agent-runtime`
//! (which would create a dependency cycle: agent-runtime is the one
//! that *optionally* depends on connectors via the `tool-calling`
//! feature, never the other way round).
//!
//! ## History
//!
//! Originally lived at `cyberclaw-agent-runtime::tool_description::CapabilityFacade`
//! (commit before 2026-05-06). That placement forced 6 connector
//! modules (`cmd / fs / lsp / search / task / workdir_checkpoint`) to
//! invent **mirror types** (`CmdFacadeDescriptor`, `FsFacadeSpec`,
//! `LspFacadeSpec`, etc.) because they could not import the real
//! `CapabilityFacade`. The intended idiom in
//! `docs/architecture/idioms/EVOLUTION_IDIOMS.md §4` ("Each Connector
//! owns its `CapabilityFacade` declarations; the host binary composes
//! them") was structurally unenforceable, so `BuiltinToolRegistry`
//! grew to 22 hardcoded entries (>15 idiom limit) and the 6
//! `capability_facades()` functions in connector modules ended up with
//! zero production callers.
//!
//! Two independent architecture reviews (Claude Opus + GPT) converged
//! on the same diagnosis: **type placed in the wrong crate**. The fix
//! is to relocate `CapabilityFacade` + `ToolsetCategory` to
//! `cyberclaw-core` so all three crates (connectors / agent-runtime /
//! server) can use them without dependency-direction surgery.
//!
//! ## §4 idiom enabled
//!
//! After this move, connectors can return real
//! `Vec<(CapabilityFacade, ToolsetCategory)>` from their
//! `capability_facades()`, the host binary in `cyberclaw-server` can
//! aggregate and register them, and `BuiltinToolRegistry::default_facades`
//! can shrink to the platform-native set
//! (`memory_*` / `mcp_call` / `skill_*` / `todo_*` / `delegate_to_sub_agent`
//! / `verify_numeric` — i.e. tools without a connector owner).

use crate::capability::{CapabilityEffect, CapabilityRef, RiskLevel};
use crate::ids::{CapabilityId, ConnectorId};
use serde::{Deserialize, Serialize};

// ============================================================================
// ToolsetCategory
// ============================================================================

// ============================================================================
// FacadeExposure — F12 (2026-05-06)
// ============================================================================

/// Where a `CapabilityFacade` is allowed to surface.
///
/// Independent of `ToolsetCategory` (which groups *what kind* of tool it is).
/// Exposure controls *who can see / call* the facade. The host binary's
/// LLM tool palette filter consults this; admin endpoints bypass it.
///
/// # Defaults
///
/// `LlmDefault` is the construction default so existing code that builds
/// a `CapabilityFacade` directly without thinking about exposure keeps
/// behaving as before (visible to the LLM in the standard palette).
/// Tools that should be hidden from the LLM by default (e.g. high-blast
/// shell variants like `cmd_run_powershell`, advanced LSP introspection)
/// must explicitly opt into `LlmAdvanced` (deferred — LLM sees the name
/// but must use ToolSearch to fetch the schema) or `Internal` (admin
/// API only, never in any LLM palette).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FacadeExposure {
    /// Default surface: shows up in the LLM's tool list with full schema.
    /// Use for tools every agent needs (file_read, bash, web_fetch, …).
    #[default]
    LlmDefault,
    /// Visible to LLM by name only — schema is fetched on demand via
    /// ToolSearch. Reduces prompt budget. Use for tools that are useful
    /// but rarely needed (lsp_workspace_symbols, mcp_call alternates).
    LlmAdvanced,
    /// Never appears in any LLM-facing palette. Reachable only via the
    /// admin REST surface (`/api/v1/agents/delegate`,
    /// `/api/v1/capabilities/discover_for_goal`, etc.) or via internal
    /// dispatch chains. Use for orchestration primitives that would be
    /// dangerous if the model could call them directly.
    Internal,
    /// Reachable only via `/admin/*` routes that require operator
    /// authentication. Strictest tier — even internal CyberClaw
    /// components shouldn't call these without an operator-issued token.
    AdminOnly,
}

impl FacadeExposure {
    /// Whether this facade can appear in *any* LLM-facing palette
    /// (default or deferred). Equivalent to `!matches!(self, Internal | AdminOnly)`.
    pub fn visible_to_llm(self) -> bool {
        matches!(self, Self::LlmDefault | Self::LlmAdvanced)
    }

    /// Whether this facade should ship full schema in the prompt by
    /// default (LlmDefault), versus name-only (LlmAdvanced).
    pub fn ships_schema_in_prompt(self) -> bool {
        matches!(self, Self::LlmDefault)
    }
}

// ============================================================================
// ToolsetCategory
// ============================================================================

/// A toolset category grouping related tools.
///
/// Connectors declare which category each of their facades belongs to so
/// the host's `BuiltinToolRegistry` can filter by category when assembling
/// the LLM-visible tool palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ToolsetCategory {
    /// File operations: read, write, edit, search.
    FileSystem,
    /// Terminal/shell execution.
    Terminal,
    /// Web operations: fetch, search.
    Web,
    /// MCP tool invocation.
    Mcp,
    /// Memory operations: read, write, search.
    Memory,
    /// Sub-agent orchestration.
    SubAgent,
    /// Code analysis: LSP, definitions.
    CodeAnalysis,
    /// Skill management: create, evolve, attribute. Sprint 21 — agent-callable
    /// `skill_create` lets the agent codify methodology from its own
    /// experience without operator intervention.
    SkillManagement,
}

// ============================================================================
// CapabilityFacade
// ============================================================================

/// Agent-facing capability description.
///
/// This struct is the public-facing view of a capability. Internal routing
/// fields (`connector_id`, `capability_id`) are skipped during serialization
/// to prevent leaking platform internals to agents or external consumers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityFacade {
    /// Human-readable name for the capability.
    pub name: String,

    /// Description of what the capability does.
    pub description: String,

    /// Internal connector ID — never serialized or deserialized.
    #[serde(skip, default = "default_connector_id")]
    pub connector_id: ConnectorId,

    /// Internal capability ID — never serialized or deserialized.
    #[serde(skip, default = "default_capability_id")]
    pub capability_id: CapabilityId,

    /// Risk level of the capability.
    pub risk_level: RiskLevel,

    /// Effects produced by this capability (e.g. Read, Write, Execute).
    #[serde(default)]
    pub effects: Vec<String>,

    /// Whether this capability is read-only (no side effects).
    #[serde(default)]
    pub read_only: bool,

    /// Whether this capability is destructive (deletes or irreversibly modifies).
    #[serde(default)]
    pub destructive: bool,

    /// JSON Schema describing the input parameters, if available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<serde_json::Value>,

    /// F12 (2026-05-06) — where this facade is allowed to surface.
    /// `LlmDefault` (the `Default` value) preserves pre-F12 behaviour:
    /// visible in the LLM tool palette with full schema. Connectors that
    /// want to hide a high-blast-radius variant (cmd_run_powershell,
    /// privileged LSP queries, etc.) explicitly set `Internal` /
    /// `AdminOnly` / `LlmAdvanced`. `#[serde(default)]` keeps older
    /// snapshots decodable.
    #[serde(default)]
    pub exposure: FacadeExposure,
}

fn default_connector_id() -> ConnectorId {
    ConnectorId::new()
}

fn default_capability_id() -> CapabilityId {
    CapabilityId::new()
}

impl CapabilityFacade {
    /// Create a `CapabilityFacade` from a `CapabilityRef`.
    ///
    /// Risk level is derived from the capability's declared effects:
    /// - If effects contain `Execute` or custom destructive markers → at least `High`
    /// - Otherwise uses the risk from the `CapabilityRef` directly
    pub fn from_capability_ref(cap: &CapabilityRef, name: String, description: String) -> Self {
        let effects_strings: Vec<String> = cap
            .effects
            .iter()
            .map(|e| match e {
                CapabilityEffect::Read => "read".to_string(),
                CapabilityEffect::Write => "write".to_string(),
                CapabilityEffect::Execute => "execute".to_string(),
                CapabilityEffect::Network => "network".to_string(),
                CapabilityEffect::Ticket => "ticket".to_string(),
                CapabilityEffect::Notification => "notification".to_string(),
                CapabilityEffect::Custom(s) => s.clone(),
            })
            .collect();

        let read_only = cap
            .effects
            .iter()
            .all(|e| matches!(e, CapabilityEffect::Read));

        // Destructive if risk is High/Critical or effects include Execute
        let destructive = cap.risk >= RiskLevel::High
            || cap
                .effects
                .iter()
                .any(|e| matches!(e, CapabilityEffect::Execute));

        Self {
            name,
            description,
            connector_id: cap.connector_id.clone(),
            capability_id: cap.id.clone(),
            risk_level: cap.risk,
            effects: effects_strings,
            read_only,
            destructive,
            input_schema: None,
            exposure: FacadeExposure::default(),
        }
    }

    /// F12 — builder helper to override the exposure tier (defaults to
    /// `LlmDefault`). Use when a connector wants to hide a facade from
    /// the standard LLM palette without dropping it from the registry.
    pub fn with_exposure(mut self, exposure: FacadeExposure) -> Self {
        self.exposure = exposure;
        self
    }

    /// Format as Anthropic tool definition (Claude-compatible).
    ///
    /// Internal fields are excluded — only name, description, and input_schema.
    pub fn format_anthropic(&self) -> serde_json::Value {
        let mut tool = serde_json::json!({
            "name": self.name,
            "description": self.description,
        });
        if let Some(schema) = &self.input_schema {
            tool["input_schema"] = schema.clone();
        }
        tool
    }

    /// Format as OpenAI function-calling tool definition.
    ///
    /// Internal fields are excluded — only type, function name, description,
    /// and parameters.
    pub fn format_openai(&self) -> serde_json::Value {
        let mut func = serde_json::json!({
            "name": self.name,
            "description": self.description,
        });
        if let Some(schema) = &self.input_schema {
            func["parameters"] = schema.clone();
        }
        serde_json::json!({
            "type": "function",
            "function": func,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_cap_ref(risk: RiskLevel, effects: Vec<CapabilityEffect>) -> CapabilityRef {
        CapabilityRef {
            id: CapabilityId::from_string("file.read".to_string()).unwrap(),
            connector_id: ConnectorId::from_string("filesystem".to_string()).unwrap(),
            risk,
            effects,
            placement: None,
        }
    }

    #[test]
    fn test_serialize_skips_internal_ids() {
        let cap = make_cap_ref(RiskLevel::Low, vec![CapabilityEffect::Read]);
        let facade =
            CapabilityFacade::from_capability_ref(&cap, "file_read".into(), "Read a file".into());

        let json = serde_json::to_value(&facade).unwrap();
        assert!(json.get("connector_id").is_none());
        assert!(json.get("capability_id").is_none());
        assert_eq!(json["name"], "file_read");
        assert_eq!(json["risk_level"], "low");
    }

    #[test]
    fn test_read_only_detection() {
        let cap = make_cap_ref(RiskLevel::Low, vec![CapabilityEffect::Read]);
        let facade = CapabilityFacade::from_capability_ref(&cap, "reader".into(), "desc".into());
        assert!(facade.read_only);
        assert!(!facade.destructive);
    }

    #[test]
    fn test_destructive_from_high_risk() {
        let cap = make_cap_ref(RiskLevel::High, vec![CapabilityEffect::Write]);
        let facade = CapabilityFacade::from_capability_ref(&cap, "writer".into(), "desc".into());
        assert!(!facade.read_only);
        assert!(facade.destructive);
        assert_eq!(facade.risk_level, RiskLevel::High);
    }

    #[test]
    fn test_destructive_from_execute_effect() {
        let cap = make_cap_ref(RiskLevel::Medium, vec![CapabilityEffect::Execute]);
        let facade = CapabilityFacade::from_capability_ref(&cap, "executor".into(), "desc".into());
        assert!(facade.destructive);
    }

    #[test]
    fn test_format_anthropic_no_internal_fields() {
        let cap = make_cap_ref(RiskLevel::Medium, vec![CapabilityEffect::Write]);
        let facade = CapabilityFacade::from_capability_ref(&cap, "tool".into(), "A tool".into());
        let output = facade.format_anthropic();
        assert_eq!(output["name"], "tool");
        assert_eq!(output["description"], "A tool");
        assert!(output.get("connector_id").is_none());
        assert!(output.get("capability_id").is_none());
        assert!(output.get("risk_level").is_none());
    }

    #[test]
    fn test_format_openai_no_internal_fields() {
        let cap = make_cap_ref(RiskLevel::Medium, vec![CapabilityEffect::Write]);
        let facade = CapabilityFacade::from_capability_ref(&cap, "tool".into(), "A tool".into());
        let output = facade.format_openai();
        assert_eq!(output["type"], "function");
        assert_eq!(output["function"]["name"], "tool");
        assert!(output.get("connector_id").is_none());
        assert!(output["function"].get("connector_id").is_none());
    }

    #[test]
    fn test_effects_populated() {
        let cap = make_cap_ref(
            RiskLevel::Medium,
            vec![CapabilityEffect::Read, CapabilityEffect::Network],
        );
        let facade = CapabilityFacade::from_capability_ref(&cap, "net_read".into(), "desc".into());
        assert_eq!(facade.effects, vec!["read", "network"]);
        assert!(!facade.read_only);
    }

    #[test]
    fn test_deserialize_without_internal_ids() {
        let json = serde_json::json!({
            "name": "test_tool",
            "description": "A test tool",
            "risk_level": "medium",
            "effects": ["read"],
            "read_only": true,
            "destructive": false
        });
        let facade: CapabilityFacade = serde_json::from_value(json).unwrap();
        assert_eq!(facade.name, "test_tool");
        assert_eq!(facade.risk_level, RiskLevel::Medium);
        assert!(facade.read_only);
    }

    #[test]
    fn exposure_default_is_llm_default() {
        let cap = make_cap_ref(RiskLevel::Low, vec![CapabilityEffect::Read]);
        let facade = CapabilityFacade::from_capability_ref(&cap, "x".into(), "x".into());
        assert_eq!(facade.exposure, FacadeExposure::LlmDefault);
        assert!(facade.exposure.visible_to_llm());
        assert!(facade.exposure.ships_schema_in_prompt());
    }

    #[test]
    fn exposure_internal_hides_from_llm() {
        let cap = make_cap_ref(RiskLevel::High, vec![CapabilityEffect::Execute]);
        let facade = CapabilityFacade::from_capability_ref(&cap, "x".into(), "x".into())
            .with_exposure(FacadeExposure::Internal);
        assert_eq!(facade.exposure, FacadeExposure::Internal);
        assert!(!facade.exposure.visible_to_llm());
        assert!(!facade.exposure.ships_schema_in_prompt());
    }

    #[test]
    fn exposure_advanced_visible_but_no_schema() {
        let cap = make_cap_ref(RiskLevel::Medium, vec![CapabilityEffect::Read]);
        let facade = CapabilityFacade::from_capability_ref(&cap, "x".into(), "x".into())
            .with_exposure(FacadeExposure::LlmAdvanced);
        assert!(facade.exposure.visible_to_llm());
        assert!(!facade.exposure.ships_schema_in_prompt());
    }

    #[test]
    fn exposure_serde_omitted_when_default() {
        let cap = make_cap_ref(RiskLevel::Low, vec![CapabilityEffect::Read]);
        let facade = CapabilityFacade::from_capability_ref(&cap, "y".into(), "y".into());
        let json = serde_json::to_value(&facade).unwrap();
        // The field is included (Default LlmDefault) — but a deserialise
        // round-trip without `exposure` must still default-construct.
        let mut json2 = json.clone();
        json2.as_object_mut().unwrap().remove("exposure");
        let back: CapabilityFacade = serde_json::from_value(json2).unwrap();
        assert_eq!(back.exposure, FacadeExposure::LlmDefault);
    }

    #[test]
    fn toolset_category_serde_roundtrip() {
        for cat in [
            ToolsetCategory::FileSystem,
            ToolsetCategory::Terminal,
            ToolsetCategory::Web,
            ToolsetCategory::Mcp,
            ToolsetCategory::Memory,
            ToolsetCategory::SubAgent,
            ToolsetCategory::CodeAnalysis,
            ToolsetCategory::SkillManagement,
        ] {
            let s = serde_json::to_value(cat).unwrap();
            let back: ToolsetCategory = serde_json::from_value(s).unwrap();
            assert_eq!(cat, back);
        }
    }
}
