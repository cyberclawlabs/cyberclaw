//! Memory Connector — bridges the LLM-side `memory_read` / `memory_write` /
//! `memory_search` tool calls to the server's `LeveledMemoryStore`.
//!
//! Sprint 19 W1 — closes the gap that was the entire reason the memory
//! tools never worked: the LLM facade in `cyberclaw_agent_runtime::
//! builtin_tools` declared `connector_id = "memory"` but no connector
//! by that name was ever registered, so dispatch returned
//! "Connector memory does not support capability memory_read".
//!
//! # Capabilities exposed
//!
//! | Capability ID    | Risk | Effects | Description |
//! |------------------|------|---------|-------------|
//! | `memory_read`    | Low  | Read    | Read a record by `(scope, key)`. |
//! | `memory_write`   | Low  | Write   | Upsert a record with TTL inferred from `scope`. |
//! | `memory_search`  | Low  | Read    | List records matching a `scope` filter, capped at 50. |
//!
//! # Scope mapping
//!
//! The LLM-facing `scope` enum maps to `MemoryLevel`:
//!   - `"session"` → `MemoryLevel::L0Full`   (raw, short TTL)
//!   - `"agent"`   → `MemoryLevel::L1Summary` (summarised, medium TTL)
//!   - `"global"`  → `MemoryLevel::L2Metadata`  (latent, long TTL)
//!
//! # Architectural compliance
//!
//! - §1 Four-object model: this is a `Connector`, not a new platform
//!   object. It bridges an existing capability surface to existing
//!   storage; no new abstractions land.
//! - §3 Execution chain: read/write goes through Capability → Connector
//!   → LeveledMemoryStore (an internal store). The LLM's tool call
//!   already flows through PolicyEngine + dispatcher; this connector
//!   only adds the final hop.

use std::sync::Arc;

use anyhow::anyhow;
use async_trait::async_trait;
use cyberclaw_connectors::types::{
    CapabilityExecutionRequest, CapabilityExecutionResult, Connector, ExecutionStatus,
};
use cyberclaw_core::capability::{CapabilityEffect, RiskLevel};
use cyberclaw_core::facade::{CapabilityFacade, FacadeExposure, ToolsetCategory};
use cyberclaw_core::ids::{CapabilityId, ConnectorId};
use cyberclaw_core::manifests::{CapabilityContract, CapabilityTimeouts, ConnectorRuntime};
use cyberclaw_store::memory_store::{LeveledMemoryRecord, LeveledMemoryStore, MemoryLevel};
use serde_json::Value;

/// Connector that exposes `memory_read` / `memory_write` / `memory_search`
/// capabilities backed by a `LeveledMemoryStore`.
#[derive(Clone)]
pub struct MemoryConnector {
    id: ConnectorId,
    store: Arc<dyn LeveledMemoryStore>,
}

impl std::fmt::Debug for MemoryConnector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryConnector")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl MemoryConnector {
    pub fn new(store: Arc<dyn LeveledMemoryStore>) -> Self {
        Self {
            id: ConnectorId::from_string("memory".to_string())
                .expect("'memory' is a valid connector id"),
            store,
        }
    }
}

#[async_trait]
impl Connector for MemoryConnector {
    fn id(&self) -> &ConnectorId {
        &self.id
    }

    fn runtime(&self) -> ConnectorRuntime {
        ConnectorRuntime::Native
    }

