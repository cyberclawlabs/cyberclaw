//! Connector error classification for retry decisions.
//!
//! Classifies connector execution errors as transient (safe to retry) or
//! non-transient (should not retry), enabling the orchestrator to make
//! informed retry decisions instead of blindly retrying or giving up.

/// Connector error type classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectorErrorKind {
    /// Transient error -- safe to retry (network timeout, service temporarily
    /// unavailable, rate limiting).
    Transient,
    /// Non-transient error -- should not retry (auth failure, permission denied,
    /// invalid parameters, resource not found).
    NonTransient,
    /// Unknown -- conservatively treated as non-transient.
    Unknown,
}

/// Connector error classifier trait.
///
/// Each connector type implements its own classification logic based on the
/// error patterns it produces.
pub trait ConnectorErrorClassifier {
    /// Classify an error into transient, non-transient, or unknown.
    fn classify_error(&self, error: &anyhow::Error) -> ConnectorErrorKind;

    /// Whether the error should be retried (convenience method).
    fn should_retry(&self, error: &anyhow::Error) -> bool {
        matches!(self.classify_error(error), ConnectorErrorKind::Transient)
    }
}

/// Error classifier for `LocalConnector`.
///
/// Classification rules:
/// - IO `TimedOut` / `ConnectionRefused` / `WouldBlock` / `BrokenPipe` -> Transient
/// - IO `NotFound` / `PermissionDenied` / `InvalidInput` / `InvalidData` -> NonTransient
/// - Path traversal or workspace boundary errors -> NonTransient
/// - Unknown capability errors -> NonTransient
/// - Other errors -> Unknown
#[derive(Debug, Default)]
pub struct LocalErrorClassifier;

impl ConnectorErrorClassifier for LocalErrorClassifier {
    fn classify_error(&self, error: &anyhow::Error) -> ConnectorErrorKind {
        let msg = error.to_string();
        let msg_lower = msg.to_lowercase();

        // Check for IO errors by downcasting first
        if let Some(io_err) = error.downcast_ref::<std::io::Error>() {
            return classify_io_error(io_err);
        }

        // Fall back to string-based classification for anyhow-wrapped errors
        if msg_lower.contains("timed out")
            || msg_lower.contains("connection refused")
            || msg_lower.contains("broken pipe")
            || msg_lower.contains("would block")
        {
            return ConnectorErrorKind::Transient;
        }

        if msg_lower.contains("not found")
            || msg_lower.contains("permission denied")
            || msg_lower.contains("invalid input")
            || msg_lower.contains("invalid data")
            || msg_lower.contains("path contains '..'")
            || msg_lower.contains("outside workspace")
            || msg_lower.contains("unknown capability")
        {
            return ConnectorErrorKind::NonTransient;
        }

        ConnectorErrorKind::Unknown
    }
}

/// Error classifier for `HttpConnector`.
///
/// Classification rules:
/// - HTTP 429 (rate limit) / 500 / 502 / 503 / 504 -> Transient
/// - HTTP 400 / 401 / 403 / 404 / 405 / 409 / 422 -> NonTransient
/// - Network-level errors (timeout, connection refused) -> Transient
/// - Unsupported HTTP method -> NonTransient
/// - Other errors -> Unknown
#[derive(Debug, Default)]
pub struct HttpErrorClassifier;

impl ConnectorErrorClassifier for HttpErrorClassifier {
    fn classify_error(&self, error: &anyhow::Error) -> ConnectorErrorKind {
        let msg = error.to_string();
        let msg_lower = msg.to_lowercase();

        // HTTP status code based classification
        if msg_lower.contains("http error status:") || msg_lower.contains("returned status") {
            // Extract status code
            if let Some(code) = extract_http_status(&msg) {
                return classify_http_status(code);
            }
        }

        // Network-level errors (reqwest failures before getting a response)
        if msg_lower.contains("timed out")
            || msg_lower.contains("timeout")
            || msg_lower.contains("connection refused")
            || msg_lower.contains("connection reset")
            || msg_lower.contains("broken pipe")
            || msg_lower.contains("dns error")
            || msg_lower.contains("connect error")
        {
            return ConnectorErrorKind::Transient;
        }

        // Invalid configuration / request errors
        if msg_lower.contains("unsupported http method")
            || msg_lower.contains("unknown capability")
            || msg_lower.contains("invalid")
        {
            return ConnectorErrorKind::NonTransient;
        }

        ConnectorErrorKind::Unknown
    }
}

