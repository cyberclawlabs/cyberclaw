//! Property-based tests equivalent to the libfuzzer fuzz targets.
//!
//! These tests run in standard `cargo test` (stable toolchain, CI-friendly)
//! and exercise the same code paths as the fuzz targets, using proptest
//! for systematic random generation instead of libFuzzer's mutation engine.
//!
//! Target coverage:
//! - capability_parser: JSON deserialization + validate_json_input
//! - execution_parser:  Execution/ExecutionBudget deserialization + ID validation
//! - prompt_injection:  PromptInjectionScanner / SecretScanner / CommandSafetyScanner
//! - workspace_path:    ID path traversal invariants + WorkspaceRef deserialization

use cyberclaw_core::capability::{ActionRequest, CapabilityEffect, CapabilityRef, RiskLevel};
use cyberclaw_core::execution::{Execution, ExecutionBudget, ExecutionStatus};
use cyberclaw_core::ids::{AgentId, ConnectorId, ExecutionId, NodeId, TraceId, WorkspaceId};
use cyberclaw_core::security_scanner::{
    CommandSafetyScanner, PromptInjectionScanner, SecretScanner,
};
use cyberclaw_core::validation::{validate_json_input, MAX_JSON_DEPTH, MAX_JSON_SIZE_BYTES};
use cyberclaw_core::workspace::WorkspaceRef;
use proptest::prelude::*;

// ---------------------------------------------------------------------------
// Strategy helpers
// ---------------------------------------------------------------------------

/// Generates arbitrary byte vectors (equivalent to libfuzzer byte input)
fn arb_bytes() -> impl Strategy<Value = Vec<u8>> {
    proptest::collection::vec(any::<u8>(), 0..512)
}

/// Generates arbitrary strings (valid UTF-8)
fn arb_string() -> impl Strategy<Value = String> {
    ".*"
}

