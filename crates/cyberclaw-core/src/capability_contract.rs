//! # Capability Behavior Contract
//!
//! Provides structured behavioral metadata for each Capability, enabling
//! the AgenticLoop to determine tool parallelism safety, result size budgets,
//! and destructiveness without inspecting the capability implementation.

use serde::{Deserialize, Serialize};

/// Behavioral contract metadata for a Capability.
///
/// Used by the AgenticLoop to determine tool parallel-safety,
/// context budget estimation, and destructiveness classification.
///
/// # Examples
///
/// ```rust
/// use cyberclaw_core::capability_contract::CapabilityBehaviorContract;
///
/// // A read-only, concurrency-safe capability
/// let contract = CapabilityBehaviorContract {
///     is_read_only: true,
///     is_destructive: false,
///     is_concurrency_safe: true,
///     max_result_size_chars: 4096,
/// };
/// assert!(contract.is_read_only);
/// assert!(contract.is_concurrency_safe);
///
/// // Default is conservative: non-read-only, not concurrency-safe
/// let default_contract = CapabilityBehaviorContract::default();
/// assert!(!default_contract.is_read_only);
/// assert!(!default_contract.is_concurrency_safe);
/// ```
// Conservative defaults: undeclared capabilities are assumed
// non-read-only and not concurrency-safe (all fields default to false/0).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct CapabilityBehaviorContract {
    /// Whether this Capability is read-only (does not mutate external state).
    pub is_read_only: bool,
    /// Whether this Capability is destructive (delete, overwrite, or other irreversible ops).
    pub is_destructive: bool,
    /// Whether this Capability can be safely executed concurrently.
    pub is_concurrency_safe: bool,
    /// Maximum result size in characters (for context budget estimation, 0 = unlimited).
    pub max_result_size_chars: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_values_are_conservative() {
        let contract = CapabilityBehaviorContract::default();
        assert!(!contract.is_read_only);
        assert!(!contract.is_destructive);
        assert!(!contract.is_concurrency_safe);
        assert_eq!(contract.max_result_size_chars, 0);
    }

    #[test]
    fn serde_roundtrip() {
        let contract = CapabilityBehaviorContract {
            is_read_only: true,
            is_destructive: false,
            is_concurrency_safe: true,
            max_result_size_chars: 8192,
        };
        let json = serde_json::to_string(&contract).unwrap();
        let deserialized: CapabilityBehaviorContract = serde_json::from_str(&json).unwrap();
        assert!(deserialized.is_read_only);
        assert!(!deserialized.is_destructive);
        assert!(deserialized.is_concurrency_safe);
        assert_eq!(deserialized.max_result_size_chars, 8192);
    }

    #[test]
    fn serde_missing_fields_use_defaults() {
        // Backward compatibility: empty JSON object should deserialize with defaults
        let json = "{}";
        let contract: CapabilityBehaviorContract = serde_json::from_str(json).unwrap();
        assert!(!contract.is_read_only);
        assert!(!contract.is_destructive);
        assert!(!contract.is_concurrency_safe);
        assert_eq!(contract.max_result_size_chars, 0);
    }

    #[test]
    fn serde_partial_fields_use_defaults_for_missing() {
        let json = r#"{"isReadOnly": true}"#;
        let contract: CapabilityBehaviorContract = serde_json::from_str(json).unwrap();
        assert!(contract.is_read_only);
        assert!(!contract.is_destructive);
        assert!(!contract.is_concurrency_safe);
        assert_eq!(contract.max_result_size_chars, 0);
    }
}
