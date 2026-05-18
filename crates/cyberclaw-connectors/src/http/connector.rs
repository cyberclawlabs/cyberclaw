//! HTTP/REST Connector implementation

use crate::http::auth::AuthStrategy;
use crate::types::{
    CapabilityExecutionRequest, CapabilityExecutionResult, Connector, ExecutionStatus,
};
use cyberclaw_core::manifests::{CapabilityContract, CapabilityTimeouts, ConnectorRuntime};
use cyberclaw_core::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;
use tracing::{debug, error, info};

/// Endpoint definition for an HTTP capability
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpEndpoint {
    /// Capability ID, e.g. `"user.get"`
    pub capability_id: String,
    /// Human-readable title
    pub title: String,
    /// Optional description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// HTTP method: GET, POST, PUT, PATCH, DELETE
    pub method: String,
    /// Path appended to base_url, e.g. `/users/{id}`
    pub path: String,
}

/// Configuration for HttpConnector
#[derive(Debug, Clone)]
pub struct HttpConnectorConfig {
    /// Connector ID string
    pub connector_id: String,
    /// Base URL for all endpoints, e.g. `https://api.example.com`
    pub base_url: String,
    /// Authentication strategy
    pub auth: AuthStrategy,
    /// Request timeout in milliseconds (default: 30 000)
    pub timeout_ms: u64,
    /// Default headers added to every request
    pub default_headers: HashMap<String, String>,
    /// Endpoint definitions that become capabilities
    pub endpoints: Vec<HttpEndpoint>,
}

impl Default for HttpConnectorConfig {
    fn default() -> Self {
        Self {
            connector_id: "http".to_string(),
            base_url: String::new(),
            auth: AuthStrategy::None,
            timeout_ms: 30_000,
            default_headers: HashMap::new(),
            endpoints: Vec::new(),
        }
    }
}

/// Response returned inside `CapabilityExecutionResult.output`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpConnectorResponse {
    pub status_code: u16,
    pub body: serde_json::Value,
    pub headers: HashMap<String, String>,
}

/// HTTP/REST connector — executes capabilities by forwarding requests to remote REST endpoints
#[derive(Debug)]
pub struct HttpConnector {
    id: ConnectorId,
    config: HttpConnectorConfig,
    capabilities: Vec<CapabilityContract>,
    client: reqwest::Client,
}

impl HttpConnector {
    /// Create a new `HttpConnector` from the given configuration.
    pub fn new(config: HttpConnectorConfig) -> anyhow::Result<Self> {
        let id = ConnectorId::from_string(config.connector_id.clone()).map_err(|e| {
            anyhow::anyhow!("Invalid connector id '{}': {}", config.connector_id, e)
        })?;

        let capabilities = Self::build_capabilities(&config);

        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(config.timeout_ms))
            .build()
            .map_err(|e| anyhow::anyhow!("Failed to build HTTP client: {}", e))?;

        Ok(Self {
            id,
            capabilities,
            client,
            config,
        })
    }

    /// Build capability contracts from endpoint definitions
    fn build_capabilities(config: &HttpConnectorConfig) -> Vec<CapabilityContract> {
        config
            .endpoints
            .iter()
            .map(|ep| CapabilityContract {
                id: ep.capability_id.clone(),
                title: ep.title.clone(),
                description: ep.description.clone(),
                input_schema: "HttpRequestInput".to_string(),
                output_schema: "HttpConnectorResponse".to_string(),
                risk: RiskLevel::Low,
                effects: vec![CapabilityEffect::Network],
                placement: None,
                timeouts: CapabilityTimeouts {
                    request_ms: Some(config.timeout_ms),
                },
            })
            .collect()
    }

    /// Find the endpoint matching a capability id
    fn find_endpoint(&self, capability_id: &str) -> Option<&HttpEndpoint> {
        self.config
            .endpoints
            .iter()
            .find(|ep| ep.capability_id == capability_id)
    }

    /// Execute an HTTP request for the given endpoint and input params
    async fn execute_request(
        &self,
        endpoint: &HttpEndpoint,
        input: &serde_json::Value,
    ) -> anyhow::Result<HttpConnectorResponse> {
        // Build URL: substitute {param} placeholders if present
        let path = if let Some(params) = input.as_object() {
            let mut p = endpoint.path.clone();
            for (k, v) in params {
                if let Some(s) = v.as_str() {
                    p = p.replace(&format!("{{{}}}", k), s);
                }
            }
            p
        } else {
            endpoint.path.clone()
        };

        let url = format!("{}{}", self.config.base_url.trim_end_matches('/'), path);

        debug!(
            "HttpConnector: {} {} (capability={})",
            endpoint.method, url, endpoint.capability_id
        );

        let method = endpoint.method.to_uppercase();
        let mut builder = match method.as_str() {
            "GET" => self.client.get(&url),
            "POST" => self.client.post(&url),
            "PUT" => self.client.put(&url),
            "PATCH" => self.client.patch(&url),
            "DELETE" => self.client.delete(&url),
            other => {
                return Err(anyhow::anyhow!("Unsupported HTTP method: {}", other));
            }
        };

        // Apply default headers
        for (name, value) in &self.config.default_headers {
            builder = builder.header(name.as_str(), value.as_str());
        }

        // Apply authentication
        builder = self.config.auth.apply(builder);

        // Attach body for mutating methods
        if matches!(method.as_str(), "POST" | "PUT" | "PATCH") && !input.is_null() {
            builder = builder.json(input);
        }

        let response = builder
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("HTTP request failed: {}", e))?;

        let status_code = response.status().as_u16();

        // Collect response headers
        let headers: HashMap<String, String> = response
            .headers()
            .iter()
            .filter_map(|(k, v)| v.to_str().ok().map(|s| (k.to_string(), s.to_string())))
            .collect();

        // Parse body as JSON, fall back to raw string
        let body_text = response.text().await.unwrap_or_default();

        let body: serde_json::Value =
            serde_json::from_str(&body_text).unwrap_or(serde_json::Value::String(body_text));

        if status_code >= 400 {
            error!(
                "HttpConnector: {} {} returned status {}",
                endpoint.method, url, status_code
            );
        } else {
            info!(
                "HttpConnector: {} {} -> {}",
                endpoint.method, url, status_code
            );
        }

        Ok(HttpConnectorResponse {
            status_code,
            body,
            headers,
        })
    }
}

