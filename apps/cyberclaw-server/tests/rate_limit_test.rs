#![allow(clippy::duplicate_mod)]
//! 速率限制和请求体大小限制测试
//!
//! 测试 API 的速率限制和请求体大小限制功能

#[path = "common/mod.rs"]
mod common;
use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use std::sync::Arc;
use tower::ServiceExt;

use cyberclaw_server::{create_router_with_config, AppState, ServerConfig};

use cyberclaw_connectors::registry::ConnectorRegistry;
use cyberclaw_control_plane::{
    event_bus::InMemoryEventBus, execution_service::InMemoryExecutionService,
    gateway_router::InMemoryGatewayRouter, lease_manager::InMemoryLeaseManager,
    membership_service::InMemoryMembershipService, orchestrator::ControlPlaneOrchestrator,
    placement_engine::InMemoryPlacementEngine, registry::InMemoryRegistry,
    resolver::InMemoryResolver, review_queue::InMemoryReviewQueue,
    task_manager::InMemoryTaskManager,
};
use cyberclaw_governance::engine::DefaultPolicyEngine;
use cyberclaw_llm::prelude::*;
use cyberclaw_observability::events::InMemoryEventRecorder;
use cyberclaw_observability::security_event_store::InMemorySecurityEventStore;

/// 创建测试用的应用状态
fn create_test_state() -> Arc<AppState> {
    // 创建 mock LLM 客户端
    let llm_client = Arc::new(
        GenericOpenAiClient::new(Some("test-key".to_string()), "http://localhost:11434/v1")
            .expect("Failed to create test LLM client"),
    );

    let connector_registry = Arc::new(ConnectorRegistry::new());
    let policy_engine = Arc::new(DefaultPolicyEngine::default());
    let event_recorder = Arc::new(InMemoryEventRecorder::new());
    let jwt_secret = "test-secret-key".to_string();

    // 创建 Control Plane 组件
    let task_manager = Arc::new(InMemoryTaskManager::new());
    let execution_service = Arc::new(InMemoryExecutionService::with_event_recorder(
        event_recorder.clone(),
    ));
    let registry = Arc::new(InMemoryRegistry::new());
    let resolver = Arc::new(InMemoryResolver::new(registry.clone()));
    let review_queue = Arc::new(InMemoryReviewQueue::new(None));

    let gateway = Arc::new(InMemoryGatewayRouter::new());
    let placement_engine = Arc::new(InMemoryPlacementEngine);
    let lease_manager = Arc::new(InMemoryLeaseManager::default());
    let membership_service = Arc::new(InMemoryMembershipService::default());
    let event_bus = Arc::new(InMemoryEventBus::default());
    let security_event_store = Arc::new(InMemorySecurityEventStore::new());

    let control_plane = Arc::new(
        ControlPlaneOrchestrator::new(
            gateway,
            resolver,
            registry,
            review_queue.clone(),
            task_manager.clone(),
            execution_service.clone(),
            placement_engine,
            lease_manager,
            membership_service,
            event_bus,
            policy_engine.clone(),
            // 创建 PluginRegistry 实例
            Arc::new(cyberclaw_plugin_runtime::PluginRegistry::new(
                std::path::PathBuf::from("/tmp/test_plugins"),
            )),
            security_event_store,
        )
        .with_event_recorder(event_recorder.clone()),
    );

    use cyberclaw_server::ControlPlaneComponents;
    let cp_components = ControlPlaneComponents {
        control_plane,
        task_manager,
        execution_service,
        review_queue,
    };
    Arc::new(AppState::new(
        llm_client,
        connector_registry,
        policy_engine,
        event_recorder,
        jwt_secret,
        cp_components,
    ))
}

/// 创建测试用的路由
fn create_test_router(config: ServerConfig) -> Router {
    let state = create_test_state();
    create_router_with_config(state, config)
}

#[tokio::test]
async fn test_rate_limit_allows_requests_within_limit() {
    let config = ServerConfig {
        rate_limit_per_second: 10, // 每秒10次
        rate_limit_burst_size: 10, // 允许突发10次
        ..Default::default()
    };

    let app = create_test_router(config);

    // 发送 5 次请求（在限制内）
    for i in 0..5 {
        let request = Request::builder()
            .uri("/health")
            .method("GET")
            .body(Body::empty())
            .unwrap();

        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "Request {} should succeed",
            i + 1
        );
    }
}

