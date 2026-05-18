//! Retrieval Connector - Search and ingest documents for retrieval-augmented operations
//!
//! Provides document storage and retrieval capabilities with pluggable backends.
//!
//! ## Capabilities
//!
//! - `retrieval.search`: Search documents by query
//! - `retrieval.ingest`: Ingest documents into the store
//! - `retrieval.delete`: Delete documents by ID

use crate::types::{
    CapabilityExecutionRequest, CapabilityExecutionResult, Connector,
    ExecutionStatus as ConnectorExecutionStatus,
};
use cyberclaw_core::manifests::{CapabilityContract, ConnectorRuntime};
use cyberclaw_core::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, error, info};

// ---------------------------------------------------------------------------
// Supporting types
// ---------------------------------------------------------------------------

/// A document to be stored and retrieved
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    pub id: String,
    pub content: String,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

/// A search result with relevance score
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub id: String,
    pub content: String,
    pub score: f64,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

/// Configuration for the retrieval connector
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalConfig {
    /// Backend type identifier (e.g. "in_memory", "pinecone", "qdrant")
    #[serde(default = "default_backend_type")]
    pub backend_type: String,
    /// Default number of results to return
    #[serde(default = "default_top_k")]
    pub top_k_default: usize,
    /// Maximum allowed results per query
    #[serde(default = "default_max_results")]
    pub max_results: usize,
}

fn default_backend_type() -> String {
    "in_memory".to_string()
}

fn default_top_k() -> usize {
    10
}

fn default_max_results() -> usize {
    100
}

impl Default for RetrievalConfig {
    fn default() -> Self {
        Self {
            backend_type: default_backend_type(),
            top_k_default: default_top_k(),
            max_results: default_max_results(),
        }
    }
}

