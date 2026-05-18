//! Sprint 27: declarative agent×capability policy rules.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RuleKind {
    Allow,
    Deny,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    pub kind: RuleKind,
    /// Match this agent (`None` matches any agent).
    pub agent_id: Option<String>,
    /// Match this capability (`None` matches any capability).
    pub capability_id: Option<String>,
    /// Sprint 30: numeric priority (higher wins within the same Deny/Allow pass).
    /// Defaults to 0 for backward compatibility with S27 yaml files. Tie-breaker
    /// is file order (earlier rule wins) so existing files behave identically.
    #[serde(default)]
    pub priority: i32,
    /// Optional human-readable reason embedded in EvaluationResult.
    #[serde(default)]
    pub reason: Option<String>,
}

impl Rule {
    pub fn matches(&self, agent_id: &str, capability_id: &str) -> bool {
        let agent_match = self.agent_id.as_deref().is_none_or(|a| a == agent_id);
        let cap_match = self
            .capability_id
            .as_deref()
            .is_none_or(|c| c == capability_id);
        agent_match && cap_match
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RuleSet {
    #[serde(default)]
    pub rules: Vec<Rule>,
}

impl RuleSet {
    /// Parse a YAML string. Convenience for tests and admin UIs that already
    /// hold the raw YAML in memory; production loading goes through
    /// [`Self::from_yaml_path`] which handles missing-file fallback.
    pub fn from_yaml_str(yaml: &str) -> Result<Self, serde_yaml::Error> {
        serde_yaml::from_str(yaml)
    }

    /// Load from YAML file. On error (file missing, parse fail) returns empty + warn/error log.
    pub fn from_yaml_path(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref();
        match std::fs::read_to_string(path) {
            Ok(content) => match serde_yaml::from_str::<RuleSet>(&content) {
                Ok(rs) => {
                    tracing::info!(
                        path = %path.display(),
                        rule_count = rs.rules.len(),
                        "S27: policy rules loaded"
                    );
                    rs
                }
                Err(e) => {
                    tracing::error!(
                        path = %path.display(),
                        error = %e,
                        "S27: failed to parse policy rules YAML; falling back to empty rule set"
                    );
                    RuleSet::default()
                }
            },
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "S27: policy rules file not readable; using empty rule set"
                );
                RuleSet::default()
            }
        }
    }

    /// Deny pass first (Sprint 27 invariant), Allow pass second; within each pass the
    /// highest-priority match wins, with file-order as tiebreaker. Sprint 30 introduced
    /// `priority`; rules without it default to 0 so the picked match degenerates to
    /// "first-match" — identical to S27 behavior.
    pub fn evaluate(&self, agent_id: &str, capability_id: &str) -> Option<&Rule> {
        let pick = |kind: RuleKind| -> Option<&Rule> {
            self.rules
                .iter()
                .enumerate()
                .filter(|(_, r)| r.kind == kind && r.matches(agent_id, capability_id))
                // higher priority wins; same priority → earlier file index wins.
                .min_by_key(|(idx, r)| (-r.priority, *idx))
                .map(|(_, r)| r)
        };
        pick(RuleKind::Deny).or_else(|| pick(RuleKind::Allow))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule_set_yaml() -> &'static str {
        r#"
rules:
  - kind: deny
    agent_id: junior
    capability_id: agent.handoff
    reason: "junior agents cannot handoff"
  - kind: allow
    agent_id: trusted
    capability_id: null
    reason: "trusted agent grant"
"#
    }

    #[test]
    fn parses_yaml_correctly() {
        let rs: RuleSet = serde_yaml::from_str(rule_set_yaml()).unwrap();
        assert_eq!(rs.rules.len(), 2);
        assert_eq!(rs.rules[0].kind, RuleKind::Deny);
        assert_eq!(rs.rules[0].agent_id.as_deref(), Some("junior"));
    }

    #[test]
    fn deny_takes_priority() {
        let rs: RuleSet = serde_yaml::from_str(rule_set_yaml()).unwrap();
        let r = rs.evaluate("junior", "agent.handoff").unwrap();
        assert_eq!(r.kind, RuleKind::Deny);
    }

    #[test]
    fn allow_matches_wildcard_capability() {
        let rs: RuleSet = serde_yaml::from_str(rule_set_yaml()).unwrap();
        let r = rs.evaluate("trusted", "anything.foo").unwrap();
        assert_eq!(r.kind, RuleKind::Allow);
    }

    #[test]
    fn no_match_returns_none() {
        let rs: RuleSet = serde_yaml::from_str(rule_set_yaml()).unwrap();
        assert!(rs.evaluate("other-agent", "other.cap").is_none());
    }

    #[test]
    fn missing_file_returns_empty_ruleset() {
        let rs = RuleSet::from_yaml_path("/nonexistent/path.yaml");
        assert!(rs.rules.is_empty());
    }

    #[test]
    fn priority_overrides_file_order_within_same_kind() {
        // Two Deny rules match the same (agent, cap); the second is higher priority.
        // Without S30 the file-order rule (first one, "default") would win; with
        // priority the second one ("urgent") must win.
        let yaml = r#"
rules:
  - kind: deny
    agent_id: a1
    capability_id: c1
    priority: 0
    reason: "default-block"
  - kind: deny
    agent_id: a1
    capability_id: c1
    priority: 10
    reason: "urgent-block"
"#;
        let rs: RuleSet = serde_yaml::from_str(yaml).unwrap();
        let r = rs.evaluate("a1", "c1").unwrap();
        assert_eq!(r.reason.as_deref(), Some("urgent-block"));
    }

    #[test]
    fn priority_default_zero_preserves_s27_first_match_semantics() {
        // No priority field anywhere → S27 file-order behavior must hold.
        let yaml = r#"
rules:
  - kind: deny
    agent_id: a1
    capability_id: c1
    reason: "first-block"
  - kind: deny
    agent_id: a1
    capability_id: c1
    reason: "second-block"
"#;
        let rs: RuleSet = serde_yaml::from_str(yaml).unwrap();
        let r = rs.evaluate("a1", "c1").unwrap();
        assert_eq!(r.reason.as_deref(), Some("first-block"));
    }

    #[test]
    fn deny_pass_still_beats_allow_even_with_lower_priority() {
        // S27 invariant: Deny pass runs first regardless of Allow priority.
        // A priority-100 Allow does NOT override a priority-0 Deny.
        let yaml = r#"
rules:
  - kind: allow
    agent_id: a1
    capability_id: c1
    priority: 100
  - kind: deny
    agent_id: a1
    capability_id: c1
    priority: 0
"#;
        let rs: RuleSet = serde_yaml::from_str(yaml).unwrap();
        let r = rs.evaluate("a1", "c1").unwrap();
        assert_eq!(r.kind, RuleKind::Deny);
    }
}
