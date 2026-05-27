//! Dangerous capability filter for auto-mode execution gating.
//!
//! Provides [`DangerousCapabilityFilter`] which checks capability identifiers
//! against a set of [`DangerousRule`]s and returns a [`FilterDecision`]
//! indicating whether the capability should be allowed, warned, or denied.
//!
//! R13-BUG-01: also provides [`DangerousCapabilityFilter::check_write_content`]
//! which performs input-side credential detection for `fs.write` operations
//! before they reach the connector layer.

use regex::Regex;
use tracing::warn;

// ---------------------------------------------------------------------------
// Credential pattern constants reused from tool_output_sanitizer
// ---------------------------------------------------------------------------

/// Input-side credential patterns checked before any `fs.write` is dispatched.
///
/// Each entry is `(name, regex_source)`.  Patterns match common credential
/// formats that must never be written to disk by an autonomous agent.
static WRITE_CREDENTIAL_PATTERNS: &[(&str, &str)] = &[
    // AWS access key ID — always starts with AKIA followed by 16 uppercase alphanumerics.
    ("aws_access_key_id", r"AKIA[0-9A-Z]{16}"),
    // Bare key-value forms used in config files / .env (case-insensitive label).
    (
        "aws_credential_field",
        r"(?i)(aws_access_key_id|aws_secret_access_key)\s*[=:]",
    ),
    // Generic credential field names that indicate secret content.
    (
        "generic_credential_field",
        r"(?i)(api_key|apikey|secret_key|secret|password|passwd|token)\s*[=:]",
    ),
    // GitHub personal-access / oauth / server / refresh / user tokens.
    ("github_token", r"gh[pousr]_[A-Za-z0-9_]{36,}"),
    // Anthropic API key.
    ("anthropic_api_key", r"sk-ant-[A-Za-z0-9_\-]{20,}"),
    // OpenAI / generic sk- keys.
    ("openai_api_key", r"sk-[A-Za-z0-9]{20,}"),
    // PEM private-key header.
    (
        "pem_private_key",
        r"-----BEGIN (?:RSA |EC )?PRIVATE KEY-----",
    ),
];

/// Severity level of a dangerous capability rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DangerSeverity {
    /// Always intercepted; no exception can override.
    Critical,
    /// Intercepted in auto mode by default; can be overridden by an exception.
    High,
    /// Logged as a warning but not blocked.
    Medium,
}

/// A rule describing a category of dangerous capabilities.
pub struct DangerousRule {
    /// Unique identifier for this rule (e.g. "D001").
    pub id: String,
    /// Glob-style capability pattern where `*` matches any substring.
    pub capability_pattern: String,
    /// Severity level that determines the filter action.
    pub severity: DangerSeverity,
    /// Human-readable explanation for why this capability is dangerous.
    pub reason: String,
}

/// An exception that allows a normally-blocked High-severity capability through.
///
/// Critical-severity rules are never overridden by exceptions.
#[derive(Debug, Clone)]
pub struct CapabilityException {
    /// Glob-style pattern identifying the capability to exempt.
    pub capability_pattern: String,
    /// Human-readable reason for granting the exception.
    pub reason: String,
}

/// The outcome of a [`DangerousCapabilityFilter::check`] call.
#[derive(Debug, Clone)]
pub enum FilterDecision {
    /// The capability is permitted.
    Allow,
    /// The capability matched a Medium rule; a warning is emitted but execution continues.
    Warn {
        /// ID of the matching rule.
        rule_id: String,
        /// Reason from the matching rule.
        reason: String,
    },
    /// The capability is blocked.
    Deny {
        /// ID of the matching rule.
        rule_id: String,
        /// Severity of the matching rule.
        severity: DangerSeverity,
        /// Reason from the matching rule.
        reason: String,
    },
}

