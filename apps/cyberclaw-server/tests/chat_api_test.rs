#![allow(clippy::duplicate_mod)]
//! Chat API 集成测试

#[path = "common/mod.rs"]
mod common;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt; // for `oneshot` and `ready`

use cyberclaw_llm::{
    ChatChunk, ChatRequest, ChatResponse, Choice, LlmClient, LlmError, LlmResult, Message, Role,
    Usage,
};
use cyberclaw_server::create_router;
use futures::stream::{self, Stream};
// JWT imports
use jsonwebtoken::{encode, EncodingKey, Header};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Claims {
    sub: String,
    exp: i64,
    iat: i64,
}

fn generate_test_jwt(secret: &str) -> String {
    let now = chrono::Utc::now().timestamp();
    let claims = Claims {
        sub: "test-user-123".to_string(),
        iat: now,
        exp: now + 86400,
    };
    let encoding_key = EncodingKey::from_secret(secret.as_bytes());
    encode(&Header::default(), &claims, &encoding_key).expect("Failed to generate test JWT")
}

fn get_auth_header() -> String {
    format!("Bearer {}", generate_test_jwt("test-jwt-secret"))
}

/// Mock LLM 客户端，用于测试
#[derive(Debug, Clone)]
struct MockLlmClient {
    should_fail: bool,
}

impl MockLlmClient {
    fn new() -> Self {
        Self { should_fail: false }
    }

    fn with_failure() -> Self {
        Self { should_fail: true }
    }
}

#[async_trait::async_trait]
impl LlmClient for MockLlmClient {
    async fn chat_completion(&self, request: ChatRequest) -> LlmResult<ChatResponse> {
        if self.should_fail {
            return Err(LlmError::ApiError {
                status: 500,
                message: "Mock API error".to_string(),
            });
        }

        Ok(ChatResponse {
            id: "chatcmpl-test-123".to_string(),
            object: "chat.completion".to_string(),
            created: 1234567890,
            model: request.model,
            choices: vec![Choice {
                index: 0,
                message: Message::assistant("Hello! This is a test response."),
                finish_reason: Some("stop".to_string()),
            }],
            usage: Some(Usage {
                prompt_tokens: 10,
                completion_tokens: 20,
                total_tokens: 30,
                ..Default::default()
            }),
        })
    }

    async fn chat_completion_stream(
        &self,
        request: ChatRequest,
    ) -> LlmResult<Box<dyn Stream<Item = LlmResult<ChatChunk>> + Send + Unpin>> {
        if self.should_fail {
            return Err(LlmError::ApiError {
                status: 500,
                message: "Mock stream error".to_string(),
            });
        }

        // 创建一个简单的流式响应
        let chunks = vec![
            Ok(ChatChunk {
                id: "chatcmpl-test-stream".to_string(),
                object: "chat.completion.chunk".to_string(),
                created: 1234567890,
                model: request.model.clone(),
                choices: vec![cyberclaw_llm::ChunkChoice {
                    index: 0,
                    delta: cyberclaw_llm::Delta {
                        role: Some(Role::Assistant),
                        content: Some("Hello".to_string()),
                        tool_calls: None,
                    },
                    finish_reason: None,
                }],
            }),
            Ok(ChatChunk {
                id: "chatcmpl-test-stream".to_string(),
                object: "chat.completion.chunk".to_string(),
                created: 1234567890,
                model: request.model,
                choices: vec![cyberclaw_llm::ChunkChoice {
                    index: 0,
                    delta: cyberclaw_llm::Delta {
                        role: None,
                        content: Some(" World!".to_string()),
                        tool_calls: None,
                    },
                    finish_reason: Some("stop".to_string()),
                }],
            }),
        ];

        Ok(Box::new(stream::iter(chunks)))
    }

    fn provider(&self) -> &str {
        "mock"
    }

    async fn validate_connection(&self) -> LlmResult<()> {
        Ok(())
    }
}

// 重新导出以便测试使用
pub use cyberclaw_server::api::chat::{ChatCompletionRequest, ChatCompletionResponse};

