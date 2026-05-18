//! Configuration validation trait and utilities.
//!
//! Provides the [`Validate`] trait for types that can validate themselves,
//! and a [`ValidationError`] type for reporting validation failures.

use thiserror::Error;

/// Maximum JSON nesting depth to prevent stack overflow attacks
pub const MAX_JSON_DEPTH: usize = 32;

/// Maximum JSON size in bytes to prevent memory exhaustion (10MB)
pub const MAX_JSON_SIZE_BYTES: usize = 10 * 1024 * 1024;

/// Error returned when validation fails.
#[derive(Debug, Error)]
pub enum ValidationError {
    /// Generic validation failure
    #[error("validation failed: {0}")]
    Failed(String),

    /// JSON payload exceeds maximum size limit
    #[error("JSON too large: {actual} bytes (max: {max} bytes)")]
    JsonTooLarge { actual: usize, max: usize },

    /// JSON nesting exceeds maximum depth limit
    #[error("JSON nesting too deep: {actual} levels (max: {max} levels)")]
    JsonTooDeep { actual: usize, max: usize },

    /// JSON parsing failed
    #[error("JSON parsing failed: {0}")]
    JsonParseError(String),
}

impl ValidationError {
    /// Create a new validation error with the given message.
    pub fn new(msg: impl Into<String>) -> Self {
        Self::Failed(msg.into())
    }
}

/// Result type for validation operations.
pub type ValidationResult = Result<(), ValidationError>;

/// Trait for types that can validate their own internal state.
///
/// Implementors should check that all field values are within acceptable
/// ranges and that any inter-field constraints are satisfied.
///
/// # Example
///
/// ```rust
/// use cyberclaw_core::validation::{Validate, ValidationResult, ValidationError};
///
/// struct Config {
///     port: u16,
/// }
///
/// impl Validate for Config {
///     fn validate(&self) -> ValidationResult {
///         if self.port < 1024 {
///             return Err(ValidationError::new(format!(
///                 "port must be >= 1024, got {}",
///                 self.port
///             )));
///         }
///         Ok(())
///     }
/// }
/// ```
pub trait Validate {
    /// Validate this value, returning an error if any constraint is violated.
    fn validate(&self) -> ValidationResult;
}

/// Validates JSON input for size and depth limits before deserialization.
///
/// # Security
///
/// This function prevents:
/// - Memory exhaustion attacks via large JSON payloads (>10MB)
/// - Stack overflow attacks via deeply nested JSON structures (>32 levels)
///
/// # Arguments
///
/// * `json_bytes` - The raw JSON bytes to validate
/// * `max_size` - Maximum allowed size in bytes (default: MAX_JSON_SIZE_BYTES)
/// * `max_depth` - Maximum allowed nesting depth (default: MAX_JSON_DEPTH)
///
/// # Returns
///
/// * `Ok(serde_json::Value)` - Parsed JSON value if validation passes
/// * `Err(ValidationError)` - Validation error with details
///
/// # Examples
///
/// ```rust
/// use cyberclaw_core::validation::{validate_json_input, MAX_JSON_SIZE_BYTES, MAX_JSON_DEPTH};
///
/// let json = br#"{"key": "value"}"#;
/// let value = validate_json_input(json, MAX_JSON_SIZE_BYTES, MAX_JSON_DEPTH).unwrap();
/// assert_eq!(value["key"], "value");
/// ```
pub fn validate_json_input(
    json_bytes: &[u8],
    max_size: usize,
    max_depth: usize,
) -> Result<serde_json::Value, ValidationError> {
    // Check size limit first (fast check before parsing)
    if json_bytes.len() > max_size {
        return Err(ValidationError::JsonTooLarge {
            actual: json_bytes.len(),
            max: max_size,
        });
    }

    // Parse JSON and check depth
    let value: serde_json::Value = serde_json::from_slice(json_bytes)
        .map_err(|e| ValidationError::JsonParseError(e.to_string()))?;

    // Validate depth
    let actual_depth = calculate_json_depth(&value);
    if actual_depth > max_depth {
        return Err(ValidationError::JsonTooDeep {
            actual: actual_depth,
            max: max_depth,
        });
    }

    Ok(value)
}