/// Filter that evaluates capability identifiers against a set of danger rules.
///
/// # Examples
///
/// ```
/// use cyberclaw_governance::dangerous_capability_filter::{
///     DangerousCapabilityFilter, FilterDecision,
/// };
///
/// let filter = DangerousCapabilityFilter::with_defaults();
/// match filter.check("shell:bash:destructive") {
///     FilterDecision::Deny { rule_id, .. } => println!("Denied by {}", rule_id),
///     other => println!("{:?}", other),
/// }
/// ```
pub struct DangerousCapabilityFilter {
    rules: Vec<DangerousRule>,
    exceptions: Vec<CapabilityException>,
}

impl DangerousCapabilityFilter {
    /// Create a filter with no rules or exceptions.
    pub fn new() -> Self {
        Self {
            rules: Vec::new(),
            exceptions: Vec::new(),
        }
    }

    /// Create a filter pre-loaded with the 8 default danger rules.
    pub fn with_defaults() -> Self {
        let mut filter = Self::new();
        filter.add_rule(DangerousRule {
            id: "D001".to_string(),
            capability_pattern: "shell:*:destructive".to_string(),
            severity: DangerSeverity::Critical,
            reason: "Destructive shell command".to_string(),
        });
        filter.add_rule(DangerousRule {
            id: "D002".to_string(),
            capability_pattern: "shell:*:network".to_string(),
            severity: DangerSeverity::High,
            reason: "Network operation via shell".to_string(),
        });
        filter.add_rule(DangerousRule {
            id: "D003".to_string(),
            // Trailing `*` catches both 3-segment `connector:k8s:deploy`
            // and 4-segment MCP-bridged `connector:mcp:{server}:deploy*`.
            capability_pattern: "connector:*:deploy*".to_string(),
            severity: DangerSeverity::Critical,
            reason: "Deployment operation".to_string(),
        });
        filter.add_rule(DangerousRule {
            id: "D004".to_string(),
            // Trailing `*` catches both `connector:k8s:delete`
            // and 4-segment MCP-bridged `connector:mcp:{server}:delete_file` etc.
            capability_pattern: "connector:*:delete*".to_string(),
            severity: DangerSeverity::Critical,
            reason: "Delete operation".to_string(),
        });
        filter.add_rule(DangerousRule {
            id: "D005".to_string(),
            capability_pattern: "agent:*:spawn".to_string(),
            severity: DangerSeverity::High,
            reason: "Autonomous child agent spawn".to_string(),
        });
        filter.add_rule(DangerousRule {
            id: "D006".to_string(),
            capability_pattern: "plugin:*:install".to_string(),
            severity: DangerSeverity::High,
            reason: "Runtime plugin installation".to_string(),
        });
        filter.add_rule(DangerousRule {
            id: "D007".to_string(),
            capability_pattern: "capability:*:credential".to_string(),
            severity: DangerSeverity::Critical,
            reason: "Credential operation".to_string(),
        });
        // F-3 (commit edbe2a1 follow-up): cmd.run executes unrestricted
        // shell on the host. The host-level safety gate (ToolPermissionMatcher
        // + sandbox) already blocks dangerous argv shapes; this rule is
        // defence-in-depth at the policy layer so an explicit allow is
        // required before the request even reaches the runner.
        filter.add_rule(DangerousRule {
            id: "D008".to_string(),
            capability_pattern: "connector:local:cmd.run".to_string(),
            severity: DangerSeverity::Critical,
            reason: "cmd.run executes unrestricted shell commands; require explicit allow"
                .to_string(),
        });
        // D1 follow-up (2026-05-12): explicit guard for `fs.delete`. D004's
        // `connector:*:delete*` pattern does NOT match `connector:local:fs.delete`
        // (the `:delete` literal segment needs `:` immediately before `delete`,
        // but the capability id has `:fs.delete`). Without this rule, an agent
        // could chain `fs.delete` recursively on host paths via an explicit
        // absolute path (validate_path only guards the shared workspace_root,
        // not arbitrary hosts). High severity: exception-allowable per actor.
        filter.add_rule(DangerousRule {
            id: "D009".to_string(),
            capability_pattern: "connector:local:fs.delete".to_string(),
            severity: DangerSeverity::High,
            reason: "fs.delete is destructive; require explicit allow per actor".to_string(),
        });
        filter
    }

