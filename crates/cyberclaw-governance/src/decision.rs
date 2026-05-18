//! Governance decision types and review types.

use serde::{Deserialize, Serialize};

/// Review type for governance decisions requiring review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewType {
    /// Human review required
    Human,
    /// Automated approval workflow
    Approval,
    /// Escalation to higher authority
    Escalation,
    /// Security review required
    Security,
}

/// Governance decision for capability usage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum GovernanceDecision {
    /// Allow the capability usage
    Allow {
        /// Reason for allowing
        reason: String,
    },
    /// Deny the capability usage
    Deny {
        /// Reason for denial
        reason: String,
    },
    /// Review required before allowing
    ReviewRequired {
        /// Reason for requiring review
        reason: String,
        /// Type of review required
        review_type: ReviewType,
    },
}

impl GovernanceDecision {
    /// Create an Allow decision with the given reason
    pub fn allow(reason: impl Into<String>) -> Self {
        Self::Allow {
            reason: reason.into(),
        }
    }

    /// Create a Deny decision with the given reason
    pub fn deny(reason: impl Into<String>) -> Self {
        Self::Deny {
            reason: reason.into(),
        }
    }

    /// Create a ReviewRequired decision with the given reason and review type
    pub fn review_required(reason: impl Into<String>, review_type: ReviewType) -> Self {
        Self::ReviewRequired {
            reason: reason.into(),
            review_type,
        }
    }

    /// Check if the decision is Allow
    pub fn is_allow(&self) -> bool {
        matches!(self, Self::Allow { .. })
    }

    /// Check if the decision is Deny
    pub fn is_deny(&self) -> bool {
        matches!(self, Self::Deny { .. })
    }

    /// Check if the decision requires review
    pub fn is_review_required(&self) -> bool {
        matches!(self, Self::ReviewRequired { .. })
    }

    /// Get the reason for the decision
    pub fn reason(&self) -> &str {
        match self {
            Self::Allow { reason } => reason,
            Self::Deny { reason } => reason,
            Self::ReviewRequired { reason, .. } => reason,
        }
    }

    /// Get the review type if the decision requires review
    pub fn review_type(&self) -> Option<&ReviewType> {
        match self {
            Self::ReviewRequired { review_type, .. } => Some(review_type),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_allow_decision() {
        let decision = GovernanceDecision::allow("Low risk capability");
        assert!(decision.is_allow());
        assert!(!decision.is_deny());
        assert!(!decision.is_review_required());
        assert_eq!(decision.reason(), "Low risk capability");
        assert!(decision.review_type().is_none());
    }

    #[test]
    fn test_deny_decision() {
        let decision = GovernanceDecision::deny("High risk capability without approval");
        assert!(!decision.is_allow());
        assert!(decision.is_deny());
        assert!(!decision.is_review_required());
        assert_eq!(decision.reason(), "High risk capability without approval");
        assert!(decision.review_type().is_none());
    }

    #[test]
    fn test_review_required_human() {
        let decision = GovernanceDecision::review_required(
            "Critical capability needs review",
            ReviewType::Human,
        );
        assert!(!decision.is_allow());
        assert!(!decision.is_deny());
        assert!(decision.is_review_required());
        assert_eq!(decision.reason(), "Critical capability needs review");
        assert_eq!(decision.review_type(), Some(&ReviewType::Human));
    }

    #[test]
    fn test_review_required_security() {
        let decision = GovernanceDecision::review_required(
            "Network capability requires security review",
            ReviewType::Security,
        );
        assert!(decision.is_review_required());
        assert_eq!(decision.review_type(), Some(&ReviewType::Security));
    }

    #[test]
    fn test_serialization() {
        let decision = GovernanceDecision::allow("test");
        let json = serde_json::to_string(&decision).unwrap();
        let deserialized: GovernanceDecision = serde_json::from_str(&json).unwrap();
        assert_eq!(decision, deserialized);
    }
}
