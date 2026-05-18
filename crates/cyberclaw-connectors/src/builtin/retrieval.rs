//! Builtin retrieval capabilities — Sprint 10 L2 layered retrieval stack.
//!
//! Provides four **read-only** capabilities that an agent can dispatch to
//! whichever retrieval tier makes sense for its query:
//!
//! | Capability id                        | Purpose                                |
//! | ------------------------------------ | -------------------------------------- |
//! | `connector:retrieval:pageindex`      | Structured in-memory BM25 search.      |
//! | `connector:retrieval:vector_search`  | Dense-vector similarity (stub in S10). |
//! | `connector:retrieval:graph_query`    | Graph query over Cypher/SPARQL (stub). |
//! | `connector:retrieval:hybrid`         | Weighted merge of the three above.     |
//!
//! # Design rationale
//! External knowledge retrieval is a `Connector -> Capability` concern; the
//! architecture explicitly forbids dumping it inside `Memory Core`.  This
//! module therefore keeps all retrieval plumbing behind capability IDs and
//! leaves `Memory Core` to deal with execution history / provenance.
//!
//! # Sprint 10 scope
//! - `pageindex` is fully implemented on top of an in-memory inverted index
//!   with classic Okapi-BM25 scoring (`k1 = 1.2`, `b = 0.75`) — no new crate
//!   dependencies are introduced.
//! - `vector_search` and `graph_query` return a well-shaped `not_configured`
//!   stub so that downstream code (governance, hybrid merge, tests) can
//!   exercise the full capability contract without an external backend.
//! - `hybrid` fans out to the three sub-capabilities, applies per-strategy
//!   weights, and reranks the merged hit list.
//!
//! # Back-end selection (future work)
//! The stubs read `CYBERCLAW_VECTOR_BACKEND` (`qdrant` | `milvus` | `local_hnsw`)
//! and `CYBERCLAW_GRAPH_BACKEND` (`neo4j` | `memgraph` | `none`) from the
//! process environment.  In this sprint the *only* supported values for those
//! env vars are `"none"` or unset, which yields the stub result.  Wiring real
//! backends is deferred to Sprint 11+ and must happen without changing the
//! capability contract.
//!
//! # Governance
//! All four capabilities advertise `RiskLevel::Low` + `CapabilityEffect::Read`.
//! They are *not* added to the `DangerousCapabilityFilter` default deny list —
//! read-only retrieval never mutates state.

use crate::types::CapabilityExecutionRequest;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use tracing::debug;

// ---------------------------------------------------------------------------
// Page-index (BM25) — input / output / document types
// ---------------------------------------------------------------------------

/// One indexed document for the in-memory BM25 page-index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageIndexDoc {
    /// Unique document identifier.
    pub id: String,
    /// Full text to be tokenised.
    pub text: String,
    /// Optional structured metadata attached to each hit.
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

/// Input for `connector:retrieval:pageindex`.
///
/// `docs` is optional; when omitted the capability answers from whatever
/// index the caller previously built (Sprint 10 defaults to a request-scoped
/// index rebuilt from `docs` each call, so `docs` is normally provided).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageIndexInput {
    /// Free-form natural-language query string.
    pub query: String,
    /// Max number of hits to return (capped at 100 internally).
    #[serde(default = "default_top_k")]
    pub top_k: usize,
    /// Documents to index for this call.  When empty/missing the capability
    /// returns `mode = "empty"`.
    #[serde(default)]
    pub docs: Vec<PageIndexDoc>,
}

fn default_top_k() -> usize {
    10
}

/// One hit returned from a retrieval capability.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RetrievalHit {
    /// Document identifier.
    pub id: String,
    /// Relevance score — semantics depend on the capability.
    pub score: f64,
    /// Short text excerpt (first ~200 chars of the document).
    pub excerpt: String,
    /// Arbitrary metadata copied through from the indexed document.
    #[serde(default)]
    pub metadata: HashMap<String, String>,
    /// Which sub-retriever produced the hit (populated by `hybrid`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// Output for `connector:retrieval:pageindex`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageIndexOutput {
    pub hits: Vec<RetrievalHit>,
    /// `"bm25"` when the query ran, `"empty"` when there was nothing to index.
    pub mode: String,
}

