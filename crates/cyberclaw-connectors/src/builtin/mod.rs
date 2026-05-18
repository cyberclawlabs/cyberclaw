//! Builtin capabilities for CyberClaw connectors.
//!
//! Provides eleven capabilities:
//! - `file:edit:replace`                   — single-file string replacement with uniqueness guard
//! - `file:edit:multi`                     — atomic multi-replacement on a single file
//! - `file:edit:patch`                     — apply unified diff to one or more files
//! - `connector:osv:check`                 — dependency vulnerability scan via OSV.dev
//! - `connector:lsp:symbols`               — list file symbols via LSP `documentSymbol`
//! - `connector:lsp:hover`                 — hover info via LSP `textDocument/hover`
//! - `connector:lsp:find_references`       — reference search via LSP `textDocument/references`
//! - `connector:retrieval:pageindex`       — structured BM25 page-index search
//! - `connector:retrieval:vector_search`   — dense-vector similarity search (stub)
//! - `connector:retrieval:graph_query`     — knowledge-graph query (stub)
//! - `connector:retrieval:hybrid`          — weighted merge over the three retrieval tiers
//!
//! These capabilities are designed to be registered on any `Connector` that
//! exposes file-system write access.  Use `capability_contracts()` to obtain
//! the `CapabilityContract` list for registration.

pub mod apply_patch;
pub mod edit;
pub mod lsp;
pub mod multiedit;
pub mod osv_check;
pub mod retrieval;

use cyberclaw_core::manifests::CapabilityContract;
use cyberclaw_core::prelude::*;

