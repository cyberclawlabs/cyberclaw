//! Production-grade SecurityGate backed by DangerousCapabilityFilter.
//!
//! [`ProductionSecurityGate`] replaces [`PassthroughSecurityGate`] for production
//! environments. It evaluates each execution result's artifacts and output against
//! the governance [`DangerousCapabilityFilter`], flagging Critical and High severity
//! capabilities.

use async_trait::async_trait;
use cyberclaw_core::autopilot::ExecutionResult;
use cyberclaw_governance::dangerous_capability_filter::{
    DangerSeverity, DangerousCapabilityFilter, FilterDecision,
};
use tracing::warn;

use crate::autopilot_runtime::{SecurityCheckResult, SecurityGate};

/// Production security gate that evaluates execution results against
/// the [`DangerousCapabilityFilter`] rule set.
///
/// Behavior:
/// - **Critical** severity match: `passed = false`, `requires_review = true`
/// - **High** severity match: `passed = true`, `requires_review = true`
/// - **Medium** or no match: `passed = true`, `requires_review = false`
pub struct ProductionSecurityGate {
    filter: DangerousCapabilityFilter,
}

impl ProductionSecurityGate {
    /// Create a gate with the default 7 danger rules.
    pub fn with_defaults() -> Self {
        Self {
            filter: DangerousCapabilityFilter::with_defaults(),
        }
    }

    /// Create a gate with a custom filter configuration.
    pub fn new(filter: DangerousCapabilityFilter) -> Self {
        Self { filter }
    }

    /// Extract capability identifiers from a single execution result.
    ///
    /// Sources checked (in order):
    /// 1. `artifacts` — each string is treated as a potential capability ID.
    /// 2. `output` JSON — looks for `"capability_id"` or `"capabilities"` fields.
    fn extract_capabilities(result: &ExecutionResult) -> Vec<String> {
        let mut caps: Vec<String> = result.artifacts.clone();

        if let Some(ref output) = result.output {
            // Single capability_id field
            if let Some(cap_id) = output.get("capability_id").and_then(|v| v.as_str()) {
                caps.push(cap_id.to_string());
            }
            // Array of capabilities
            if let Some(arr) = output.get("capabilities").and_then(|v| v.as_array()) {
                for item in arr {
                    if let Some(s) = item.as_str() {
                        caps.push(s.to_string());
                    }
                }
            }
        }

        caps
    }
}

#[async_trait]
impl SecurityGate for ProductionSecurityGate {
    async fn check_execution_results(
        &self,
        results: &[ExecutionResult],
    ) -> anyhow::Result<SecurityCheckResult> {
        let mut issues: Vec<String> = Vec::new();
        let mut has_critical = false;
        let mut has_high = false;

        for result in results {
            let capabilities = Self::extract_capabilities(result);

            for cap in &capabilities {
                match self.filter.check(cap) {
                    FilterDecision::Deny {
                        rule_id,
                        severity,
                        reason,
                    } => {
                        let severity_label = match &severity {
                            DangerSeverity::Critical => "CRITICAL",
                            DangerSeverity::High => "HIGH",
                            DangerSeverity::Medium => "MEDIUM",
                        };
                        let issue = format!(
                            "[{}] rule {} denied capability '{}': {}",
                            severity_label, rule_id, cap, reason
                        );
                        warn!(
                            execution_id = %result.execution_id,
                            rule_id = %rule_id,
                            capability = %cap,
                            severity = %severity_label,
                            "Security gate flagged dangerous capability"
                        );
                        issues.push(issue);

                        match severity {
                            DangerSeverity::Critical => has_critical = true,
                            DangerSeverity::High => has_high = true,
                            DangerSeverity::Medium => {}
                        }
                    }
                    FilterDecision::Warn {
                        rule_id, reason, ..
                    } => {
                        let issue = format!(
                            "[MEDIUM] rule {} warned on capability '{}': {}",
                            rule_id, cap, reason
                        );
                        issues.push(issue);
                    }
                    FilterDecision::Allow => {}
                }
            }
        }

        let passed = !has_critical;
        let requires_review = has_critical || has_high;

        Ok(SecurityCheckResult {
            passed,
            issues,
            requires_review,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyberclaw_core::execution::ExecutionStatus;
    use cyberclaw_core::ids::ExecutionId;

    fn make_result(artifacts: Vec<&str>, output: Option<serde_json::Value>) -> ExecutionResult {
        ExecutionResult {
            execution_id: ExecutionId::new(),
            status: ExecutionStatus::Completed,
            output,
            error: None,
            artifacts: artifacts.into_iter().map(String::from).collect(),
            duration_ms: 100,
        }
    }

    #[tokio::test]
    async fn all_safe_capabilities_pass() {
        let gate = ProductionSecurityGate::with_defaults();
        let results = vec![
            make_result(vec!["fs:local:read", "fs:local:write"], None),
            make_result(vec!["tool:grep:search"], None),
        ];
        let check = gate.check_execution_results(&results).await.unwrap();
        assert!(check.passed, "safe capabilities should pass");
        assert!(!check.requires_review, "safe capabilities need no review");
        assert!(check.issues.is_empty(), "no issues expected");
    }

    #[tokio::test]
    async fn critical_capability_fails() {
        let gate = ProductionSecurityGate::with_defaults();
        let results = vec![make_result(vec!["shell:bash:destructive"], None)];
        let check = gate.check_execution_results(&results).await.unwrap();
        assert!(!check.passed, "critical capability must fail");
        assert!(check.requires_review, "critical must require review");
        assert_eq!(check.issues.len(), 1);
        assert!(check.issues[0].contains("CRITICAL"));
    }

    #[tokio::test]
    async fn high_capability_passes_with_review() {
        let gate = ProductionSecurityGate::with_defaults();
        let results = vec![make_result(vec!["shell:curl:network"], None)];
        let check = gate.check_execution_results(&results).await.unwrap();
        assert!(check.passed, "high severity should still pass");
        assert!(check.requires_review, "high severity must require review");
        assert_eq!(check.issues.len(), 1);
        assert!(check.issues[0].contains("HIGH"));
    }

    #[tokio::test]
    async fn mixed_critical_and_high_fails() {
        let gate = ProductionSecurityGate::with_defaults();
        let results = vec![
            make_result(vec!["shell:curl:network"], None),   // High
            make_result(vec!["connector:k8s:deploy"], None), // Critical
        ];
        let check = gate.check_execution_results(&results).await.unwrap();
        assert!(!check.passed, "critical in mix must fail");
        assert!(check.requires_review);
        assert_eq!(check.issues.len(), 2);
    }

    #[tokio::test]
    async fn capabilities_from_output_json() {
        let gate = ProductionSecurityGate::with_defaults();
        let output = serde_json::json!({
            "capability_id": "connector:aws:delete",
            "capabilities": ["agent:worker:spawn"]
        });
        let results = vec![make_result(vec![], Some(output))];
        let check = gate.check_execution_results(&results).await.unwrap();
        assert!(!check.passed, "critical from output should fail");
        assert!(check.requires_review);
        // connector:aws:delete = Critical (D004), agent:worker:spawn = High (D005)
        assert_eq!(check.issues.len(), 2);
    }

    #[tokio::test]
    async fn empty_results_pass() {
        let gate = ProductionSecurityGate::with_defaults();
        let check = gate.check_execution_results(&[]).await.unwrap();
        assert!(check.passed);
        assert!(!check.requires_review);
        assert!(check.issues.is_empty());
    }
}
