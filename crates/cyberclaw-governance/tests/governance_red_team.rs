//! Governance red-team integration test.
//!
//! Validates the most important architectural invariant of CyberClaw:
//! **Skills are not executors.** A skill that tries to dispatch a dangerous
//! capability MUST be denied by the governance gate even if it manages to
//! reach the dispatch surface.
//!
//! Covers the DangerousCapabilityFilter default 9 rules (D001..D009).
//! These tests exercise the publicly exported governance API and verify
//! that denials are deterministic and not bypassable via id shape variants.

use cyberclaw_governance::dangerous_capability_filter::{
    DangerSeverity, DangerousCapabilityFilter, FilterDecision,
};

/// Each default rule must produce a Deny (or Warn) for a matching capability id.
#[test]
fn red_team_default_rules_engage_on_matching_capability_ids() {
    let filter = DangerousCapabilityFilter::with_defaults();

    let cases = [
        ("shell:bash:destructive", DangerSeverity::Critical), // D001
        ("shell:zsh:network", DangerSeverity::High),          // D002
        ("connector:k8s:deploy_pod", DangerSeverity::Critical), // D003
        ("connector:k8s:delete_pod", DangerSeverity::Critical), // D004
        ("agent:malicious:spawn", DangerSeverity::High),      // D005
        ("plugin:foo:install", DangerSeverity::High),         // D006
        ("capability:vault:credential", DangerSeverity::Critical), // D007
        ("connector:local:cmd.run", DangerSeverity::Critical), // D008
        ("connector:local:fs.delete", DangerSeverity::High),  // D009
    ];

    for (cap_id, expected_severity) in cases {
        let decision = filter.check(cap_id);
        match decision {
            FilterDecision::Deny { severity, .. } => {
                assert_eq!(
                    severity, expected_severity,
                    "capability {cap_id}: severity mismatch"
                );
            }
            other => {
                panic!("RED-TEAM FAIL: capability {cap_id} was not denied — decision={other:?}")
            }
        }
    }
}

/// Low-risk capabilities (memory.read, capability list, etc.) must pass through.
/// This ensures the filter is not over-blocking benign traffic.
#[test]
fn red_team_low_risk_capability_passes_filter() {
    let filter = DangerousCapabilityFilter::with_defaults();

    let benign = [
        "connector:local:fs.read",
        "memory.search",
        "skill.list",
        "channel:slack:message.receive",
    ];

    for cap_id in benign {
        let decision = filter.check(cap_id);
        assert!(
            matches!(decision, FilterDecision::Allow),
            "benign capability {cap_id} unexpectedly intercepted — decision={decision:?}"
        );
    }
}

/// Red-team: trailing-segment variants of dangerous patterns must still be denied.
/// This ensures wildcard expansion can't be bypassed by appending segments.
#[test]
fn red_team_wildcard_variants_still_denied() {
    let filter = DangerousCapabilityFilter::with_defaults();

    let variants = [
        "connector:mcp:any:delete_file",   // 4-segment MCP-bridged delete
        "connector:mcp:any:deploy_secret", // 4-segment MCP-bridged deploy
        "shell:python:network",            // matches D002
    ];

    for cap_id in variants {
        let decision = filter.check(cap_id);
        assert!(
            matches!(decision, FilterDecision::Deny { .. }),
            "wildcard variant {cap_id} not denied — decision={decision:?}"
        );
    }
}

/// Critical-severity rules must NEVER be bypassable by exceptions.
/// This is the IronLaw at the filter layer.
#[test]
fn red_team_critical_rules_never_bypassable_via_exception() {
    use cyberclaw_governance::dangerous_capability_filter::CapabilityException;

    let mut filter = DangerousCapabilityFilter::with_defaults();
    // Try to whitelist the most dangerous capability.
    filter.add_exception(CapabilityException {
        capability_pattern: "connector:local:cmd.run".to_string(),
        reason: "red-team test — should NOT take effect for Critical".to_string(),
    });

    let decision = filter.check("connector:local:cmd.run");
    assert!(
        matches!(
            decision,
            FilterDecision::Deny {
                severity: DangerSeverity::Critical,
                ..
            }
        ),
        "RED-TEAM FAIL: Critical rule D008 bypassed by exception — decision={decision:?}"
    );
}

/// High-severity rules CAN be unlocked by an explicit exception (per-actor allow).
/// This is the legitimate escape valve; verify it works and is auditable
/// by checking the decision flips to Allow only when the exception is present.
#[test]
fn red_team_high_rule_overridable_by_explicit_exception() {
    use cyberclaw_governance::dangerous_capability_filter::CapabilityException;

    let mut filter = DangerousCapabilityFilter::with_defaults();
    // Without exception: should be Deny (D009 High).
    assert!(
        matches!(
            filter.check("connector:local:fs.delete"),
            FilterDecision::Deny { .. }
        ),
        "fs.delete not denied without exception"
    );
    // Add explicit allow.
    filter.add_exception(CapabilityException {
        capability_pattern: "connector:local:fs.delete".to_string(),
        reason: "operator approved per-actor".to_string(),
    });
    assert!(
        matches!(
            filter.check("connector:local:fs.delete"),
            FilterDecision::Allow
        ),
        "fs.delete still denied after explicit exception — escape valve broken"
    );
}