/// Error classifier for `McpConnector`.
///
/// Classification rules:
/// - Connection failures / timeouts -> Transient
/// - JSON-RPC internal errors (-32603) -> Transient (server-side issue)
/// - JSON-RPC parse errors (-32700) -> NonTransient (bad data)
/// - JSON-RPC invalid request (-32600) / invalid params (-32602) -> NonTransient
/// - JSON-RPC method not found (-32601) -> NonTransient
/// - Protocol errors -> NonTransient
/// - Lock poisoned -> NonTransient
/// - Other errors -> Unknown
#[derive(Debug, Default)]
pub struct McpErrorClassifier;

impl ConnectorErrorClassifier for McpErrorClassifier {
    fn classify_error(&self, error: &anyhow::Error) -> ConnectorErrorKind {
        let msg = error.to_string();
        let msg_lower = msg.to_lowercase();

        // Check for MCP protocol error codes via downcast
        if let Some(mcp_err) = error.downcast_ref::<crate::mcp::McpError>() {
            return classify_mcp_error_code(mcp_err.code);
        }

        // Connection / transport level errors -> Transient
        if msg_lower.contains("timed out")
            || msg_lower.contains("timeout")
            || msg_lower.contains("connection refused")
            || msg_lower.contains("connection reset")
            || msg_lower.contains("broken pipe")
            || msg_lower.contains("connect error")
            || msg_lower.contains("transport error")
        {
            return ConnectorErrorKind::Transient;
        }

        // MCP error code patterns in string form
        if msg_lower.contains("mcp error -32603") {
            return ConnectorErrorKind::Transient;
        }
        if msg_lower.contains("mcp error -32700")
            || msg_lower.contains("mcp error -32600")
            || msg_lower.contains("mcp error -32602")
            || msg_lower.contains("mcp error -32601")
        {
            return ConnectorErrorKind::NonTransient;
        }

        // Protocol / deserialization errors -> NonTransient
        if msg_lower.contains("invalid input")
            || msg_lower.contains("invalid params")
            || msg_lower.contains("method not found")
            || msg_lower.contains("parse error")
            || msg_lower.contains("lock poisoned")
            || msg_lower.contains("no mcp mapping found")
        {
            return ConnectorErrorKind::NonTransient;
        }

        ConnectorErrorKind::Unknown
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Classify an `std::io::Error` by its `ErrorKind`.
fn classify_io_error(err: &std::io::Error) -> ConnectorErrorKind {
    match err.kind() {
        std::io::ErrorKind::TimedOut
        | std::io::ErrorKind::ConnectionRefused
        | std::io::ErrorKind::ConnectionReset
        | std::io::ErrorKind::ConnectionAborted
        | std::io::ErrorKind::BrokenPipe
        | std::io::ErrorKind::WouldBlock
        | std::io::ErrorKind::Interrupted => ConnectorErrorKind::Transient,

        std::io::ErrorKind::NotFound
        | std::io::ErrorKind::PermissionDenied
        | std::io::ErrorKind::InvalidInput
        | std::io::ErrorKind::InvalidData
        | std::io::ErrorKind::AlreadyExists => ConnectorErrorKind::NonTransient,

        _ => ConnectorErrorKind::Unknown,
    }
}

/// Classify an HTTP status code.
fn classify_http_status(code: u16) -> ConnectorErrorKind {
    match code {
        429 | 500 | 502 | 503 | 504 => ConnectorErrorKind::Transient,
        400 | 401 | 403 | 404 | 405 | 409 | 422 => ConnectorErrorKind::NonTransient,
        _ if code >= 400 => ConnectorErrorKind::Unknown,
        _ => ConnectorErrorKind::Unknown,
    }
}

/// Classify an MCP JSON-RPC error code.
fn classify_mcp_error_code(code: i32) -> ConnectorErrorKind {
    match code {
        -32603 => ConnectorErrorKind::Transient, // Internal error (server-side)
        -32700 | -32600 | -32602 | -32601 => ConnectorErrorKind::NonTransient, // Parse / invalid / not found
        _ => ConnectorErrorKind::Unknown,
    }
}

/// Extract an HTTP status code from an error message string.
fn extract_http_status(msg: &str) -> Option<u16> {
    // Match patterns like "HTTP error status: 429" or "returned status 503"
    let patterns = ["status: ", "status "];
    for pattern in &patterns {
        if let Some(idx) = msg.find(pattern) {
            let after = &msg[idx + pattern.len()..];
            let code_str: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(code) = code_str.parse::<u16>() {
                return Some(code);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // LocalErrorClassifier tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_local_io_not_found_is_non_transient() {
        let classifier = LocalErrorClassifier;
        let err = anyhow::Error::new(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "file not found",
        ));
        assert_eq!(
            classifier.classify_error(&err),
            ConnectorErrorKind::NonTransient
        );
        assert!(!classifier.should_retry(&err));
    }

    #[test]
    fn test_local_io_permission_denied_is_non_transient() {
        let classifier = LocalErrorClassifier;
        let err = anyhow::Error::new(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "access denied",
        ));
        assert_eq!(
            classifier.classify_error(&err),
            ConnectorErrorKind::NonTransient
        );
    }

    #[test]
    fn test_local_io_timed_out_is_transient() {
        let classifier = LocalErrorClassifier;
        let err = anyhow::Error::new(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "operation timed out",
        ));
        assert_eq!(
            classifier.classify_error(&err),
            ConnectorErrorKind::Transient
        );
        assert!(classifier.should_retry(&err));
    }

    #[test]
    fn test_local_io_connection_refused_is_transient() {
        let classifier = LocalErrorClassifier;
        let err = anyhow::Error::new(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "connection refused",
        ));
        assert_eq!(
            classifier.classify_error(&err),
            ConnectorErrorKind::Transient
        );
    }