    fn capabilities(&self) -> Vec<CapabilityContract> {
        vec![
            CapabilityContract {
                id: "memory_read".to_string(),
                title: "Read agent memory".to_string(),
                description: Some(
                    "Retrieve a previously stored record by scope+key.".to_string(),
                ),
                input_schema: r#"{"scope":"session|agent|global","key":"string"}"#.to_string(),
                output_schema: r#"{"value":"string|null","level":"string","updated_at":"rfc3339"}"#
                    .to_string(),
                risk: RiskLevel::Low,
                effects: vec![CapabilityEffect::Read],
                placement: None,
                timeouts: CapabilityTimeouts::default(),
            },
            CapabilityContract {
                id: "memory_write".to_string(),
                title: "Write agent memory".to_string(),
                description: Some(
                    "Upsert a record. Scope picks the level (session→L0, agent→L1, global→L2)."
                        .to_string(),
                ),
                input_schema: r#"{"scope":"session|agent|global","key":"string","value":"string"}"#
                    .to_string(),
                output_schema: r#"{"id":"string","level":"string","updated_at":"rfc3339"}"#
                    .to_string(),
                risk: RiskLevel::Low,
                effects: vec![CapabilityEffect::Write],
                placement: None,
                timeouts: CapabilityTimeouts::default(),
            },
            CapabilityContract {
                id: "memory_search".to_string(),
                title: "Search agent memory".to_string(),
                description: Some(
                    "List records in a given scope, optionally filtered by key prefix. Limited to \
                     50 results."
                        .to_string(),
                ),
                input_schema: r#"{"scope":"session|agent|global","key_prefix":"string?"}"#
                    .to_string(),
                output_schema: r#"{"records":[{"id":"string","key":"string","value":"string","level":"string","updated_at":"rfc3339"}]}"#
                    .to_string(),
                risk: RiskLevel::Low,
                effects: vec![CapabilityEffect::Read],
                placement: None,
                timeouts: CapabilityTimeouts::default(),
            },
        ]
    }

    async fn execute(
        &self,
        request: CapabilityExecutionRequest,
    ) -> anyhow::Result<CapabilityExecutionResult> {
        let cap_id = request.capability_id.as_ref().to_string();
        let session_id = request
            .input
            .get("session_id")
            .and_then(Value::as_str)
            .unwrap_or("global")
            .to_string();

        let outcome: anyhow::Result<Value> = match cap_id.as_str() {
            "memory_read" => self.do_read(&session_id, &request.input).await,
            "memory_write" => self.do_write(&session_id, &request.input).await,
            "memory_search" => self.do_search(&session_id, &request.input).await,
            other => Err(anyhow!("memory connector: unknown capability '{}'", other)),
        };

        match outcome {
            Ok(output) => Ok(CapabilityExecutionResult {
                execution_id: request.execution_id,
                trace_id: request.trace_id,
                connector_id: request.connector_id,
                capability_id: request.capability_id,
                output,
                status: ExecutionStatus::Success,
                error: None,
                actual_runtime: None,
            }),
            Err(e) => Ok(CapabilityExecutionResult {
                execution_id: request.execution_id,
                trace_id: request.trace_id,
                connector_id: request.connector_id,
                capability_id: request.capability_id,
                output: serde_json::json!({"error": e.to_string()}),
                status: ExecutionStatus::Failed,
                error: Some(e.to_string()),
                actual_runtime: None,
            }),
        }
    }
}

impl MemoryConnector {
    async fn do_read(&self, session_id: &str, input: &Value) -> anyhow::Result<Value> {
        let key = input
            .get("key")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("missing 'key'"))?;
        let record = self.store.query_by_key(session_id, key).await?;
        Ok(match record {
            Some(r) => serde_json::json!({
                "value": value_as_string(&r.content),
                "level": memory_level_str(r.level),
                "updated_at": r.updated_at.to_rfc3339(),
                "id": r.id,
            }),
            None => serde_json::json!({"value": null}),
        })
    }

    async fn do_write(&self, session_id: &str, input: &Value) -> anyhow::Result<Value> {
        let key = input
            .get("key")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("missing 'key'"))?
            .to_string();
        let value = input
            .get("value")
            .ok_or_else(|| anyhow!("missing 'value'"))?
            .clone();
        let level = scope_to_level(input.get("scope").and_then(Value::as_str));
        let now = chrono::Utc::now();
        let record = LeveledMemoryRecord {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            agent_id: input
                .get("agent_id")
                .and_then(Value::as_str)
                .unwrap_or("default")
                .to_string(),
            level,
            key: key.clone(),
            content: if value.is_string() {
                value.clone()
            } else {
                Value::String(value.to_string())
            },
            created_at: now,
            updated_at: now,
            ttl_seconds: level.default_ttl_seconds(),
            source_execution_id: None,
            embedding: None,
            // BT-09: optional tag list for filtered retrieval.
            tags: input
                .get("tags")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
        };
        self.store.store_leveled(record.clone()).await?;
        Ok(serde_json::json!({
            "id": record.id,
            "level": memory_level_str(level),
            "updated_at": record.updated_at.to_rfc3339(),
        }))
    }

    async fn do_search(&self, session_id: &str, input: &Value) -> anyhow::Result<Value> {
        let level = scope_to_level(input.get("scope").and_then(Value::as_str));
        let key_prefix = input
            .get("key_prefix")
            .and_then(Value::as_str)
            .unwrap_or("");
        let records = self.store.query_by_level(session_id, level).await?;
        let filtered: Vec<Value> = records
            .into_iter()
            .filter(|r| key_prefix.is_empty() || r.key.starts_with(key_prefix))
            .take(50)
            .map(|r| {
                serde_json::json!({
                    "id": r.id,
                    "key": r.key,
                    "value": value_as_string(&r.content),
                    "level": memory_level_str(r.level),
                    "updated_at": r.updated_at.to_rfc3339(),
                })
            })
            .collect();
        Ok(serde_json::json!({"records": filtered}))
    }
}

