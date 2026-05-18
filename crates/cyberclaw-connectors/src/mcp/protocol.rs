//! MCP JSON-RPC 2.0 protocol implementation

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC 2.0 request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpRequest {
    /// JSON-RPC version (always "2.0")
    pub jsonrpc: String,
    /// Method name
    pub method: String,
    /// Parameters (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
    /// Request ID
    pub id: RequestId,
}

/// JSON-RPC 2.0 response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResponse {
    /// JSON-RPC version (always "2.0")
    pub jsonrpc: String,
    /// Result (present on success)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Error (present on failure)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<McpError>,
    /// Request ID (same as request)
    pub id: RequestId,
}

/// JSON-RPC error object
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpError {
    /// Error code
    pub code: i32,
    /// Error message
    pub message: String,
    /// Additional error data (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// Request ID (can be string, number, or null)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(untagged)]
pub enum RequestId {
    String(String),
    Number(i64),
    Null,
}

impl McpRequest {
    /// Create a new request
    pub fn new(method: impl Into<String>, params: Option<Value>, id: RequestId) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            method: method.into(),
            params,
            id,
        }
    }

    /// Create a tools/list request
    pub fn tools_list(id: RequestId) -> Self {
        Self::new("tools/list", None, id)
    }

    /// Create a tools/call request
    pub fn tools_call(tool_name: impl Into<String>, arguments: Value, id: RequestId) -> Self {
        Self::new(
            "tools/call",
            Some(serde_json::json!({
                "name": tool_name.into(),
                "arguments": arguments,
            })),
            id,
        )
    }

    /// Create a resources/list request
    pub fn resources_list(id: RequestId) -> Self {
        Self::new("resources/list", None, id)
    }

    /// Create a resources/read request
    pub fn resources_read(uri: impl Into<String>, id: RequestId) -> Self {
        Self::new(
            "resources/read",
            Some(serde_json::json!({
                "uri": uri.into(),
            })),
            id,
        )
    }

    /// Create a prompts/list request
    pub fn prompts_list(id: RequestId) -> Self {
        Self::new("prompts/list", None, id)
    }

    /// Create a prompts/get request
    pub fn prompts_get(
        prompt_name: impl Into<String>,
        arguments: Option<Value>,
        id: RequestId,
    ) -> Self {
        let mut params = serde_json::json!({
            "name": prompt_name.into(),
        });

        if let Some(args) = arguments {
            params["arguments"] = args;
        }

        Self::new("prompts/get", Some(params), id)
    }
}

impl McpResponse {
    /// Check if response is successful
    pub fn is_success(&self) -> bool {
        self.error.is_none()
    }

    /// Check if response is error
    pub fn is_error(&self) -> bool {
        self.error.is_some()
    }

    /// Get result or error
    pub fn into_result(self) -> Result<Value, McpError> {
        match (self.result, self.error) {
            (Some(result), None) => Ok(result),
            (None, Some(error)) => Err(error),
            (Some(_), Some(error)) => {
                // Invalid state: both result and error present
                Err(error)
            }
            (None, None) => {
                // Invalid state: neither result nor error present
                Err(McpError {
                    code: -32603,
                    message: "Invalid response: missing both result and error".to_string(),
                    data: None,
                })
            }
        }
    }
}

impl McpError {
    /// Create a parse error (-32700)
    pub fn parse_error(message: impl Into<String>) -> Self {
        Self {
            code: -32700,
            message: message.into(),
            data: None,
        }
    }

    /// Create an invalid request error (-32600)
    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self {
            code: -32600,
            message: message.into(),
            data: None,
        }
    }

    /// Create a method not found error (-32601)
    pub fn method_not_found(method: impl Into<String>) -> Self {
        Self {
            code: -32601,
            message: format!("Method not found: {}", method.into()),
            data: None,
        }
    }

    /// Create an invalid params error (-32602)
    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: message.into(),
            data: None,
        }
    }

    /// Create an internal error (-32603)
    pub fn internal_error(message: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: message.into(),
            data: None,
        }
    }

    /// Create a custom error
    pub fn custom(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    /// Add error data
    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }
}

impl std::fmt::Display for McpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MCP Error {}: {}", self.code, self.message)
    }
}

impl std::error::Error for McpError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_serialization() {
        let req = McpRequest::tools_list(RequestId::String("test-1".to_string()));

        let json = serde_json::to_string(&req).unwrap();
        let parsed: McpRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.jsonrpc, "2.0");
        assert_eq!(parsed.method, "tools/list");
        assert_eq!(parsed.id, RequestId::String("test-1".to_string()));
    }

    #[test]
    fn test_tools_call_request() {
        let req = McpRequest::tools_call(
            "test_tool",
            serde_json::json!({"arg1": "value1"}),
            RequestId::Number(42),
        );

        assert_eq!(req.method, "tools/call");
        assert!(req.params.is_some());

        let params = req.params.unwrap();
        assert_eq!(params["name"], "test_tool");
        assert_eq!(params["arguments"]["arg1"], "value1");
    }

    #[test]
    fn test_response_success() {
        let resp = McpResponse {
            jsonrpc: "2.0".to_string(),
            result: Some(serde_json::json!({"data": "test"})),
            error: None,
            id: RequestId::Number(1),
        };

        assert!(resp.is_success());
        assert!(!resp.is_error());

        let result = resp.into_result().unwrap();
        assert_eq!(result["data"], "test");
    }

    #[test]
    fn test_response_error() {
        let resp = McpResponse {
            jsonrpc: "2.0".to_string(),
            result: None,
            error: Some(McpError::method_not_found("test")),
            id: RequestId::Number(1),
        };

        assert!(!resp.is_success());
        assert!(resp.is_error());

        let err = resp.into_result().unwrap_err();
        assert_eq!(err.code, -32601);
    }

    #[test]
    fn test_error_display() {
        let err = McpError::internal_error("Something went wrong");
        let display = format!("{}", err);
        assert!(display.contains("32603"));
        assert!(display.contains("Something went wrong"));
    }
}