#[tokio::test]
async fn test_rate_limit_blocks_requests_over_limit() {
    let config = ServerConfig {
        rate_limit_per_second: 1, // 每秒1次
        rate_limit_burst_size: 5, // 允许突发5次
        ..Default::default()
    };

    let app = create_test_router(config);

    // 发送 10 次请求（超过突发限制）。使用 `/api/v1/admin/me` —— 它走 JWT
    // 保护过的 protected_routes lane（`rate_limited_routes` 内），所以
    // 速率限制 middleware 一定会过；又因为不带 Authorization header 必然
    // 401，所以"未被限速"的成功标志是 401 而不是 200。Sprint 20 W1
    // 之后 `/health` + `/metrics` 已经被搬到独立的 `scrape_routes` lane
    // 跳过限流（commit bc089f0），原 URI 不再触发 429。
    let mut success_count = 0;
    let mut rate_limited_count = 0;

    for i in 0..10 {
        let request = Request::builder()
            .uri("/api/v1/admin/me")
            .method("GET")
            .body(Body::empty())
            .unwrap();

        let response = app.clone().oneshot(request).await.unwrap();

        match response.status() {
            // Rate-limit pass-through. Auth-fail 401 is expected because we
            // intentionally do not provide a JWT — the goal is to test the
            // rate limiter, not auth.
            StatusCode::OK | StatusCode::UNAUTHORIZED => success_count += 1,
            StatusCode::TOO_MANY_REQUESTS => rate_limited_count += 1,
            status => panic!("Unexpected status code: {} for request {}", status, i + 1),
        }
    }

    // 至少前 5 次应该成功（突发限制）
    assert!(
        success_count >= 5,
        "At least 5 requests should succeed (burst_size=5), got {}",
        success_count
    );

    // 至少一些请求应该被速率限制
    assert!(
        rate_limited_count > 0,
        "Some requests should be rate limited, got {}",
        rate_limited_count
    );
}

#[tokio::test]
async fn test_body_limit_accepts_small_body() {
    let config = ServerConfig {
        max_body_size: 1024, // 1 KB
        ..Default::default()
    };

    let app = create_test_router(config);

    let small_body = serde_json::json!({
        "title": "Test Task",
        "summary": "This is a small request body",
        "kind": "Analysis"
    })
    .to_string();

    let request = Request::builder()
        .uri("/api/v1/tasks")
        .method("POST")
        .header("content-type", "application/json")
        .header("content-length", small_body.len().to_string())
        .body(Body::from(small_body))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    // 可能因为缺少认证而返回 401，但不应该返回 413
    assert_ne!(
        response.status(),
        StatusCode::PAYLOAD_TOO_LARGE,
        "Small body should not be rejected as too large"
    );
}

#[tokio::test]
async fn test_body_limit_rejects_large_body() {
    let config = ServerConfig {
        max_body_size: 1024, // 1 KB
        ..Default::default()
    };

    let app = create_test_router(config);

    // 创建一个超过 1 KB 的请求体
    let large_body = "x".repeat(2048); // 2 KB

    let request = Request::builder()
        .uri("/api/v1/tasks")
        .method("POST")
        .header("content-type", "application/json")
        .header("content-length", large_body.len().to_string())
        .body(Body::from(large_body))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(
        response.status(),
        StatusCode::PAYLOAD_TOO_LARGE,
        "Large body should be rejected"
    );
}

#[tokio::test]
async fn test_body_limit_default_size() {
    // 使用默认配置（10 MB 限制）
    let config = ServerConfig::default();
    let app = create_test_router(config);

    // 创建一个 5 MB 的请求体（在默认 10 MB 限制内）
    let medium_body = "x".repeat(5 * 1024 * 1024); // 5 MB

    let request = Request::builder()
        .uri("/api/v1/tasks")
        .method("POST")
        .header("content-type", "application/json")
        .header("content-length", medium_body.len().to_string())
        .body(Body::from(medium_body))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    // 应该不会因为请求体过大而被拒绝
    assert_ne!(
        response.status(),
        StatusCode::PAYLOAD_TOO_LARGE,
        "5 MB body should be accepted with default 10 MB limit"
    );
}

#[tokio::test]
async fn test_health_endpoint_not_rate_limited() {
    let config = ServerConfig {
        rate_limit_per_second: 1,
        rate_limit_burst_size: 2,
        ..Default::default()
    };

    let app = create_test_router(config);

    // 健康检查端点应该可以多次调用
    // 注意：当前实现中健康检查也受速率限制影响
    // 如果需要豁免健康检查，需要在路由配置中特殊处理

    let request = Request::builder()
        .uri("/health")
        .method("GET")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}