fn scope_to_level(scope: Option<&str>) -> MemoryLevel {
    match scope.unwrap_or("session") {
        "agent" => MemoryLevel::L1Summary,
        "global" => MemoryLevel::L2Metadata,
        _ => MemoryLevel::L0Full,
    }
}

fn memory_level_str(level: MemoryLevel) -> &'static str {
    match level {
        MemoryLevel::L0Full => "session",
        MemoryLevel::L1Summary => "agent",
        MemoryLevel::L2Metadata => "global",
    }
}

fn value_as_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// §4 — authoritative facade declarations for the leveled memory connector.
///
/// These facades map to the `MemoryConnector` (connector_id="memory") which
/// routes to `LeveledMemoryStore`. The semantic memory connector in
/// `crates/cyberclaw-connectors/src/local/memory.rs` exposes `memory.read` /
/// `memory.write` (file-backed); these facades expose the leveled store via
/// distinct capability IDs to avoid conflicts.
#[allow(dead_code)]
pub fn capability_facades() -> Vec<(CapabilityFacade, ToolsetCategory)> {
    let connector_id = ConnectorId::from_string("memory".to_string()).unwrap();
    vec![
        (
            CapabilityFacade {
                name: "memory_read".to_string(),
                description: "Read from the agent memory store. Retrieve previously stored facts, \
                    context, or conversation summaries by key."
                    .to_string(),
                connector_id: connector_id.clone(),
                capability_id: CapabilityId::from_string("memory_read".to_string()).unwrap(),
                risk_level: RiskLevel::Low,
                effects: vec!["read".to_string()],
                read_only: true,
                destructive: false,
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "key": { "type": "string", "description": "Memory key to read" },
                        "scope": {
                            "type": "string",
                            "enum": ["session", "agent", "global"],
                            "description": "Memory scope to query (default: session)"
                        }
                    },
                    "required": ["key"]
                })),
                exposure: FacadeExposure::LlmDefault,
            },
            ToolsetCategory::Memory,
        ),
        (
            CapabilityFacade {
                name: "memory_write".to_string(),
                description: "Write a value to the agent memory store for later retrieval. \
                    Supports session, agent, and global scopes."
                    .to_string(),
                connector_id: connector_id.clone(),
                capability_id: CapabilityId::from_string("memory_write".to_string()).unwrap(),
                risk_level: RiskLevel::Low,
                effects: vec!["read".to_string()],
                read_only: true,
                destructive: false,
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "key": { "type": "string", "description": "Memory key to write" },
                        "value": { "type": "string", "description": "Value to store" },
                        "scope": {
                            "type": "string",
                            "enum": ["session", "agent", "global"],
                            "description": "Memory scope to write to (default: session)"
                        }
                    },
                    "required": ["key", "value"]
                })),
                exposure: FacadeExposure::LlmDefault,
            },
            ToolsetCategory::Memory,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyberclaw_store::memory_store::InMemoryLeveledStore;

    fn req(cap: &str, input: Value) -> CapabilityExecutionRequest {
        use cyberclaw_core::identity::{ActorRef, ActorType};
        use cyberclaw_core::ids::{ActorId, CapabilityId, ConnectorId, ExecutionId, WorkspaceId};
        use cyberclaw_core::workspace::{WorkspaceMode, WorkspaceRef};

        CapabilityExecutionRequest {
            execution_id: ExecutionId::new(),
            trace_id: "test-trace".to_string(),
            actor: ActorRef {
                id: ActorId::new(),
                actor_type: ActorType::System,
                tenant_id: None,
                home_node_id: None,
                display_name: "test".to_string(),
            },
            workspace: WorkspaceRef {
                id: WorkspaceId::new(),
                mode: WorkspaceMode::Ephemeral,
                materialization_mode: None,
                home_node_id: None,
                backing_store: None,
                root: "/tmp/test".to_string(),
                writable_roots: vec![],
            },
            connector_id: ConnectorId::from_string("memory".to_string()).unwrap(),
            capability_id: CapabilityId::from_string(cap.to_string()).unwrap(),
            input,
        }
    }

    #[tokio::test]
    async fn write_then_read_round_trip() {
        let store = Arc::new(InMemoryLeveledStore::new());
        let conn = MemoryConnector::new(store);

        let write_resp = conn
            .execute(req(
                "memory_write",
                serde_json::json!({
                    "session_id": "sess-1",
                    "key": "remember_me",
                    "value": "the cake is a lie",
                    "scope": "agent"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(write_resp.status, ExecutionStatus::Success);
        assert_eq!(write_resp.output["level"], "agent");

        let read_resp = conn
            .execute(req(
                "memory_read",
                serde_json::json!({
                    "session_id": "sess-1",
                    "key": "remember_me",
                }),
            ))
            .await
            .unwrap();
        assert_eq!(read_resp.status, ExecutionStatus::Success);
        assert_eq!(read_resp.output["value"], "the cake is a lie");
        assert_eq!(read_resp.output["level"], "agent");
    }

    #[tokio::test]
    async fn read_missing_returns_null_value() {
        let store = Arc::new(InMemoryLeveledStore::new());
        let conn = MemoryConnector::new(store);

        let resp = conn
            .execute(req(
                "memory_read",
                serde_json::json!({
                    "session_id": "sess-1",
                    "key": "never_written",
                }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status, ExecutionStatus::Success);
        assert!(resp.output["value"].is_null());
    }

    #[tokio::test]
    async fn search_filters_by_scope_and_prefix() {
        let store = Arc::new(InMemoryLeveledStore::new());
        let conn = MemoryConnector::new(store);
        for (k, scope) in [
            ("alpha_one", "session"),
            ("alpha_two", "session"),
            ("beta_one", "session"),
            ("alpha_global", "global"),
        ] {
            conn.execute(req(
                "memory_write",
                serde_json::json!({
                    "session_id": "sess-1",
                    "key": k,
                    "value": "v",
                    "scope": scope,
                }),
            ))
            .await
            .unwrap();
        }
        let resp = conn
            .execute(req(
                "memory_search",
                serde_json::json!({
                    "session_id": "sess-1",
                    "scope": "session",
                    "key_prefix": "alpha_",
                }),
            ))
            .await
            .unwrap();
        let records = resp.output["records"].as_array().unwrap();
        let keys: Vec<&str> = records.iter().map(|r| r["key"].as_str().unwrap()).collect();
        assert_eq!(keys.len(), 2);
        assert!(keys.contains(&"alpha_one"));
        assert!(keys.contains(&"alpha_two"));
    }

    #[tokio::test]
    async fn unknown_capability_returns_failed() {
        let store = Arc::new(InMemoryLeveledStore::new());
        let conn = MemoryConnector::new(store);

        let resp = conn
            .execute(req("nope", serde_json::json!({})))
            .await
            .unwrap();
        assert_eq!(resp.status, ExecutionStatus::Failed);
        assert!(resp.error.unwrap().contains("unknown capability"));
    }

    #[test]
    fn capabilities_list_has_three_low_risk_entries() {
        let store = Arc::new(InMemoryLeveledStore::new());
        let conn = MemoryConnector::new(store);
        let caps = conn.capabilities();
        assert_eq!(caps.len(), 3);
        let ids: Vec<&str> = caps.iter().map(|c| c.id.as_str()).collect();
        assert!(ids.contains(&"memory_read"));
        assert!(ids.contains(&"memory_write"));
        assert!(ids.contains(&"memory_search"));
        for c in &caps {
            assert_eq!(c.risk, RiskLevel::Low);
        }
    }
}