    #[test]
    fn test_local_path_traversal_is_non_transient() {
        let classifier = LocalErrorClassifier;
        let err = anyhow::anyhow!("Path contains '..' components which are not allowed");
        assert_eq!(
            classifier.classify_error(&err),
            ConnectorErrorKind::NonTransient
        );
    }

    #[test]
    fn test_local_outside_workspace_is_non_transient() {
        let classifier = LocalErrorClassifier;
        let err = anyhow::anyhow!("Path is outside workspace boundary");
        assert_eq!(
            classifier.classify_error(&err),
            ConnectorErrorKind::NonTransient
        );
    }

    #[test]
    fn test_local_unknown_capability_is_non_transient() {
        let classifier = LocalErrorClassifier;
        let err = anyhow::anyhow!("Unknown capability: foo.bar");
        assert_eq!(
            classifier.classify_error(&err),
            ConnectorErrorKind::NonTransient
        );
    }

    #[test]
    fn test_local_unknown_error_is_unknown() {
        let classifier = LocalErrorClassifier;
        let err = anyhow::anyhow!("something unexpected happened");
        assert_eq!(classifier.classify_error(&err), ConnectorErrorKind::Unknown);
        assert!(!classifier.should_retry(&err));
    }

    // -----------------------------------------------------------------------
    // HttpErrorClassifier tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_http_429_is_transient() {
        let classifier = HttpErrorClassifier;
        let err = anyhow::anyhow!("HTTP error status: 429");
        assert_eq!(
            classifier.classify_error(&err),
            ConnectorErrorKind::Transient
        );
        assert!(classifier.should_retry(&err));
    }

    #[test]
    fn test_http_500_is_transient() {
        let classifier = HttpErrorClassifier;
        let err = anyhow::anyhow!("HTTP error status: 500");
        assert_eq!(
            classifier.classify_error(&err),
            ConnectorErrorKind::Transient
        );
    }

    #[test]
    fn test_http_502_is_transient() {
        let classifier = HttpErrorClassifier;
        let err = anyhow::anyhow!("HTTP error status: 502");
        assert_eq!(
            classifier.classify_error(&err),
            ConnectorErrorKind::Transient
        );
    }

    #[test]
    fn test_http_503_is_transient() {
        let classifier = HttpErrorClassifier;
        let err = anyhow::anyhow!("HTTP error status: 503");
        assert_eq!(
            classifier.classify_error(&err),
            ConnectorErrorKind::Transient
        );
    }

    #[test]
    fn test_http_504_is_transient() {
        let classifier = HttpErrorClassifier;
        let err = anyhow::anyhow!("returned status 504");
        assert_eq!(
            classifier.classify_error(&err),
            ConnectorErrorKind::Transient
        );
    }

    #[test]
    fn test_http_400_is_non_transient() {
        let classifier = HttpErrorClassifier;
        let err = anyhow::anyhow!("HTTP error status: 400");
        assert_eq!(
            classifier.classify_error(&err),
            ConnectorErrorKind::NonTransient
        );
        assert!(!classifier.should_retry(&err));
    }

    #[test]
    fn test_http_401_is_non_transient() {
        let classifier = HttpErrorClassifier;
        let err = anyhow::anyhow!("HTTP error status: 401");
        assert_eq!(
            classifier.classify_error(&err),
            ConnectorErrorKind::NonTransient
        );
    }

    #[test]
    fn test_http_403_is_non_transient() {
        let classifier = HttpErrorClassifier;
        let err = anyhow::anyhow!("HTTP error status: 403");
        assert_eq!(
            classifier.classify_error(&err),
            ConnectorErrorKind::NonTransient
        );
    }

    #[test]
    fn test_http_404_is_non_transient() {
        let classifier = HttpErrorClassifier;
        let err = anyhow::anyhow!("HTTP error status: 404");
        assert_eq!(
            classifier.classify_error(&err),
            ConnectorErrorKind::NonTransient
        );
    }

    #[test]
    fn test_http_timeout_is_transient() {
        let classifier = HttpErrorClassifier;
        let err = anyhow::anyhow!("HTTP request failed: operation timed out");
        assert_eq!(
            classifier.classify_error(&err),
            ConnectorErrorKind::Transient
        );
    }

    #[test]
    fn test_http_connection_refused_is_transient() {
        let classifier = HttpErrorClassifier;
        let err = anyhow::anyhow!("HTTP request failed: connection refused");
        assert_eq!(
            classifier.classify_error(&err),
            ConnectorErrorKind::Transient
        );
    }

    #[test]
    fn test_http_unsupported_method_is_non_transient() {
        let classifier = HttpErrorClassifier;
        let err = anyhow::anyhow!("Unsupported HTTP method: TRACE");
        assert_eq!(
            classifier.classify_error(&err),
            ConnectorErrorKind::NonTransient
        );
    }

    #[test]
    fn test_http_unknown_error_is_unknown() {
        let classifier = HttpErrorClassifier;
        let err = anyhow::anyhow!("something unexpected");
        assert_eq!(classifier.classify_error(&err), ConnectorErrorKind::Unknown);
    }

    // -----------------------------------------------------------------------
    // McpErrorClassifier tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_mcp_connection_refused_is_transient() {
        let classifier = McpErrorClassifier;
        let err = anyhow::anyhow!("Failed to connect: connection refused");
        assert_eq!(
            classifier.classify_error(&err),
            ConnectorErrorKind::Transient
        );
        assert!(classifier.should_retry(&err));
    }

    #[test]
    fn test_mcp_timeout_is_transient() {
        let classifier = McpErrorClassifier;
        let err = anyhow::anyhow!("MCP request timed out after 30s");
        assert_eq!(
            classifier.classify_error(&err),
            ConnectorErrorKind::Transient
        );
    }

    #[test]
    fn test_mcp_internal_error_code_string_is_transient() {
        let classifier = McpErrorClassifier;
        let err = anyhow::anyhow!("MCP Error -32603: Internal error");
        assert_eq!(
            classifier.classify_error(&err),
            ConnectorErrorKind::Transient
        );
    }

    #[test]
    fn test_mcp_parse_error_code_string_is_non_transient() {
        let classifier = McpErrorClassifier;
        let err = anyhow::anyhow!("MCP Error -32700: Parse error");
        assert_eq!(
            classifier.classify_error(&err),
            ConnectorErrorKind::NonTransient
        );
    }

    #[test]
    fn test_mcp_invalid_request_code_string_is_non_transient() {
        let classifier = McpErrorClassifier;
        let err = anyhow::anyhow!("MCP Error -32600: Invalid Request");
        assert_eq!(
            classifier.classify_error(&err),
            ConnectorErrorKind::NonTransient
        );
    }

    #[test]
    fn test_mcp_method_not_found_code_string_is_non_transient() {
        let classifier = McpErrorClassifier;
        let err = anyhow::anyhow!("MCP Error -32601: Method not found");
        assert_eq!(
            classifier.classify_error(&err),
            ConnectorErrorKind::NonTransient
        );
    }

    #[test]
    fn test_mcp_invalid_params_code_string_is_non_transient() {
        let classifier = McpErrorClassifier;
        let err = anyhow::anyhow!("MCP Error -32602: Invalid params");
        assert_eq!(
            classifier.classify_error(&err),
            ConnectorErrorKind::NonTransient
        );
    }

    #[test]
    fn test_mcp_downcast_internal_error_is_transient() {
        let classifier = McpErrorClassifier;
        let mcp_err = crate::mcp::McpError::internal_error("server overloaded");
        let err = anyhow::Error::new(mcp_err);
        assert_eq!(
            classifier.classify_error(&err),
            ConnectorErrorKind::Transient
        );
    }

    #[test]
    fn test_mcp_downcast_method_not_found_is_non_transient() {
        let classifier = McpErrorClassifier;
        let mcp_err = crate::mcp::McpError::method_not_found("tools/unknown");
        let err = anyhow::Error::new(mcp_err);
        assert_eq!(
            classifier.classify_error(&err),
            ConnectorErrorKind::NonTransient
        );
    }

    #[test]
    fn test_mcp_downcast_parse_error_is_non_transient() {
        let classifier = McpErrorClassifier;
        let mcp_err = crate::mcp::McpError::parse_error("bad json");
        let err = anyhow::Error::new(mcp_err);
        assert_eq!(
            classifier.classify_error(&err),
            ConnectorErrorKind::NonTransient
        );
    }

    #[test]
    fn test_mcp_no_mapping_is_non_transient() {
        let classifier = McpErrorClassifier;
        let err = anyhow::anyhow!("No MCP mapping found for capability: mcp.tool.xyz");
        assert_eq!(
            classifier.classify_error(&err),
            ConnectorErrorKind::NonTransient
        );
    }

    #[test]
    fn test_mcp_lock_poisoned_is_non_transient() {
        let classifier = McpErrorClassifier;
        let err = anyhow::anyhow!("MCP capabilities lock poisoned");
        assert_eq!(
            classifier.classify_error(&err),
            ConnectorErrorKind::NonTransient
        );
    }

    #[test]
    fn test_mcp_unknown_error_is_unknown() {
        let classifier = McpErrorClassifier;
        let err = anyhow::anyhow!("something unexpected");
        assert_eq!(classifier.classify_error(&err), ConnectorErrorKind::Unknown);
        assert!(!classifier.should_retry(&err));
    }

    // -----------------------------------------------------------------------
    // Helper function tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_extract_http_status_with_colon() {
        assert_eq!(extract_http_status("HTTP error status: 429"), Some(429));
        assert_eq!(extract_http_status("HTTP error status: 503"), Some(503));
    }

    #[test]
    fn test_extract_http_status_without_colon() {
        assert_eq!(extract_http_status("returned status 404"), Some(404));
    }

    #[test]
    fn test_extract_http_status_no_match() {
        assert_eq!(extract_http_status("no status here"), None);
    }

    #[test]
    fn test_classify_http_status_codes() {
        assert_eq!(classify_http_status(429), ConnectorErrorKind::Transient);
        assert_eq!(classify_http_status(500), ConnectorErrorKind::Transient);
        assert_eq!(classify_http_status(503), ConnectorErrorKind::Transient);
        assert_eq!(classify_http_status(400), ConnectorErrorKind::NonTransient);
        assert_eq!(classify_http_status(401), ConnectorErrorKind::NonTransient);
        assert_eq!(classify_http_status(404), ConnectorErrorKind::NonTransient);
        assert_eq!(classify_http_status(418), ConnectorErrorKind::Unknown); // I'm a teapot
    }

    #[test]
    fn test_classify_io_error_kinds() {
        let transient_kinds = [
            std::io::ErrorKind::TimedOut,
            std::io::ErrorKind::ConnectionRefused,
            std::io::ErrorKind::ConnectionReset,
            std::io::ErrorKind::BrokenPipe,
            std::io::ErrorKind::WouldBlock,
            std::io::ErrorKind::Interrupted,
        ];
        for kind in &transient_kinds {
            let err = std::io::Error::new(*kind, "test");
            assert_eq!(
                classify_io_error(&err),
                ConnectorErrorKind::Transient,
                "Expected {:?} to be Transient",
                kind
            );
        }

        let non_transient_kinds = [
            std::io::ErrorKind::NotFound,
            std::io::ErrorKind::PermissionDenied,
            std::io::ErrorKind::InvalidInput,
            std::io::ErrorKind::AlreadyExists,
        ];
        for kind in &non_transient_kinds {
            let err = std::io::Error::new(*kind, "test");
            assert_eq!(
                classify_io_error(&err),
                ConnectorErrorKind::NonTransient,
                "Expected {:?} to be NonTransient",
                kind
            );
        }
    }

    #[test]
    fn test_classify_mcp_error_codes() {
        assert_eq!(
            classify_mcp_error_code(-32603),
            ConnectorErrorKind::Transient
        );
        assert_eq!(
            classify_mcp_error_code(-32700),
            ConnectorErrorKind::NonTransient
        );
        assert_eq!(
            classify_mcp_error_code(-32600),
            ConnectorErrorKind::NonTransient
        );
        assert_eq!(
            classify_mcp_error_code(-32602),
            ConnectorErrorKind::NonTransient
        );
        assert_eq!(
            classify_mcp_error_code(-32601),
            ConnectorErrorKind::NonTransient
        );
        assert_eq!(classify_mcp_error_code(-1), ConnectorErrorKind::Unknown);
    }
}
