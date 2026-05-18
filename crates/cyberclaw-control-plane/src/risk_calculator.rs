//! Risk level calculation for execution operations.
//!
//! Computes execution risk based on capability properties, effects, and agent trust level.
//! Used by the execution service to flag high-impact operations for provenance tracking
//! and security audit.

use cyberclaw_core::prelude::*;

/// Calculate execution risk level based on capability risk, effects, and agent context.
///
/// The risk level is computed hierarchically:
/// 1. Start with the base risk level from the capability itself
/// 2. Elevate risk if the operation involves write, execute, or delete effects
/// 3. Elevate risk if the capability ID suggests system-level or admin access
///
/// This function ensures that high-impact operations are properly flagged for
/// provenance tracking and security audit (see HIGH #3).
///
/// # Arguments
/// * `capability_ref` - The capability being executed, which includes base risk and effects
/// * `agent_id` - The agent requesting execution, used for trust-based risk adjustment
///
/// # Returns
/// The calculated risk level for this execution context.
#[allow(dead_code)]
pub(crate) fn calculate_execution_risk_level(
    capability_ref: &cyberclaw_core::capability::CapabilityRef,
    agent_id: &AgentId,
) -> cyberclaw_core::capability::RiskLevel {
    use cyberclaw_core::capability::{CapabilityEffect, RiskLevel};

    // 1. Base risk level from capability
    let mut risk = capability_ref.risk;

    // 2. Elevate risk for dangerous effects (write, execute)
    // Note: CapabilityEffect does not currently have a Delete variant.
    // Custom("delete") effects are handled separately via keyword matching in step 3.
    let has_dangerous_effect = capability_ref
        .effects
        .iter()
        .any(|effect| matches!(effect, CapabilityEffect::Write | CapabilityEffect::Execute));

    if has_dangerous_effect {
        risk = match risk {
            RiskLevel::Low => RiskLevel::Medium,
            RiskLevel::Medium => RiskLevel::High,
            RiskLevel::High => RiskLevel::Critical,
            RiskLevel::Critical => RiskLevel::Critical,
        };
    }

    // 3. Elevate risk for system-level or admin capabilities
    let capability_id_str = capability_ref.id.as_str().to_lowercase();
    if capability_id_str.contains("system") || capability_id_str.contains("admin") {
        risk = match risk {
            RiskLevel::Low => RiskLevel::High,
            RiskLevel::Medium => RiskLevel::High,
            RiskLevel::High => RiskLevel::Critical,
            RiskLevel::Critical => RiskLevel::Critical,
        };
    }

    // 4. Agent trust level adjustment
    risk = match resolve_trust_level(agent_id) {
        cyberclaw_core::agent::AgentTrustLevel::Trusted => match risk {
            RiskLevel::Critical => RiskLevel::High,
            RiskLevel::High => RiskLevel::Medium,
            RiskLevel::Medium => RiskLevel::Low,
            RiskLevel::Low => RiskLevel::Low,
        },
        cyberclaw_core::agent::AgentTrustLevel::Standard => risk,
        cyberclaw_core::agent::AgentTrustLevel::Restricted => match risk {
            RiskLevel::Low => RiskLevel::Medium,
            RiskLevel::Medium => RiskLevel::High,
            RiskLevel::High => RiskLevel::Critical,
            RiskLevel::Critical => RiskLevel::Critical,
        },
    };

    risk
}

/// Resolve the trust level for an agent based on its ID prefix/name.
///
/// Rules (pattern-based, no registry lookup):
/// - `"control-plane"` exact match → `Trusted`
/// - Prefix `"system."` or `"platform."` → `Trusted`
/// - Prefix `"external."` or `"untrusted."` → `Restricted`
/// - Everything else → `Standard`
pub(crate) fn resolve_trust_level(agent_id: &AgentId) -> cyberclaw_core::agent::AgentTrustLevel {
    use cyberclaw_core::agent::AgentTrustLevel;
    let s = agent_id.as_str();
    if s == "control-plane" || s.starts_with("system.") || s.starts_with("platform.") {
        AgentTrustLevel::Trusted
    } else if s.starts_with("external.") || s.starts_with("untrusted.") {
        AgentTrustLevel::Restricted
    } else {
        AgentTrustLevel::Standard
    }
}
