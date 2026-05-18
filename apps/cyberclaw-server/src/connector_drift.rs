//! Connector / Facade / Mapper drift detector.
//!
//! Sprint 20 W1 — closes the recurring "facade declared but connector
//! never registered" anti-pattern that bit Sprint 19 four separate
//! times (memory, todo, MCP, LSP). Each time the symptom was the
//! same: an LLM tool call dispatched to a connector_id that no
//! `connector_registry.register()` call had ever produced, so the
//! tool returned "Connector X does not support capability Y".
//!
//! This module audits three independent truth sources at server
//! startup and reports any disagreement between them:
//!
//!   1. `BuiltinToolRegistry::with_defaults().get_facades(...)` —
//!      what facades the agent runtime advertises to the LLM
//!   2. `ConnectorRegistry::list_connectors() / list_capabilities()` —
//!      what's actually registered + dispatchable
//!   3. `ToolCallMapper::list_tools()` + each mapping's
//!      `(connector_id, capability_id)` target — what the bridge will
//!      route to
//!
//! Each disagreement is reported as a `DriftFinding`. A non-empty
//! finding list is logged at WARN level (not ERROR — the platform
//! still boots; the affected tools just fail at dispatch time).
//! Operators can re-run the audit on a live server via the
//! `audit_drift_report` field on `AppState`.
//!
//! # Why log + serve, not panic
//!
//! Hard-failing startup on drift would break operators who deliberately
//! disable certain connectors (e.g. LSP via `CYBERCLAW_LSP_ENABLED=false`).
//! Soft-warn lets opt-in connectors behave correctly: registered
//! when wanted, declared facades fail-soft when not.

use std::collections::HashSet;
use std::sync::Arc;

use cyberclaw_agent_runtime::builtin_tools::{BuiltinToolRegistry, ToolsetConfig};
use cyberclaw_connectors::registry::ConnectorRegistry;
use cyberclaw_llm_bridge::mapper::ToolCallMapper;
use serde::Serialize;
use tracing::{info, warn};

/// Opt-in connector ids that ship facades / mapper aliases unconditionally
/// but are only registered when the operator opts in via `CYBERCLAW_*_ENABLED`
/// or by providing the matching credentials.
///
/// When the audit sees a facade or mapper that targets one of these
/// connectors but the connector is not registered, the finding is downgraded
/// from `warn` to `info` — the platform behaves correctly: the tool is
/// declared so well-known agent prompts still type-check; dispatch fails
/// soft when the operator hasn't configured the connector.
///
/// R-2 (2026-05-05) — closed the eval drift surface: 38 startup warnings →
/// 0 warnings (info entries for opt-in connectors only).
///
/// `skill` is special: it is intercepted directly inside `chat_handler.rs`
/// (the SkillHub lives in the server crate; the connectors crate intentionally
/// doesn't depend on the server) so the LLM-visible `skill_create` /
/// `skill_search` facades never reach the dispatcher. They're declared as
/// `connector_id="skill"` purely as a routing marker. Treating the marker as
/// opt-in keeps drift quiet without weakening the audit on real unregistered
/// connectors.
/// `agent` is also intercepted inline by `chat_handler.rs`
/// (`delegate_to_sub_agent` routes through `SubAgentOrchestrator` which is
/// request-scoped; no standalone "agent" connector is ever registered). Treat
/// it as opt-in so the drift audit stays quiet for this facade.
const OPT_IN_CONNECTOR_IDS: &[&str] = &["browser", "lsp", "mcp", "slack", "http", "skill", "agent"];

fn is_opt_in_connector(id: &str) -> bool {
    OPT_IN_CONNECTOR_IDS.contains(&id) || id.starts_with("mcp-")
}

/// One disagreement between facades / connectors / mapper.
#[derive(Debug, Clone, Serialize)]
pub struct DriftFinding {
    /// Severity level: `"warn"` (most cases) or `"info"` (orphans).
    pub severity: &'static str,
    /// Stable category for grouping.
    pub kind: &'static str,
    /// Human-readable description.
    pub message: String,
}

/// Full audit report. Empty `findings` = the three sources agree.
#[derive(Debug, Clone, Serialize)]
pub struct DriftReport {
    pub findings: Vec<DriftFinding>,
    pub registered_connectors: Vec<String>,
    pub registered_capabilities: Vec<String>,
    pub facade_count: usize,
    pub mapper_tool_count: usize,
}

impl DriftReport {
    pub fn warning_count(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| f.severity == "warn")
            .count()
    }
}