// ---------------------------------------------------------------------------
// Vector search — input / output
// ---------------------------------------------------------------------------

/// Input for `connector:retrieval:vector_search`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorSearchInput {
    pub query: String,
    #[serde(default = "default_top_k")]
    pub top_k: usize,
    /// Logical collection / namespace name in the underlying vector DB.
    #[serde(default = "default_collection")]
    pub collection: String,
}

fn default_collection() -> String {
    "default".to_string()
}

/// Output for `connector:retrieval:vector_search`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorSearchOutput {
    pub hits: Vec<RetrievalHit>,
    /// `"qdrant" | "milvus" | "local_hnsw" | "not_configured"`.
    pub mode: String,
    /// Human-readable explanation when `mode == "not_configured"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

// ---------------------------------------------------------------------------
// Graph query — input / output
// ---------------------------------------------------------------------------

/// Input for `connector:retrieval:graph_query`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphQueryInput {
    /// Raw Cypher (Neo4j/Memgraph) or SPARQL string.  The connector picks the
    /// dialect based on `CYBERCLAW_GRAPH_BACKEND`.
    pub cypher_or_sparql: String,
}

/// Output for `connector:retrieval:graph_query`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphQueryOutput {
    /// Column names (tabular result shape).
    pub columns: Vec<String>,
    /// Row-major values; each inner vec has the same length as `columns`.
    pub results: Vec<Vec<Value>>,
    /// `"neo4j" | "memgraph" | "not_configured"`.
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

// ---------------------------------------------------------------------------
// Hybrid merge — input / output
// ---------------------------------------------------------------------------

/// Input for `connector:retrieval:hybrid`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HybridInput {
    pub query: String,
    /// Subset of `["pageindex", "vector", "graph"]`.  Order is irrelevant.
    #[serde(default = "default_strategies")]
    pub strategies: Vec<String>,
    #[serde(default = "default_top_k")]
    pub top_k: usize,
    /// Per-strategy weight multiplier used during rerank.  Missing entries
    /// default to 1.0.  Keys: `"pageindex"`, `"vector"`, `"graph"`.
    #[serde(default)]
    pub weights: HashMap<String, f64>,
    /// Optional docs passed through to the pageindex sub-capability.
    #[serde(default)]
    pub docs: Vec<PageIndexDoc>,
    /// Optional collection forwarded to vector_search.
    #[serde(default = "default_collection")]
    pub collection: String,
}

fn default_strategies() -> Vec<String> {
    vec![
        "pageindex".to_string(),
        "vector".to_string(),
        "graph".to_string(),
    ]
}

/// Output for `connector:retrieval:hybrid`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HybridOutput {
    pub hits: Vec<RetrievalHit>,
    /// List of strategies that actually contributed at least one considered
    /// hit (useful for debugging when stubs are wired in).
    pub strategies_used: Vec<String>,
}

// ---------------------------------------------------------------------------
// BM25 implementation
// ---------------------------------------------------------------------------

/// Okapi BM25 parameters (hard-coded defaults suitable for prose).
const BM25_K1: f64 = 1.2;
const BM25_B: f64 = 0.75;

/// Tokenise a string into lowercase alphanumeric terms.
///
/// Unicode letters/digits are preserved; any other character (punctuation,
/// whitespace, symbols) is treated as a separator.  Empty tokens are dropped.
fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_lowercase())
        .collect()
}

/// Return a short excerpt of `text` capped at 200 chars (Unicode safe).
fn excerpt(text: &str) -> String {
    const MAX: usize = 200;
    if text.chars().count() <= MAX {
        return text.to_string();
    }
    let mut out = String::with_capacity(MAX + 1);
    for ch in text.chars().take(MAX) {
        out.push(ch);
    }
    out.push('…');
    out
}