/// Calculates the maximum nesting depth of a JSON value.
///
/// # Depth Calculation
///
/// - Primitive values (number, string, bool, null) have depth 0
/// - Empty objects `{}` and empty arrays `[]` have depth 1
/// - `{"a": 1}` has depth 1 (one level of nesting)
/// - `{"a": {"b": 1}}` has depth 2 (two levels of nesting)
/// - `[[[1]]]` has depth 3 (three levels of nesting)
fn calculate_json_depth(value: &serde_json::Value) -> usize {
    match value {
        serde_json::Value::Object(map) => {
            if map.is_empty() {
                1
            } else {
                1 + map.values().map(calculate_json_depth).max().unwrap_or(0)
            }
        }
        serde_json::Value::Array(arr) => {
            if arr.is_empty() {
                1
            } else {
                1 + arr.iter().map(calculate_json_depth).max().unwrap_or(0)
            }
        }
        // Primitive values have depth 0 (not counted as nesting)
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json_depth_calculation() {
        // Primitive values have depth 0
        let val = serde_json::json!(42);
        assert_eq!(calculate_json_depth(&val), 0);

        let val = serde_json::json!("hello");
        assert_eq!(calculate_json_depth(&val), 0);

        let val = serde_json::json!(true);
        assert_eq!(calculate_json_depth(&val), 0);

        let val = serde_json::json!(null);
        assert_eq!(calculate_json_depth(&val), 0);

        // Simple object (depth 1)
        let val = serde_json::json!({"key": "value"});
        assert_eq!(calculate_json_depth(&val), 1);

        // Simple array (depth 1)
        let val = serde_json::json!([1, 2, 3]);
        assert_eq!(calculate_json_depth(&val), 1);

        // Nested object (depth 2)
        let val = serde_json::json!({"a": {"b": 1}});
        assert_eq!(calculate_json_depth(&val), 2);

        // Nested array (depth 3)
        let val = serde_json::json!([[[1]]]);
        assert_eq!(calculate_json_depth(&val), 3);

        // Mixed nesting (depth 4)
        let val = serde_json::json!({"a": [{"b": [1]}]});
        assert_eq!(calculate_json_depth(&val), 4);

        // Empty containers (depth 1)
        let val = serde_json::json!({});
        assert_eq!(calculate_json_depth(&val), 1);

        let val = serde_json::json!([]);
        assert_eq!(calculate_json_depth(&val), 1);
    }

    #[test]
    fn test_validate_json_within_limits() {
        let json = br#"{"key": "value", "nested": {"level": 2}}"#;
        let result = validate_json_input(json, MAX_JSON_SIZE_BYTES, MAX_JSON_DEPTH);
        assert!(result.is_ok(), "valid JSON within limits should pass");

        let value = result.unwrap();
        assert_eq!(value["key"], "value");
        assert_eq!(value["nested"]["level"], 2);
    }

    #[test]
    fn test_validate_json_size_limit_exceeded() {
        let json = br#"{"key": "value"}"#;
        let result = validate_json_input(json, 10, MAX_JSON_DEPTH); // 10 bytes max

        assert!(result.is_err(), "oversized JSON should be rejected");
        match result.unwrap_err() {
            ValidationError::JsonTooLarge { actual, max } => {
                assert_eq!(actual, json.len());
                assert_eq!(max, 10);
            }
            e => panic!("expected JsonTooLarge error, got: {:?}", e),
        }
    }

    #[test]
    fn test_validate_json_depth_limit_exceeded() {
        // Create deeply nested JSON: {"a": {"a": {"a": ...}}} (35 levels)
        let mut json = String::from(r#"{"a":"#);
        for _ in 0..34 {
            json.push_str(r#"{"a":"#);
        }
        json.push('1');
        for _ in 0..35 {
            json.push('}');
        }

        let result = validate_json_input(json.as_bytes(), MAX_JSON_SIZE_BYTES, MAX_JSON_DEPTH);

        assert!(result.is_err(), "deeply nested JSON should be rejected");
        match result.unwrap_err() {
            ValidationError::JsonTooDeep { actual, max } => {
                assert!(
                    actual > MAX_JSON_DEPTH,
                    "actual depth {} should exceed max {}",
                    actual,
                    max
                );
                assert_eq!(max, MAX_JSON_DEPTH);
            }
            e => panic!("expected JsonTooDeep error, got: {:?}", e),
        }
    }

    #[test]
    fn test_validate_json_malicious_100_levels() {
        // Create malicious deeply nested JSON (100+ levels)
        let mut json = String::new();
        for _ in 0..100 {
            json.push_str(r#"{"a":"#);
        }
        json.push('1');
        for _ in 0..100 {
            json.push('}');
        }

        let result = validate_json_input(json.as_bytes(), MAX_JSON_SIZE_BYTES, MAX_JSON_DEPTH);

        assert!(result.is_err(), "100-level nested JSON should be rejected");
        match result.unwrap_err() {
            ValidationError::JsonTooDeep { actual, max } => {
                assert!(
                    actual >= 100,
                    "actual depth should be >= 100, got {}",
                    actual
                );
                assert_eq!(max, MAX_JSON_DEPTH);
            }
            e => panic!("expected JsonTooDeep error, got: {:?}", e),
        }
    }

    #[test]
    fn test_validate_json_malicious_large_size() {
        // Create JSON that exceeds 10MB limit
        // Generate a large JSON string by repeating a pattern
        // 10MB = 10,485,760 bytes, we want > 11MB = 11,534,336 bytes
        let mut large_json = String::from("[");

        // Create strings with consistent length to reach 11MB+
        // Each iteration adds ~20 bytes: "1234567890123456",
        // Need ~600,000 iterations for 12MB
        for i in 0..600_000 {
            if i > 0 {
                large_json.push(',');
            }
            // Add 16-character string each time (plus quotes and comma = ~19 bytes per iteration)
            large_json.push_str(&format!("\"{:016}\"", i));
        }
        large_json.push(']');

        let actual_size = large_json.len();
        assert!(
            actual_size > MAX_JSON_SIZE_BYTES,
            "test JSON should be > 10MB, got {} bytes",
            actual_size
        );

        let result =
            validate_json_input(large_json.as_bytes(), MAX_JSON_SIZE_BYTES, MAX_JSON_DEPTH);

        assert!(
            result.is_err(),
            "{}MB JSON should be rejected",
            actual_size / 1024 / 1024
        );
        match result.unwrap_err() {
            ValidationError::JsonTooLarge { actual, max } => {
                assert!(
                    actual > MAX_JSON_SIZE_BYTES,
                    "actual size {} should exceed max {}",
                    actual,
                    max
                );
                assert_eq!(max, MAX_JSON_SIZE_BYTES);
            }
            e => panic!("expected JsonTooLarge error, got: {:?}", e),
        }
    }

    #[test]
    fn test_validate_json_invalid_syntax() {
        let json = br#"{"key": invalid}"#; // Invalid JSON
        let result = validate_json_input(json, MAX_JSON_SIZE_BYTES, MAX_JSON_DEPTH);

        assert!(result.is_err(), "invalid JSON should be rejected");
        match result.unwrap_err() {
            ValidationError::JsonParseError(_) => {
                // Expected
            }
            e => panic!("expected JsonParseError, got: {:?}", e),
        }
    }

    #[test]
    fn test_validate_json_at_depth_limit() {
        // Create JSON at exactly the depth limit (32 levels)
        let mut json = String::new();
        for _ in 0..MAX_JSON_DEPTH {
            json.push_str(r#"{"a":"#);
        }
        json.push('1');
        for _ in 0..MAX_JSON_DEPTH {
            json.push('}');
        }

        let result = validate_json_input(json.as_bytes(), MAX_JSON_SIZE_BYTES, MAX_JSON_DEPTH);

        // Should be accepted at the limit
        assert!(
            result.is_ok(),
            "JSON at depth limit should be accepted: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_validate_json_empty_containers() {
        // Empty object
        let json = br#"{}"#;
        let result = validate_json_input(json, MAX_JSON_SIZE_BYTES, MAX_JSON_DEPTH);
        assert!(result.is_ok());
        let value = result.unwrap();
        assert_eq!(calculate_json_depth(&value), 1);

        // Empty array
        let json = br#"[]"#;
        let result = validate_json_input(json, MAX_JSON_SIZE_BYTES, MAX_JSON_DEPTH);
        assert!(result.is_ok());
        let value = result.unwrap();
        assert_eq!(calculate_json_depth(&value), 1);
    }

    #[test]
    fn test_validate_json_complex_realistic() {
        // Realistic complex JSON with moderate nesting
        let json = br#"
        {
            "task": {
                "id": "task-123",
                "input": {
                    "payload": {
                        "data": [1, 2, 3],
                        "config": {
                            "nested": {
                                "value": "test"
                            }
                        }
                    }
                }
            }
        }
        "#;
        let result = validate_json_input(json, MAX_JSON_SIZE_BYTES, MAX_JSON_DEPTH);
        assert!(result.is_ok(), "realistic complex JSON should be accepted");

        let value = result.unwrap();
        assert_eq!(value["task"]["id"], "task-123");
    }
}
