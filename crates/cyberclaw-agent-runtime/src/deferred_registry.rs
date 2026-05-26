//! Deferred Tool Registry
//!
//! Supports deferred tool loading to save prompt tokens. Tools can be in one of
//! two states: **Active** (full schema in prompt) or **Deferred** (name-only in
//! prompt, full schema loaded on-demand via search).
//!
//! Inspired by:
//! - Claude Code's `shouldDefer` / `alwaysLoad` / `ToolSearch` pattern
//! - The general concept of lazy tool loading to reduce context window usage
//!
//! # Design
//!
//! `DeferredToolRegistry` wraps `CapabilityFacade` entries with a `ToolLoadState`
//! tag. Active tools have their full schema injected into system prompts. Deferred
//! tools only expose their names; their schemas are fetched via `search()` when
//! the LLM needs them.
//!
//! An `active_limit` cap prevents unbounded prompt growth. `promote()` fails
//! when the limit is reached; callers should `demote()` unused tools first.

use std::collections::HashMap;

use super::tool_description::CapabilityFacade;

// ---------------------------------------------------------------------------
// ToolLoadState
// ---------------------------------------------------------------------------

/// Loading state of a tool in the registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolLoadState {
    /// Tool is active -- its full schema is included in the system prompt.
    Active,
    /// Tool is deferred -- only its name appears in the system prompt.
    /// Full schema is loaded on-demand via tool_search.
    Deferred,
}

// ---------------------------------------------------------------------------
// SearchMode
// ---------------------------------------------------------------------------

/// Query mode for searching deferred tools.
#[derive(Debug, Clone)]
pub enum SearchMode {
    /// Exact match by name.
    Exact(String),
    /// Substring / keyword match (case-insensitive).
    Keyword(String),
    /// Select specific tools by name list.
    Select(Vec<String>),
}

// ---------------------------------------------------------------------------
// Internal entry
// ---------------------------------------------------------------------------

/// Internal entry pairing a facade with its load state.
#[derive(Debug, Clone)]
struct Entry {
    facade: CapabilityFacade,
    state: ToolLoadState,
}

// ---------------------------------------------------------------------------
// DeferredToolRegistry
// ---------------------------------------------------------------------------

/// Registry that supports deferred tool loading to save prompt tokens.
///
/// Tools registered as `Active` have their full schema included in prompts.
/// Tools registered as `Deferred` only expose their names; the LLM can
/// retrieve full schemas via the `search()` method (analogous to `ToolSearch`).
///
/// An `active_limit` cap prevents unbounded prompt growth.
#[derive(Debug)]
pub struct DeferredToolRegistry {
    /// Tool name -> entry (preserves uniqueness by name).
    entries: HashMap<String, Entry>,
    /// Maximum number of simultaneously active tools.
    active_limit: usize,
}

impl DeferredToolRegistry {
    /// Create a new empty registry with the given active tool limit.
    ///
    /// `active_limit` controls how many tools can be in the `Active` state
    /// simultaneously. Default recommendation is 50.
    pub fn new(active_limit: usize) -> Self {
        Self {
            entries: HashMap::new(),
            active_limit,
        }
    }

    /// Register a tool with the specified load state.
    pub fn register(&mut self, facade: CapabilityFacade, state: ToolLoadState) {
        let name = facade.name.clone();
        self.entries.insert(name, Entry { facade, state });
    }

    /// Register a tool as deferred (name-only in prompt).
    pub fn register_deferred(&mut self, facade: CapabilityFacade) {
        self.register(facade, ToolLoadState::Deferred);
    }

    /// Register a tool as active (full schema in prompt).
    pub fn register_active(&mut self, facade: CapabilityFacade) {
        self.register(facade, ToolLoadState::Active);
    }

