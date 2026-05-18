//! OpenViking memory connector — Phase A: 7 read-only capabilities.
//!
//! Communicates with an independently deployed OpenViking instance via REST API.
//! AGPL safety: no Python SDK embedding, no code linking — pure HTTP client.

use crate::openviking::circuit_breaker::{CbState, OvCircuitBreaker};
use crate::openviking::types::*;
use crate::types::{
    CapabilityExecutionRequest, CapabilityExecutionResult, Connector,
    ExecutionStatus as ConnectorExecutionStatus,
};
use cyberclaw_core::capability::CapabilityEffect;
use cyberclaw_core::manifests::{CapabilityContract, CapabilityTimeouts, ConnectorRuntime};
use cyberclaw_core::prelude::*;
use reqwest::Client;
use std::time::Duration;
use tracing::{debug, error, info, warn};

// ---------------------------------------------------------------------------
// Minimal percent-encoding helper (no extra dependency)
// ---------------------------------------------------------------------------

fn pct_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => {
                out.push('%');
                out.push_str(&format!("{:02X}", byte));
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Connector
// ---------------------------------------------------------------------------

/// OpenViking memory connector — read-only capabilities (Phase A).
#[derive(Debug)]
pub struct OpenVikingConnector {
    id: ConnectorId,
    config: OpenVikingConfig,
    client: Client,
    circuit_breaker: OvCircuitBreaker,
    capabilities: Vec<CapabilityContract>,
}

impl OpenVikingConnector {
    /// Create a new connector with the given configuration.
    pub fn new(config: OpenVikingConfig) -> Self {
        let cb = OvCircuitBreaker::new(
            config.circuit_breaker_threshold,
            config.circuit_breaker_cooldown_ms,
        );
        let client = Client::builder()
            .timeout(Duration::from_millis(config.detail_timeout_ms))
            .build()
            .expect("Failed to build reqwest client");

        Self {
            id: ConnectorId::from_string("openviking-memory".to_string())
                .expect("valid connector id"),
            config,
            client,
            circuit_breaker: cb,
            capabilities: Self::build_capabilities(),
        }
    }

    fn build_capabilities() -> Vec<CapabilityContract> {
        vec![
            Self::cap("openviking.memory.ls", "List memory namespace", 10_000),
            Self::cap("openviking.memory.tree", "Tree view of memory", 10_000),
            Self::cap("openviking.memory.read", "Read memory resource", 30_000),
            Self::cap("openviking.memory.search", "Search memory", 30_000),
            Self::cap("openviking.memory.find", "Find by pattern", 10_000),
            Self::cap(
                "openviking.memory.abstract",
                "Get L0 abstract (~100 tokens)",
                10_000,
            ),
            Self::cap(
                "openviking.memory.overview",
                "Get L1 overview (~2000 tokens)",
                10_000,
            ),
        ]
    }

    fn cap(id: &str, desc: &str, timeout_ms: u64) -> CapabilityContract {
        CapabilityContract {
            id: id.to_string(),
            title: desc.to_string(),
            description: Some(desc.to_string()),
            input_schema: format!("{}Input", id),
            output_schema: format!("{}Output", id),
            risk: RiskLevel::Low,
            effects: vec![CapabilityEffect::Read],
            placement: None,
            timeouts: CapabilityTimeouts {
                request_ms: Some(timeout_ms),
            },
        }
    }

    // -----------------------------------------------------------------------
    // HTTP helpers
    // -----------------------------------------------------------------------

    fn base_url(&self) -> &str {
        self.config.base_url.trim_end_matches('/')
    }

    fn timeout_for(&self, depth: &OvRetrievalDepth) -> Duration {
        if depth.is_fast() {
            Duration::from_millis(self.config.fast_timeout_ms)
        } else {
            Duration::from_millis(self.config.detail_timeout_ms)
        }
    }

    async fn get_json(&self, path: &str, timeout: Duration) -> anyhow::Result<serde_json::Value> {
        if !self.circuit_breaker.allow_request() {
            anyhow::bail!("OpenViking circuit breaker is open — returning degraded result");
        }

        let url = format!("{}{}", self.base_url(), path);
        debug!("OpenViking GET {}", url);

        let mut req = self.client.get(&url).timeout(timeout);
        if let Some(key) = &self.config.api_key {
            req = req.header("Authorization", format!("Bearer {}", key));
        }

        match req.send().await {
            Ok(resp) => {
                if resp.status().is_success() {
                    self.circuit_breaker.record_success();
                    let body = resp.json::<serde_json::Value>().await?;
                    Ok(body)
                } else {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    self.circuit_breaker.record_failure();
                    anyhow::bail!("OpenViking returned HTTP {}: {}", status, body)
                }
            }
            Err(e) => {
                self.circuit_breaker.record_failure();
                anyhow::bail!("OpenViking request failed: {}", e)
            }
        }
    }

    async fn post_json(
        &self,
        path: &str,
        body: &serde_json::Value,
        timeout: Duration,
    ) -> anyhow::Result<serde_json::Value> {
        if !self.circuit_breaker.allow_request() {
            anyhow::bail!("OpenViking circuit breaker is open — returning degraded result");
        }

        let url = format!("{}{}", self.base_url(), path);
        debug!("OpenViking POST {}", url);

        let mut req = self.client.post(&url).json(body).timeout(timeout);
        if let Some(key) = &self.config.api_key {
            req = req.header("Authorization", format!("Bearer {}", key));
        }

        match req.send().await {
            Ok(resp) => {
                if resp.status().is_success() {
                    self.circuit_breaker.record_success();
                    let result = resp.json::<serde_json::Value>().await?;
                    Ok(result)
                } else {
                    let status = resp.status();
                    let body_text = resp.text().await.unwrap_or_default();
                    self.circuit_breaker.record_failure();
                    anyhow::bail!("OpenViking returned HTTP {}: {}", status, body_text)
                }
            }
            Err(e) => {
                self.circuit_breaker.record_failure();
                anyhow::bail!("OpenViking request failed: {}", e)
            }
        }
    }

    // -----------------------------------------------------------------------
    // Capability handlers
    // -----------------------------------------------------------------------

    async fn handle_ls(&self, input: OvLsInput) -> anyhow::Result<serde_json::Value> {
        let path = pct_encode(&input.path);
        let timeout = Duration::from_millis(self.config.fast_timeout_ms);
        self.get_json(&format!("/api/v1/ls?path={}", path), timeout)
            .await
    }

    async fn handle_tree(&self, input: OvTreeInput) -> anyhow::Result<serde_json::Value> {
        let path = pct_encode(&input.path);
        let mut url = format!("/api/v1/tree?path={}", path);
        if let Some(depth) = input.max_depth {
            url.push_str(&format!("&max_depth={}", depth));
        }
        let timeout = Duration::from_millis(self.config.fast_timeout_ms);
        self.get_json(&url, timeout).await
    }

    async fn handle_read(&self, input: OvReadInput) -> anyhow::Result<serde_json::Value> {
        let uri = pct_encode(&input.uri);
        let level = input.depth.as_api_level();
        let timeout = self.timeout_for(&input.depth);
        self.get_json(
            &format!("/api/v1/read?uri={}&level={}", uri, level),
            timeout,
        )
        .await
    }

    async fn handle_search(&self, input: OvSearchInput) -> anyhow::Result<serde_json::Value> {
        let body = serde_json::json!({
            "query": input.query,
            "top_k": input.top_k,
            "level": input.depth.as_api_level(),
        });
        let timeout = self.timeout_for(&input.depth);
        self.post_json("/api/v1/search", &body, timeout).await
    }

    async fn handle_find(&self, input: OvFindInput) -> anyhow::Result<serde_json::Value> {
        let pattern = pct_encode(&input.pattern);
        let timeout = Duration::from_millis(self.config.fast_timeout_ms);
        self.get_json(&format!("/api/v1/find?pattern={}", pattern), timeout)
            .await
    }

    async fn handle_abstract(&self, input: OvAbstractInput) -> anyhow::Result<serde_json::Value> {
        let uri = pct_encode(&input.uri);
        let timeout = Duration::from_millis(self.config.fast_timeout_ms);
        self.get_json(&format!("/api/v1/abstract?uri={}", uri), timeout)
            .await
    }

    async fn handle_overview(&self, input: OvOverviewInput) -> anyhow::Result<serde_json::Value> {
        let uri = pct_encode(&input.uri);
        let timeout = Duration::from_millis(self.config.fast_timeout_ms);
        self.get_json(&format!("/api/v1/overview?uri={}", uri), timeout)
            .await
    }

    /// Build a degraded (empty) result when the circuit breaker is open.
    fn degraded_result(&self, request: &CapabilityExecutionRequest) -> CapabilityExecutionResult {
        warn!(
            "OpenViking circuit breaker open — returning degraded result for {}",
            request.capability_id
        );
        CapabilityExecutionResult {
            execution_id: request.execution_id.clone(),
            trace_id: request.trace_id.clone(),
            connector_id: request.connector_id.clone(),
            capability_id: request.capability_id.clone(),
            output: serde_json::json!({
                "degraded": true,
                "reason": "OpenViking circuit breaker is open",
                "results": []
            }),
            status: ConnectorExecutionStatus::Success, // fail-open
            error: None,
            actual_runtime: None,
        }
    }

    /// Check circuit breaker state; report degraded source name if open.
    pub fn degraded_source_name(&self) -> Option<&'static str> {
        if self.circuit_breaker.state() == CbState::Open {
            Some("openviking-memory")
        } else {
            None
        }
    }
}