/// Return `CapabilityContract` entries for all three builtin file-edit capabilities.
///
/// These should be appended to a connector's capability list and their
/// corresponding `execute` functions called when the matching `capability_id`
/// is dispatched.
pub fn capability_contracts() -> Vec<CapabilityContract> {
    vec![
        CapabilityContract {
            id: "file:edit:replace".to_string(),
            title: "File Edit Replace".to_string(),
            description: Some(
                "Replace text in a file. old_string must appear exactly count_hint times \
                 (default 1). A backup is written before any mutation."
                    .to_string(),
            ),
            input_schema: "FileEditReplaceInput".to_string(),
            output_schema: "FileEditReplaceOutput".to_string(),
            risk: RiskLevel::Medium,
            effects: vec![CapabilityEffect::Write],
            placement: None,
            timeouts: CapabilityTimeouts {
                request_ms: Some(5000),
            },
        },
        CapabilityContract {
            id: "file:edit:multi".to_string(),
            title: "File Edit Multi".to_string(),
            description: Some(
                "Apply a list of old→new replacements atomically to one file. \
                 If any replacement fails, no changes are written."
                    .to_string(),
            ),
            input_schema: "FileEditMultiInput".to_string(),
            output_schema: "FileEditMultiOutput".to_string(),
            risk: RiskLevel::Medium,
            effects: vec![CapabilityEffect::Write],
            placement: None,
            timeouts: CapabilityTimeouts {
                request_ms: Some(5000),
            },
        },
        CapabilityContract {
            id: "file:edit:patch".to_string(),
            title: "File Edit Patch".to_string(),
            description: Some(
                "Apply a unified diff (git-style) to files. Supports modify, create, and \
                 delete operations."
                    .to_string(),
            ),
            input_schema: "FileEditPatchInput".to_string(),
            output_schema: "FileEditPatchOutput".to_string(),
            risk: RiskLevel::Medium,
            effects: vec![CapabilityEffect::Write],
            placement: None,
            timeouts: CapabilityTimeouts {
                request_ms: Some(10000),
            },
        },
        CapabilityContract {
            id: "connector:osv:check".to_string(),
            title: "OSV Vulnerability Check".to_string(),
            description: Some(
                "Scan dependencies for known vulnerabilities via the public OSV.dev API \
                 (https://api.osv.dev/v1/querybatch). Accepts an explicit package list with \
                 ecosystem, or a path to a Cargo.lock / package-lock.json file. Returns a \
                 list of CVE/GHSA findings with severity and fixed-version information. \
                 Returns an empty result with offline=true when the network is unavailable."
                    .to_string(),
            ),
            input_schema: "OsvCheckInput".to_string(),
            output_schema: "OsvCheckOutput".to_string(),
            risk: RiskLevel::Low,
            effects: vec![CapabilityEffect::Read],
            placement: None,
            timeouts: CapabilityTimeouts {
                request_ms: Some(30000),
            },
        },
        CapabilityContract {
            id: "connector:lsp:symbols".to_string(),
            title: "LSP Document Symbols".to_string(),
            description: Some(
                "List all symbols in a file via the Language Server Protocol \
                 `textDocument/documentSymbol` request. Returns a hierarchical tree of \
                 { name, kind, range, children }. Returns `{ available: false, reason }` \
                 when no language server is installed or the spawn fails."
                    .to_string(),
            ),
            input_schema: "LspSymbolsInput".to_string(),
            output_schema: "LspSymbolsOutput".to_string(),
            risk: RiskLevel::Low,
            effects: vec![CapabilityEffect::Read],
            placement: None,
            timeouts: CapabilityTimeouts {
                request_ms: Some(15000),
            },
        },
        CapabilityContract {
            id: "connector:lsp:hover".to_string(),
            title: "LSP Hover".to_string(),
            description: Some(
                "Resolve hover (signature/type/doc) at a given `{file, position}` via the \
                 Language Server Protocol `textDocument/hover` request. Returns `{ markdown, \
                 range }` from the server's MarkupContent. Returns `{ available: false, \
                 reason }` when no language server is installed or the spawn fails."
                    .to_string(),
            ),
            input_schema: "LspHoverInput".to_string(),
            output_schema: "LspHoverOutput".to_string(),
            risk: RiskLevel::Low,
            effects: vec![CapabilityEffect::Read],
            placement: None,
            timeouts: CapabilityTimeouts {
                request_ms: Some(15000),
            },
        },
        CapabilityContract {
            id: "connector:lsp:find_references".to_string(),
            title: "LSP Find References".to_string(),
            description: Some(
                "Find all references to the symbol at a given `{file, position}` via the \
                 Language Server Protocol `textDocument/references` request. Returns a list \
                 of `{ file, range }` locations. Returns `{ available: false, reason }` when \
                 no language server is installed or the spawn fails."
                    .to_string(),
            ),
            input_schema: "LspFindReferencesInput".to_string(),
            output_schema: "LspFindReferencesOutput".to_string(),
            risk: RiskLevel::Low,
            effects: vec![CapabilityEffect::Read],
            placement: None,
            timeouts: CapabilityTimeouts {
                request_ms: Some(30000),
            },
        },
        CapabilityContract {
            id: "connector:retrieval:pageindex".to_string(),
            title: "Retrieval PageIndex".to_string(),
            description: Some(
                "Structured page-index retrieval using Okapi-BM25 scoring over an in-memory \
                 inverted index.  Inputs: `{ query, top_k, docs: [{id, text, metadata}] }`. \
                 Outputs: `{ hits: [{id, score, excerpt, metadata}], mode }` where `mode` is \
                 `\"bm25\"` for a populated corpus or `\"empty\"` when no documents were \
                 supplied.  Read-only, deterministic."
                    .to_string(),
            ),
            input_schema: "PageIndexInput".to_string(),
            output_schema: "PageIndexOutput".to_string(),
            risk: RiskLevel::Low,
            effects: vec![CapabilityEffect::Read],
            placement: None,
            timeouts: CapabilityTimeouts {
                request_ms: Some(5000),
            },
        },
        CapabilityContract {
            id: "connector:retrieval:vector_search".to_string(),
            title: "Retrieval Vector Search".to_string(),
            description: Some(
                "Dense-vector similarity search over a named `collection`.  Inputs: \
                 `{ query, top_k, collection }`.  Outputs: `{ hits, mode, reason }`.  Backend \
                 is selected via `CYBERCLAW_VECTOR_BACKEND` (`qdrant` | `milvus` | \
                 `local_hnsw`).  In Sprint 10 the driver layer is not yet wired so this \
                 capability returns `{ mode: \"not_configured\", reason: ... }`."
                    .to_string(),
            ),
            input_schema: "VectorSearchInput".to_string(),
            output_schema: "VectorSearchOutput".to_string(),
            risk: RiskLevel::Low,
            effects: vec![CapabilityEffect::Read],
            placement: None,
            timeouts: CapabilityTimeouts {
                request_ms: Some(15000),
            },
        },
        CapabilityContract {
            id: "connector:retrieval:graph_query".to_string(),
            title: "Retrieval Graph Query".to_string(),
            description: Some(
                "Knowledge-graph query via Cypher or SPARQL.  Inputs: `{ cypher_or_sparql }`. \
                 Outputs: `{ columns, results: [[value, ...]], mode, reason }`.  Backend \
                 selected via `CYBERCLAW_GRAPH_BACKEND` (`neo4j` | `memgraph` | `none`).  In \
                 Sprint 10 the driver layer is not yet wired so this capability returns \
                 `{ mode: \"not_configured\", reason: ... }`."
                    .to_string(),
            ),
            input_schema: "GraphQueryInput".to_string(),
            output_schema: "GraphQueryOutput".to_string(),
            risk: RiskLevel::Low,
            effects: vec![CapabilityEffect::Read],
            placement: None,
            timeouts: CapabilityTimeouts {
                request_ms: Some(15000),
            },
        },
        CapabilityContract {
            id: "connector:retrieval:hybrid".to_string(),
            title: "Retrieval Hybrid Merge".to_string(),
            description: Some(
                "Layered retrieval that fans out to `pageindex`, `vector_search`, and \
                 `graph_query`, applies per-strategy `weights`, and reranks the union of \
                 hits.  Inputs: `{ query, strategies, top_k, weights, docs, collection }`. \
                 Outputs: `{ hits, strategies_used }`.  Sub-capabilities that are stubbed \
                 out contribute no hits but remain listed in `strategies_used` when they \
                 would have run."
                    .to_string(),
            ),
            input_schema: "HybridInput".to_string(),
            output_schema: "HybridOutput".to_string(),
            risk: RiskLevel::Low,
            effects: vec![CapabilityEffect::Read],
            placement: None,
            timeouts: CapabilityTimeouts {
                request_ms: Some(30000),
            },
        },
    ]
}
