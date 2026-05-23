//! Unit tests for the DispatchInterceptor architecture.
//!
//! Each interceptor is tested in isolation against a synthetic
//! [`DispatchCtx`] / fake connector. End-to-end coverage through
//! `CapabilityDispatcher` lives in `dispatcher.rs` to keep that test
//! suite as the source of truth for dispatch behavior.

use std::sync::Arc;
use std::time::{Duration, Instant};

use super::sandbox_injection_interceptor::{
    SandboxInjectionInterceptor, HINT_DEV, HINT_ISOLATED, HINT_MINIMAL,
};
use super::truncation_metadata_interceptor::TruncationMetadataInterceptor;
use super::wall_clock_interceptor::WallClockInterceptor;
use super::{DispatchCtx, DispatchInterceptor};
use crate::dispatcher::CapabilityDispatcher;
use crate::registry::ConnectorRegistry;
use crate::runtime::RuntimeMode;
use crate::types::{
    CapabilityExecutionRequest, CapabilityExecutionResult, Connector, ExecutionStatus,
};
use cyberclaw_core::capability::{CapabilityEffect, RiskLevel};
use cyberclaw_core::identity::{ActorRef, ActorType};
use cyberclaw_core::ids::{ActorId, CapabilityId, ConnectorId, ExecutionId, WorkspaceId};
use cyberclaw_core::manifests::{CapabilityContract, CapabilityTimeouts, ConnectorRuntime};
use cyberclaw_core::workspace::{WorkspaceMode, WorkspaceRef};

// ─── Test fixtures ─────────────────────────────────────────────────────────

fn make_contract(id: &str, request_ms: Option<u64>) -> CapabilityContract {
    CapabilityContract {
        id: id.to_string(),
        title: id.to_string(),
        description: None,
        input_schema: "{}".to_string(),
        output_schema: "{}".to_string(),
        risk: RiskLevel::Low,
        effects: vec![CapabilityEffect::Read],
        placement: None,
        timeouts: CapabilityTimeouts { request_ms },
    }
}

fn make_request(capability_id: &str) -> CapabilityExecutionRequest {
    CapabilityExecutionRequest {
        execution_id: ExecutionId::new(),
        trace_id: "interceptor-test".to_string(),
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
        connector_id: ConnectorId::from_string("test-connector".to_string()).unwrap(),
        capability_id: CapabilityId::from_string(capability_id.to_string()).unwrap(),
        input: serde_json::json!({}),
    }
}

fn make_result(output: serde_json::Value) -> CapabilityExecutionResult {
    CapabilityExecutionResult {
        execution_id: ExecutionId::new(),
        trace_id: "interceptor-test".to_string(),
        connector_id: ConnectorId::from_string("test-connector".to_string()).unwrap(),
        capability_id: CapabilityId::from_string("test.cap".to_string()).unwrap(),
        output,
        status: ExecutionStatus::Success,
        error: None,
        actual_runtime: None,
    }
}

// ─── 1. WallClockInterceptor.before sets deadline ──────────────────────────

#[tokio::test]
async fn wall_clock_interceptor_sets_deadline() {
    let interceptor = WallClockInterceptor::new();
    let request = make_request("fs.read");
    let contract = make_contract("fs.read", Some(5_000));
    let mut ctx = DispatchCtx::new(request, contract);
    let before = Instant::now();

    interceptor.before(&mut ctx).await.unwrap();

    let deadline = ctx.deadline.expect("deadline must be set");
    let dur = deadline.saturating_duration_since(before);
    assert!(
        dur >= Duration::from_millis(4_900) && dur <= Duration::from_millis(5_100),
        "deadline should be ~5s out, got {:?}",
        dur
    );
}

#[tokio::test]
async fn wall_clock_interceptor_uses_default_when_contract_silent() {
    let interceptor = WallClockInterceptor::new();
    let request = make_request("fs.read");
    let contract = make_contract("fs.read", None);
    let mut ctx = DispatchCtx::new(request, contract);

    interceptor.before(&mut ctx).await.unwrap();

    let dur = ctx
        .deadline
        .unwrap()
        .saturating_duration_since(ctx.started_at);
    // Default is 120s; allow ±2s for scheduling jitter.
    assert!(
        dur >= Duration::from_secs(118) && dur <= Duration::from_secs(122),
        "default deadline should be ~120s, got {:?}",
        dur
    );
}

// ─── 2. Native runtime now times out (GAP-3 regression guard) ──────────────

/// Fake connector that sleeps far longer than the test wall-clock.
#[derive(Debug)]
struct SlowConnector {
    id: ConnectorId,
}

#[async_trait::async_trait]
impl Connector for SlowConnector {
    fn id(&self) -> &ConnectorId {
        &self.id
    }

    fn runtime(&self) -> ConnectorRuntime {
        ConnectorRuntime::Native
    }