/// Run the drift audit. Designed to be called once at startup after all
/// connectors + mapper aliases are wired.
pub fn audit(registry: &Arc<ConnectorRegistry>, mapper: &ToolCallMapper) -> DriftReport {
    let mut findings = Vec::new();

    // 1) Snapshot: what's actually registered + dispatchable.
    let connector_ids: HashSet<String> = registry
        .list_connectors()
        .into_iter()
        .map(|c| c.as_str().to_string())
        .collect();
    let capability_keys: HashSet<String> = registry
        .list_capabilities()
        .into_iter()
        .map(|(c, cap)| format!("{}::{}", c.as_str(), cap.as_str()))
        .collect();

    // 2) Snapshot: what facades declare.
    let facade_registry = BuiltinToolRegistry::with_defaults();
    let facades = facade_registry.get_facades(&ToolsetConfig::default_config());

    for facade in &facades {
        let target_connector = facade.connector_id.as_str().to_string();
        let target_cap = facade.capability_id.as_str().to_string();
        let target_key = format!("{}::{}", target_connector, target_cap);

        if !connector_ids.contains(&target_connector) {
            // R-2: opt-in connectors (browser/lsp/mcp/slack/http) downgrade
            // unregistered-target findings from warn to info — the operator
            // hasn't enabled the connector, which is the documented default.
            let severity = if is_opt_in_connector(&target_connector) {
                "info"
            } else {
                "warn"
            };
            let kind = if severity == "info" {
                "facade_targets_optin_connector"
            } else {
                "facade_targets_unregistered_connector"
            };
            findings.push(DriftFinding {
                severity,
                kind,
                message: format!(
                    "facade `{}` targets connector `{}` which is not registered (capability `{}`){}",
                    facade.name,
                    target_connector,
                    target_cap,
                    if severity == "info" { " — opt-in" } else { "" },
                ),
            });
        } else if !capability_keys.contains(&target_key) {
            findings.push(DriftFinding {
                severity: "warn",
                kind: "facade_targets_unknown_capability",
                message: format!(
                    "facade `{}` targets `{}::{}` — connector exists but does not advertise the capability",
                    facade.name, target_connector, target_cap,
                ),
            });
        }
    }

    // 3) Snapshot: what mapper aliases route to.
    let mapper_targets = mapper.list_mappings_with_targets().unwrap_or_default();
    for (name, conn, cap) in &mapper_targets {
        let target_connector = conn.as_str().to_string();
        let target_cap = cap.as_str().to_string();
        let target_key = format!("{}::{}", target_connector, target_cap);
        if !connector_ids.contains(&target_connector) {
            // R-2: same opt-in downgrade logic as facade scan.
            let severity = if is_opt_in_connector(&target_connector) {
                "info"
            } else {
                "warn"
            };
            let kind = if severity == "info" {
                "mapper_routes_to_optin_connector"
            } else {
                "mapper_routes_to_unregistered_connector"
            };
            findings.push(DriftFinding {
                severity,
                kind,
                message: format!(
                    "mapper alias `{}` routes to connector `{}` which is not registered (capability `{}`){}",
                    name,
                    target_connector,
                    target_cap,
                    if severity == "info" { " — opt-in" } else { "" },
                ),
            });
        } else if !capability_keys.contains(&target_key) {
            findings.push(DriftFinding {
                severity: "warn",
                kind: "mapper_routes_to_unknown_capability",
                message: format!(
                    "mapper alias `{}` routes to `{}::{}` — connector exists but capability does not",
                    name, target_connector, target_cap,
                ),
            });
        }
    }
    let tool_names: Vec<String> = mapper_targets.iter().map(|(n, _, _)| n.clone()).collect();
    let mapper_name_set: HashSet<String> = tool_names.iter().cloned().collect();

    // 3b) Inverse check: every facade that the /v1/chat/completions
    // palette will expose MUST have a corresponding mapper alias.
    // Without this, an LLM tool_call returns 500 "Unknown tool: X" at
    // dispatch time (the bug we shipped in v0.2.2 for skill_search and
    // v0.2.1 for delegate_to_sub_agent).
    //
    // SubAgent and SkillManagement categories are deliberately
    // inline-intercepted by chat_handler.rs (not via mapper) — those are
    // chat-palette excluded at build_default_chat_palette and therefore
    // exempt here.
    use cyberclaw_core::facade::ToolsetCategory;
    let mut chat_palette_config = ToolsetConfig::default_config();
    chat_palette_config
        .enabled_categories
        .remove(&ToolsetCategory::SubAgent);
    chat_palette_config
        .enabled_categories
        .remove(&ToolsetCategory::SkillManagement);
    let chat_palette_facades = facade_registry.get_facades(&chat_palette_config);
    for facade in &chat_palette_facades {
        if !mapper_name_set.contains(&facade.name) {
            findings.push(DriftFinding {
                severity: "warn",
                kind: "facade_in_chat_palette_without_mapper",
                message: format!(
                    "facade `{}` is in /v1/chat/completions palette but has no \
                     ToolCallMapper alias — LLM tool_call will 500 'Unknown tool'",
                    facade.name
                ),
            });
        }
    }

    // 4) Orphan detection — connectors with no facade or mapper hit them.
    // Informational only; some connectors (handoff, mcp-*) are addressed
    // directly by server code, not via LLM tools.
    let mut referenced: HashSet<String> = facades
        .iter()
        .map(|f| f.connector_id.as_str().to_string())
        .collect();
    for (_, conn, _) in &mapper_targets {
        referenced.insert(conn.as_str().to_string());
    }
    for id in &connector_ids {
        if !referenced.contains(id) {
            findings.push(DriftFinding {
                severity: "info",
                kind: "connector_orphan",
                message: format!(
                    "connector `{}` is registered but has no facade nor mapper alias targeting it (server-direct only?)",
                    id
                ),
            });
        }
    }

    let report = DriftReport {
        findings,
        registered_connectors: connector_ids.into_iter().collect(),
        registered_capabilities: capability_keys.into_iter().collect(),
        facade_count: facades.len(),
        mapper_tool_count: tool_names.len(),
    };

    let warn_count = report.warning_count();
    if warn_count == 0 {
        info!(
            facade_count = report.facade_count,
            mapper_tool_count = report.mapper_tool_count,
            connector_count = report.registered_connectors.len(),
            "Sprint 20 W1: connector drift audit passed — facades / mapper / registry agree"
        );
    } else {
        warn!(
            warn_count,
            "Sprint 20 W1: connector drift audit detected {} warnings (LLM tools may fail at dispatch)",
            warn_count
        );
        for f in &report.findings {
            if f.severity == "warn" {
                warn!(kind = f.kind, "{}", f.message);
            }
        }
    }

    report
}

