//! Input validation framework for capability parameters, agent input, and connector configuration.
//!
//! Provides defense-in-depth checks for null bytes, control characters, path traversal,
//! SQL injection patterns, and other suspicious content before it reaches execution.

use std::collections::HashSet;

/// Result of validating input.
#[derive(Debug, Clone)]
pub struct ValidationResult {
    /// Whether the input passed all checks.
    pub is_valid: bool,
    /// Validation errors (blocking).
    pub errors: Vec<ValidationError>,
    /// Warnings that do not block processing.
    pub warnings: Vec<String>,
}

impl ValidationResult {
    /// Create a successful validation result.
    pub fn ok() -> Self {
        Self {
            is_valid: true,
            errors: vec![],
            warnings: vec![],
        }
    }

    /// Create a validation result with a single error.
    pub fn error(error: ValidationError) -> Self {
        Self {
            is_valid: false,
            errors: vec![error],
            warnings: vec![],
        }
    }

    /// Add a warning to the result.
    pub fn with_warning(mut self, warning: impl Into<String>) -> Self {
        self.warnings.push(warning.into());
        self
    }

    /// Merge another validation result into this one.
    pub fn merge(mut self, other: Self) -> Self {
        self.is_valid = self.is_valid && other.is_valid;
        self.errors.extend(other.errors);
        self.warnings.extend(other.warnings);
        self
    }
}

impl Default for ValidationResult {
    fn default() -> Self {
        Self::ok()
    }
}

/// A single validation error.
#[derive(Debug, Clone)]
pub struct ValidationError {
    /// Field or aspect that failed validation.
    pub field: String,
    /// Human-readable error message.
    pub message: String,
    /// Machine-readable error code.
    pub code: ValidationErrorCode,
}

/// Error codes for programmatic handling of validation failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ValidationErrorCode {
    /// Input is empty when a value is required.
    Empty,
    /// Input exceeds maximum allowed length.
    TooLong,
    /// Input is shorter than minimum required length.
    TooShort,
    /// Input does not match the expected format.
    InvalidFormat,
    /// Input contains content that is explicitly forbidden.
    ForbiddenContent,
    /// Input contains invalid encoding (e.g. null bytes).
    InvalidEncoding,
    /// Input matches a known attack or injection pattern.
    SuspiciousPattern,
}

/// Default maximum input length in bytes.
const DEFAULT_MAX_LENGTH: usize = 100_000;

/// Depth limit for recursive JSON traversal to prevent stack overflow.
const MAX_JSON_DEPTH: usize = 32;

/// SQL injection patterns to detect (case-insensitive).
const SQL_INJECTION_PATTERNS: &[&str] = &[
    "' or '1'='1",
    "'; drop table",
    "'; delete from",
    "union select",
    "' or 1=1--",
    "'; exec ",
    "' or ''='",
];

/// Path traversal patterns to detect.
const PATH_TRAVERSAL_PATTERNS: &[&str] = &[
    // Canonical forms
    "../",
    "..\\",
    // Single URL-encoded
    "%2e%2e%2f",
    "%2e%2e/",
    "..%2f",
    "%2e%2e%5c",
    "..%5c",
    // Double URL-encoded
    "%252e%252e%252f",
    "%252e%252e/",
    "%252e%252e%255c",
    // Fullwidth Unicode variants
    "\u{ff0e}\u{ff0e}\u{ff0f}",
    "\u{ff0e}\u{ff0e}\u{ff3c}",
];

/// Input validator that checks capability parameters, agent input, and connector
/// configuration for dangerous or malformed content.
pub struct InputValidator {
    max_length: usize,
    extra_forbidden: HashSet<String>,
}

impl InputValidator {
    /// Create a new validator with default settings.
    pub fn new() -> Self {
        Self {
            max_length: DEFAULT_MAX_LENGTH,
            extra_forbidden: HashSet::new(),
        }
    }

    /// Override the maximum input length in bytes.
    pub fn with_max_length(mut self, max: usize) -> Self {
        self.max_length = max;
        self
    }

    /// Add a custom forbidden pattern (matched case-insensitively).
    pub fn forbid_pattern(mut self, pattern: impl Into<String>) -> Self {
        self.extra_forbidden.insert(pattern.into().to_lowercase());
        self
    }

    /// Validate capability execution parameters.
    ///
    /// Recursively inspects all string values in the JSON tree (up to [`MAX_JSON_DEPTH`]).
    pub fn validate_capability_params(&self, params: &serde_json::Value) -> ValidationResult {
        let mut result = ValidationResult::ok();
        Self::check_json_strings(params, "", self, &mut result, 0);
        result
    }

    /// Validate free-form agent input text.
    pub fn validate_agent_input(&self, input: &str) -> ValidationResult {
        if input.is_empty() {
            return ValidationResult::error(ValidationError {
                field: "input".to_string(),
                message: "Agent input cannot be empty".to_string(),
                code: ValidationErrorCode::Empty,
            });
        }
        self.check_string(input, "input")
    }