#[async_trait::async_trait]
impl Connector for OpenVikingConnector {
    fn id(&self) -> &ConnectorId {
        &self.id
    }

    fn runtime(&self) -> ConnectorRuntime {
        ConnectorRuntime::Native
    }

    fn capabilities(&self) -> Vec<CapabilityContract> {
        self.capabilities.clone()
    }

    async fn execute(
        &self,
        request: CapabilityExecutionRequest,
    ) -> anyhow::Result<CapabilityExecutionResult> {
        debug!(
            "OpenVikingConnector executing {} for execution {}",
            request.capability_id, request.execution_id
        );

        // Fail-open: if breaker is open, return degraded result immediately
        if !self.circuit_breaker.allow_request() {
            return Ok(self.degraded_result(&request));
        }

        let result = match request.capability_id.as_str() {
            "openviking.memory.ls" => {
                let input: OvLsInput = serde_json::from_value(request.input.clone())?;
                self.handle_ls(input).await
            }
            "openviking.memory.tree" => {
                let input: OvTreeInput = serde_json::from_value(request.input.clone())?;
                self.handle_tree(input).await
            }
            "openviking.memory.read" => {
                let input: OvReadInput = serde_json::from_value(request.input.clone())?;
                self.handle_read(input).await
            }
            "openviking.memory.search" => {
                let input: OvSearchInput = serde_json::from_value(request.input.clone())?;
                self.handle_search(input).await
            }
            "openviking.memory.find" => {
                let input: OvFindInput = serde_json::from_value(request.input.clone())?;
                self.handle_find(input).await
            }
            "openviking.memory.abstract" => {
                let input: OvAbstractInput = serde_json::from_value(request.input.clone())?;
                self.handle_abstract(input).await
            }
            "openviking.memory.overview" => {
                let input: OvOverviewInput = serde_json::from_value(request.input.clone())?;
                self.handle_overview(input).await
            }
            other => {
                let msg = format!("Unknown OpenViking capability: {}", other);
                error!("{}", msg);
                return Ok(CapabilityExecutionResult {
                    execution_id: request.execution_id,
                    trace_id: request.trace_id,
                    connector_id: request.connector_id,
                    capability_id: request.capability_id,
                    output: serde_json::json!({ "error": msg.clone() }),
                    status: ConnectorExecutionStatus::Failed,
                    error: Some(msg),
                    actual_runtime: None,
                });
            }
        };

        match result {
            Ok(output) => {
                info!("OpenViking {} executed successfully", request.capability_id);
                Ok(CapabilityExecutionResult {
                    execution_id: request.execution_id,
                    trace_id: request.trace_id,
                    connector_id: request.connector_id,
                    capability_id: request.capability_id,
                    output,
                    status: ConnectorExecutionStatus::Success,
                    error: None,
                    actual_runtime: None,
                })
            }
            Err(e) => {
                error!("OpenViking {} failed: {:?}", request.capability_id, e);
                Ok(CapabilityExecutionResult {
                    execution_id: request.execution_id,
                    trace_id: request.trace_id,
                    connector_id: request.connector_id,
                    capability_id: request.capability_id,
                    output: serde_json::json!({ "error": e.to_string() }),
                    status: ConnectorExecutionStatus::Failed,
                    error: Some(e.to_string()),
                    actual_runtime: None,
                })
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_connector() -> OpenVikingConnector {
        OpenVikingConnector::new(OpenVikingConfig::default())
    }

    #[test]
    fn connector_id() {
        let c = test_connector();
        assert_eq!(c.id().as_str(), "openviking-memory");
    }

    #[test]
    fn has_seven_capabilities() {
        let c = test_connector();
        let caps = c.capabilities();
        assert_eq!(caps.len(), 7);
        let ids: Vec<&str> = caps.iter().map(|c| c.id.as_str()).collect();
        assert!(ids.contains(&"openviking.memory.ls"));
        assert!(ids.contains(&"openviking.memory.tree"));
        assert!(ids.contains(&"openviking.memory.read"));
        assert!(ids.contains(&"openviking.memory.search"));
        assert!(ids.contains(&"openviking.memory.find"));
        assert!(ids.contains(&"openviking.memory.abstract"));
        assert!(ids.contains(&"openviking.memory.overview"));
    }

    #[test]
    fn all_capabilities_are_read_only_low_risk() {
        let c = test_connector();
        for cap in c.capabilities() {
            assert_eq!(
                cap.risk,
                RiskLevel::Low,
                "cap {} should be Low risk",
                cap.id
            );
            assert_eq!(
                cap.effects,
                vec![CapabilityEffect::Read],
                "cap {} should be read-only",
                cap.id
            );
        }
    }

    #[test]
    fn degraded_source_name_when_open() {
        let c = OpenVikingConnector::new(OpenVikingConfig {
            circuit_breaker_threshold: 1,
            ..Default::default()
        });
        assert!(c.degraded_source_name().is_none());
        c.circuit_breaker.record_failure();
        // After 1 failure with threshold=1, breaker may be Open or HalfOpen
        // depending on cooldown. With default 60s cooldown it's Open.
        assert_eq!(c.degraded_source_name(), Some("openviking-memory"));
    }

    #[test]
    fn runtime_is_native() {
        let c = test_connector();
        assert!(matches!(c.runtime(), ConnectorRuntime::Native));
    }
}
