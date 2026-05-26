//! Builtin Tool Registry
//!
//! Defines the default tool set available to agents as `CapabilityFacade` instances.
//! Each facade maps to an existing Connector + Capability pair, providing a read-only
//! LLM-visible view without introducing Tool as a first-class platform object.
//!
//! # Design
//!
//! The builtin tool set is inspired by the tool registries of Claude Code (37+ tools),
//! Cline (20+ tools), and Hermes Agent (self-registering tool modules). Each tool is
//! represented as a `CapabilityFacade` that bridges the Connector/Capability layer
//! with the agent-visible tool interface.
//!
//! # Categories
//!
//! Tools are grouped into `ToolsetCategory` values: FileSystem, Terminal, Web, Mcp,
//! Memory, SubAgent, and CodeAnalysis. Agents receive tools based on their
//! `ToolsetConfig`, which specifies enabled categories and explicit exclusions.

use super::tool_description::CapabilityFacade;
use cyberclaw_core::capability::RiskLevel;
use cyberclaw_core::facade::FacadeExposure;
use cyberclaw_core::ids::{CapabilityId, ConnectorId};
// `Deserialize` / `Serialize` were imported here for the local
// ToolsetCategory derive; that type now lives in cyberclaw-core::facade
// (re-exported above), so this import is no longer needed.
use std::collections::HashSet;

/// Helper to build a `CapabilityFacade` from simple parameters.
/// Derives `effects`, `read_only`, and `destructive` from `risk_level`.
fn make_facade(
    name: &str,
    description: &str,
    input_schema: serde_json::Value,
    connector: &str,
    capability: &str,
    risk_level: RiskLevel,
) -> CapabilityFacade {
    let read_only = risk_level == RiskLevel::Low;
    let destructive = risk_level >= RiskLevel::High;
    let effects = if read_only {
        vec!["read".to_string()]
    } else if destructive {
        vec!["execute".to_string()]
    } else {
        vec!["write".to_string()]
    };
    CapabilityFacade {
        name: name.to_string(),
        description: description.to_string(),
        input_schema: Some(input_schema),
        connector_id: ConnectorId::from_string(connector.to_string()).unwrap(),
        capability_id: CapabilityId::from_string(capability.to_string()).unwrap(),
        risk_level,
        effects,
        read_only,
        destructive,
        exposure: FacadeExposure::LlmDefault,
        workspace_root: None,
    }
}

// ---------------------------------------------------------------------------
// ToolsetCategory
// ---------------------------------------------------------------------------

// `ToolsetCategory` lives in `cyberclaw-core::facade` since 2026-05-06
// (F8 architecture review). Re-exported here so existing
// `cyberclaw_agent_runtime::builtin_tools::ToolsetCategory` imports keep
// working. New code should prefer `cyberclaw_core::facade::ToolsetCategory`.
pub use cyberclaw_core::facade::ToolsetCategory;

// ---------------------------------------------------------------------------
// CategorisedFacade (internal pairing)
// ---------------------------------------------------------------------------

/// Internal entry pairing a `CapabilityFacade` with its `ToolsetCategory`.
///
/// `CapabilityFacade` is defined in `tool_description` without a category field
/// (it is a pure LLM-view). This wrapper adds the category for registry filtering.
#[derive(Debug, Clone)]
struct CategorisedFacade {
    facade: CapabilityFacade,
    category: ToolsetCategory,
}

// ---------------------------------------------------------------------------
// ToolsetConfig
// ---------------------------------------------------------------------------

/// Configuration for which tool sets an agent has access to.
#[derive(Debug, Clone)]
pub struct ToolsetConfig {
    /// Enabled categories.
    pub enabled_categories: HashSet<ToolsetCategory>,
    /// Additional custom facades (always included regardless of category).
    pub custom_facades: Vec<CapabilityFacade>,
    /// Explicitly disabled tool names (applied after category filtering).
    pub disabled_tools: HashSet<String>,
}