    /// Add a custom rule to the filter.
    pub fn add_rule(&mut self, rule: DangerousRule) {
        self.rules.push(rule);
    }

    /// Read-only accessor over the rule set (used by admin UIs / diagnostics).
    pub fn rules(&self) -> &[DangerousRule] {
        &self.rules
    }

    /// Add an exception that allows High-severity matches to pass through.
    ///
    /// Exceptions have no effect on Critical-severity rules.
    pub fn add_exception(&mut self, exception: CapabilityException) {
        self.exceptions.push(exception);
    }

    /// Check whether writing `content` via `capability_id` is safe.
    ///
    /// This is the **input-side** credential check performed before any
    /// `fs.write` (or similar write-to-disk) capability is dispatched.
    /// It is independent of the capability-pattern rules checked by [`check`].
    ///
    /// Returns [`FilterDecision::Deny`] (Critical) if `content` contains
    /// a credential pattern.  Returns [`FilterDecision::Allow`] otherwise.
    ///
    /// The caller should still invoke [`check`] on the capability identifier
    /// itself — this method only inspects the write payload.
    ///
    /// # R13-BUG-01
    ///
    /// QA R13 TC3: agent wrote `aws-credentials.txt` (116 bytes) containing
    /// AWS credentials without any governance interception.  Adding this
    /// pre-dispatch content scan closes that gap for all `fs.write` paths.
    pub fn check_write_content(&self, capability_id: &str, content: &str) -> FilterDecision {
        // Only inspect write capabilities (and shell execution paths that can
        // write credentials to disk via redirect — R13-BUG-01 extension, QA R15).
        let cap_lower = capability_id.to_lowercase();
        if !cap_lower.contains("fs.write")
            && !cap_lower.contains("file.write")
            && !cap_lower.contains("write_file")
            && !cap_lower.contains("cmd.run")
        {
            return FilterDecision::Allow;
        }

        for (name, pattern_src) in WRITE_CREDENTIAL_PATTERNS {
            let re = match Regex::new(pattern_src) {
                Ok(r) => r,
                Err(_) => continue,
            };
            if re.is_match(content) {
                warn!(
                    capability = %capability_id,
                    pattern = %name,
                    "check_write_content: credential pattern detected in fs.write content — DENY"
                );
                return FilterDecision::Deny {
                    rule_id: "D010".to_string(),
                    severity: DangerSeverity::Critical,
                    reason: format!(
                        "writing credential-pattern content blocked by policy (matched: {})",
                        name
                    ),
                };
            }
        }
        FilterDecision::Allow
    }

    /// Check whether `capability_id` is permitted, warned, or denied.
    ///
    /// Rules are evaluated in insertion order. The first matching rule
    /// determines the decision. Medium rules always produce [`FilterDecision::Warn`].
    /// High rules produce [`FilterDecision::Deny`] unless an exception matches,
    /// in which case they produce [`FilterDecision::Allow`].
    /// Critical rules always produce [`FilterDecision::Deny`] regardless of exceptions.
    pub fn check(&self, capability_id: &str) -> FilterDecision {
        for rule in &self.rules {
            if !wildcard_match(&rule.capability_pattern, capability_id) {
                continue;
            }
            match rule.severity {
                DangerSeverity::Critical => {
                    return FilterDecision::Deny {
                        rule_id: rule.id.clone(),
                        severity: DangerSeverity::Critical,
                        reason: rule.reason.clone(),
                    };
                }
                DangerSeverity::High => {
                    if self.has_exception(capability_id) {
                        return FilterDecision::Allow;
                    }
                    return FilterDecision::Deny {
                        rule_id: rule.id.clone(),
                        severity: DangerSeverity::High,
                        reason: rule.reason.clone(),
                    };
                }
                DangerSeverity::Medium => {
                    warn!(
                        rule_id = %rule.id,
                        capability = %capability_id,
                        reason = %rule.reason,
                        "Dangerous capability matched Medium rule"
                    );
                    return FilterDecision::Warn {
                        rule_id: rule.id.clone(),
                        reason: rule.reason.clone(),
                    };
                }
            }
        }
        FilterDecision::Allow
    }

