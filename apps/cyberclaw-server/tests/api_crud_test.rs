#![allow(clippy::duplicate_mod)]
//! REST API CRUD 集成测试
//!
//! 测试 Agents 和 Tasks API 的完整 CRUD 功能

#[path = "common/mod.rs"]
mod common;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt; // for `oneshot`

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
use cyberclaw_llm::prelude::GenericOpenAiClient;
use cyberclaw_llm::LlmClient;
use cyberclaw_observability::events::InMemoryEventRecorder;
use cyberclaw_observability::security_event_store::InMemorySecurityEventStore;
use cyberclaw_server::{create_router, AppState};

// JWT 生成
use jsonwebtoken::{encode, EncodingKey, Header};
use serde::{Deserialize, Serialize};

/// 创建测试用的 AppState
fn create_test_state() -> Arc<AppState> {
    let llm_client: Arc<dyn LlmClient> = Arc::new(
        GenericOpenAiClient::new(Some("test-key".to_string()), "http://localhost:8080")
            .expect("Failed to create LLM client"),
    );
    let connector_registry = Arc::new(ConnectorRegistry::new());
    let policy_engine = Arc::new(DefaultPolicyEngine::default());
    let event_recorder = Arc::new(InMemoryEventRecorder::new());

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

    // 注册并激活一个测试节点（修复 "no active nodes available" 错误）
    use chrono::Utc;
    use cyberclaw_control_plane::membership_service::MembershipService;
    use cyberclaw_core::cluster::{
        MembershipState, NodeCapacity, NodeHealth, NodeRecord, NodeRole,
    };
    use cyberclaw_core::ids::NodeId;
    let test_node_id = NodeId::from_string("test-node-1".to_string()).expect("valid node id");
    let test_node = NodeRecord {
        id: test_node_id.clone(),
        role: NodeRole::Hybrid,
        labels: vec![],
        region: None,
        zone: None,
        health: NodeHealth::Healthy,
        membership_state: MembershipState::Active, // 直接设置为 Active
        capacity: NodeCapacity::default(),
        current_executions: 0,
        last_heartbeat_at: Utc::now(),
    };
    membership_service
        .join(test_node)
        .expect("failed to join test node");
    membership_service
        .heartbeat(&test_node_id)
        .expect("failed to heartbeat test node");

    // 创建 PluginRegistry 实例
    let plugin_registry = Arc::new(cyberclaw_plugin_runtime::PluginRegistry::new(
        std::path::PathBuf::from("/tmp/test_plugins"),
    ));

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
            plugin_registry,
            security_event_store,
        )
        .with_event_recorder(event_recorder.clone()),
    );

    // 使用 AppState::new() 构造器，它会自动初始化所有字段（包括 task_store 和 agent_status_store）
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
        "test-jwt-secret-key".to_string(),
        cp_components,
    ))
}

/// 辅助函数：将 Body 转换为 JSON Value
async fn body_to_json(body: Body) -> Value {
    let bytes = axum::body::to_bytes(body, usize::MAX)
        .await
        .expect("Failed to collect body");
    serde_json::from_slice(&bytes).expect("Failed to parse JSON")
}

/// JWT Claims 结构（与 auth.rs 保持一致）
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Claims {
    sub: String,
    exp: i64,
    iat: i64,
}

/// 生成测试用的 JWT token
fn generate_test_jwt(secret: &str) -> String {
    let now = chrono::Utc::now().timestamp();
    let claims = Claims {
        sub: "test-user-123".to_string(),
        iat: now,
        exp: now + 86400, // 24 hours
    };

    let encoding_key = EncodingKey::from_secret(secret.as_bytes());
    encode(&Header::default(), &claims, &encoding_key).expect("Failed to generate test JWT")
}

/// 获取测试用的 Authorization header 值
fn get_auth_header() -> String {
    format!("Bearer {}", generate_test_jwt("test-jwt-secret-key"))
}

// ===== Tasks API 测试 =====