// ---------------------------------------------------------------------------
// Property: capability_parser — JSON deserialization never panics
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(500))]

    /// Arbitrary bytes fed to CapabilityRef/ActionRequest deserialization must not panic.
    #[test]
    fn prop_capability_json_deserialization_never_panics(bytes in arb_bytes()) {
        let _ = serde_json::from_slice::<CapabilityRef>(&bytes);
        let _ = serde_json::from_slice::<ActionRequest>(&bytes);
        let _ = serde_json::from_slice::<RiskLevel>(&bytes);
        let _ = serde_json::from_slice::<CapabilityEffect>(&bytes);
    }

    /// validate_json_input never panics for any byte input.
    #[test]
    fn prop_validate_json_input_never_panics(bytes in arb_bytes()) {
        let _ = validate_json_input(&bytes, MAX_JSON_SIZE_BYTES, MAX_JSON_DEPTH);
        let _ = validate_json_input(&bytes, 1, MAX_JSON_DEPTH);
        let _ = validate_json_input(&bytes, MAX_JSON_SIZE_BYTES, 1);
    }

    /// If JSON validates successfully, the size and depth must be within limits.
    #[test]
    fn prop_validate_json_respects_size_limit(
        bytes in arb_bytes(),
        max_size in 1usize..256
    ) {
        match validate_json_input(&bytes, max_size, MAX_JSON_DEPTH) {
            Ok(_) => {
                // If Ok, input was within size limit
                prop_assert!(bytes.len() <= max_size);
            }
            Err(cyberclaw_core::validation::ValidationError::JsonTooLarge { actual, max }) => {
                prop_assert!(actual > max);
                prop_assert_eq!(max, max_size);
            }
            Err(_) => {
                // Other errors (depth, parse) are also valid outcomes
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Property: execution_parser — Execution/ID validation
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(500))]

    /// Arbitrary bytes fed to Execution/ExecutionBudget deserialization must not panic.
    #[test]
    fn prop_execution_json_deserialization_never_panics(bytes in arb_bytes()) {
        let _ = serde_json::from_slice::<Execution>(&bytes);
        let _ = serde_json::from_slice::<ExecutionBudget>(&bytes);
        let _ = serde_json::from_slice::<ExecutionStatus>(&bytes);
        let _ = serde_json::from_slice::<ExecutionId>(&bytes);
        let _ = serde_json::from_slice::<NodeId>(&bytes);
    }

    /// from_string on arbitrary UTF-8 strings never panics.
    #[test]
    fn prop_id_from_string_never_panics(s in arb_string()) {
        let _ = ExecutionId::from_string(s.clone());
        let _ = NodeId::from_string(s.clone());
        let _ = TraceId::from_string(s);
    }

    /// If a string contains "..", from_string must return Err.
    #[test]
    fn prop_id_rejects_path_traversal(prefix in "[a-zA-Z0-9-]{0,10}", suffix in "[a-zA-Z0-9-]{0,10}") {
        let s = format!("{}..{}", prefix, suffix);
        prop_assert!(NodeId::from_string(s.clone()).is_err(),
            "NodeId must reject string containing '..': {:?}", s);
        prop_assert!(ExecutionId::from_string(s.clone()).is_err(),
            "ExecutionId must reject string containing '..': {:?}", s);
    }

    /// If a string contains a backslash, from_string must return Err.
    #[test]
    fn prop_id_rejects_backslash(prefix in "[a-zA-Z0-9-]{0,20}", suffix in "[a-zA-Z0-9-]{0,20}") {
        let s = format!("{}\\{}", prefix, suffix);
        prop_assert!(NodeId::from_string(s.clone()).is_err(),
            "NodeId must reject string containing backslash: {:?}", s);
    }

    /// If a string is empty, from_string must return Err.
    #[test]
    fn prop_id_rejects_empty(_unused in Just(0u8)) {
        prop_assert!(NodeId::from_string("".to_string()).is_err());
        prop_assert!(ExecutionId::from_string("".to_string()).is_err());
        prop_assert!(AgentId::from_string("".to_string()).is_err());
    }

    /// Strings longer than 128 chars must be rejected.
    #[test]
    fn prop_id_rejects_excessive_length(extra in 1usize..=200) {
        let s = "x".repeat(128 + extra);
        prop_assert!(NodeId::from_string(s.clone()).is_err(),
            "NodeId must reject strings > 128 chars (len={})", s.len());
    }

    /// Valid IDs (no control chars, no .., no backslash, 1–128 chars) must be accepted.
    #[test]
    fn prop_id_accepts_valid(
        s in "[a-zA-Z0-9/\\-_]{1,64}"
    ) {
        // Exclude strings with ".." which are path traversal
        if !s.contains("..") {
            let result = NodeId::from_string(s.clone());
            prop_assert!(result.is_ok(),
                "NodeId must accept valid ID '{}': {:?}", s, result.err());
        }
    }

    /// Serde deserialization of IDs enforces the same validation as from_string.
    #[test]
    fn prop_id_serde_enforces_validation(s in arb_string()) {
        let json = serde_json::to_string(&s).unwrap();
        let node_result: Result<NodeId, _> = serde_json::from_str(&json);
        let direct_result = NodeId::from_string(s.clone());
        // Both must agree: Ok iff Ok
        prop_assert_eq!(
            node_result.is_ok(), direct_result.is_ok(),
            "Serde and from_string must agree for '{}': serde={:?} direct={:?}",
            s, node_result.is_ok(), direct_result.is_ok()
        );
    }
}

