use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;
use uuid::Uuid;

/// Maximum length for ID strings (prevents DoS via excessive memory allocation)
const MAX_ID_LENGTH: usize = 128;

/// Minimum length for ID strings
const MIN_ID_LENGTH: usize = 1;

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
        pub struct $name(String); // Private field

        impl $name {
            /// Create a new ID with a generated UUID v4
            pub fn new() -> Self {
                Self(Uuid::new_v4().to_string())
            }

            /// Create an ID from a string with validation
            ///
            /// # Security
            /// This method validates the input string to prevent:
            /// - DoS via excessive length (max 128 chars)
            /// - Injection attacks via control characters
            /// - Path traversal via path separators
            /// - Empty IDs
            ///
            /// # Errors
            /// Returns an error if the string fails validation
            pub fn from_string(s: String) -> anyhow::Result<Self> {
                Self::validate_id_string(&s)?;
                Ok(Self(s))
            }

            /// Validate an ID string
            fn validate_id_string(s: &str) -> anyhow::Result<()> {
                // Check length bounds
                if s.is_empty() {
                    anyhow::bail!("ID string cannot be empty");
                }

                if s.len() < MIN_ID_LENGTH {
                    anyhow::bail!("ID string too short (min {} chars)", MIN_ID_LENGTH);
                }

                if s.len() > MAX_ID_LENGTH {
                    anyhow::bail!(
                        "ID string too long (max {} chars, got {})",
                        MAX_ID_LENGTH,
                        s.len()
                    );
                }

                // Check for control characters and null bytes
                if s.chars().any(|c| c.is_control() || c == '\0') {
                    anyhow::bail!("ID string cannot contain control characters");
                }

                // Check for Windows path separators (prevent path traversal on Windows)
                if s.contains('\\') {
                    anyhow::bail!("ID string cannot contain backslash (\\)");
                }

                // Check for parent directory references (prevent path traversal)
                if s.contains("..") {
                    anyhow::bail!("ID string cannot contain '..'");
                }

                // Note: Forward slash (/) is allowed for namespace-style IDs like "cyberclaw/master-agent"

                Ok(())
            }

            /// Get the underlying string (read-only access)
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Convert to String (consumes self)
            pub fn into_string(self) -> String {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        // Custom Deserialize implementation that enforces validation
        // SECURITY: This prevents Serde from bypassing from_string() validation
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let s = String::deserialize(deserializer)?;
                Self::from_string(s).map_err(serde::de::Error::custom)
            }
        }
    };
}