/// Score every doc in `docs` against `query` using Okapi-BM25 and return
/// the top `top_k` hits ordered by descending score.
fn bm25_score(query: &str, docs: &[PageIndexDoc], top_k: usize) -> Vec<RetrievalHit> {
    if docs.is_empty() {
        return Vec::new();
    }
    let query_terms = tokenize(query);
    if query_terms.is_empty() {
        return Vec::new();
    }

    // 1) Tokenise corpus once.
    let tokenised: Vec<Vec<String>> = docs.iter().map(|d| tokenize(&d.text)).collect();

    // 2) Term-frequency per doc + doc-frequency per term.
    let mut doc_lens: Vec<f64> = Vec::with_capacity(docs.len());
    let mut tfs: Vec<HashMap<String, u32>> = Vec::with_capacity(docs.len());
    let mut df: HashMap<String, u32> = HashMap::new();
    for terms in &tokenised {
        doc_lens.push(terms.len() as f64);
        let mut tf: HashMap<String, u32> = HashMap::new();
        for t in terms {
            *tf.entry(t.clone()).or_insert(0) += 1;
        }
        for t in tf.keys() {
            *df.entry(t.clone()).or_insert(0) += 1;
        }
        tfs.push(tf);
    }

    let n_docs = docs.len() as f64;
    let avg_dl = doc_lens.iter().sum::<f64>() / n_docs;
    // Guard: all-empty corpus would otherwise divide by zero downstream.
    let avg_dl = if avg_dl == 0.0 { 1.0 } else { avg_dl };

    // 3) Score every doc.
    let mut scored: Vec<(usize, f64)> = Vec::with_capacity(docs.len());
    for (i, tf) in tfs.iter().enumerate() {
        let dl = doc_lens[i];
        let mut score = 0.0_f64;
        for term in &query_terms {
            let n_q = df.get(term).copied().unwrap_or(0) as f64;
            if n_q == 0.0 {
                continue;
            }
            let f_q = tf.get(term).copied().unwrap_or(0) as f64;
            if f_q == 0.0 {
                continue;
            }
            // Okapi IDF with +1 smoothing (always positive).
            let idf = (((n_docs - n_q + 0.5) / (n_q + 0.5)) + 1.0).ln();
            let denom = f_q + BM25_K1 * (1.0 - BM25_B + BM25_B * (dl / avg_dl));
            let contribution = idf * (f_q * (BM25_K1 + 1.0)) / denom;
            score += contribution;
        }
        if score > 0.0 {
            scored.push((i, score));
        }
    }

    // 4) Sort & truncate.
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| docs[a.0].id.cmp(&docs[b.0].id))
    });
    scored.truncate(top_k.min(100));

    scored
        .into_iter()
        .map(|(i, score)| RetrievalHit {
            id: docs[i].id.clone(),
            score,
            excerpt: excerpt(&docs[i].text),
            metadata: docs[i].metadata.clone(),
            source: None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Capability entry points
// ---------------------------------------------------------------------------

/// Execute `connector:retrieval:pageindex`.
pub fn execute_pageindex(
    _workspace: &std::path::Path,
    request: CapabilityExecutionRequest,
) -> anyhow::Result<Value> {
    let input: PageIndexInput = serde_json::from_value(request.input)?;
    debug!(
        "connector:retrieval:pageindex query='{}' docs={} top_k={}",
        input.query,
        input.docs.len(),
        input.top_k
    );

    if input.docs.is_empty() {
        return Ok(serde_json::to_value(PageIndexOutput {
            hits: vec![],
            mode: "empty".to_string(),
        })?);
    }

    let hits = bm25_score(&input.query, &input.docs, input.top_k);
    Ok(serde_json::to_value(PageIndexOutput {
        hits,
        mode: "bm25".to_string(),
    })?)
}

/// Execute `connector:retrieval:vector_search` (Sprint 10 stub).
pub fn execute_vector_search(
    _workspace: &std::path::Path,
    request: CapabilityExecutionRequest,
) -> anyhow::Result<Value> {
    let input: VectorSearchInput = serde_json::from_value(request.input)?;
    debug!(
        "connector:retrieval:vector_search query='{}' collection={} top_k={}",
        input.query, input.collection, input.top_k
    );

    let backend = std::env::var("CYBERCLAW_VECTOR_BACKEND").unwrap_or_else(|_| "none".to_string());
    match backend.as_str() {
        "qdrant" | "milvus" | "local_hnsw" => {
            // Real backends not wired in Sprint 10 — fall through to stub but
            // surface the configured backend name for observability.
            Ok(serde_json::to_value(VectorSearchOutput {
                hits: vec![],
                mode: "not_configured".to_string(),
                reason: Some(format!(
                    "vector backend '{}' selected but driver not compiled in Sprint 10",
                    backend
                )),
            })?)
        }
        _ => Ok(serde_json::to_value(VectorSearchOutput {
            hits: vec![],
            mode: "not_configured".to_string(),
            reason: Some("vector backend not wired".to_string()),
        })?),
    }
}

/// Execute `connector:retrieval:graph_query` (Sprint 10 stub).
pub fn execute_graph_query(
    _workspace: &std::path::Path,
    request: CapabilityExecutionRequest,
) -> anyhow::Result<Value> {
    let input: GraphQueryInput = serde_json::from_value(request.input)?;
    debug!(
        "connector:retrieval:graph_query bytes={}",
        input.cypher_or_sparql.len()
    );

    let backend = std::env::var("CYBERCLAW_GRAPH_BACKEND").unwrap_or_else(|_| "none".to_string());
    match backend.as_str() {
        "neo4j" | "memgraph" => Ok(serde_json::to_value(GraphQueryOutput {
            columns: vec![],
            results: vec![],
            mode: "not_configured".to_string(),
            reason: Some(format!(
                "graph backend '{}' selected but driver not compiled in Sprint 10",
                backend
            )),
        })?),
        _ => Ok(serde_json::to_value(GraphQueryOutput {
            columns: vec![],
            results: vec![],
            mode: "not_configured".to_string(),
            reason: Some("graph backend not wired".to_string()),
        })?),
    }
}

/// Execute `connector:retrieval:hybrid`.
///
/// Calls each requested sub-capability in turn (they are cheap pure-Rust
/// computations in Sprint 10 — no async I/O) and returns a weighted rerank.
pub fn execute_hybrid(
    workspace: &std::path::Path,
    request: CapabilityExecutionRequest,
) -> anyhow::Result<Value> {
    // Destructure so we can both deserialise the input and reuse the
    // surrounding identity fields for sub-capability calls.
    let CapabilityExecutionRequest {
        execution_id,
        trace_id,
        actor,
        workspace: ws_ref,
        connector_id,
        capability_id,
        input: raw_input,
    } = request;
    let input: HybridInput = serde_json::from_value(raw_input)?;
    debug!(
        "connector:retrieval:hybrid query='{}' strategies={:?} top_k={}",
        input.query, input.strategies, input.top_k
    );

    let base = CapabilityExecutionRequest {
        execution_id,
        trace_id,
        actor,
        workspace: ws_ref,
        connector_id,
        capability_id,
        input: Value::Null,
    };

    let mut merged: Vec<RetrievalHit> = Vec::new();
    let mut used: Vec<String> = Vec::new();

    for strategy in &input.strategies {
        let weight = input.weights.get(strategy).copied().unwrap_or(1.0);
        match strategy.as_str() {
            "pageindex" => {
                let sub_input = serde_json::to_value(PageIndexInput {
                    query: input.query.clone(),
                    top_k: input.top_k,
                    docs: input.docs.clone(),
                })?;
                let sub_req = clone_request_with_input(&base, sub_input);
                let v = execute_pageindex(workspace, sub_req)?;
                let out: PageIndexOutput = serde_json::from_value(v)?;
                if !out.hits.is_empty() {
                    used.push("pageindex".to_string());
                }
                for mut h in out.hits {
                    h.score *= weight;
                    h.source = Some("pageindex".to_string());
                    merged.push(h);
                }
            }
            "vector" => {
                let sub_input = serde_json::to_value(VectorSearchInput {
                    query: input.query.clone(),
                    top_k: input.top_k,
                    collection: input.collection.clone(),
                })?;
                let sub_req = clone_request_with_input(&base, sub_input);
                let v = execute_vector_search(workspace, sub_req)?;
                let out: VectorSearchOutput = serde_json::from_value(v)?;
                if !out.hits.is_empty() {
                    used.push("vector".to_string());
                }
                for mut h in out.hits {
                    h.score *= weight;
                    h.source = Some("vector".to_string());
                    merged.push(h);
                }
            }
            "graph" => {
                let sub_input = serde_json::to_value(GraphQueryInput {
                    cypher_or_sparql: input.query.clone(),
                })?;
                let sub_req = clone_request_with_input(&base, sub_input);
                let v = execute_graph_query(workspace, sub_req)?;
                let _: GraphQueryOutput = serde_json::from_value(v)?;
                // Graph sub-capability returns tabular rows, not hits — in
                // Sprint 10 the stub produces no hits, so nothing to merge.
                // Future work: project `columns`/`results` into
                // RetrievalHit entries via a projection config.
            }
            other => {
                debug!(
                    "connector:retrieval:hybrid: ignoring unknown strategy '{}'",
                    other
                );
            }
        }
    }

    // Dedupe by id keeping the *maximum* weighted score.
    let mut best: HashMap<String, RetrievalHit> = HashMap::new();
    for hit in merged {
        best.entry(hit.id.clone())
            .and_modify(|existing| {
                if hit.score > existing.score {
                    *existing = hit.clone();
                }
            })
            .or_insert(hit);
    }

    let mut final_hits: Vec<RetrievalHit> = best.into_values().collect();
    final_hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.id.cmp(&b.id))
    });
    final_hits.truncate(input.top_k.min(100));

    Ok(serde_json::to_value(HybridOutput {
        hits: final_hits,
        strategies_used: used,
    })?)
}