// ---------------------------------------------------------------------------
// Property: prompt_injection_detector — scanners never panic, deterministic
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    /// PromptInjectionScanner.scan never panics on arbitrary text.
    #[test]
    fn prop_prompt_injection_scanner_never_panics(s in arb_string()) {
        let scanner = PromptInjectionScanner::new();
        let trace_id = TraceId::new();
        let _ = scanner.scan(&s, trace_id, None, None);
    }

    /// SecretScanner.scan never panics on arbitrary text.
    #[test]
    fn prop_secret_scanner_never_panics(s in arb_string()) {
        let scanner = SecretScanner::new();
        let trace_id = TraceId::new();
        let _ = scanner.scan(&s, trace_id, None);
    }

    /// SecretScanner.redact_all never panics and returns valid UTF-8.
    #[test]
    fn prop_secret_scanner_redact_all_is_valid_utf8(s in arb_string()) {
        let scanner = SecretScanner::new();
        let redacted = scanner.redact_all(&s);
        prop_assert!(
            std::str::from_utf8(redacted.as_bytes()).is_ok(),
            "redact_all output must be valid UTF-8"
        );
    }

    /// CommandSafetyScanner.scan never panics on arbitrary text.
    #[test]
    fn prop_command_safety_scanner_never_panics(s in arb_string()) {
        let scanner = CommandSafetyScanner::new();
        let trace_id = TraceId::new();
        let _ = scanner.scan(&s, trace_id, None, None);
    }

    /// All scanners are deterministic: same input → same number of events.
    #[test]
    fn prop_scanners_are_deterministic(s in arb_string()) {
        let trace_id = TraceId::new();

        let pi_scanner = PromptInjectionScanner::new();
        let events1 = pi_scanner.scan(&s, trace_id.clone(), None, None);
        let events2 = pi_scanner.scan(&s, trace_id.clone(), None, None);
        prop_assert_eq!(events1.len(), events2.len(),
            "PromptInjectionScanner must be deterministic");

        let secret_scanner = SecretScanner::new();
        let secret1 = secret_scanner.scan(&s, trace_id.clone(), None);
        let secret2 = secret_scanner.scan(&s, trace_id.clone(), None);
        prop_assert_eq!(secret1.len(), secret2.len(),
            "SecretScanner must be deterministic");
    }
}

// ---------------------------------------------------------------------------
// Property: workspace_path_validator — WorkspaceRef deserialization + ID invariants
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(500))]

    /// Arbitrary bytes fed to WorkspaceRef deserialization must not panic.
    #[test]
    fn prop_workspace_ref_deserialization_never_panics(bytes in arb_bytes()) {
        let _ = serde_json::from_slice::<WorkspaceRef>(&bytes);
    }

    /// WorkspaceId from_string rejects path traversal sequences.
    #[test]
    fn prop_workspace_id_rejects_traversal(
        prefix in "[a-zA-Z0-9-]{0,5}",
        suffix in "[a-zA-Z0-9-]{0,5}"
    ) {
        let s = format!("{}..{}", prefix, suffix);
        prop_assert!(WorkspaceId::from_string(s.clone()).is_err(),
            "WorkspaceId must reject '..': {:?}", s);
    }

    /// ConnectorId from_string rejects path traversal sequences.
    #[test]
    fn prop_connector_id_rejects_traversal(
        prefix in "[a-zA-Z0-9-]{0,5}",
        suffix in "[a-zA-Z0-9-]{0,5}"
    ) {
        let s = format!("{}..{}", prefix, suffix);
        prop_assert!(ConnectorId::from_string(s.clone()).is_err(),
            "ConnectorId must reject '..': {:?}", s);
    }

    /// AgentId rejects strings with control characters.
    #[test]
    fn prop_agent_id_rejects_control_chars(
        prefix in "[a-zA-Z0-9-]{1,10}",
        suffix in "[a-zA-Z0-9-]{1,10}"
    ) {
        // Insert various control characters
        for ctrl in &['\n', '\t', '\0', '\r', '\x1b'] {
            let s = format!("{}{}{}", prefix, ctrl, suffix);
            prop_assert!(AgentId::from_string(s.clone()).is_err(),
                "AgentId must reject control char {:?} in '{:?}'", ctrl, s);
        }
    }
}