id_type!(TaskId);
id_type!(CaseId);
id_type!(WorkflowId);
id_type!(ExecutionId);
id_type!(ArtifactId);
id_type!(ReviewId);
id_type!(ApprovalStepId);
id_type!(AgentId);
id_type!(SkillId);
id_type!(ConnectorId);
id_type!(PlatformPluginId);
id_type!(CapabilityId);
id_type!(TenantId);
id_type!(ActorId);
id_type!(WorkspaceId);
id_type!(SessionId);
id_type!(TraceId);
id_type!(SecurityEventId);
id_type!(PolicyDecisionId);
id_type!(ProvenanceId);
id_type!(NodeId);
id_type!(LeaseId);
id_type!(AutopilotJobId);
id_type!(AutopilotRunId);
id_type!(MemCellId);
id_type!(FactId);
id_type!(VariantId);
id_type!(UserId);
id_type!(ClarifyId);
id_type!(HandoffId);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_id_new_generates_valid_uuid() {
        let id = ExecutionId::new();
        let id_str = id.as_str();

        // Should be a valid UUID v4 format (36 chars with hyphens)
        assert_eq!(id_str.len(), 36);
        assert!(id_str.chars().filter(|c| *c == '-').count() == 4);
    }

    #[test]
    fn test_id_from_string_valid() {
        let valid_ids = vec![
            "valid-id-123".to_string(),
            "node-worker-01".to_string(),
            "execution-abc-def".to_string(),
            "cyberclaw/master-agent".to_string(), // Namespace format
            "github/connector".to_string(),       // Namespace format
            "org/project/subproject".to_string(), // Multi-level namespace
            "a".to_string(),                      // Min length (1 char)
            "a".repeat(MAX_ID_LENGTH),            // Max length
        ];

        for id_str in valid_ids {
            let result = NodeId::from_string(id_str.clone());
            assert!(
                result.is_ok(),
                "Expected '{}' to be valid, got error: {:?}",
                id_str,
                result.err()
            );

            let id = result.unwrap();
            assert_eq!(id.as_str(), id_str);
        }
    }

    #[test]
    fn test_id_from_string_empty() {
        let result = ExecutionId::from_string("".to_string());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("cannot be empty"));
    }

    #[test]
    fn test_id_from_string_too_long() {
        let long_id = "x".repeat(MAX_ID_LENGTH + 1);
        let result = NodeId::from_string(long_id);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("too long"));
    }

    #[test]
    fn test_id_from_string_control_characters() {
        let invalid_ids = vec![
            "id-with-\nnewline",
            "id-with-\ttab",
            "id-with-\0null",
            "id-with-\rcarriage-return",
        ];

        for id_str in invalid_ids {
            let result = ExecutionId::from_string(id_str.to_string());
            assert!(result.is_err(), "Expected '{}' to be rejected", id_str);
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("control characters"),
                "Expected control character error for '{}'",
                id_str
            );
        }
    }

    #[test]
    fn test_id_from_string_backslash() {
        // Backslash is rejected (Windows path separator, can cause path traversal on Windows)
        let invalid_ids = vec![
            "id\\with\\backslashes".to_string(),
            "..\\windows\\path".to_string(),
            "path\\to\\file".to_string(),
        ];

        for id_str in invalid_ids {
            let result = NodeId::from_string(id_str.clone());
            assert!(result.is_err(), "Expected '{}' to be rejected", id_str);
            let err_msg = result.unwrap_err().to_string();
            assert!(
                err_msg.contains("backslash") || err_msg.contains("'..'"),
                "Expected backslash or '..' error for '{}', got: {}",
                id_str,
                err_msg
            );
        }
    }

    #[test]
    fn test_id_from_string_parent_directory() {
        let invalid_ids = vec!["id-with-...", "id..more", "..id", "id.."];

        for id_str in invalid_ids {
            let result = ExecutionId::from_string(id_str.to_string());
            assert!(result.is_err(), "Expected '{}' to be rejected", id_str);
            assert!(
                result.unwrap_err().to_string().contains("'..'"),
                "Expected '..' error for '{}'",
                id_str
            );
        }
    }

    #[test]
    fn test_id_as_str() {
        let id = NodeId::from_string("test-node-123".to_string()).unwrap();
        assert_eq!(id.as_str(), "test-node-123");
    }

    #[test]
    fn test_id_into_string() {
        let id = ExecutionId::from_string("test-exec-456".to_string()).unwrap();
        let s = id.into_string();
        assert_eq!(s, "test-exec-456");
    }

    #[test]
    fn test_id_display() {
        let id = LeaseId::from_string("test-lease-789".to_string()).unwrap();
        assert_eq!(format!("{}", id), "test-lease-789");
    }

    #[test]
    fn test_id_as_ref() {
        let id = NodeId::from_string("test-node".to_string()).unwrap();
        let s: &str = id.as_ref();
        assert_eq!(s, "test-node");
    }

    #[test]
    fn test_id_clone_and_equality() {
        let id1 = ExecutionId::from_string("same-id".to_string()).unwrap();
        let id2 = id1.clone();
        assert_eq!(id1, id2);
    }

    // Security Tests

    #[test]
    fn test_id_injection_attack_sql() {
        // Simulate SQL injection attempt (control chars would be caught)
        let injection_attempts = vec!["'; DROP TABLE users; --", "1' OR '1'='1", "admin'--"];

        for attempt in injection_attempts {
            let result = NodeId::from_string(attempt.to_string());
            // These should either be rejected or safely contained
            // Since they don't contain control chars or path separators, they might pass
            // but they're now safely encapsulated as strings and won't be interpreted
            if let Ok(id) = result {
                // Verify the string is exactly what was provided (no interpretation)
                assert_eq!(id.as_str(), attempt);
            }
        }
    }

    #[test]
    fn test_id_dos_attack_extreme_length() {
        // Attempt to create extremely long ID (should be rejected)
        let huge_id = "x".repeat(10_000);
        let result = ExecutionId::from_string(huge_id);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("too long"));
    }

    #[test]
    fn test_id_path_traversal_attacks() {
        let attacks = vec![
            "../../../etc/passwd",
            "..\\..\\..\\windows\\system32",
            "valid-id/../../../attack",
            "../../attack/./valid-id",
        ];

        for attack in attacks {
            let result = NodeId::from_string(attack.to_string());
            assert!(
                result.is_err(),
                "Path traversal attack '{}' should be rejected",
                attack
            );
        }
    }

    #[test]
    fn test_id_unicode_safety() {
        // Test various Unicode characters
        let unicode_ids = vec!["id-with-émojis-🔥", "中文-id", "Русский-id", "العربية-id"];

        for id_str in unicode_ids {
            let result = ExecutionId::from_string(id_str.to_string());
            // Unicode is allowed as long as no control chars or path separators
            if let Ok(id) = result {
                assert_eq!(id.as_str(), id_str);
            }
        }
    }

    #[test]
    fn test_id_null_byte_injection() {
        // Null byte injection attempt
        let null_byte_id = "valid-id\0malicious-suffix";
        let result = NodeId::from_string(null_byte_id.to_string());
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("control characters"));
    }

    // Security Tests - Serde Deserialization Validation

    #[test]
    fn test_deserialize_enforces_validation() {
        // CRITICAL-1 FIX: Verify that Serde deserialization enforces from_string() validation
        let invalid_json = r#""../../../etc/passwd""#;
        let result: Result<ExecutionId, _> = serde_json::from_str(invalid_json);
        assert!(
            result.is_err(),
            "Serde deserialization must reject path traversal"
        );
    }

    #[test]
    fn test_deserialize_rejects_control_chars() {
        // Verify control character rejection via Serde
        let invalid_json = r#""id-with-\nnewline""#;
        let result: Result<NodeId, _> = serde_json::from_str(invalid_json);
        assert!(
            result.is_err(),
            "Serde deserialization must reject control characters"
        );
    }

    #[test]
    fn test_deserialize_rejects_empty_string() {
        // Verify empty string rejection via Serde
        let invalid_json = r#""""#;
        let result: Result<ExecutionId, _> = serde_json::from_str(invalid_json);
        assert!(
            result.is_err(),
            "Serde deserialization must reject empty strings"
        );
    }

    #[test]
    fn test_deserialize_rejects_excessive_length() {
        // Verify length limit enforcement via Serde
        let long_id = "x".repeat(MAX_ID_LENGTH + 1);
        let invalid_json = format!(r#""{}""#, long_id);
        let result: Result<NodeId, _> = serde_json::from_str(&invalid_json);
        assert!(
            result.is_err(),
            "Serde deserialization must enforce length limits"
        );
    }

    #[test]
    fn test_deserialize_accepts_valid_id() {
        // Verify valid IDs pass through Serde
        let valid_json = r#""valid-test-id-123""#;
        let result: Result<ExecutionId, _> = serde_json::from_str(valid_json);
        assert!(
            result.is_ok(),
            "Serde deserialization must accept valid IDs"
        );
        assert_eq!(result.unwrap().as_str(), "valid-test-id-123");
    }

    #[test]
    fn test_serialize_deserialize_roundtrip() {
        // Verify roundtrip serialization/deserialization
        let original = NodeId::from_string("test-roundtrip-id".to_string()).unwrap();
        let json = serde_json::to_string(&original).unwrap();
        let deserialized: NodeId = serde_json::from_str(&json).unwrap();
        assert_eq!(original, deserialized);
    }

    #[test]
    fn test_memcell_id_creation() {
        let id = MemCellId::new();
        assert_eq!(id.as_str().len(), 36); // UUID v4
    }

    #[test]
    fn test_fact_id_creation() {
        let id = FactId::new();
        assert_eq!(id.as_str().len(), 36);
    }

    #[test]
    fn test_memcell_id_from_string() {
        let id = MemCellId::from_string("memcell-test-001".to_string());
        assert!(id.is_ok());
        assert_eq!(id.unwrap().as_str(), "memcell-test-001");
    }

    #[test]
    fn test_fact_id_from_string() {
        let id = FactId::from_string("fact-test-001".to_string());
        assert!(id.is_ok());
        assert_eq!(id.unwrap().as_str(), "fact-test-001");
    }
}