    /// Return all capabilities from `active_capabilities` that would be blocked.
    ///
    /// Each entry is a `(capability_id, rule_id, severity)` triple.
    /// Only capabilities that result in [`FilterDecision::Deny`] are included.
    pub fn list_stripped_capabilities(
        &self,
        active_capabilities: &[String],
    ) -> Vec<(String, String, DangerSeverity)> {
        active_capabilities
            .iter()
            .filter_map(|cap| match self.check(cap) {
                FilterDecision::Deny {
                    rule_id, severity, ..
                } => Some((cap.clone(), rule_id, severity)),
                _ => None,
            })
            .collect()
    }

    /// Return `true` if any registered exception pattern matches `capability_id`.
    fn has_exception(&self, capability_id: &str) -> bool {
        self.exceptions
            .iter()
            .any(|e| wildcard_match(&e.capability_pattern, capability_id))
    }
}

impl Default for DangerousCapabilityFilter {
    fn default() -> Self {
        Self::new()
    }
}

/// Match `capability_id` against `pattern` where `*` matches any substring.
///
/// The pattern is split on `*` into segments; the capability must contain
/// each segment in order, without overlapping.
fn wildcard_match(pattern: &str, capability_id: &str) -> bool {
    // Case-insensitive matching to prevent bypass via casing (e.g. "Shell:Bash:Destructive")
    let pattern_lower = pattern.to_lowercase();
    let cap_lower = capability_id.to_lowercase();
    let segments: Vec<&str> = pattern_lower.split('*').collect();
    let mut remaining = cap_lower.as_str();
    for (i, segment) in segments.iter().enumerate() {
        if segment.is_empty() {
            continue;
        }
        if i == 0 {
            // First segment must match at the start when there is no leading wildcard.
            if !pattern.starts_with('*') {
                if !remaining.starts_with(segment) {
                    return false;
                }
                remaining = &remaining[segment.len()..];
            } else if let Some(pos) = remaining.find(segment) {
                remaining = &remaining[pos + segment.len()..];
            } else {
                return false;
            }
        } else if i == segments.len() - 1 && !pattern.ends_with('*') {
            // Last segment must match at the end when there is no trailing wildcard.
            if !remaining.ends_with(segment) {
                return false;
            }
        } else if let Some(pos) = remaining.find(segment) {
            remaining = &remaining[pos + segment.len()..];
        } else {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_rules_count() {
        let filter = DangerousCapabilityFilter::with_defaults();
        assert_eq!(filter.rules.len(), 9, "should have exactly 9 default rules");
    }

    // R13-BUG-01: input-side credential detection for fs.write
    #[test]
    fn test_fs_write_aws_access_key_blocked() {
        let filter = DangerousCapabilityFilter::with_defaults();
        let content = "aws_access_key_id = AKIAIOSFODNN7EXAMPLE\naws_secret_access_key = wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY\n";
        match filter.check_write_content("connector:local:fs.write", content) {
            FilterDecision::Deny {
                rule_id,
                severity,
                reason,
            } => {
                assert_eq!(rule_id, "D010");
                assert_eq!(severity, DangerSeverity::Critical);
                assert!(reason.contains("credential-pattern"), "reason: {reason}");
            }
            other => panic!("expected Deny for AWS credential content, got {:?}", other),
        }
    }

    #[test]
    fn test_fs_write_non_credential_allowed() {
        let filter = DangerousCapabilityFilter::with_defaults();
        let content = "hello world\nThis is a plain text file with no secrets.\n";
        match filter.check_write_content("connector:local:fs.write", content) {
            FilterDecision::Allow => {}
            other => panic!("expected Allow for non-credential content, got {:?}", other),
        }
    }

    /// D1 follow-up (2026-05-12): D009 must explicitly cover `fs.delete`.
    /// D004's `connector:*:delete*` wildcard does NOT match `:fs.delete`
    /// because `:delete` requires `:` immediately before `delete`.
    #[test]
    fn test_fs_delete_blocked_by_default_rule() {
        let filter = DangerousCapabilityFilter::with_defaults();
        match filter.check("connector:local:fs.delete") {
            FilterDecision::Deny {
                rule_id, severity, ..
            } => {
                assert_eq!(rule_id, "D009");
                assert_eq!(severity, DangerSeverity::High);
            }
            other => panic!("fs.delete should be denied by default, got {:?}", other),
        }
    }

    /// F-3 (commit edbe2a1 follow-up): cmd.run must be in the default rule
    /// set as Critical so the policy layer rejects it before the host
    /// runner ever sees the argv. Defence-in-depth on top of the existing
    /// 22 ToolPermissionMatcher safety gates.
    #[test]
    fn test_cmd_run_blocked_by_default_rule() {
        let filter = DangerousCapabilityFilter::with_defaults();
        match filter.check("connector:local:cmd.run") {
            FilterDecision::Deny {
                rule_id, severity, ..
            } => {
                assert_eq!(rule_id, "D008");
                assert_eq!(severity, DangerSeverity::Critical);
            }
            other => panic!("expected Critical Deny for cmd.run, got {:?}", other),
        }
    }

    /// F-3: a registered exception MUST NOT override D008 — Critical rules
    /// ignore exceptions by design (mirrors test_critical_rule_ignores_exception).
    #[test]
    fn test_cmd_run_critical_ignores_exception() {
        let mut filter = DangerousCapabilityFilter::with_defaults();
        filter.add_exception(CapabilityException {
            capability_pattern: "connector:local:cmd.run".to_string(),
            reason: "attempted override".to_string(),
        });
        match filter.check("connector:local:cmd.run") {
            FilterDecision::Deny { severity, .. } => {
                assert_eq!(severity, DangerSeverity::Critical);
            }
            other => panic!("expected Deny, got {:?}", other),
        }
    }

    #[test]
    fn test_critical_rule_always_denies() {
        let filter = DangerousCapabilityFilter::with_defaults();
        match filter.check("shell:bash:destructive") {
            FilterDecision::Deny {
                rule_id, severity, ..
            } => {
                assert_eq!(rule_id, "D001");
                assert_eq!(severity, DangerSeverity::Critical);
            }
            other => panic!("expected Deny, got {:?}", other),
        }
    }

    #[test]
    fn test_high_rule_denies_without_exception() {
        let filter = DangerousCapabilityFilter::with_defaults();
        match filter.check("shell:curl:network") {
            FilterDecision::Deny {
                rule_id, severity, ..
            } => {
                assert_eq!(rule_id, "D002");
                assert_eq!(severity, DangerSeverity::High);
            }
            other => panic!("expected Deny, got {:?}", other),
        }
    }

    #[test]
    fn test_high_rule_allows_with_exception() {
        let mut filter = DangerousCapabilityFilter::with_defaults();
        filter.add_exception(CapabilityException {
            capability_pattern: "shell:curl:network".to_string(),
            reason: "Approved for CI pipeline".to_string(),
        });
        match filter.check("shell:curl:network") {
            FilterDecision::Allow => {}
            other => panic!("expected Allow, got {:?}", other),
        }
    }

    #[test]
    fn test_critical_rule_ignores_exception() {
        // Exceptions must NOT override Critical rules.
        let mut filter = DangerousCapabilityFilter::with_defaults();
        filter.add_exception(CapabilityException {
            capability_pattern: "shell:*:destructive".to_string(),
            reason: "Attempted override".to_string(),
        });
        match filter.check("shell:bash:destructive") {
            FilterDecision::Deny { severity, .. } => {
                assert_eq!(severity, DangerSeverity::Critical);
            }
            other => panic!("expected Deny, got {:?}", other),
        }
    }

    #[test]
    fn test_medium_rule_warns() {
        let mut filter = DangerousCapabilityFilter::new();
        filter.add_rule(DangerousRule {
            id: "T001".to_string(),
            capability_pattern: "fs:*:read".to_string(),
            severity: DangerSeverity::Medium,
            reason: "File read access".to_string(),
        });
        match filter.check("fs:local:read") {
            FilterDecision::Warn { rule_id, .. } => {
                assert_eq!(rule_id, "T001");
            }
            other => panic!("expected Warn, got {:?}", other),
        }
    }

    #[test]
    fn test_no_match_allows() {
        let filter = DangerousCapabilityFilter::with_defaults();
        match filter.check("fs:local:read") {
            FilterDecision::Allow => {}
            other => panic!("expected Allow, got {:?}", other),
        }
    }

    #[test]
    fn test_wildcard_matching() {
        // Exact match via wildcard expansion
        assert!(wildcard_match(
            "shell:*:destructive",
            "shell:bash:destructive"
        ));
        assert!(wildcard_match(
            "shell:*:destructive",
            "shell:powershell:destructive"
        ));
        assert!(wildcard_match("connector:*:deploy", "connector:k8s:deploy"));
        assert!(wildcard_match(
            "capability:*:credential",
            "capability:vault:credential"
        ));

        // Should NOT match different suffix
        assert!(!wildcard_match("shell:*:destructive", "shell:bash:network"));
        // Should NOT match different prefix
        assert!(!wildcard_match(
            "shell:*:destructive",
            "plugin:bash:destructive"
        ));
        // No wildcard, exact only
        assert!(wildcard_match("fs:local:read", "fs:local:read"));
        assert!(!wildcard_match("fs:local:read", "fs:remote:read"));

        // Case-insensitive matching — prevents bypass via casing
        assert!(wildcard_match(
            "shell:*:destructive",
            "Shell:Bash:Destructive"
        ));
        assert!(wildcard_match(
            "shell:*:destructive",
            "SHELL:BASH:DESTRUCTIVE"
        ));
        assert!(wildcard_match("fs:local:read", "FS:LOCAL:READ"));
    }

    #[test]
    fn test_mcp_bridged_capability_blocked_by_connector_rules() {
        // Regression guard for Sprint 2026-04-19: MCP tool_bridge now emits
        // `connector:mcp:{server}:{tool}` capability ids (previously
        // `mcp.tool.{server}.{tool}`), which bypassed the connector-scoped
        // default rules silently. Verify the new format is correctly
        // intercepted by D004 (delete) and D003 (deploy).
        let filter = DangerousCapabilityFilter::with_defaults();
        match filter.check("connector:mcp:fs:delete_file") {
            FilterDecision::Deny {
                rule_id, severity, ..
            } => {
                assert_eq!(rule_id, "D004");
                assert_eq!(severity, DangerSeverity::Critical);
            }
            other => panic!("expected Deny for mcp delete, got {:?}", other),
        }
        match filter.check("connector:mcp:k8s:deploy") {
            FilterDecision::Deny {
                rule_id, severity, ..
            } => {
                assert_eq!(rule_id, "D003");
                assert_eq!(severity, DangerSeverity::Critical);
            }
            other => panic!("expected Deny for mcp deploy, got {:?}", other),
        }
        // Benign reads still flow through.
        match filter.check("connector:mcp:fs:read_file") {
            FilterDecision::Allow => {}
            other => panic!("expected Allow, got {:?}", other),
        }
    }

    #[test]
    fn test_list_stripped_capabilities() {
        let filter = DangerousCapabilityFilter::with_defaults();
        let caps = vec![
            "shell:bash:destructive".to_string(),
            "fs:local:read".to_string(),
            "connector:k8s:deploy".to_string(),
            "agent:worker:spawn".to_string(),
        ];
        let stripped = filter.list_stripped_capabilities(&caps);
        let ids: Vec<&str> = stripped.iter().map(|(id, _, _)| id.as_str()).collect();
        assert!(
            ids.contains(&"shell:bash:destructive"),
            "destructive shell should be stripped"
        );
        assert!(
            ids.contains(&"connector:k8s:deploy"),
            "deploy connector should be stripped"
        );
        assert!(
            ids.contains(&"agent:worker:spawn"),
            "agent spawn should be stripped"
        );
        assert!(
            !ids.contains(&"fs:local:read"),
            "safe cap should not be stripped"
        );
    }
}