    fn capabilities(&self) -> Vec<CapabilityContract> {
        vec![CapabilityContract {
            id: "slow.cap".to_string(),
            title: "slow".to_string(),
            description: None,
            input_schema: "{}".to_string(),
            output_schema: "{}".to_string(),
            risk: RiskLevel::Low,
            effects: vec![CapabilityEffect::Read],
            placement: None,
            timeouts: CapabilityTimeouts {
                // 100 ms budget — connector sleeps far longer.
                request_ms: Some(100),
            },
        }]
    }

    async fn execute(
        &self,
        request: CapabilityExecutionRequest,
    ) -> anyhow::Result<CapabilityExecutionResult> {
        tokio::time::sleep(Duration::from_secs(200)).await;
        Ok(CapabilityExecutionResult {
            execution_id: request.execution_id,
            trace_id: request.trace_id,
            connector_id: request.connector_id,
            capability_id: request.capability_id,
            output: serde_json::json!({"impossible": true}),
            status: ExecutionStatus::Success,
            error: None,
            actual_runtime: None,
        })
    }
}

#[tokio::test]
async fn wall_clock_native_runtime_now_times_out() {
    let connector_id = ConnectorId::from_string("slow-connector".to_string()).unwrap();
    let cap_id = "slow.cap";

    let registry = Arc::new(ConnectorRegistry::new());
    let mock = Arc::new(SlowConnector {
        id: connector_id.clone(),
    });
    registry.register(mock).unwrap();

    let dispatcher = CapabilityDispatcher::new(registry);
    let request = CapabilityExecutionRequest {
        execution_id: ExecutionId::new(),
        trace_id: "slow-test".to_string(),
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
            root: "/tmp/slow-test".to_string(),
            writable_roots: vec![],
        },
        connector_id,
        capability_id: CapabilityId::from_string(cap_id.to_string()).unwrap(),
        input: serde_json::json!({}),
    };

    let started = Instant::now();
    let result = dispatcher.dispatch(request).await.unwrap();
    let elapsed = started.elapsed();

    assert_eq!(
        result.status,
        ExecutionStatus::Failed,
        "Native dispatch must surface timeout as Failed status"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "Native dispatch must return within ~100ms+overhead, took {:?}",
        elapsed
    );
    let err = result.error.as_deref().unwrap_or("");
    assert!(
        err.to_lowercase().contains("timeout"),
        "Error must mention timeout, got: {err}"
    );
}

// ─── 3. TruncationMetadataInterceptor — marker present ─────────────────────

#[tokio::test]
async fn truncation_metadata_adds_meta_field_when_marker_present() {
    let interceptor = TruncationMetadataInterceptor::new();
    let request = make_request("fs.read");
    let contract = make_contract("fs.read", None);
    let ctx = DispatchCtx::new(request, contract);

    // Output contains the literal truncation marker substring.
    let mut result = make_result(serde_json::json!({
        "stdout": "head text\n...[truncated 12345 bytes]...\ntail text",
        "exit_code": 0
    }));

    interceptor.after(&ctx, &mut result).await;

    let meta = result
        .output
        .get("_meta")
        .expect("_meta must be present after annotation");
    assert_eq!(meta.get("truncated").and_then(|v| v.as_bool()), Some(true));
    assert_eq!(
        meta.get("truncation_marker_present")
            .and_then(|v| v.as_bool()),
        Some(true)
    );
    // Sibling fields preserved at top level (no `original` wrapping for objects).
    assert_eq!(
        result.output.get("exit_code").and_then(|v| v.as_i64()),
        Some(0)
    );
    assert!(result.output.get("stdout").is_some());
}

#[tokio::test]
async fn truncation_metadata_adds_meta_field_when_truncated_field_present() {
    let interceptor = TruncationMetadataInterceptor::new();
    let request = make_request("fs.read");
    let contract = make_contract("fs.read", None);
    let ctx = DispatchCtx::new(request, contract);

    // Dispatcher cap path: {"truncated": true, "original_size_bytes": ...}
    let mut result = make_result(serde_json::json!({
        "truncated": true,
        "original_size_bytes": 500_000,
        "max_size_bytes": 256_000,
        "preview": "xxx"
    }));

    interceptor.after(&ctx, &mut result).await;

    let meta = result
        .output
        .get("_meta")
        .expect("_meta must be present when truncated:true field exists");
    assert_eq!(meta.get("truncated").and_then(|v| v.as_bool()), Some(true));
}

// ─── 4. TruncationMetadataInterceptor — clean payload no-op ────────────────

#[tokio::test]
async fn truncation_metadata_no_op_when_clean() {
    let interceptor = TruncationMetadataInterceptor::new();
    let request = make_request("fs.read");
    let contract = make_contract("fs.read", None);
    let ctx = DispatchCtx::new(request, contract);

    let original = serde_json::json!({
        "content": "hello world",
        "size": 11
    });
    let mut result = make_result(original.clone());

    interceptor.after(&ctx, &mut result).await;

    assert_eq!(
        result.output, original,
        "Clean output must pass through unchanged"
    );
    assert!(result.output.get("_meta").is_none());
}