// ---------------------------------------------------------------------------
// Capability I/O types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalSearchInput {
    pub query: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalSearchOutput {
    pub results: Vec<SearchResult>,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalIngestInput {
    pub documents: Vec<Document>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalIngestOutput {
    pub ids: Vec<String>,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalDeleteInput {
    pub ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalDeleteOutput {
    pub deleted: usize,
}

// ---------------------------------------------------------------------------
// RetrievalBackend trait
// ---------------------------------------------------------------------------

/// Pluggable backend for document storage and retrieval
#[async_trait::async_trait]
pub trait RetrievalBackend: Send + Sync + std::fmt::Debug {
    /// Search for documents matching the query
    async fn search(
        &self,
        query: &str,
        top_k: usize,
        filter: Option<serde_json::Value>,
    ) -> anyhow::Result<Vec<SearchResult>>;

    /// Ingest documents, returning their IDs
    async fn ingest(&self, documents: Vec<Document>) -> anyhow::Result<Vec<String>>;

    /// Delete documents by ID, returning count deleted
    async fn delete(&self, ids: Vec<String>) -> anyhow::Result<usize>;
}

// ---------------------------------------------------------------------------
// InMemoryRetrievalBackend
// ---------------------------------------------------------------------------

/// Simple in-memory backend using keyword matching for testing
#[derive(Debug)]
pub struct InMemoryRetrievalBackend {
    store: tokio::sync::RwLock<HashMap<String, Document>>,
}

impl InMemoryRetrievalBackend {
    /// Create a new empty in-memory backend
    pub fn new() -> Self {
        Self {
            store: tokio::sync::RwLock::new(HashMap::new()),
        }
    }
}

impl Default for InMemoryRetrievalBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl RetrievalBackend for InMemoryRetrievalBackend {
    async fn search(
        &self,
        query: &str,
        top_k: usize,
        filter: Option<serde_json::Value>,
    ) -> anyhow::Result<Vec<SearchResult>> {
        let store = self.store.read().await;
        let query_terms: Vec<&str> = query.split_whitespace().collect();

        if query_terms.is_empty() {
            return Ok(Vec::new());
        }

        let mut results: Vec<SearchResult> = store
            .values()
            .filter(|doc| {
                // Apply metadata filter if provided
                if let Some(ref filter_val) = filter {
                    if let Some(filter_obj) = filter_val.as_object() {
                        for (key, val) in filter_obj {
                            if let Some(expected) = val.as_str() {
                                match doc.metadata.get(key) {
                                    Some(actual) if actual == expected => {}
                                    _ => return false,
                                }
                            }
                        }
                    }
                }
                true
            })
            .filter_map(|doc| {
                let content_lower = doc.content.to_lowercase();
                let match_count = query_terms
                    .iter()
                    .filter(|term| content_lower.contains(&term.to_lowercase()))
                    .count();

                if match_count > 0 {
                    let score = match_count as f64 / query_terms.len() as f64;
                    Some(SearchResult {
                        id: doc.id.clone(),
                        content: doc.content.clone(),
                        score,
                        metadata: doc.metadata.clone(),
                    })
                } else {
                    None
                }
            })
            .collect();

        // Sort by score descending, then by id for stable ordering
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.id.cmp(&b.id))
        });
        results.truncate(top_k);

        Ok(results)
    }

    async fn ingest(&self, documents: Vec<Document>) -> anyhow::Result<Vec<String>> {
        let mut store = self.store.write().await;
        let ids: Vec<String> = documents
            .into_iter()
            .map(|doc| {
                let id = doc.id.clone();
                store.insert(id.clone(), doc);
                id
            })
            .collect();
        Ok(ids)
    }

    async fn delete(&self, ids: Vec<String>) -> anyhow::Result<usize> {
        let mut store = self.store.write().await;
        let mut count = 0;
        for id in ids {
            if store.remove(&id).is_some() {
                count += 1;
            }
        }
        Ok(count)
    }
}

// ---------------------------------------------------------------------------
// RetrievalConnector
// ---------------------------------------------------------------------------

/// Connector providing document retrieval capabilities
#[derive(Debug)]
pub struct RetrievalConnector {
    id: ConnectorId,
    backend: Arc<dyn RetrievalBackend>,
    config: RetrievalConfig,
    capabilities: Vec<CapabilityContract>,
}

impl RetrievalConnector {
    /// Create a new RetrievalConnector with the given backend and config
    pub fn new(
        connector_id: &str,
        backend: Arc<dyn RetrievalBackend>,
        config: RetrievalConfig,
    ) -> Self {
        let id = ConnectorId::from_string(connector_id.to_string())
            .unwrap_or_else(|_| ConnectorId::from_string("retrieval".to_string()).unwrap());
        let capabilities = Self::build_capabilities();

        Self {
            id,
            backend,
            config,
            capabilities,
        }
    }

    /// Create a RetrievalConnector with the default in-memory backend
    pub fn in_memory(connector_id: &str) -> Self {
        Self::new(
            connector_id,
            Arc::new(InMemoryRetrievalBackend::new()),
            RetrievalConfig::default(),
        )
    }

    fn build_capabilities() -> Vec<CapabilityContract> {
        vec![
            CapabilityContract {
                id: "retrieval.search".to_string(),
                title: "Retrieval Search".to_string(),
                description: Some("Search documents by query".to_string()),
                input_schema: "RetrievalSearchInput".to_string(),
                output_schema: "RetrievalSearchOutput".to_string(),
                risk: RiskLevel::Low,
                effects: vec![CapabilityEffect::Read],
                placement: None,
                timeouts: CapabilityTimeouts {
                    request_ms: Some(30000),
                },
            },
            CapabilityContract {
                id: "retrieval.ingest".to_string(),
                title: "Retrieval Ingest".to_string(),
                description: Some("Ingest documents into the retrieval store".to_string()),
                input_schema: "RetrievalIngestInput".to_string(),
                output_schema: "RetrievalIngestOutput".to_string(),
                risk: RiskLevel::Medium,
                effects: vec![CapabilityEffect::Write],
                placement: None,
                timeouts: CapabilityTimeouts {
                    request_ms: Some(60000),
                },
            },
            CapabilityContract {
                id: "retrieval.delete".to_string(),
                title: "Retrieval Delete".to_string(),
                description: Some("Delete documents from the retrieval store".to_string()),
                input_schema: "RetrievalDeleteInput".to_string(),
                output_schema: "RetrievalDeleteOutput".to_string(),
                risk: RiskLevel::Medium,
                effects: vec![CapabilityEffect::Write],
                placement: None,
                timeouts: CapabilityTimeouts {
                    request_ms: Some(30000),
                },
            },
        ]
    }

    async fn handle_search(
        &self,
        input: RetrievalSearchInput,
    ) -> anyhow::Result<serde_json::Value> {
        let top_k = input
            .top_k
            .unwrap_or(self.config.top_k_default)
            .min(self.config.max_results);

        let results = self
            .backend
            .search(&input.query, top_k, input.filter)
            .await?;
        let count = results.len();

        let output = RetrievalSearchOutput { results, count };
        Ok(serde_json::to_value(output)?)
    }

    async fn handle_ingest(
        &self,
        input: RetrievalIngestInput,
    ) -> anyhow::Result<serde_json::Value> {
        let ids = self.backend.ingest(input.documents).await?;
        let count = ids.len();

        let output = RetrievalIngestOutput { ids, count };
        Ok(serde_json::to_value(output)?)
    }

    async fn handle_delete(
        &self,
        input: RetrievalDeleteInput,
    ) -> anyhow::Result<serde_json::Value> {
        let deleted = self.backend.delete(input.ids).await?;

        let output = RetrievalDeleteOutput { deleted };
        Ok(serde_json::to_value(output)?)
    }
}

#[async_trait::async_trait]
impl Connector for RetrievalConnector {
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
            "RetrievalConnector executing capability {} for execution {}",
            request.capability_id, request.execution_id
        );

        let result = match request.capability_id.as_str() {
            "retrieval.search" => {
                let input: RetrievalSearchInput = serde_json::from_value(request.input.clone())?;
                self.handle_search(input).await
            }
            "retrieval.ingest" => {
                let input: RetrievalIngestInput = serde_json::from_value(request.input.clone())?;
                self.handle_ingest(input).await
            }
            "retrieval.delete" => {
                let input: RetrievalDeleteInput = serde_json::from_value(request.input.clone())?;
                self.handle_delete(input).await
            }
            _ => {
                let error_msg = format!("Unknown capability: {}", request.capability_id);
                error!("{}", error_msg);
                return Ok(CapabilityExecutionResult {
                    execution_id: request.execution_id,
                    trace_id: request.trace_id,
                    connector_id: request.connector_id,
                    capability_id: request.capability_id,
                    output: serde_json::json!({ "error": error_msg.clone() }),
                    status: ConnectorExecutionStatus::Failed,
                    error: Some(error_msg),
                    actual_runtime: None,
                });
            }
        };

        match result {
            Ok(output) => {
                info!("Capability {} executed successfully", request.capability_id);
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
                error!("Capability {} failed: {:?}", request.capability_id, e);
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

    fn test_actor() -> ActorRef {
        ActorRef {
            id: ActorId::from_string("test-actor".to_string()).unwrap(),
            actor_type: ActorType::System,
            tenant_id: None,
            home_node_id: None,
            display_name: "Test Actor".to_string(),
        }
    }

    fn test_workspace() -> WorkspaceRef {
        WorkspaceRef {
            id: WorkspaceId::from_string("test-ws".to_string()).unwrap(),
            mode: WorkspaceMode::Ephemeral,
            materialization_mode: None,
            home_node_id: None,
            backing_store: None,
            root: "/tmp/test".to_string(),
            writable_roots: vec![],
        }
    }

    fn test_request(capability: &str, input: serde_json::Value) -> CapabilityExecutionRequest {
        CapabilityExecutionRequest {
            execution_id: ExecutionId::new(),
            trace_id: "test-trace".to_string(),
            actor: test_actor(),
            workspace: test_workspace(),
            connector_id: ConnectorId::from_string("test-retrieval".to_string()).unwrap(),
            capability_id: CapabilityId::from_string(capability.to_string()).unwrap(),
            input,
        }
    }

    fn sample_documents() -> Vec<Document> {
        vec![
            Document {
                id: "doc-1".to_string(),
                content: "Rust is a systems programming language focused on safety".to_string(),
                metadata: HashMap::from([("category".to_string(), "programming".to_string())]),
            },
            Document {
                id: "doc-2".to_string(),
                content: "Python is a high-level programming language".to_string(),
                metadata: HashMap::from([("category".to_string(), "programming".to_string())]),
            },
            Document {
                id: "doc-3".to_string(),
                content: "CyberClaw is a controlled agent platform for developers".to_string(),
                metadata: HashMap::from([("category".to_string(), "platform".to_string())]),
            },
            Document {
                id: "doc-4".to_string(),
                content: "Rust ownership model ensures memory safety without garbage collection"
                    .to_string(),
                metadata: HashMap::from([
                    ("category".to_string(), "programming".to_string()),
                    ("topic".to_string(), "rust".to_string()),
                ]),
            },
        ]
    }

    #[tokio::test]
    async fn test_ingest_and_search() {
        let backend = InMemoryRetrievalBackend::new();
        let ids = backend.ingest(sample_documents()).await.unwrap();
        assert_eq!(ids.len(), 4);

        let results = backend.search("Rust safety", 10, None).await.unwrap();
        assert!(!results.is_empty());
        // doc-1 and doc-4 both mention "Rust" and "safety"
        assert!(results.iter().any(|r| r.id == "doc-1"));
        assert!(results.iter().any(|r| r.id == "doc-4"));
    }

    #[tokio::test]
    async fn test_empty_search_returns_empty() {
        let backend = InMemoryRetrievalBackend::new();
        backend.ingest(sample_documents()).await.unwrap();

        let results = backend.search("", 10, None).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_search_no_match() {
        let backend = InMemoryRetrievalBackend::new();
        backend.ingest(sample_documents()).await.unwrap();

        let results = backend
            .search("quantum computing blockchain", 10, None)
            .await
            .unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_delete_documents() {
        let backend = InMemoryRetrievalBackend::new();
        backend.ingest(sample_documents()).await.unwrap();

        let deleted = backend
            .delete(vec!["doc-1".to_string(), "doc-2".to_string()])
            .await
            .unwrap();
        assert_eq!(deleted, 2);

        // Verify deleted docs no longer appear in search
        let results = backend.search("programming", 10, None).await.unwrap();
        assert!(results.iter().all(|r| r.id != "doc-1" && r.id != "doc-2"));
    }

    #[tokio::test]
    async fn test_delete_nonexistent() {
        let backend = InMemoryRetrievalBackend::new();
        let deleted = backend
            .delete(vec!["nonexistent".to_string()])
            .await
            .unwrap();
        assert_eq!(deleted, 0);
    }

    #[tokio::test]
    async fn test_score_ordering() {
        let backend = InMemoryRetrievalBackend::new();
        backend.ingest(sample_documents()).await.unwrap();

        // "Rust safety" matches 2 terms in doc-1/doc-4, but only "programming" matches 1 term in doc-2
        let results = backend.search("Rust safety", 10, None).await.unwrap();
        assert!(results.len() >= 2);

        // The top results should have score 1.0 (both terms match)
        assert!((results[0].score - 1.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_filter_by_metadata() {
        let backend = InMemoryRetrievalBackend::new();
        backend.ingest(sample_documents()).await.unwrap();

        let filter = serde_json::json!({ "category": "platform" });
        let results = backend
            .search("agent platform", 10, Some(filter))
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "doc-3");
    }

    #[tokio::test]
    async fn test_top_k_limit() {
        let backend = InMemoryRetrievalBackend::new();
        backend.ingest(sample_documents()).await.unwrap();

        let results = backend.search("programming", 2, None).await.unwrap();
        assert!(results.len() <= 2);
    }

    #[tokio::test]
    async fn test_config_defaults() {
        let config = RetrievalConfig::default();
        assert_eq!(config.backend_type, "in_memory");
        assert_eq!(config.top_k_default, 10);
        assert_eq!(config.max_results, 100);
    }

    #[tokio::test]
    async fn test_connector_capabilities() {
        let connector = RetrievalConnector::in_memory("test-retrieval");
        assert_eq!(connector.id().as_str(), "test-retrieval");

        let caps = connector.capabilities();
        assert_eq!(caps.len(), 3);
        assert!(caps.iter().any(|c| c.id == "retrieval.search"));
        assert!(caps.iter().any(|c| c.id == "retrieval.ingest"));
        assert!(caps.iter().any(|c| c.id == "retrieval.delete"));
    }

    #[tokio::test]
    async fn test_connector_execute_ingest_and_search() {
        let connector = RetrievalConnector::in_memory("test-retrieval");

        // Ingest via connector execute
        let ingest_request = test_request(
            "retrieval.ingest",
            serde_json::to_value(RetrievalIngestInput {
                documents: sample_documents(),
            })
            .unwrap(),
        );

        let result = connector.execute(ingest_request).await.unwrap();
        assert_eq!(result.status, ConnectorExecutionStatus::Success);

        let output: RetrievalIngestOutput = serde_json::from_value(result.output).unwrap();
        assert_eq!(output.count, 4);

        // Search via connector execute
        let search_request = test_request(
            "retrieval.search",
            serde_json::json!({
                "query": "Rust programming",
                "top_k": 5
            }),
        );

        let result = connector.execute(search_request).await.unwrap();
        assert_eq!(result.status, ConnectorExecutionStatus::Success);

        let output: RetrievalSearchOutput = serde_json::from_value(result.output).unwrap();
        assert!(!output.results.is_empty());
    }

    #[tokio::test]
    async fn test_connector_unknown_capability() {
        let connector = RetrievalConnector::in_memory("test-retrieval");

        let request = test_request("retrieval.unknown", serde_json::json!({}));

        let result = connector.execute(request).await.unwrap();
        assert_eq!(result.status, ConnectorExecutionStatus::Failed);
        assert!(result.error.is_some());
    }

    #[tokio::test]
    async fn test_search_respects_max_results_config() {
        let backend = Arc::new(InMemoryRetrievalBackend::new());
        let config = RetrievalConfig {
            max_results: 2,
            top_k_default: 10,
            ..Default::default()
        };
        let connector = RetrievalConnector::new("test-retrieval", backend, config);

        // Ingest documents
        let ingest_request = test_request(
            "retrieval.ingest",
            serde_json::to_value(RetrievalIngestInput {
                documents: sample_documents(),
            })
            .unwrap(),
        );
        connector.execute(ingest_request).await.unwrap();

        // Search - top_k(10) should be clamped to max_results(2)
        let search_request = test_request(
            "retrieval.search",
            serde_json::json!({ "query": "programming language" }),
        );

        let result = connector.execute(search_request).await.unwrap();
        let output: RetrievalSearchOutput = serde_json::from_value(result.output).unwrap();
        assert!(output.results.len() <= 2);
    }
}
