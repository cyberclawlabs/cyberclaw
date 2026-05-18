//! MCP type definitions

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

/// MCP Tool definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    /// Tool name (unique identifier)
    pub name: String,
    /// Human-readable description
    pub description: String,
    /// JSON Schema for input parameters
    pub input_schema: serde_json::Value,
}

/// MCP Resource definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResource {
    /// Resource URI
    pub uri: String,
    /// Human-readable name
    pub name: String,
    /// MIME type (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// Resource description (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// MCP Prompt template definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPrompt {
    /// Prompt name
    pub name: String,
    /// Prompt description
    pub description: String,
    /// Argument definitions
    #[serde(default)]
    pub arguments: Vec<McpPromptArgument>,
}

/// MCP Prompt argument
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPromptArgument {
    /// Argument name
    pub name: String,
    /// Argument description
    pub description: String,
    /// Whether the argument is required
    #[serde(default)]
    pub required: bool,
}

/// MCP Server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Server name
    pub name: String,
    /// Transport configuration
    pub transport: TransportConfig,
    /// Request timeout (default: 30s)
    #[serde(default = "default_timeout")]
    pub timeout: Duration,
    /// Enable caching (default: true)
    #[serde(default = "default_true")]
    pub enable_cache: bool,
}

fn default_timeout() -> Duration {
    Duration::from_secs(30)
}

fn default_true() -> bool {
    true
}

/// Transport configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum TransportConfig {
    /// Standard I/O transport
    Stdio {
        /// Command to execute
        command: String,
        /// Command arguments
        #[serde(default)]
        args: Vec<String>,
        /// Working directory
        #[serde(skip_serializing_if = "Option::is_none")]
        workdir: Option<String>,
    },
    /// HTTP/HTTPS transport
    Http {
        /// Server URL
        url: String,
        /// Optional HTTP headers
        #[serde(default)]
        headers: HashMap<String, String>,
    },
}

/// Mapping from MCP Tool/Resource to CyberClaw Capability
#[derive(Debug, Clone)]
pub struct McpCapabilityMapping {
    /// MCP entity type
    pub entity_type: McpEntityType,
    /// MCP entity name/URI
    pub entity_id: String,
    /// Mapped capability ID
    pub capability_id: String,
}

/// MCP entity type
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpEntityType {
    Tool,
    Resource,
    Prompt,
}

/// MCP response cache
#[derive(Debug, Clone)]
pub struct McpCache {
    /// Cache entries (key: resource URI, value: cached response)
    entries: HashMap<String, CachedResponse>,
}

/// Cached MCP response
#[derive(Debug, Clone)]
pub struct CachedResponse {
    /// Response data
    pub data: serde_json::Value,
    /// Timestamp when cached
    pub cached_at: std::time::Instant,
    /// TTL for cache entry
    pub ttl: Duration,
}

impl McpCache {
    /// Create a new empty cache
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Get cached response if valid
    pub fn get(&self, key: &str) -> Option<&serde_json::Value> {
        self.entries.get(key).and_then(|entry| {
            if entry.cached_at.elapsed() < entry.ttl {
                Some(&entry.data)
            } else {
                None
            }
        })
    }

    /// Insert response into cache
    pub fn insert(&mut self, key: String, data: serde_json::Value, ttl: Duration) {
        self.entries.insert(
            key,
            CachedResponse {
                data,
                cached_at: std::time::Instant::now(),
                ttl,
            },
        );
    }

    /// Clear expired entries
    pub fn cleanup(&mut self) {
        self.entries
            .retain(|_, entry| entry.cached_at.elapsed() < entry.ttl);
    }

    /// Clear all cache entries
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

impl Default for McpCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_basic() {
        let mut cache = McpCache::new();
        let data = serde_json::json!({"result": "test"});

        cache.insert("key1".to_string(), data.clone(), Duration::from_secs(60));

        assert_eq!(cache.get("key1"), Some(&data));
        assert_eq!(cache.get("nonexistent"), None);
    }

    #[test]
    fn test_cache_expiry() {
        let mut cache = McpCache::new();
        let data = serde_json::json!({"result": "test"});

        // Insert with very short TTL
        cache.insert("key1".to_string(), data, Duration::from_millis(1));

        // Wait for expiry
        std::thread::sleep(Duration::from_millis(10));

        assert_eq!(cache.get("key1"), None);
    }

    #[test]
    fn test_cache_cleanup() {
        let mut cache = McpCache::new();

        cache.insert(
            "valid".to_string(),
            serde_json::json!({"valid": true}),
            Duration::from_secs(60),
        );
        cache.insert(
            "expired".to_string(),
            serde_json::json!({"expired": true}),
            Duration::from_millis(1),
        );

        std::thread::sleep(Duration::from_millis(10));
        cache.cleanup();

        assert!(cache.get("valid").is_some());
        assert!(cache.get("expired").is_none());
    }
}