// ─── 5-7. SandboxInjectionInterceptor — prefix routing ─────────────────────

#[tokio::test]
async fn sandbox_injection_picks_dev_for_cmd_capability() {
    let interceptor = SandboxInjectionInterceptor::new();
    let request = make_request("cmd.run");
    let contract = make_contract("cmd.run", None);
    let mut ctx = DispatchCtx::new(request, contract);

    interceptor.before(&mut ctx).await.unwrap();

    assert_eq!(ctx.sandbox_hint.as_deref(), Some(HINT_DEV));
}

#[tokio::test]
async fn sandbox_injection_picks_isolated_for_web_capability() {
    let interceptor = SandboxInjectionInterceptor::new();
    let request = make_request("web.fetch");
    let contract = make_contract("web.fetch", None);
    let mut ctx = DispatchCtx::new(request, contract);

    interceptor.before(&mut ctx).await.unwrap();

    assert_eq!(ctx.sandbox_hint.as_deref(), Some(HINT_ISOLATED));
}

#[tokio::test]
async fn sandbox_injection_picks_minimal_for_fs_and_search() {
    let interceptor = SandboxInjectionInterceptor::new();

    let mut ctx_fs = DispatchCtx::new(make_request("fs.read"), make_contract("fs.read", None));
    interceptor.before(&mut ctx_fs).await.unwrap();
    assert_eq!(ctx_fs.sandbox_hint.as_deref(), Some(HINT_MINIMAL));

    let mut ctx_search = DispatchCtx::new(
        make_request("search.grep"),
        make_contract("search.grep", None),
    );
    interceptor.before(&mut ctx_search).await.unwrap();
    assert_eq!(ctx_search.sandbox_hint.as_deref(), Some(HINT_MINIMAL));
}

#[tokio::test]
async fn sandbox_injection_no_hint_for_unknown_capability() {
    let interceptor = SandboxInjectionInterceptor::new();
    // mcp.* and browser.* are network-bound but not in our prefix table —
    // they self-isolate via the external process boundary.
    let request = make_request("mcp.list_tools");
    let contract = make_contract("mcp.list_tools", None);
    let mut ctx = DispatchCtx::new(request, contract);

    interceptor.before(&mut ctx).await.unwrap();

    assert!(
        ctx.sandbox_hint.is_none(),
        "Unknown prefix must leave sandbox_hint None, got {:?}",
        ctx.sandbox_hint
    );
}

// Regression: native dispatch path must tag actual_runtime AND honour the
// deadline emitted by WallClockInterceptor.
#[tokio::test]
async fn native_dispatch_tags_runtime_after_timeout_wrapper() {
    // Fast-returning connector: must NOT be timed out by the new wrapper.
    #[derive(Debug)]
    struct FastConnector {
        id: ConnectorId,
    }

    #[async_trait::async_trait]
    impl Connector for FastConnector {
        fn id(&self) -> &ConnectorId {
            &self.id
        }
        fn runtime(&self) -> ConnectorRuntime {
            ConnectorRuntime::Native
        }
        fn capabilities(&self) -> Vec<CapabilityContract> {
            vec![make_contract("fast.cap", Some(5_000))]
        }
        async fn execute(
            &self,
            request: CapabilityExecutionRequest,
        ) -> anyhow::Result<CapabilityExecutionResult> {
            Ok(CapabilityExecutionResult {
                execution_id: request.execution_id,
                trace_id: request.trace_id,
                connector_id: request.connector_id,
                capability_id: request.capability_id,
                output: serde_json::json!({"ok": true}),
                status: ExecutionStatus::Success,
                error: None,
                actual_runtime: None,
            })
        }
    }

    let connector_id = ConnectorId::from_string("fast-connector".to_string()).unwrap();
    let registry = Arc::new(ConnectorRegistry::new());
    let mock = Arc::new(FastConnector {
        id: connector_id.clone(),
    });
    registry.register(mock).unwrap();

    let dispatcher = CapabilityDispatcher::new(registry);
    let request = CapabilityExecutionRequest {
        execution_id: ExecutionId::new(),
        trace_id: "fast-test".to_string(),
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
            root: "/tmp/fast-test".to_string(),
            writable_roots: vec![],
        },
        connector_id,
        capability_id: CapabilityId::from_string("fast.cap".to_string()).unwrap(),
        input: serde_json::json!({}),
    };

    let result = dispatcher.dispatch(request).await.unwrap();
    assert_eq!(result.status, ExecutionStatus::Success);
    assert_eq!(result.actual_runtime, Some(RuntimeMode::Native));
}
