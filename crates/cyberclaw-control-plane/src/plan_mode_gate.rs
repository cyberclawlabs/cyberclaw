//! PlanModeGate — enter/exit plan-mode with read-only permission scoping.
//!
//! Plan mode (inspired by the OMC `plan` skill) is a research/planning phase
//! where the agent may **read** the repository and query tools but must not
//! mutate filesystem state, deploy artifacts, spawn sub-agents, or execute
//! shell commands. This gate strips dangerous capabilities on entry and
//! restores them from a [`PlanPermissionSnapshot`] on exit.
//!
//! Unlike [`crate::auto_mode_gate`], which tracks session lifecycle over
//! `ExecutionId`, `PlanModeGate` is a **pure filter** over a capability list:
//! callers own the snapshot and are responsible for threading it back through
//! `exit_plan_mode`. This keeps the gate stateless and safe to share across
//! parallel planning lanes.

use cyberclaw_core::capability::CapabilityRef;

// ─────────────────────────────────────────────────────────────────────────────
// Configuration
// ─────────────────────────────────────────────────────────────────────────────

/// Default glob patterns that describe **read-only** capabilities permitted in
/// plan mode. A capability is kept if its ID matches **any** pattern here.
pub const DEFAULT_READONLY_PATTERNS: &[&str] = &[
    "fs:*:read",
    "fs:*:list",
    "fs:*:stat",
    "connector:*:query",
    "connector:*:read",
    "connector:*:list",
    "connector:lsp:*",
    "agent:*:think",
    "agent:*:plan",
];

/// Default glob patterns that describe **mutating** capabilities which must be
/// stripped when entering plan mode. A capability is stripped if its ID
/// matches any of these patterns and is **not** rescued by
/// `readonly_patterns`.
pub const DEFAULT_STRIP_PATTERNS: &[&str] = &[
    "fs:*:write",
    "fs:*:delete",
    "fs:*:create",
    "fs:*:edit",
    "connector:*:write",
    "connector:*:delete",
    "connector:*:delete*",
    "connector:*:deploy",
    "connector:*:deploy*",
    "connector:*:mutate",
    "connector:*:publish",
    "shell:*",
    "shell:*:exec",
    "agent:*:spawn",
];

/// Configuration controlling plan-mode filtering.
#[derive(Debug, Clone)]
pub struct PlanModeConfig {
    /// Glob patterns allow-listing read-only capabilities. Any capability
    /// whose ID matches one of these patterns is **always** kept, even if it
    /// also matches a `strip_patterns` entry.
    pub readonly_patterns: Vec<String>,
    /// Glob patterns that identify capabilities to strip when entering plan
    /// mode.
    pub strip_patterns: Vec<String>,
}