impl ToolsetConfig {
    /// Returns the default configuration.
    ///
    /// Enables FileSystem, Terminal, Web, Memory, and SkillManagement
    /// categories. Mcp and CodeAnalysis must be opted in explicitly.
    ///
    /// SubAgent: see TODO(human) below — F5 closure shipped
    /// `delegate_to_sub_agent` as a default platform primitive but the
    /// category filter was never updated, leaving the tool unreachable
    /// from the LLM. Pick option A (enable SubAgent here) or option B
    /// (special-case the 3 chat-intercepted facades in state.rs seeding).
    pub fn default_config() -> Self {
        let mut enabled = HashSet::new();
        enabled.insert(ToolsetCategory::FileSystem);
        enabled.insert(ToolsetCategory::Terminal);
        enabled.insert(ToolsetCategory::Web);
        enabled.insert(ToolsetCategory::Memory);
        enabled.insert(ToolsetCategory::SkillManagement);
        // F5 closure (2026-05-06): SubAgent is a platform primitive — the
        // `delegate_to_sub_agent` facade is intercepted in chat_handler and
        // backed by SubAgentOrchestrator. Without this enable, the facade
        // is declared but never reaches the LLM tool palette.
        enabled.insert(ToolsetCategory::SubAgent);
        Self {
            enabled_categories: enabled,
            custom_facades: Vec::new(),
            disabled_tools: HashSet::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// BuiltinToolRegistry
// ---------------------------------------------------------------------------

/// Registry of default builtin tools.
///
/// Holds a list of `CapabilityFacade` entries (each tagged with a category)
/// that map to existing Connector capabilities. Use `with_defaults()` to
/// create a registry pre-populated with the standard tool set.
#[derive(Debug, Clone)]
pub struct BuiltinToolRegistry {
    entries: Vec<CategorisedFacade>,
}

impl BuiltinToolRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Create a registry pre-populated with all default builtin tools.
    pub fn with_defaults() -> Self {
        let mut registry = Self::new();
        for (facade, category) in default_facades() {
            registry
                .entries
                .push(CategorisedFacade { facade, category });
        }
        registry
    }

    /// Register a custom capability facade under a given category.
    pub fn register(&mut self, facade: CapabilityFacade, category: ToolsetCategory) {
        self.entries.push(CategorisedFacade { facade, category });
    }

    /// Return facades filtered by the given `ToolsetConfig`.
    ///
    /// The result includes:
    /// 1. Registry facades whose category is in `config.enabled_categories`
    ///    and whose name is not in `config.disabled_tools`.
    /// 2. Custom facades from the config (always included).
    pub fn get_facades(&self, config: &ToolsetConfig) -> Vec<CapabilityFacade> {
        let mut result: Vec<CapabilityFacade> = self
            .entries
            .iter()
            .filter(|e| config.enabled_categories.contains(&e.category))
            .filter(|e| !config.disabled_tools.contains(&e.facade.name))
            .map(|e| e.facade.clone())
            .collect();
        result.extend(config.custom_facades.iter().cloned());
        result
    }

    /// Return all registered facades.
    pub fn get_all(&self) -> Vec<&CapabilityFacade> {
        self.entries.iter().map(|e| &e.facade).collect()
    }

    /// Look up a facade by tool name.
    pub fn get_by_name(&self, name: &str) -> Option<&CapabilityFacade> {
        self.entries
            .iter()
            .find(|e| e.facade.name == name)
            .map(|e| &e.facade)
    }

    /// Return all facades belonging to a given category.
    pub fn get_by_category(&self, category: ToolsetCategory) -> Vec<&CapabilityFacade> {
        self.entries
            .iter()
            .filter(|e| e.category == category)
            .map(|e| &e.facade)
            .collect()
    }

    /// Return the category for a given tool name.
    pub fn get_category(&self, name: &str) -> Option<ToolsetCategory> {
        self.entries
            .iter()
            .find(|e| e.facade.name == name)
            .map(|e| e.category)
    }

    /// Return the default `ToolsetConfig`.
    pub fn default_config() -> ToolsetConfig {
        ToolsetConfig::default_config()
    }
}

impl Default for BuiltinToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Default facade definitions
// ---------------------------------------------------------------------------

/// Build the minimal set of default capability facades with their categories.
///
/// F12 Phase C (2026-05-06) — connector-owned modules now declare their own
/// facades via `capability_facades()` and the host binary (state.rs) aggregates
/// them into `DeferredToolRegistry`. This function retains ONLY facades that
/// have no connector owner and are intercepted inline by chat_handler:
///
///   - `skill_create`       → chat_handler inline intercept (SkillHub lives in server)
///   - `skill_search`       → chat_handler inline intercept
///   - `delegate_to_sub_agent` → chat_handler inline intercept
///     (SubAgentOrchestrator is request-scoped)
///
/// All other facades (file_*, bash, web_*, browser_*, mcp_call, memory_*,
/// lsp.*, todo_*, verify_numeric) are now declared by their owner connectors
/// and registered by the server startup path. They MUST NOT be duplicated here.
fn default_facades() -> Vec<(CapabilityFacade, ToolsetCategory)> {
    vec![
        // -- Skill management (Sprint 21) -------------------------------------
        // skill_create lets the agent codify a successful methodology from
        // its own experience — closes the Hermes-style learning loop in a
        // platform-native way. The dispatch is intercepted in
        // chat_handler.rs (not routed through the connector graph) because
        // SkillHub lives in cyberclaw-server and the connectors crate
        // intentionally doesn't depend on the server. The
        // `connector_id="skill"` marker is informational only — the
        // server-side intercept handles it before the dispatcher sees it.
        (
            make_facade(
                "skill_create",
                "Codify a successful methodology as a reusable Skill bundle. \
                 Use this AFTER you have completed a non-trivial task and \
                 noticed a generalizable pattern (e.g., the deck-authoring \
                 4-part concept slide pattern; the resilience-by-fallback \
                 reflex). The skill becomes available to all future agents \
                 via skill_ids binding. Provenance (source request id) is \
                 attached automatically.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "Slug, ^[a-z][a-z0-9-]{2,31}$. Examples: 'marp-deck-crafting', 'rust-bug-triage'."
                        },
                        "description": {
                            "type": "string",
                            "description": "One-paragraph natural-language description of what the skill does and when it should fire."
                        },
                        "methodology": {
                            "type": "string",
                            "description": "The SKILL.md body in Markdown. Should include When triggered, Steps, Reporting contract, Anti-patterns sections."
                        },
                        "trigger_examples": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "3-5 example user prompts that should trigger this skill, for later eval-based optimization."
                        }
                    },
                    "required": ["name", "description", "methodology"]
                }),
                "skill",
                "skill_create",
                RiskLevel::Medium,
            ),
            ToolsetCategory::SkillManagement,
        ),
        // Sprint 21 path #3 — agent searches the skill registry before
        // creating duplicates or to find a relevant existing skill for
        // the current task. Backed by FTS5 full-text search over name +
        // description + category.
        (
            make_facade(
                "skill_search",
                "Search the installed skill registry by natural-language query. \
                 Use BEFORE skill_create to check whether a similar skill \
                 already exists. Use during planning to find skills relevant \
                 to the current task that you could bind via skill_ids in a \
                 future call. Empty query returns most-recently-installed.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "q": {
                            "type": "string",
                            "description": "Free-text query. Examples: 'deck authoring', 'rust debugging', 'memory'."
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Max results to return. Default 20."
                        }
                    },
                    "required": ["q"]
                }),
                "skill",
                "skill_search",
                RiskLevel::Low,
            ),
            ToolsetCategory::SkillManagement,
        ),
        // -- SubAgent (F5 closure 2026-05-06) -------------------------------
        // delegate_to_sub_agent lets the LLM autonomously spawn a child
        // agent for a focused subtask. Backed by SubAgentOrchestrator
        // which enforces depth limit 3 + max children 5 + budget fraction
        // 0.5. Chat handler intercepts this tool name inline (sub-agent
        // construction is per-request because caller_identity is request
        // -scoped) and routes through SubAgentOrchestrator::spawn_child +
        // run_child. Risk: High — children can recurse.
        (
            make_facade(
                "delegate_to_sub_agent",
                "Spawn a focused child agent to handle a delegated subtask. The child runs an \
                 independent agentic loop with its own LLM context and budget (50% of parent's \
                 remaining), then returns its final text. Use this when a goal naturally \
                 decomposes into sub-goals that benefit from a fresh context window. Depth \
                 capped at 3, max 5 concurrent children per parent.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "task_description": {
                            "type": "string",
                            "description": "Plain-language subtask the child must complete."
                        },
                        "max_iterations": {
                            "type": "integer",
                            "description": "Optional iteration budget for the child (default 8)."
                        }
                    },
                    "required": ["task_description"]
                }),
                "agent",
                "agent.delegate",
                RiskLevel::High,
            ),
            ToolsetCategory::SubAgent,
        ),
    ]
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// F12 Phase C (2026-05-06) — `default_facades()` now contains ONLY the 3
    /// chat_handler-intercepted facades. All connector-owned facades (file_*,
    /// bash, web_*, browser_*, mcp_call, memory_*, lsp.*, todo_*, verify_numeric)
    /// are registered by the server startup path via each connector's
    /// `capability_facades()`. This test verifies the shrunken set.
    #[test]
    fn with_defaults_contains_all_default_tools() {
        let registry = BuiltinToolRegistry::with_defaults();
        let all = registry.get_all();
        let names: Vec<&str> = all.iter().map(|f| f.name.as_str()).collect();
        let expected = [
            "skill_create",
            "skill_search",
            // F5 closure (2026-05-06) — sub-agent delegation facade.
            "delegate_to_sub_agent",
        ];
        for name in &expected {
            assert!(names.contains(name), "missing default tool: {name}");
        }
        assert_eq!(all.len(), expected.len());
    }

    #[test]
    fn get_by_name_finds_skill_create() {
        // F12 Phase C: default registry only has the 3 chat_handler-intercepted
        // facades. skill_create is the canonical check for "registry works".
        let registry = BuiltinToolRegistry::with_defaults();
        let facade = registry.get_by_name("skill_create");
        assert!(facade.is_some());
        let f = facade.unwrap();
        assert_eq!(f.connector_id.as_str(), "skill");
        assert_eq!(f.capability_id.as_str(), "skill_create");
    }

    #[test]
    fn get_by_name_returns_none_for_unknown() {
        let registry = BuiltinToolRegistry::with_defaults();
        assert!(registry.get_by_name("nonexistent_tool").is_none());
    }

    // F12 Phase C: FileSystem / Terminal facades are now connector-owned.
    // These category tests now use the SkillManagement category which IS
    // still in default_facades.
    #[test]
    fn get_by_category_skill_management_returns_two() {
        let registry = BuiltinToolRegistry::with_defaults();
        let tools = registry.get_by_category(ToolsetCategory::SkillManagement);
        assert_eq!(tools.len(), 2, "expected skill_create + skill_search");
        let names: Vec<&str> = tools.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"skill_create"));
        assert!(names.contains(&"skill_search"));
    }

    #[test]
    fn get_by_category_filesystem_returns_zero_after_f12_phase_c() {
        // FileSystem facades are now connector-owned; default registry is empty
        // for this category. Server startup adds them via fs::capability_facades().
        let registry = BuiltinToolRegistry::with_defaults();
        let fs_tools = registry.get_by_category(ToolsetCategory::FileSystem);
        assert_eq!(
            fs_tools.len(),
            0,
            "FileSystem facades moved to connector; default registry should be empty"
        );
    }

    #[test]
    fn toolset_config_disabling_skill_management_excludes_skill_create() {
        let registry = BuiltinToolRegistry::with_defaults();
        let mut config = ToolsetConfig::default_config();
        config
            .enabled_categories
            .remove(&ToolsetCategory::SkillManagement);
        let facades = registry.get_facades(&config);
        assert!(
            !facades.iter().any(|f| f.name == "skill_create"),
            "skill_create should be excluded when SkillManagement is disabled"
        );
    }

    #[test]
    fn disabled_tools_excludes_specific_tool() {
        let registry = BuiltinToolRegistry::with_defaults();
        let mut config = ToolsetConfig::default_config();
        config.disabled_tools.insert("skill_create".into());
        let facades = registry.get_facades(&config);
        assert!(
            !facades.iter().any(|f| f.name == "skill_create"),
            "skill_create should be excluded via disabled_tools"
        );
        assert!(facades.iter().any(|f| f.name == "skill_search"));
    }

    #[test]
    fn custom_facades_are_included() {
        let registry = BuiltinToolRegistry::with_defaults();
        let mut config = ToolsetConfig::default_config();
        config.custom_facades.push(make_facade(
            "custom_tool",
            "A custom tool",
            serde_json::json!({"type": "object", "properties": {}}),
            "custom",
            "do_thing",
            RiskLevel::Low,
        ));
        let facades = registry.get_facades(&config);
        assert!(
            facades.iter().any(|f| f.name == "custom_tool"),
            "custom facade should be included"
        );
    }

    #[test]
    fn default_tool_input_schemas_are_valid_json_objects() {
        let registry = BuiltinToolRegistry::with_defaults();
        for facade in registry.get_all() {
            let schema = facade
                .input_schema
                .as_ref()
                .unwrap_or_else(|| panic!("input_schema for {} should be Some", facade.name));
            assert!(
                schema.is_object(),
                "input_schema for {} should be a JSON object",
                facade.name
            );
            let obj = schema.as_object().unwrap();
            assert_eq!(
                obj.get("type").and_then(|v| v.as_str()),
                Some("object"),
                "input_schema for {} should have type=object",
                facade.name
            );
            assert!(
                obj.contains_key("properties"),
                "input_schema for {} should contain 'properties'",
                facade.name
            );
        }
    }

    #[test]
    fn all_default_tools_have_correct_risk_levels() {
        let registry = BuiltinToolRegistry::with_defaults();

        // F12 Phase C: only 3 facades remain in default_facades.
        let cases = [
            ("skill_create", RiskLevel::Medium),
            ("skill_search", RiskLevel::Low),
            ("delegate_to_sub_agent", RiskLevel::High),
        ];

        for (name, expected_risk) in &cases {
            let facade = registry
                .get_by_name(name)
                .unwrap_or_else(|| panic!("tool {name} not found"));
            assert_eq!(
                &facade.risk_level, expected_risk,
                "risk level mismatch for {name}"
            );
        }
    }

    #[test]
    fn default_config_enables_expected_categories() {
        // Default categories (F5 closure 2026-05-06):
        //   FileSystem / Terminal / Web / Memory / SkillManagement / SubAgent
        // Mcp + CodeAnalysis still require explicit opt-in.
        let config = BuiltinToolRegistry::default_config();
        assert!(config
            .enabled_categories
            .contains(&ToolsetCategory::FileSystem));
        assert!(config
            .enabled_categories
            .contains(&ToolsetCategory::Terminal));
        assert!(config.enabled_categories.contains(&ToolsetCategory::Web));
        assert!(config.enabled_categories.contains(&ToolsetCategory::Memory));
        assert!(config
            .enabled_categories
            .contains(&ToolsetCategory::SkillManagement));
        assert!(config
            .enabled_categories
            .contains(&ToolsetCategory::SubAgent));
        assert!(!config.enabled_categories.contains(&ToolsetCategory::Mcp));
        assert!(!config
            .enabled_categories
            .contains(&ToolsetCategory::CodeAnalysis));
    }

    #[test]
    fn register_adds_custom_facade_to_registry() {
        let mut registry = BuiltinToolRegistry::new();
        assert_eq!(registry.get_all().len(), 0);
        registry.register(
            make_facade(
                "my_tool",
                "test",
                serde_json::json!({"type": "object", "properties": {}}),
                "test",
                "test_cap",
                RiskLevel::Low,
            ),
            ToolsetCategory::FileSystem,
        );
        assert_eq!(registry.get_all().len(), 1);
        assert!(registry.get_by_name("my_tool").is_some());
    }

    #[test]
    fn get_facades_with_default_config_excludes_mcp() {
        // F12 Phase C: mcp_call is now connector-owned; default registry has
        // no MCP entry. Verify it stays absent even after filtering.
        let registry = BuiltinToolRegistry::with_defaults();
        let config = ToolsetConfig::default_config();
        let facades = registry.get_facades(&config);
        assert!(
            !facades.iter().any(|f| f.name == "mcp_call"),
            "mcp_call should not appear in default config"
        );
    }

    #[test]
    fn get_category_returns_correct_category() {
        let registry = BuiltinToolRegistry::with_defaults();
        assert_eq!(
            registry.get_category("skill_create"),
            Some(ToolsetCategory::SkillManagement)
        );
        assert_eq!(
            registry.get_category("delegate_to_sub_agent"),
            Some(ToolsetCategory::SubAgent)
        );
        assert_eq!(registry.get_category("nonexistent"), None);
    }
}