/// 创建测试 app
fn create_test_app(client: MockLlmClient) -> axum::Router {
    let connector_registry = Arc::new(cyberclaw_connectors::registry::ConnectorRegistry::new());
    let policy_engine = Arc::new(cyberclaw_governance::engine::DefaultPolicyEngine::default());
    let event_recorder = Arc::new(cyberclaw_observability::events::InMemoryEventRecorder::new());
    let jwt_secret = "test-jwt-secret".to_string();

    // 创建 Control Plane 组件
    use cyberclaw_control_plane::*;
    let task_manager = Arc::new(task_manager::InMemoryTaskManager::new());
    let execution_service = Arc::new(
        execution_service::InMemoryExecutionService::with_event_recorder(event_recorder.clone()),
    );
    let registry = Arc::new(registry::InMemoryRegistry::new());
    let resolver = Arc::new(resolver::InMemoryResolver::new(registry.clone()));
    let review_queue = Arc::new(review_queue::InMemoryReviewQueue::new(None));
    let gateway = Arc::new(gateway_router::InMemoryGatewayRouter::new());
    let placement_engine = Arc::new(placement_engine::InMemoryPlacementEngine);
    let lease_manager = Arc::new(lease_manager::InMemoryLeaseManager::default());
    let membership_service = Arc::new(membership_service::InMemoryMembershipService::default());

    // 注册并激活一个测试节点（修复 "no active nodes available" 错误）
    {
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
            membership_state: MembershipState::Active,
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
    }

    let event_bus = Arc::new(event_bus::InMemoryEventBus::default());
    let security_event_store =
        Arc::new(cyberclaw_observability::security_event_store::InMemorySecurityEventStore::new());
    let control_plane = Arc::new(orchestrator::ControlPlaneOrchestrator::new(
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
    ));

    use cyberclaw_server::ControlPlaneComponents;
    let cp_components = ControlPlaneComponents {
        control_plane,
        task_manager,
        execution_service,
        review_queue,
    };

    let state = Arc::new(cyberclaw_server::AppState::new(
        Arc::new(client),
        connector_registry,
        policy_engine,
        event_recorder,
        jwt_secret,
        cp_components,
    ));
    create_router(state)
}

#[tokio::test]
async fn test_health_check() {
    let app = create_test_app(MockLlmClient::new());

    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], b"OK");
}

#[tokio::test]
async fn test_chat_completion_non_streaming() {
    let app = create_test_app(MockLlmClient::new());

    let request_body = json!({
        "model": "gpt-4",
        "messages": [
            {"role": "user", "content": "Hello"}
        ],
        "temperature": 0.7,
        "max_tokens": 100,
        "stream": false
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .header("authorization", get_auth_header())
                .body(Body::from(serde_json::to_string(&request_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let response_json: Value = serde_json::from_slice(&body).unwrap();

    // 验证响应格式
    assert_eq!(response_json["object"], "chat.completion");
    assert_eq!(response_json["model"], "gpt-4");
    assert_eq!(response_json["choices"][0]["message"]["role"], "assistant");
    assert!(response_json["usage"].is_object());
}

#[tokio::test]
async fn test_chat_completion_streaming() {
    let app = create_test_app(MockLlmClient::new());

    let request_body = json!({
        "model": "gpt-4",
        "messages": [
            {"role": "user", "content": "Hello"}
        ],
        "stream": true
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .header("authorization", get_auth_header())
                .body(Body::from(serde_json::to_string(&request_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "text/event-stream"
    );
}

#[tokio::test]
async fn test_chat_completion_error_handling() {
    let app = create_test_app(MockLlmClient::with_failure());

    let request_body = json!({
        "model": "gpt-4",
        "messages": [
            {"role": "user", "content": "Hello"}
        ],
        "stream": false
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .header("authorization", get_auth_header())
                .body(Body::from(serde_json::to_string(&request_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    // 应该返回错误状态码
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let error_json: Value = serde_json::from_slice(&body).unwrap();

    // 验证错误响应格式
    assert!(error_json["error"].is_object());
    assert!(error_json["error"]["message"].is_string());
}

#[tokio::test]
async fn test_invalid_request() {
    let app = create_test_app(MockLlmClient::new());

    // 发送无效的 JSON
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .header("authorization", get_auth_header())
                .body(Body::from("{invalid json}"))
                .unwrap(),
        )
        .await
        .unwrap();

    // 应该返回 400
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
