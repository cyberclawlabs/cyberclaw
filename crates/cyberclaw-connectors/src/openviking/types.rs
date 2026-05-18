//! OpenViking connector types.
//!
//! Key design decision: OpenViking's L0/L1/L2 semantics are **reversed** relative
//! to CyberClaw's `MemoryLevel`:
//!
//! | CyberClaw MemoryLevel | OpenViking ContextLevel |
//! |-----------------------|------------------------|
//! | L0 (Full / most detailed) | L2 (Detail) |
//! | L1 (Summary) | L1 (Overview) |
//! | L2 (Metadata / least detailed) | L0 (Abstract) |
//!
//! We use an independent [`OvRetrievalDepth`] enum to avoid accidental misuse.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Retrieval depth (independent of MemoryLevel)
// ---------------------------------------------------------------------------

/// Retrieval depth for OpenViking queries.
///
/// Maps 1:1 to OpenViking's `ContextLevel` (Abstract=0, Overview=1, Detail=2).
/// **Do NOT reuse `MemoryLevel`** — the ordinal direction is reversed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OvRetrievalDepth {
    /// ~100 tokens — abstract / metadata only (OpenViking L0)
    Abstract,
    /// ~2000 tokens — overview with structure (OpenViking L1)
    #[default]
    Overview,
    /// Full text — complete content (OpenViking L2)
    Detail,
}

impl OvRetrievalDepth {
    /// Convert to OpenViking API query parameter value (0, 1, 2).
    pub fn as_api_level(&self) -> u8 {
        match self {
            Self::Abstract => 0,
            Self::Overview => 1,
            Self::Detail => 2,
        }
    }

    /// Timeout hint: Abstract/Overview use fast path, Detail uses slow path.
    pub fn is_fast(&self) -> bool {
        !matches!(self, Self::Detail)
    }
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for connecting to an OpenViking instance via REST API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenVikingConfig {
    /// Base URL of the OpenViking REST API (e.g. "http://localhost:8787")
    #[serde(default = "default_base_url")]
    pub base_url: String,

    /// Default retrieval depth for queries
    #[serde(default)]
    pub default_depth: OvRetrievalDepth,

    /// Request timeout for L0/L1 operations (ms)
    #[serde(default = "default_fast_timeout_ms")]
    pub fast_timeout_ms: u64,

    /// Request timeout for L2 (detail) operations (ms)
    #[serde(default = "default_detail_timeout_ms")]
    pub detail_timeout_ms: u64,

    /// Circuit breaker failure threshold before opening
    #[serde(default = "default_cb_threshold")]
    pub circuit_breaker_threshold: u32,

    /// Circuit breaker cooldown period (ms)
    #[serde(default = "default_cb_cooldown_ms")]
    pub circuit_breaker_cooldown_ms: u64,

    /// Optional API key for authenticated access
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
}

fn default_base_url() -> String {
    "http://localhost:8787".to_string()
}
fn default_fast_timeout_ms() -> u64 {
    300
}
fn default_detail_timeout_ms() -> u64 {
    1000
}
fn default_cb_threshold() -> u32 {
    3
}
fn default_cb_cooldown_ms() -> u64 {
    60_000
}

impl Default for OpenVikingConfig {
    fn default() -> Self {
        Self {
            base_url: default_base_url(),
            default_depth: OvRetrievalDepth::default(),
            fast_timeout_ms: default_fast_timeout_ms(),
            detail_timeout_ms: default_detail_timeout_ms(),
            circuit_breaker_threshold: default_cb_threshold(),
            circuit_breaker_cooldown_ms: default_cb_cooldown_ms(),
            api_key: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Request / Response types — aligned to OpenViking REST API
// ---------------------------------------------------------------------------

/// Input for `openviking.memory.ls`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OvLsInput {
    /// URI path to list (default: "viking://")
    #[serde(default = "default_root_uri")]
    pub path: String,
}

fn default_root_uri() -> String {
    "viking://".to_string()
}

/// Input for `openviking.memory.tree`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OvTreeInput {
    /// URI path for tree view (default: "viking://")
    #[serde(default = "default_root_uri")]
    pub path: String,
    /// Maximum depth to traverse
    #[serde(default)]
    pub max_depth: Option<u32>,
}

/// Input for `openviking.memory.read`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OvReadInput {
    /// Resource URI to read (e.g. "viking://memory/profile")
    pub uri: String,
    /// Retrieval depth
    #[serde(default)]
    pub depth: OvRetrievalDepth,
}

/// Input for `openviking.memory.search`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OvSearchInput {
    /// Natural-language or keyword query
    pub query: String,
    /// Maximum number of results
    #[serde(default = "default_top_k")]
    pub top_k: usize,
    /// Retrieval depth for results
    #[serde(default)]
    pub depth: OvRetrievalDepth,
}

fn default_top_k() -> usize {
    10
}

/// Input for `openviking.memory.find`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OvFindInput {
    /// Pattern to match (glob-like, e.g. "*.rs")
    pub pattern: String,
}

/// Input for `openviking.memory.abstract` (L0 abstract)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OvAbstractInput {
    /// Resource URI
    pub uri: String,
}

/// Input for `openviking.memory.overview` (L1 overview)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OvOverviewInput {
    /// Resource URI
    pub uri: String,
}

/// A single search result from OpenViking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OvSearchResult {
    pub uri: String,
    pub content: String,
    #[serde(default)]
    pub score: f64,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

/// Input for `openviking.health`
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OvHealthInput {}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depth_default_is_overview() {
        assert_eq!(OvRetrievalDepth::default(), OvRetrievalDepth::Overview);
    }

    #[test]
    fn depth_api_level_mapping() {
        assert_eq!(OvRetrievalDepth::Abstract.as_api_level(), 0);
        assert_eq!(OvRetrievalDepth::Overview.as_api_level(), 1);
        assert_eq!(OvRetrievalDepth::Detail.as_api_level(), 2);
    }

    #[test]
    fn depth_is_fast() {
        assert!(OvRetrievalDepth::Abstract.is_fast());
        assert!(OvRetrievalDepth::Overview.is_fast());
        assert!(!OvRetrievalDepth::Detail.is_fast());
    }

    #[test]
    fn config_default_values() {
        let config = OpenVikingConfig::default();
        assert_eq!(config.base_url, "http://localhost:8787");
        assert_eq!(config.fast_timeout_ms, 300);
        assert_eq!(config.detail_timeout_ms, 1000);
        assert_eq!(config.circuit_breaker_threshold, 3);
        assert_eq!(config.circuit_breaker_cooldown_ms, 60_000);
        assert!(config.api_key.is_none());
    }

    #[test]
    fn config_serialization_roundtrip() {
        let config = OpenVikingConfig {
            base_url: "http://ov:9999".to_string(),
            api_key: Some("secret".to_string()),
            ..Default::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: OpenVikingConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.base_url, "http://ov:9999");
        assert_eq!(parsed.api_key.as_deref(), Some("secret"));
    }
}