    /// Validate connector configuration values.
    ///
    /// Recursively inspects all string values in the JSON tree (up to [`MAX_JSON_DEPTH`]).
    pub fn validate_connector_config(&self, config: &serde_json::Value) -> ValidationResult {
        let mut result = ValidationResult::ok();
        Self::check_json_strings(config, "", self, &mut result, 0);
        result
    }

    // ── Internal helpers ────────────────────────────────────────────

    /// Run all string-level checks on a single value.
    fn check_string(&self, value: &str, field: &str) -> ValidationResult {
        let mut result = ValidationResult::ok();

        // Length check
        if value.len() > self.max_length {
            result = result.merge(ValidationResult::error(ValidationError {
                field: field.to_string(),
                message: format!(
                    "Value too long: {} bytes (max {})",
                    value.len(),
                    self.max_length
                ),
                code: ValidationErrorCode::TooLong,
            }));
        }

        // Null bytes
        if value.contains('\x00') {
            result = result.merge(ValidationResult::error(ValidationError {
                field: field.to_string(),
                message: "Value contains null bytes".to_string(),
                code: ValidationErrorCode::InvalidEncoding,
            }));
        }

        // Control characters (U+0001..U+001F excluding tab, newline, carriage return)
        if value
            .chars()
            .any(|c| c.is_control() && c != '\t' && c != '\n' && c != '\r' && c != '\x00')
        {
            result = result.merge(ValidationResult::error(ValidationError {
                field: field.to_string(),
                message: "Value contains control characters".to_string(),
                code: ValidationErrorCode::InvalidEncoding,
            }));
        }

        // Path traversal
        let lower = value.to_lowercase();
        for pat in PATH_TRAVERSAL_PATTERNS {
            if lower.contains(pat) {
                result = result.merge(ValidationResult::error(ValidationError {
                    field: field.to_string(),
                    message: format!("Potential path traversal detected: {}", pat),
                    code: ValidationErrorCode::SuspiciousPattern,
                }));
                break;
            }
        }

        // SQL injection
        for pat in SQL_INJECTION_PATTERNS {
            if lower.contains(pat) {
                result = result.merge(ValidationResult::error(ValidationError {
                    field: field.to_string(),
                    message: "Potential SQL injection pattern detected".to_string(),
                    code: ValidationErrorCode::SuspiciousPattern,
                }));
                break;
            }
        }

        // Custom forbidden patterns
        for pat in &self.extra_forbidden {
            if lower.contains(pat.as_str()) {
                result = result.merge(ValidationResult::error(ValidationError {
                    field: field.to_string(),
                    message: format!("Input contains forbidden pattern: {}", pat),
                    code: ValidationErrorCode::ForbiddenContent,
                }));
            }
        }

        result
    }

    /// Recursively walk a JSON value and validate all embedded strings.
    fn check_json_strings(
        value: &serde_json::Value,
        path: &str,
        validator: &InputValidator,
        result: &mut ValidationResult,
        depth: usize,
    ) {
        if depth > MAX_JSON_DEPTH {
            result.warnings.push(format!(
                "JSON nesting exceeds depth limit ({}); deeper values were not validated",
                MAX_JSON_DEPTH
            ));
            return;
        }
        match value {
            serde_json::Value::String(s) if !s.is_empty() => {
                let string_result = validator.check_string(s, path);
                *result = std::mem::take(result).merge(string_result);
            }
            serde_json::Value::String(_) => {}
            serde_json::Value::Array(arr) => {
                for (i, item) in arr.iter().enumerate() {
                    let child_path = format!("{path}[{i}]");
                    Self::check_json_strings(item, &child_path, validator, result, depth + 1);
                }
            }
            serde_json::Value::Object(obj) => {
                for (k, v) in obj {
                    let child_path = if path.is_empty() {
                        k.clone()
                    } else {
                        format!("{path}.{k}")
                    };
                    Self::check_json_strings(v, &child_path, validator, result, depth + 1);
                }
            }
            _ => {}
        }
    }
}