/// Clone the incoming capability execution request, replacing only the
/// `input` field.  Lets `hybrid` reuse the caller's actor/workspace/trace
/// identity when fanning out to sub-capabilities.
fn clone_request_with_input(
    base: &CapabilityExecutionRequest,
    input: Value,
) -> CapabilityExecutionRequest {
    CapabilityExecutionRequest {
        execution_id: base.execution_id.clone(),
        trace_id: base.trace_id.clone(),
        actor: base.actor.clone(),
        workspace: base.workspace.clone(),
        connector_id: base.connector_id.clone(),
        capability_id: base.capability_id.clone(),
        input,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use cyberclaw_core::prelude::*;

    fn make_req(input: Value) -> CapabilityExecutionRequest {
        CapabilityExecutionRequest {
            execution_id: ExecutionId::new(),
            trace_id: "test-trace".to_string(),
            actor: ActorRef {
                id: ActorId::from_string("test-actor".to_string()).unwrap(),
                actor_type: ActorType::System,
                tenant_id: None,
                home_node_id: None,
                display_name: "Test".to_string(),
            },
            workspace: WorkspaceRef {
                id: WorkspaceId::from_string("test-ws".to_string()).unwrap(),
                mode: WorkspaceMode::Ephemeral,
                materialization_mode: None,
                home_node_id: None,
                backing_store: None,
                root: "/tmp/test".to_string(),
                writable_roots: vec![],
            },
            connector_id: ConnectorId::from_string("test-retrieval".to_string()).unwrap(),
            capability_id: CapabilityId::from_string("connector:retrieval:pageindex".to_string())
                .unwrap(),
            input,
        }
    }

    fn sample_docs() -> Vec<PageIndexDoc> {
        vec![
            PageIndexDoc {
                id: "d1".to_string(),
                text: "Rust is a systems programming language focused on memory safety".to_string(),
                metadata: HashMap::from([("lang".to_string(), "en".to_string())]),
            },
            PageIndexDoc {
                id: "d2".to_string(),
                text: "Python is a high-level interpreted programming language".to_string(),
                metadata: HashMap::new(),
            },
            PageIndexDoc {
                id: "d3".to_string(),
                text: "CyberClaw is a controlled agent platform built in Rust".to_string(),
                metadata: HashMap::new(),
            },
            PageIndexDoc {
                id: "d4".to_string(),
                text: "The Rust ownership model prevents data races at compile time, Rust Rust"
                    .to_string(),
                metadata: HashMap::new(),
            },
        ]
    }

    // -----------------------------------------------------------------------
    // pageindex
    // -----------------------------------------------------------------------

    #[test]
    fn pageindex_bm25_scores_hits_by_term_frequency() {
        let req = make_req(
            serde_json::to_value(PageIndexInput {
                query: "rust".to_string(),
                top_k: 10,
                docs: sample_docs(),
            })
            .unwrap(),
        );
        let v = execute_pageindex(std::path::Path::new("/tmp"), req).unwrap();
        let out: PageIndexOutput = serde_json::from_value(v).unwrap();

        assert_eq!(out.mode, "bm25");
        // d1, d3, d4 all mention Rust; d2 does not.
        let ids: Vec<&str> = out.hits.iter().map(|h| h.id.as_str()).collect();
        assert!(ids.contains(&"d1"));
        assert!(ids.contains(&"d3"));
        assert!(ids.contains(&"d4"));
        assert!(!ids.contains(&"d2"));

        // d4 mentions "Rust" three times and is the shortest — should outrank
        // both d1 and d3 under BM25.
        assert_eq!(out.hits[0].id, "d4");

        // Scores are strictly descending.
        for w in out.hits.windows(2) {
            assert!(w[0].score >= w[1].score);
        }

        // Metadata flows through.
        let d1 = out.hits.iter().find(|h| h.id == "d1").unwrap();
        assert_eq!(d1.metadata.get("lang"), Some(&"en".to_string()));
    }

    #[test]
    fn pageindex_empty_index_returns_empty() {
        let req = make_req(
            serde_json::to_value(PageIndexInput {
                query: "rust".to_string(),
                top_k: 5,
                docs: vec![],
            })
            .unwrap(),
        );
        let v = execute_pageindex(std::path::Path::new("/tmp"), req).unwrap();
        let out: PageIndexOutput = serde_json::from_value(v).unwrap();
        assert_eq!(out.mode, "empty");
        assert!(out.hits.is_empty());
    }

    #[test]
    fn pageindex_empty_query_returns_no_hits() {
        let req = make_req(
            serde_json::to_value(PageIndexInput {
                query: "   ".to_string(),
                top_k: 5,
                docs: sample_docs(),
            })
            .unwrap(),
        );
        let v = execute_pageindex(std::path::Path::new("/tmp"), req).unwrap();
        let out: PageIndexOutput = serde_json::from_value(v).unwrap();
        // Corpus exists, so mode is "bm25" but score_list is empty.
        assert_eq!(out.mode, "bm25");
        assert!(out.hits.is_empty());
    }

    // -----------------------------------------------------------------------
    // vector_search
    // -----------------------------------------------------------------------

    #[test]
    fn vector_not_configured_returns_stub() {
        // Remove any ambient env var so the test is hermetic.
        // SAFETY: tests in the same crate must not mutate this env var concurrently.
        std::env::remove_var("CYBERCLAW_VECTOR_BACKEND");

        let req = make_req(
            serde_json::to_value(VectorSearchInput {
                query: "rust".to_string(),
                top_k: 5,
                collection: "default".to_string(),
            })
            .unwrap(),
        );
        let v = execute_vector_search(std::path::Path::new("/tmp"), req).unwrap();
        let out: VectorSearchOutput = serde_json::from_value(v).unwrap();

        assert_eq!(out.mode, "not_configured");
        assert!(out.hits.is_empty());
        assert!(out.reason.is_some());
    }

    // -----------------------------------------------------------------------
    // graph_query
    // -----------------------------------------------------------------------

    #[test]
    fn graph_not_configured_returns_stub() {
        std::env::remove_var("CYBERCLAW_GRAPH_BACKEND");

        let req = make_req(
            serde_json::to_value(GraphQueryInput {
                cypher_or_sparql: "MATCH (n) RETURN n LIMIT 10".to_string(),
            })
            .unwrap(),
        );
        let v = execute_graph_query(std::path::Path::new("/tmp"), req).unwrap();
        let out: GraphQueryOutput = serde_json::from_value(v).unwrap();

        assert_eq!(out.mode, "not_configured");
        assert!(out.results.is_empty());
        assert!(out.columns.is_empty());
        assert!(out.reason.is_some());
    }

    // -----------------------------------------------------------------------
    // hybrid
    // -----------------------------------------------------------------------

    #[test]
    fn hybrid_merge_combines_three_sources() {
        std::env::remove_var("CYBERCLAW_VECTOR_BACKEND");
        std::env::remove_var("CYBERCLAW_GRAPH_BACKEND");

        let req = make_req(
            serde_json::to_value(HybridInput {
                query: "rust".to_string(),
                strategies: default_strategies(),
                top_k: 10,
                weights: HashMap::new(),
                docs: sample_docs(),
                collection: "default".to_string(),
            })
            .unwrap(),
        );
        let v = execute_hybrid(std::path::Path::new("/tmp"), req).unwrap();
        let out: HybridOutput = serde_json::from_value(v).unwrap();

        // Only pageindex has real hits in Sprint 10; vector+graph are stubs.
        assert_eq!(out.strategies_used, vec!["pageindex".to_string()]);
        assert!(!out.hits.is_empty());
        // All merged hits must be tagged with their source retriever.
        for h in &out.hits {
            assert_eq!(h.source.as_deref(), Some("pageindex"));
        }
    }

    #[test]
    fn hybrid_weighted_rerank() {
        std::env::remove_var("CYBERCLAW_VECTOR_BACKEND");
        std::env::remove_var("CYBERCLAW_GRAPH_BACKEND");

        // Run once with weight = 1.0, once with weight = 5.0 for pageindex.
        let base_req = make_req(
            serde_json::to_value(HybridInput {
                query: "rust".to_string(),
                strategies: vec!["pageindex".to_string()],
                top_k: 10,
                weights: HashMap::new(),
                docs: sample_docs(),
                collection: "default".to_string(),
            })
            .unwrap(),
        );
        let v = execute_hybrid(std::path::Path::new("/tmp"), base_req).unwrap();
        let base: HybridOutput = serde_json::from_value(v).unwrap();
        let base_top_score = base.hits[0].score;

        let weighted_req = make_req(
            serde_json::to_value(HybridInput {
                query: "rust".to_string(),
                strategies: vec!["pageindex".to_string()],
                top_k: 10,
                weights: HashMap::from([("pageindex".to_string(), 5.0)]),
                docs: sample_docs(),
                collection: "default".to_string(),
            })
            .unwrap(),
        );
        let v = execute_hybrid(std::path::Path::new("/tmp"), weighted_req).unwrap();
        let weighted: HybridOutput = serde_json::from_value(v).unwrap();
        let weighted_top_score = weighted.hits[0].score;

        // Same ordering, scaled score.
        assert_eq!(base.hits[0].id, weighted.hits[0].id);
        assert!((weighted_top_score - base_top_score * 5.0).abs() < 1e-9);
    }
}