// ============================================================================
// HTTP endpoint — promised by the module doc comment but never wired before.
// Returns a live re-audit on every request; cost is negligible (in-memory).
// ============================================================================

use axum::{extract::State, routing::get, Json, Router};
use std::sync::Arc as StdArc;

/// `GET /api/v1/system/connector-drift` — live drift audit.
///
/// Returns the same JSON shape used at startup audit. Useful for operators
/// who deploy a new mapper or facade change without restarting the server,
/// or who want a periodic sanity check from an external monitor.
pub fn create_connector_drift_router() -> Router<StdArc<crate::state::AppState>> {
    Router::new().route("/api/v1/system/connector-drift", get(get_drift_report))
}

async fn get_drift_report(
    State(state): State<StdArc<crate::state::AppState>>,
) -> Json<DriftReport> {
    let report = audit(&state.connector_registry, &state.tool_mapper);
    Json(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// F12 Phase C (2026-05-06) — `default_facades()` now contains ONLY the
    /// 3 chat_handler-intercepted facades (`skill_create`, `skill_search`,
    /// `delegate_to_sub_agent`). All use opt-in connector_ids (`skill` /
    /// `agent`), so an empty registry produces **only** info findings (no
    /// warns). Connector-owned facades (file_*, bash, web_*, etc.) are
    /// registered at startup via `capability_facades()` and are not part of
    /// `BuiltinToolRegistry::with_defaults()` any more.
    #[test]
    fn empty_registry_with_facades_reports_drift() {
        let registry = Arc::new(ConnectorRegistry::new());
        let mapper = ToolCallMapper::new();
        let report = audit(&registry, &mapper);

        let total_findings = report.findings.len();
        // With the F12 Phase C shrunken defaults there are exactly 3 facades,
        // all opt-in — so at most 3 info findings, 0 warns.
        // (mapper has 0 entries too → no mapper drift.)
        assert!(
            total_findings <= 3,
            "expected ≤3 info findings from 3 opt-in facades, got {total_findings}"
        );
        let warn_findings: Vec<_> = report
            .findings
            .iter()
            .filter(|f| f.severity == "warn")
            .collect();
        assert!(
            warn_findings.is_empty(),
            "all default facades are opt-in; no warn findings expected: {warn_findings:?}"
        );
        let info_findings: Vec<_> = report
            .findings
            .iter()
            .filter(|f| f.severity == "info")
            .collect();
        // skill/agent connector ids are opt-in → produce info, not warn.
        assert!(
            !info_findings.is_empty(),
            "opt-in facades (skill_create/skill_search/delegate_to_sub_agent) should produce info entries"
        );
    }

    #[test]
    fn opt_in_connectors_classified_as_info() {
        assert!(is_opt_in_connector("browser"));
        assert!(is_opt_in_connector("lsp"));
        assert!(is_opt_in_connector("mcp"));
        assert!(is_opt_in_connector("slack"));
        assert!(is_opt_in_connector("http"));
        assert!(is_opt_in_connector("skill"));
        assert!(is_opt_in_connector("mcp-foo"));
        assert!(!is_opt_in_connector("local"));
        assert!(!is_opt_in_connector("memory"));
        assert!(!is_opt_in_connector("todo"));
    }

    #[test]
    fn report_serializes_to_json() {
        let registry = Arc::new(ConnectorRegistry::new());
        let mapper = ToolCallMapper::new();
        let report = audit(&registry, &mapper);
        let json = serde_json::to_string(&report).expect("serialize");
        assert!(json.contains("findings"));
        assert!(json.contains("registered_connectors"));
    }
}