#[tokio::test]
async fn test_create_task_success() {
    let state = create_test_state();
    let app = create_router(state);

    let request_body = json!({
        "title": "Test Task",
        "summary": "This is a test task",
        "kind": "Analysis",
        "priority": "high",
        "labels": ["test", "api"]
    });

    let request = Request::builder()
        .uri("/api/v1/tasks")
        .method("POST")
        .header("content-type", "application/json")
        .header("authorization", get_auth_header())
        .body(Body::from(request_body.to_string()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    // DEBUG: Print status and body before assertion
    let status = response.status();
    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_str = String::from_utf8_lossy(&body_bytes);
    eprintln!("DEBUG: Status = {}", status);
    eprintln!("DEBUG: Body = {}", body_str);

    assert_eq!(status, StatusCode::CREATED);

    let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(body["title"], "Test Task");
    assert_eq!(body["summary"], "This is a test task");
    assert_eq!(body["kind"], "Analysis");
    assert_eq!(body["priority"], "high");
    assert_eq!(body["status"], "pending");
    assert!(body["id"].is_string());
}

#[tokio::test]
async fn test_create_task_invalid_empty_title() {
    let state = create_test_state();
    let app = create_router(state);

    let request_body = json!({
        "title": "",
        "summary": "This should fail",
        "kind": "Analysis"
    });

    let request = Request::builder()
        .uri("/api/v1/tasks")
        .method("POST")
        .header("content-type", "application/json")
        .header("authorization", get_auth_header())
        .body(Body::from(request_body.to_string()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_list_tasks() {
    let state = create_test_state();
    let app = create_router(state);

    let request = Request::builder()
        .uri("/api/v1/tasks")
        .method("GET")
        .header("authorization", get_auth_header())
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = body_to_json(response.into_body()).await;
    assert!(body.is_array());
}

#[tokio::test]
async fn test_task_lifecycle() {
    let state = create_test_state();
    let app = create_router(state);

    // 1. 创建任务
    let create_request = Request::builder()
        .uri("/api/v1/tasks")
        .method("POST")
        .header("content-type", "application/json")
        .header("authorization", get_auth_header())
        .body(Body::from(
            json!({
                "title": "Lifecycle Test Task",
                "summary": "Testing full lifecycle",
                "kind": "Analysis"
            })
            .to_string(),
        ))
        .unwrap();

    let create_response = app.clone().oneshot(create_request).await.unwrap();
    assert_eq!(create_response.status(), StatusCode::CREATED);

    let create_body = body_to_json(create_response.into_body()).await;
    let task_id = create_body["id"].as_str().unwrap();

    // 2. 获取任务详情
    let get_request = Request::builder()
        .uri(format!("/api/v1/tasks/{}", task_id))
        .method("GET")
        .header("authorization", get_auth_header())
        .body(Body::empty())
        .unwrap();

    let get_response = app.clone().oneshot(get_request).await.unwrap();
    assert_eq!(get_response.status(), StatusCode::OK);

    let get_body = body_to_json(get_response.into_body()).await;
    assert_eq!(get_body["id"], task_id);
    assert_eq!(get_body["title"], "Lifecycle Test Task");

    // 3. 获取任务状态
    let status_request = Request::builder()
        .uri(format!("/api/v1/tasks/{}/status", task_id))
        .method("GET")
        .header("authorization", get_auth_header())
        .body(Body::empty())
        .unwrap();

    let status_response = app.clone().oneshot(status_request).await.unwrap();
    assert_eq!(status_response.status(), StatusCode::OK);

    let status_body = body_to_json(status_response.into_body()).await;
    assert_eq!(status_body["id"], task_id);
    assert_eq!(status_body["status"], "pending");

    // 4. 取消任务
    let cancel_request = Request::builder()
        .uri(format!("/api/v1/tasks/{}", task_id))
        .method("DELETE")
        .header("authorization", get_auth_header())
        .body(Body::empty())
        .unwrap();

    let cancel_response = app.clone().oneshot(cancel_request).await.unwrap();
    assert_eq!(cancel_response.status(), StatusCode::NO_CONTENT);

    // 5. 验证任务已取消
    let verify_request = Request::builder()
        .uri(format!("/api/v1/tasks/{}/status", task_id))
        .method("GET")
        .header("authorization", get_auth_header())
        .body(Body::empty())
        .unwrap();

    let verify_response = app.oneshot(verify_request).await.unwrap();
    let verify_body = body_to_json(verify_response.into_body()).await;
    assert_eq!(verify_body["status"], "cancelled");
}

#[tokio::test]
async fn test_get_nonexistent_task() {
    let state = create_test_state();
    let app = create_router(state);

    let request = Request::builder()
        .uri("/api/v1/tasks/nonexistent-task-id")
        .method("GET")
        .header("authorization", get_auth_header())
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

// ===== Agents API 测试 =====

#[tokio::test]
async fn test_list_agents() {
    let state = create_test_state();
    let app = create_router(state);

    let request = Request::builder()
        .uri("/api/v1/agents")
        .method("GET")
        .header("authorization", get_auth_header())
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = body_to_json(response.into_body()).await;
    assert!(body.is_array());
}

#[tokio::test]
async fn test_get_agent_status() {
    // Contract (apps/cyberclaw-server/src/api/agents.rs:711-715):
    // GET /api/v1/agents/:name/status returns 404 for unknown agents so callers
    // can distinguish "exists but idle" from "does not exist". This test
    // exercises that 404 path — for the success path see smoke-business.sh
    // which registers an agent first.
    let state = create_test_state();
    let app = create_router(state);

    let request = Request::builder()
        .uri("/api/v1/agents/test-agent/status")
        .method("GET")
        .header("authorization", get_auth_header())
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

// ===== Health API 测试 =====

#[tokio::test]
async fn test_health_check() {
    let state = create_test_state();
    let app = create_router(state);

    let request = Request::builder()
        .uri("/health")
        .method("GET")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], b"OK");
}

#[tokio::test]
async fn test_cors_headers() {
    let state = create_test_state();
    let app = create_router(state);

    let request = Request::builder()
        .uri("/health")
        .method("OPTIONS")
        .header("origin", "http://localhost:3000")
        .header("access-control-request-method", "POST")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let headers = response.headers();
    assert!(headers.contains_key("access-control-allow-origin"));
    assert!(headers.contains_key("access-control-allow-methods"));

    // 验证返回的是白名单中的来源（不是 '*'）
    let allow_origin = headers
        .get("access-control-allow-origin")
        .and_then(|v| v.to_str().ok())
        .unwrap();
    assert_eq!(allow_origin, "http://localhost:3000");
}

#[tokio::test]
async fn test_security_headers() {
    let state = create_test_state();
    let app = create_router(state);

    let request = Request::builder()
        .uri("/health")
        .method("GET")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    let headers = response.headers();

    // 验证所有安全响应头存在
    assert_eq!(
        headers.get("x-content-type-options").unwrap(),
        "nosniff",
        "Missing X-Content-Type-Options header"
    );

    assert_eq!(
        headers.get("x-frame-options").unwrap(),
        "DENY",
        "Missing X-Frame-Options header"
    );

    assert_eq!(
        headers.get("x-xss-protection").unwrap(),
        "1; mode=block",
        "Missing X-XSS-Protection header"
    );

    assert!(
        headers.contains_key("content-security-policy"),
        "Missing Content-Security-Policy header"
    );

    // HSTS 仅在生产环境启用
    if std::env::var("ENVIRONMENT")
        .unwrap_or_else(|_| "development".to_string())
        .to_lowercase()
        == "production"
    {
        assert!(
            headers.contains_key("strict-transport-security"),
            "Missing Strict-Transport-Security header in production"
        );
    }
}

#[tokio::test]
async fn test_cors_rejects_unknown_origin() {
    let state = create_test_state();
    let app = create_router(state);

    let request = Request::builder()
        .uri("/health")
        .method("OPTIONS")
        .header("origin", "https://evil.com")
        .header("access-control-request-method", "POST")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    // CORS 预检请求应该返回 OK，但不应包含 allow-origin 头（或为 null）
    assert_eq!(response.status(), StatusCode::OK);

    let headers = response.headers();
    let allow_origin = headers.get("access-control-allow-origin");

    // 未授权的来源不应出现在响应头中
    if let Some(origin) = allow_origin {
        let origin_str = origin.to_str().unwrap();
        assert_ne!(
            origin_str, "https://evil.com",
            "CORS should not allow unknown origin"
        );
    }
}

// ===== JWT 认证测试 =====

#[tokio::test]
async fn test_auth_missing_token() {
    let state = create_test_state();
    let app = create_router(state);

    // 尝试访问受保护的端点，不带 token
    let request = Request::builder()
        .uri("/api/v1/tasks")
        .method("GET")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_auth_invalid_token() {
    let state = create_test_state();
    let app = create_router(state);

    // 使用无效的 token
    let request = Request::builder()
        .uri("/api/v1/tasks")
        .method("GET")
        .header("authorization", "Bearer invalid.token.here")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_auth_expired_token() {
    let state = create_test_state();
    let app = create_router(state);

    // 生成一个已过期的 token
    let now = chrono::Utc::now().timestamp();
    let expired_claims = Claims {
        sub: "test-user".to_string(),
        iat: now - 1000,
        exp: now - 100, // 已过期
    };

    let encoding_key = EncodingKey::from_secret("test-jwt-secret-key".as_bytes());
    let expired_token =
        encode(&Header::default(), &expired_claims, &encoding_key).expect("Failed to encode");

    let request = Request::builder()
        .uri("/api/v1/tasks")
        .method("GET")
        .header("authorization", format!("Bearer {}", expired_token))
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_auth_valid_token() {
    let state = create_test_state();
    let app = create_router(state);

    // 使用有效的 token
    let request = Request::builder()
        .uri("/api/v1/tasks")
        .method("GET")
        .header("authorization", get_auth_header())
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_health_endpoint_no_auth() {
    let state = create_test_state();
    let app = create_router(state);

    // Health 端点应该不需要认证
    let request = Request::builder()
        .uri("/health")
        .method("GET")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}