impl Default for InputValidator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── validate_agent_input ────────────────────────────────────────

    #[test]
    fn valid_agent_input_passes() {
        let v = InputValidator::new();
        let r = v.validate_agent_input("Hello, please read the config file.");
        assert!(r.is_valid);
        assert!(r.errors.is_empty());
    }

    #[test]
    fn empty_agent_input_rejected() {
        let v = InputValidator::new();
        let r = v.validate_agent_input("");
        assert!(!r.is_valid);
        assert!(r
            .errors
            .iter()
            .any(|e| e.code == ValidationErrorCode::Empty));
    }

    #[test]
    fn null_bytes_rejected() {
        let v = InputValidator::new();
        let r = v.validate_agent_input("hello\x00world");
        assert!(!r.is_valid);
        assert!(r
            .errors
            .iter()
            .any(|e| e.code == ValidationErrorCode::InvalidEncoding));
    }

    #[test]
    fn control_characters_rejected() {
        let v = InputValidator::new();
        // BEL character (0x07) is a control char
        let r = v.validate_agent_input("hello\x07world");
        assert!(!r.is_valid);
        assert!(r
            .errors
            .iter()
            .any(|e| e.code == ValidationErrorCode::InvalidEncoding));
    }

    #[test]
    fn path_traversal_rejected() {
        let v = InputValidator::new();
        let r = v.validate_agent_input("read file at ../../etc/passwd");
        assert!(!r.is_valid);
        assert!(r
            .errors
            .iter()
            .any(|e| e.code == ValidationErrorCode::SuspiciousPattern));
    }

    #[test]
    fn sql_injection_rejected() {
        let v = InputValidator::new();
        let r = v.validate_agent_input("query: ' OR '1'='1");
        assert!(!r.is_valid);
        assert!(r
            .errors
            .iter()
            .any(|e| e.code == ValidationErrorCode::SuspiciousPattern));
    }

    #[test]
    fn too_long_input_rejected() {
        let v = InputValidator::new().with_max_length(20);
        let r = v.validate_agent_input("this input is definitely longer than twenty bytes");
        assert!(!r.is_valid);
        assert!(r
            .errors
            .iter()
            .any(|e| e.code == ValidationErrorCode::TooLong));
    }

    #[test]
    fn custom_forbidden_pattern_rejected() {
        let v = InputValidator::new().forbid_pattern("evil");
        let r = v.validate_agent_input("do something EVIL please");
        assert!(!r.is_valid);
        assert!(r
            .errors
            .iter()
            .any(|e| e.code == ValidationErrorCode::ForbiddenContent));
    }

    // ── validate_capability_params ──────────────────────────────────

    #[test]
    fn capability_params_clean_json_passes() {
        let v = InputValidator::new();
        let r = v.validate_capability_params(&serde_json::json!({
            "path": "/tmp/config.yaml",
            "mode": "read"
        }));
        assert!(r.is_valid);
    }

    #[test]
    fn capability_params_nested_null_byte_rejected() {
        let v = InputValidator::new();
        let r = v.validate_capability_params(&serde_json::json!({
            "meta": { "tag": "bad\x00value" }
        }));
        assert!(!r.is_valid);
        assert!(r
            .errors
            .iter()
            .any(|e| e.code == ValidationErrorCode::InvalidEncoding));
    }

    #[test]
    fn capability_params_path_traversal_in_array_rejected() {
        let v = InputValidator::new();
        let r = v.validate_capability_params(&serde_json::json!({
            "files": ["ok.txt", "../../../etc/shadow"]
        }));
        assert!(!r.is_valid);
        let err = r
            .errors
            .iter()
            .find(|e| e.code == ValidationErrorCode::SuspiciousPattern)
            .expect("expected suspicious pattern error");
        assert_eq!(err.field, "files[1]");
    }

    #[test]
    fn capability_params_depth_limit_prevents_stack_overflow() {
        let v = InputValidator::new().forbid_pattern("evil");
        let mut value = serde_json::json!("evil payload");
        for _ in 0..50 {
            value = serde_json::json!({ "nested": value });
        }
        let r = v.validate_capability_params(&value);
        // The payload is beyond MAX_JSON_DEPTH so it is silently skipped.
        assert!(
            r.is_valid,
            "strings beyond depth limit should be skipped, got errors: {:?}",
            r.errors
        );
    }

    // ── validate_connector_config ───────────────────────────────────

    #[test]
    fn connector_config_sql_injection_rejected() {
        let v = InputValidator::new();
        let r = v.validate_connector_config(&serde_json::json!({
            "query": "'; DROP TABLE users;--"
        }));
        assert!(!r.is_valid);
        assert!(r
            .errors
            .iter()
            .any(|e| e.code == ValidationErrorCode::SuspiciousPattern));
    }

    // ── ValidationResult helpers ────────────────────────────────────

    #[test]
    fn merge_combines_errors_and_warnings() {
        let a = ValidationResult::ok().with_warning("warn-a");
        let b = ValidationResult::error(ValidationError {
            field: "x".to_string(),
            message: "bad".to_string(),
            code: ValidationErrorCode::Empty,
        });
        let merged = a.merge(b);
        assert!(!merged.is_valid);
        assert_eq!(merged.errors.len(), 1);
        assert_eq!(merged.warnings.len(), 1);
    }

    #[test]
    fn tabs_and_newlines_are_allowed() {
        let v = InputValidator::new();
        let r = v.validate_agent_input("line1\nline2\ttab");
        assert!(r.is_valid);
    }
}