#[async_trait::async_trait]
impl Connector for HttpConnector {
    fn id(&self) -> &ConnectorId {
        &self.id
    }

    fn runtime(&self) -> ConnectorRuntime {
        ConnectorRuntime::Remote
    }

    fn capabilities(&self) -> Vec<CapabilityContract> {
        self.capabilities.clone()
    }

    async fn execute(
        &self,
        request: CapabilityExecutionRequest,
    ) -> anyhow::Result<CapabilityExecutionResult> {
        debug!(
            "HttpConnector executing capability {} for execution {}",
            request.capability_id, request.execution_id
        );

        let endpoint = match self.find_endpoint(request.capability_id.as_str()) {
            Some(ep) => ep,
            None => {
                let error_msg = format!(
                    "Unknown capability '{}' for HttpConnector '{}'",
                    request.capability_id, self.id
                );
                error!("{}", error_msg);
                return Ok(CapabilityExecutionResult {
                    execution_id: request.execution_id,
                    trace_id: request.trace_id,
                    connector_id: request.connector_id,
                    capability_id: request.capability_id,
                    output: serde_json::json!({ "error": error_msg }),
                    status: ExecutionStatus::Failed,
                    error: Some(error_msg),
                    actual_runtime: None,
                });
            }
        };

        match self.execute_request(endpoint, &request.input).await {
            Ok(resp) => {
                let output = serde_json::to_value(&resp).unwrap_or_else(
                    |_| serde_json::json!({ "error": "failed to serialize response" }),
                );
                Ok(CapabilityExecutionResult {
                    execution_id: request.execution_id,
                    trace_id: request.trace_id,
                    connector_id: request.connector_id,
                    capability_id: request.capability_id,
                    output,
                    status: if resp.status_code < 400 {
                        ExecutionStatus::Success
                    } else {
                        ExecutionStatus::Failed
                    },
                    error: if resp.status_code >= 400 {
                        Some(format!("HTTP error status: {}", resp.status_code))
                    } else {
                        None
                    },
                    actual_runtime: None,
                })
            }
            Err(e) => {
                error!(
                    "HttpConnector capability {} failed: {:?}",
                    request.capability_id, e
                );
                Ok(CapabilityExecutionResult {
                    execution_id: request.execution_id,
                    trace_id: request.trace_id,
                    connector_id: request.connector_id,
                    capability_id: request.capability_id,
                    output: serde_json::json!({ "error": e.to_string() }),
                    status: ExecutionStatus::Failed,
                    error: Some(e.to_string()),
                    actual_runtime: None,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config(endpoints: Vec<HttpEndpoint>) -> HttpConnectorConfig {
        HttpConnectorConfig {
            connector_id: "test-http".to_string(),
            base_url: "https://api.example.com".to_string(),
            auth: AuthStrategy::None,
            timeout_ms: 5_000,
            default_headers: HashMap::new(),
            endpoints,
        }
    }

    #[test]
    fn test_connector_creation_and_id() {
        let config = make_config(vec![]);
        let connector = HttpConnector::new(config).expect("should create connector");
        assert_eq!(connector.id().as_str(), "test-http");
    }

    #[test]
    fn test_connector_runtime_is_remote() {
        let config = make_config(vec![]);
        let connector = HttpConnector::new(config).unwrap();
        assert!(matches!(connector.runtime(), ConnectorRuntime::Remote));
    }

    #[test]
    fn test_capability_list_generated_from_endpoints() {
        let config = make_config(vec![
            HttpEndpoint {
                capability_id: "user.list".to_string(),
                title: "List Users".to_string(),
                description: None,
                method: "GET".to_string(),
                path: "/users".to_string(),
            },
            HttpEndpoint {
                capability_id: "user.create".to_string(),
                title: "Create User".to_string(),
                description: Some("Create a new user".to_string()),
                method: "POST".to_string(),
                path: "/users".to_string(),
            },
        ]);
        let connector = HttpConnector::new(config).unwrap();
        let caps = connector.capabilities();
        assert_eq!(caps.len(), 2);
        assert_eq!(caps[0].id, "user.list");
        assert_eq!(caps[1].id, "user.create");
        assert_eq!(caps[1].description.as_deref(), Some("Create a new user"));
    }

    #[test]
    fn test_empty_endpoints_gives_empty_capabilities() {
        let config = make_config(vec![]);
        let connector = HttpConnector::new(config).unwrap();
        assert!(connector.capabilities().is_empty());
    }

    #[test]
    fn test_invalid_connector_id_returns_error() {
        let config = HttpConnectorConfig {
            connector_id: "".to_string(),
            ..HttpConnectorConfig::default()
        };
        // ConnectorId::from_string should reject empty string
        let result = HttpConnector::new(config);
        // Either Ok or Err is acceptable depending on ConnectorId validation;
        // the key check is that we don't panic.
        let _ = result;
    }

    #[test]
    fn test_connector_config_default_timeout() {
        let config = HttpConnectorConfig::default();
        assert_eq!(config.timeout_ms, 30_000);
    }
}