    /// Search all tools (both active and deferred) matching the given mode.
    ///
    /// This is the Rust equivalent of Claude Code's `ToolSearch` -- it lets
    /// the LLM discover deferred tools and retrieve their full schemas.
    pub fn search(&self, mode: SearchMode) -> Vec<&CapabilityFacade> {
        match mode {
            SearchMode::Exact(ref name) => self
                .entries
                .get(name)
                .map(|e| vec![&e.facade])
                .unwrap_or_default(),
            SearchMode::Keyword(ref keyword) => {
                let lower = keyword.to_lowercase();
                self.entries
                    .values()
                    .filter(|e| {
                        e.facade.name.to_lowercase().contains(&lower)
                            || e.facade.description.to_lowercase().contains(&lower)
                    })
                    .map(|e| &e.facade)
                    .collect()
            }
            SearchMode::Select(ref names) => names
                .iter()
                .filter_map(|n| self.entries.get(n).map(|e| &e.facade))
                .collect(),
        }
    }

    /// Promote a deferred tool to active state.
    ///
    /// Returns `true` if the tool was successfully promoted. Returns `false` if:
    /// - The tool does not exist in the registry.
    /// - The tool is already active.
    /// - The active limit has been reached.
    pub fn promote(&mut self, name: &str) -> bool {
        self.promote_with_risk_gate(name, cyberclaw_core::capability::RiskLevel::Critical)
    }

    /// Promote a deferred tool to active state with a risk ceiling.
    ///
    /// Returns `true` if the tool was successfully promoted. Returns `false` if:
    /// - The tool does not exist in the registry.
    /// - The tool is already active.
    /// - The active limit has been reached.
    /// - The tool's risk level exceeds `max_allowed_risk`.
    pub fn promote_with_risk_gate(
        &mut self,
        name: &str,
        max_allowed_risk: cyberclaw_core::capability::RiskLevel,
    ) -> bool {
        let entry = match self.entries.get(name) {
            Some(e) => e,
            None => return false,
        };
        if entry.state == ToolLoadState::Active {
            return false;
        }
        if entry.facade.risk_level > max_allowed_risk {
            return false;
        }
        if self.active_count() >= self.active_limit {
            return false;
        }
        // Safe: we checked existence above.
        self.entries.get_mut(name).unwrap().state = ToolLoadState::Active;
        true
    }

    /// Demote an active tool to deferred state.
    ///
    /// Returns `true` if the tool was successfully demoted. Returns `false` if:
    /// - The tool does not exist in the registry.
    /// - The tool is already deferred.
    pub fn demote(&mut self, name: &str) -> bool {
        let entry = match self.entries.get(name) {
            Some(e) => e,
            None => return false,
        };
        if entry.state == ToolLoadState::Deferred {
            return false;
        }
        self.entries.get_mut(name).unwrap().state = ToolLoadState::Deferred;
        true
    }

    /// Return all active tool facades (for system prompt injection).
    pub fn active_facades(&self) -> Vec<&CapabilityFacade> {
        self.entries
            .values()
            .filter(|e| e.state == ToolLoadState::Active)
            .map(|e| &e.facade)
            .collect()
    }

    /// Return the names of all deferred tools (for prompt hint listing).
    pub fn deferred_names(&self) -> Vec<&str> {
        self.entries
            .values()
            .filter(|e| e.state == ToolLoadState::Deferred)
            .map(|e| e.facade.name.as_str())
            .collect()
    }

    /// Return the number of currently active tools.
    pub fn active_count(&self) -> usize {
        self.entries
            .values()
            .filter(|e| e.state == ToolLoadState::Active)
            .count()
    }