impl Default for PlanModeConfig {
    fn default() -> Self {
        Self {
            readonly_patterns: DEFAULT_READONLY_PATTERNS
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            strip_patterns: DEFAULT_STRIP_PATTERNS
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Snapshot types
// ─────────────────────────────────────────────────────────────────────────────

/// Reason a capability was stripped on plan-mode entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StripReason {
    /// Capability performs writes (`fs:*:write`, `connector:*:write`, ...).
    WriteDenied,
    /// Capability deletes data (`fs:*:delete`, `connector:*:delete*`).
    DeleteDenied,
    /// Capability deploys artifacts (`connector:*:deploy*`).
    DeployDenied,
    /// Capability executes shell commands (`shell:*`).
    ShellDenied,
    /// Capability spawns sub-agents (`agent:*:spawn`).
    SpawnDenied,
    /// Capability matched a custom strip pattern with no obvious
    /// classification.
    OtherDenied,
}

/// Snapshot captured when entering plan mode, used to restore the original
/// capability set on exit.
#[derive(Debug, Clone)]
pub struct PlanPermissionSnapshot {
    /// Full list of capability IDs that were in effect before plan mode was
    /// entered. `exit_plan_mode` returns the original [`CapabilityRef`]
    /// values in the same order.
    pub original: Vec<String>,
    /// Capabilities that were stripped, each tagged with the classification
    /// reason. Order matches the iteration order over the input slice.
    pub stripped: Vec<(String, StripReason)>,
    /// Capabilities that passed the filter and remain active during plan
    /// mode.
    pub kept: Vec<String>,
    /// The original capability references, retained verbatim so
    /// `exit_plan_mode` can rehydrate without requiring the caller to track
    /// them separately.
    original_refs: Vec<CapabilityRef>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Trait
// ─────────────────────────────────────────────────────────────────────────────

/// Stateless filter that classifies capabilities for plan-mode execution.
pub trait PlanModeGate: Send + Sync {
    /// Partition `caps` into kept (read-only) and stripped (mutating) sets,
    /// returning a snapshot that can later be replayed via
    /// [`exit_plan_mode`].
    fn enter_plan_mode(&self, caps: &[CapabilityRef]) -> PlanPermissionSnapshot;

    /// Restore the original capability set from `snapshot`. Called when plan
    /// mode exits so the agent regains its full permission envelope.
    fn exit_plan_mode(&self, snapshot: PlanPermissionSnapshot) -> Vec<CapabilityRef>;
}

// ─────────────────────────────────────────────────────────────────────────────
// Default implementation
// ─────────────────────────────────────────────────────────────────────────────

/// Default [`PlanModeGate`] implementation backed by a [`PlanModeConfig`].
#[derive(Debug, Clone, Default)]
pub struct DefaultPlanModeGate {
    config: PlanModeConfig,
}

impl DefaultPlanModeGate {
    /// Create a gate with default allow/strip rules.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a gate with a custom [`PlanModeConfig`].
    pub fn with_config(config: PlanModeConfig) -> Self {
        Self { config }
    }

    /// Classify a single capability ID. Returns `Ok(())` if the capability
    /// should be kept, or `Err(StripReason)` if it must be stripped.
    fn classify(&self, cap_id: &str) -> Result<(), StripReason> {
        // Allow-list wins: any readonly match keeps the capability even if a
        // strip pattern would otherwise match. This lets operators rescue
        // specific read-only subpaths of a broadly-stripped namespace (e.g.
        // `connector:lsp:*` while still stripping `connector:*:write`).
        if self
            .config
            .readonly_patterns
            .iter()
            .any(|pat| glob_match(pat, cap_id))
        {
            return Ok(());
        }

        if let Some(pat) = self
            .config
            .strip_patterns
            .iter()
            .find(|pat| glob_match(pat, cap_id))
        {
            return Err(classify_pattern(pat));
        }

        Ok(())
    }
}

impl PlanModeGate for DefaultPlanModeGate {
    fn enter_plan_mode(&self, caps: &[CapabilityRef]) -> PlanPermissionSnapshot {
        let mut original = Vec::with_capacity(caps.len());
        let mut kept = Vec::new();
        let mut stripped = Vec::new();

        for cap in caps {
            let cap_id = cap.id.as_str().to_string();
            original.push(cap_id.clone());

            match self.classify(&cap_id) {
                Ok(()) => kept.push(cap_id),
                Err(reason) => stripped.push((cap_id, reason)),
            }
        }

        PlanPermissionSnapshot {
            original,
            stripped,
            kept,
            original_refs: caps.to_vec(),
        }
    }

    fn exit_plan_mode(&self, snapshot: PlanPermissionSnapshot) -> Vec<CapabilityRef> {
        snapshot.original_refs
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Pattern matching helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Minimal glob matcher supporting `*` wildcards (matching any run of
/// characters, including `:`). Patterns without `*` fall back to exact
/// equality. This mirrors the simple pattern style used in
/// [`crate::autopilot_security`] and keeps plan-mode filtering dependency-free.
fn glob_match(pattern: &str, input: &str) -> bool {
    // Fast path: no wildcard -> exact match.
    if !pattern.contains('*') {
        return pattern == input;
    }

    let parts: Vec<&str> = pattern.split('*').collect();
    let mut cursor = 0usize;

    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }

        if i == 0 {
            // Leading anchor must match at the start.
            if !input[cursor..].starts_with(part) {
                return false;
            }
            cursor += part.len();
        } else if i == parts.len() - 1 {
            // Trailing anchor must match at the end.
            if !input[cursor..].ends_with(part) {
                return false;
            }
            // Ensure the trailing match is at/after cursor.
            let tail_start = input.len().saturating_sub(part.len());
            if tail_start < cursor {
                return false;
            }
        } else {
            // Middle segment: find it from cursor onwards.
            match input[cursor..].find(part) {
                Some(offset) => cursor += offset + part.len(),
                None => return false,
            }
        }
    }

    true
}

/// Derive a [`StripReason`] from a matched strip pattern. Falls back to
/// [`StripReason::OtherDenied`] for unrecognised custom patterns.
fn classify_pattern(pattern: &str) -> StripReason {
    // Most specific matches first.
    if pattern.contains(":deploy") {
        StripReason::DeployDenied
    } else if pattern.contains(":delete") {
        StripReason::DeleteDenied
    } else if pattern.starts_with("shell:") {
        StripReason::ShellDenied
    } else if pattern.contains(":spawn") {
        StripReason::SpawnDenied
    } else if pattern.contains(":write")
        || pattern.contains(":create")
        || pattern.contains(":edit")
        || pattern.contains(":mutate")
        || pattern.contains(":publish")
    {
        StripReason::WriteDenied
    } else {
        StripReason::OtherDenied
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use cyberclaw_core::capability::{CapabilityEffect, RiskLevel};
    use cyberclaw_core::ids::{CapabilityId, ConnectorId};

    fn mk_cap(id: &str) -> CapabilityRef {
        CapabilityRef {
            id: CapabilityId::from_string(id.to_string()).expect("cap id"),
            connector_id: ConnectorId::from_string("test".to_string()).expect("conn id"),
            risk: RiskLevel::Low,
            effects: vec![CapabilityEffect::Read],
            placement: None,
        }
    }

    #[test]
    fn plan_mode_readonly_capabilities_pass_through() {
        let gate = DefaultPlanModeGate::new();
        let caps = vec![
            mk_cap("fs:local:read"),
            mk_cap("connector:github:query"),
            mk_cap("connector:lsp:hover"),
        ];

        let snapshot = gate.enter_plan_mode(&caps);

        assert_eq!(snapshot.kept.len(), 3, "all read-only caps should be kept");
        assert!(
            snapshot.stripped.is_empty(),
            "no caps should be stripped in an all-readonly set, got {:?}",
            snapshot.stripped
        );
        assert_eq!(snapshot.original.len(), 3);
    }

    #[test]
    fn plan_mode_write_capabilities_stripped() {
        let gate = DefaultPlanModeGate::new();
        let caps = vec![
            mk_cap("fs:local:write"),
            mk_cap("connector:github:write"),
            mk_cap("fs:local:read"),
        ];

        let snapshot = gate.enter_plan_mode(&caps);

        assert_eq!(snapshot.kept, vec!["fs:local:read".to_string()]);
        assert_eq!(snapshot.stripped.len(), 2);
        assert!(snapshot
            .stripped
            .iter()
            .all(|(_, reason)| *reason == StripReason::WriteDenied));
    }

    #[test]
    fn plan_mode_delete_and_deploy_stripped() {
        let gate = DefaultPlanModeGate::new();
        let caps = vec![
            mk_cap("connector:s3:delete"),
            mk_cap("connector:s3:delete_bucket"),
            mk_cap("connector:k8s:deploy"),
            mk_cap("connector:k8s:deploy_rollout"),
        ];

        let snapshot = gate.enter_plan_mode(&caps);

        assert!(snapshot.kept.is_empty(), "no caps should survive");
        assert_eq!(snapshot.stripped.len(), 4);

        let reasons: Vec<StripReason> = snapshot.stripped.iter().map(|(_, r)| r.clone()).collect();
        assert!(reasons.contains(&StripReason::DeleteDenied));
        assert!(reasons.contains(&StripReason::DeployDenied));
    }

    #[test]
    fn plan_mode_shell_and_spawn_stripped() {
        let gate = DefaultPlanModeGate::new();
        let caps = vec![
            mk_cap("shell:bash"),
            mk_cap("shell:zsh:exec"),
            mk_cap("agent:subagent:spawn"),
        ];

        let snapshot = gate.enter_plan_mode(&caps);

        assert!(snapshot.kept.is_empty());
        assert_eq!(snapshot.stripped.len(), 3);

        let reasons: Vec<StripReason> = snapshot.stripped.iter().map(|(_, r)| r.clone()).collect();
        assert!(reasons.contains(&StripReason::ShellDenied));
        assert!(reasons.contains(&StripReason::SpawnDenied));
    }

    #[test]
    fn plan_mode_snapshot_exit_restores_full_set() {
        let gate = DefaultPlanModeGate::new();
        let caps = vec![
            mk_cap("fs:local:read"),
            mk_cap("fs:local:write"),
            mk_cap("shell:bash"),
        ];

        let snapshot = gate.enter_plan_mode(&caps);
        assert_eq!(snapshot.kept.len(), 1);
        assert_eq!(snapshot.stripped.len(), 2);

        let restored = gate.exit_plan_mode(snapshot);

        assert_eq!(restored.len(), caps.len());
        let restored_ids: Vec<String> =
            restored.iter().map(|c| c.id.as_str().to_string()).collect();
        let original_ids: Vec<String> = caps.iter().map(|c| c.id.as_str().to_string()).collect();
        assert_eq!(
            restored_ids, original_ids,
            "exit_plan_mode must restore caps in original order"
        );
    }

    #[test]
    fn plan_mode_with_custom_allow_list() {
        // Custom allow list rescues `connector:custom:review` from the
        // strip pattern, while still stripping `connector:custom:write`.
        let config = PlanModeConfig {
            readonly_patterns: vec![
                "fs:*:read".to_string(),
                "connector:custom:review".to_string(),
            ],
            strip_patterns: vec![
                "connector:*:write".to_string(),
                "connector:*:review".to_string(),
            ],
        };
        let gate = DefaultPlanModeGate::with_config(config);

        let caps = vec![
            mk_cap("fs:local:read"),
            mk_cap("connector:custom:review"),
            mk_cap("connector:custom:write"),
        ];

        let snapshot = gate.enter_plan_mode(&caps);

        assert_eq!(snapshot.kept.len(), 2);
        assert!(snapshot.kept.contains(&"fs:local:read".to_string()));
        assert!(snapshot
            .kept
            .contains(&"connector:custom:review".to_string()));
        assert_eq!(snapshot.stripped.len(), 1);
        assert_eq!(snapshot.stripped[0].0, "connector:custom:write");
        assert_eq!(snapshot.stripped[0].1, StripReason::WriteDenied);
    }

    #[test]
    fn plan_mode_glob_match_basic() {
        assert!(glob_match("fs:*:read", "fs:local:read"));
        assert!(glob_match("fs:*:read", "fs:remote:read"));
        assert!(!glob_match("fs:*:read", "fs:local:write"));
        assert!(glob_match("shell:*", "shell:bash"));
        assert!(glob_match("shell:*", "shell:zsh:exec"));
        assert!(glob_match("exact", "exact"));
        assert!(!glob_match("exact", "exacter"));
        assert!(glob_match("*", "anything:goes"));
    }
}