    /// Check whether the active tool limit has been reached.
    pub fn is_at_limit(&self) -> bool {
        self.active_count() >= self.active_limit
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use cyberclaw_core::capability::RiskLevel;
    use serde_json::json;

    /// Helper to create a test facade.
    fn make_facade(name: &str) -> CapabilityFacade {
        CapabilityFacade {
            name: name.to_string(),
            description: format!("{name} tool description"),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "input": { "type": "string" }
                }
            })),
            connector_id: cyberclaw_core::ids::ConnectorId::from_string(
                "test-connector".to_string(),
            )
            .unwrap(),
            capability_id: cyberclaw_core::ids::CapabilityId::from_string(format!("{name}_cap"))
                .unwrap(),
            risk_level: RiskLevel::Low,
            effects: vec!["read".to_string()],
            read_only: true,
            destructive: false,
            exposure: cyberclaw_core::facade::FacadeExposure::LlmDefault,
            workspace_root: None,
        }
    }

    // 1. Empty registry returns empty lists.
    #[test]
    fn empty_registry_returns_empty() {
        let reg = DeferredToolRegistry::new(50);
        assert!(reg.active_facades().is_empty());
        assert!(reg.deferred_names().is_empty());
        assert_eq!(reg.active_count(), 0);
        assert!(!reg.is_at_limit());
    }

    // 2. Active registration appears in active_facades.
    #[test]
    fn active_registration_in_active_facades() {
        let mut reg = DeferredToolRegistry::new(50);
        reg.register_active(make_facade("bash"));
        let active = reg.active_facades();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].name, "bash");
    }

    // 3. Deferred registration does NOT appear in active_facades.
    #[test]
    fn deferred_not_in_active_facades() {
        let mut reg = DeferredToolRegistry::new(50);
        reg.register_deferred(make_facade("lsp_hover"));
        assert!(reg.active_facades().is_empty());
    }

    // 4. Deferred registration appears in deferred_names.
    #[test]
    fn deferred_in_deferred_names() {
        let mut reg = DeferredToolRegistry::new(50);
        reg.register_deferred(make_facade("lsp_hover"));
        let names = reg.deferred_names();
        assert_eq!(names.len(), 1);
        assert!(names.contains(&"lsp_hover"));
    }

    // 5. Promote moves deferred to active.
    #[test]
    fn promote_deferred_to_active() {
        let mut reg = DeferredToolRegistry::new(50);
        reg.register_deferred(make_facade("web_search"));
        assert!(reg.active_facades().is_empty());

        let ok = reg.promote("web_search");
        assert!(ok);
        assert_eq!(reg.active_facades().len(), 1);
        assert_eq!(reg.active_facades()[0].name, "web_search");
        assert!(reg.deferred_names().is_empty());
    }

    // 6. Promote nonexistent tool returns false.
    #[test]
    fn promote_nonexistent_returns_false() {
        let mut reg = DeferredToolRegistry::new(50);
        assert!(!reg.promote("nonexistent"));
    }

    // 7. Demote moves active to deferred.
    #[test]
    fn demote_active_to_deferred() {
        let mut reg = DeferredToolRegistry::new(50);
        reg.register_active(make_facade("bash"));
        assert_eq!(reg.active_count(), 1);

        let ok = reg.demote("bash");
        assert!(ok);
        assert_eq!(reg.active_count(), 0);
        assert!(reg.deferred_names().contains(&"bash"));
    }

    // 8. Promote fails when active limit is reached.
    #[test]
    fn promote_fails_at_limit() {
        let mut reg = DeferredToolRegistry::new(2);
        reg.register_active(make_facade("tool_a"));
        reg.register_active(make_facade("tool_b"));
        reg.register_deferred(make_facade("tool_c"));

        assert!(reg.is_at_limit());
        assert!(!reg.promote("tool_c"));
        // tool_c remains deferred.
        assert!(reg.deferred_names().contains(&"tool_c"));
    }

    // 9. Search Exact mode finds the right tool.
    #[test]
    fn search_exact_mode() {
        let mut reg = DeferredToolRegistry::new(50);
        reg.register_deferred(make_facade("file_read"));
        reg.register_active(make_facade("bash"));

        let results = reg.search(SearchMode::Exact("file_read".into()));
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "file_read");

        let empty = reg.search(SearchMode::Exact("nonexistent".into()));
        assert!(empty.is_empty());
    }

    // 10. Search Keyword mode matches substring in name or description.
    #[test]
    fn search_keyword_mode() {
        let mut reg = DeferredToolRegistry::new(50);
        reg.register_deferred(make_facade("lsp_hover"));
        reg.register_deferred(make_facade("lsp_diagnostics"));
        reg.register_active(make_facade("bash"));

        // Keyword "lsp" matches two tools.
        let results = reg.search(SearchMode::Keyword("lsp".into()));
        assert_eq!(results.len(), 2);

        // Keyword "diagnostics" matches one tool (by name substring).
        let results = reg.search(SearchMode::Keyword("diagnostics".into()));
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "lsp_diagnostics");

        // Case-insensitive match.
        let results = reg.search(SearchMode::Keyword("LSP".into()));
        assert_eq!(results.len(), 2);
    }

    // 11. Search Select mode picks multiple tools by name.
    #[test]
    fn search_select_mode() {
        let mut reg = DeferredToolRegistry::new(50);
        reg.register_deferred(make_facade("tool_a"));
        reg.register_deferred(make_facade("tool_b"));
        reg.register_active(make_facade("tool_c"));

        let results = reg.search(SearchMode::Select(vec![
            "tool_a".into(),
            "tool_c".into(),
            "nonexistent".into(),
        ]));
        assert_eq!(results.len(), 2);
        let names: Vec<&str> = results.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"tool_a"));
        assert!(names.contains(&"tool_c"));
    }

    // 12. Promote already-active tool returns false.
    #[test]
    fn promote_already_active_returns_false() {
        let mut reg = DeferredToolRegistry::new(50);
        reg.register_active(make_facade("bash"));
        assert!(!reg.promote("bash"));
    }

    // 13. Demote already-deferred tool returns false.
    #[test]
    fn demote_already_deferred_returns_false() {
        let mut reg = DeferredToolRegistry::new(50);
        reg.register_deferred(make_facade("lsp_hover"));
        assert!(!reg.demote("lsp_hover"));
    }

    // 14. active_count tracks correctly across promote/demote.
    #[test]
    fn active_count_tracks_mutations() {
        let mut reg = DeferredToolRegistry::new(50);
        assert_eq!(reg.active_count(), 0);

        reg.register_active(make_facade("a"));
        assert_eq!(reg.active_count(), 1);

        reg.register_deferred(make_facade("b"));
        assert_eq!(reg.active_count(), 1);

        reg.promote("b");
        assert_eq!(reg.active_count(), 2);

        reg.demote("a");
        assert_eq!(reg.active_count(), 1);
    }

    // -- Risk gate tests (M2 fix) ------------------------------------------

    #[test]
    fn promote_with_risk_gate_blocks_high_risk() {
        let mut reg = DeferredToolRegistry::new(10);
        let mut facade = make_facade("dangerous_tool");
        facade.risk_level = RiskLevel::Critical;
        reg.register(facade, ToolLoadState::Deferred);

        // Gate at Medium should block Critical
        assert!(!reg.promote_with_risk_gate("dangerous_tool", RiskLevel::Medium));
        assert_eq!(reg.active_count(), 0);
    }

    #[test]
    fn promote_with_risk_gate_allows_within_ceiling() {
        let mut reg = DeferredToolRegistry::new(10);
        let mut facade = make_facade("safe_tool");
        facade.risk_level = RiskLevel::Low;
        reg.register(facade, ToolLoadState::Deferred);

        // Gate at Medium should allow Low
        assert!(reg.promote_with_risk_gate("safe_tool", RiskLevel::Medium));
        assert_eq!(reg.active_count(), 1);
    }

    #[test]
    fn promote_with_risk_gate_allows_exact_ceiling() {
        let mut reg = DeferredToolRegistry::new(10);
        let mut facade = make_facade("medium_tool");
        facade.risk_level = RiskLevel::Medium;
        reg.register(facade, ToolLoadState::Deferred);

        // Gate at Medium should allow Medium (<=)
        assert!(reg.promote_with_risk_gate("medium_tool", RiskLevel::Medium));
        assert_eq!(reg.active_count(), 1);
    }

    #[test]
    fn promote_default_allows_critical() {
        // Default promote() should allow Critical (backward compatible)
        let mut reg = DeferredToolRegistry::new(10);
        let mut facade = make_facade("critical_tool");
        facade.risk_level = RiskLevel::Critical;
        reg.register(facade, ToolLoadState::Deferred);

        assert!(reg.promote("critical_tool"));
        assert_eq!(reg.active_count(), 1);
    }
}
