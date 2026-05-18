//! 公共测试辅助模块
//!
//! 提供测试服务器、Mock LLM 客户端、测试数据生成器等测试基础设施

use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use futures::stream::{self, Stream};
use jsonwebtoken::{encode, EncodingKey, Header};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use tower::ServiceExt;

use cyberclaw_connectors::{registry::ConnectorRegistry, CapabilityDispatcher, LocalConnector};
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
use cyberclaw_llm::{
    ChatChunk, ChatRequest, ChatResponse, Choice, ChunkChoice, Delta, LlmClient, Message, Role,
    Usage,
};
use cyberclaw_llm::{LlmError, LlmResult};
use cyberclaw_observability::{
    events::InMemoryEventRecorder, security_event_store::InMemorySecurityEventStore,
};
use cyberclaw_server::{AppState, ServerConfig};

/// Mock LLM 客户端 - 用于不需要真实 LLM 服务的测试
///
/// 返回预设的标准响应，允许测试验证 API 结构和流程，
/// 而不依赖外部 LLM 服务。
pub struct MockLlmClient {
    /// 预设的响应内容
    response_content: String,
    /// 预设的模型名称
    model_name: String,
}

impl MockLlmClient {
    /// 创建默认 Mock 客户端
    pub fn new() -> Self {
        Self {
            response_content: "This is a mock response from the test LLM client.".to_string(),
            model_name: std::env::var("LLM_DEFAULT_MODEL").unwrap_or_else(|_| "gpt-4".to_string()),
        }
    }

    /// 创建带自定义响应的 Mock 客户端
    #[allow(dead_code)]
    pub fn with_response(content: impl Into<String>) -> Self {
        Self {
            response_content: content.into(),
            model_name: std::env::var("LLM_DEFAULT_MODEL").unwrap_or_else(|_| "gpt-4".to_string()),
        }
    }
}

impl Default for MockLlmClient {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl LlmClient for MockLlmClient {
    async fn chat_completion(&self, request: ChatRequest) -> LlmResult<ChatResponse> {
        // 验证请求有消息
        if request.messages.is_empty() {
            return Err(LlmError::Internal(
                "messages array cannot be empty".to_string(),
            ));
        }

        let model = if request.model.is_empty() {
            self.model_name.clone()
        } else {
            request.model.clone()
        };

        // 计算 token 数量（简单估算：每个词约 1.3 token）
        let total_input_chars: usize = request.messages.iter().map(|m| m.content.len()).sum();
        let prompt_tokens = (total_input_chars / 4 + 1) as u32;
        let completion_tokens = (self.response_content.len() / 4 + 1) as u32;

        Ok(ChatResponse {
            id: format!("chatcmpl-mock-{}", uuid::Uuid::new_v4()),
            object: "chat.completion".to_string(),
            created: chrono::Utc::now().timestamp(),
            model,
            choices: vec![Choice {
                index: 0,
                message: Message::assistant(self.response_content.clone()),
                finish_reason: Some("stop".to_string()),
            }],
            usage: Some(Usage {
                prompt_tokens,
                completion_tokens,
                total_tokens: prompt_tokens + completion_tokens,
                ..Default::default()
            }),
        })
    }

    async fn chat_completion_stream(
        &self,
        request: ChatRequest,
    ) -> LlmResult<Box<dyn Stream<Item = LlmResult<ChatChunk>> + Send + Unpin>> {
        if request.messages.is_empty() {
            return Err(LlmError::Internal(
                "messages array cannot be empty".to_string(),
            ));
        }

        let model = if request.model.is_empty() {
            self.model_name.clone()
        } else {
            request.model.clone()
        };

        // 生成多个流式块来模拟真实的流式响应
        let words: Vec<&str> = self.response_content.split_whitespace().collect();
        let base_id = format!("chatcmpl-mock-stream-{}", uuid::Uuid::new_v4());
        let created = chrono::Utc::now().timestamp();
        let model_clone = model.clone();

        let mut chunks: Vec<LlmResult<ChatChunk>> = Vec::new();

        // 第一个 chunk 包含角色信息
        chunks.push(Ok(ChatChunk {
            id: base_id.clone(),
            object: "chat.completion.chunk".to_string(),
            created,
            model: model_clone.clone(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: Delta {
                    role: Some(Role::Assistant),
                    content: None,
                    tool_calls: None,
                },
                finish_reason: None,
            }],
        }));

        // 将内容拆分成多个块
        for word in words {
            let word_with_space = format!("{} ", word);
            chunks.push(Ok(ChatChunk {
                id: base_id.clone(),
                object: "chat.completion.chunk".to_string(),
                created,
                model: model_clone.clone(),
                choices: vec![ChunkChoice {
                    index: 0,
                    delta: Delta {
                        role: None,
                        content: Some(word_with_space),
                        tool_calls: None,
                    },
                    finish_reason: None,
                }],
            }));
        }

        // 最后一个 chunk 包含结束原因
        chunks.push(Ok(ChatChunk {
            id: base_id.clone(),
            object: "chat.completion.chunk".to_string(),
            created,
            model: model_clone,
            choices: vec![ChunkChoice {
                index: 0,
                delta: Delta {
                    role: None,
                    content: None,
                    tool_calls: None,
                },
                finish_reason: Some("stop".to_string()),
            }],
        }));

        Ok(Box::new(stream::iter(chunks)))
    }

    fn provider(&self) -> &str {
        "mock"
    }

    async fn validate_connection(&self) -> LlmResult<()> {
        Ok(())
    }
}

/// JWT Claims 结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub exp: i64,
    pub iat: i64,
}

/// 测试服务器配置
#[allow(dead_code)]
pub struct TestServer {
    pub app: Router,
    pub state: Arc<AppState>,
    pub jwt_secret: String,
}

impl TestServer {
    /// 创建测试服务器实例
    ///
    /// 默认固定使用 MockLlmClient，确保发布门禁测试不受外部 LLM 波动影响。
    pub fn new() -> Self {
        Self::new_with_mock_llm_config(ServerConfig::default())
    }

    /// 创建明确使用 Mock LLM 的测试服务器
    ///
    /// 不受环境变量影响，始终使用 MockLlmClient。
    #[allow(dead_code)]
    pub fn new_with_mock_llm() -> Self {
        Self::build(Arc::new(MockLlmClient::new()), ServerConfig::default())
    }

    /// 创建明确使用 Mock LLM 且可自定义配置的测试服务器
    ///
    /// 不受环境变量影响，始终使用 MockLlmClient。
    #[allow(dead_code)]
    pub fn new_with_mock_llm_config(config: ServerConfig) -> Self {
        Self::build(Arc::new(MockLlmClient::new()), config)
    }

    /// 创建带配置的测试服务器实例
    ///
    /// 默认固定使用 MockLlmClient，确保测试稳定可复现。
    #[allow(dead_code)]
    pub fn new_with_config(config: ServerConfig) -> Self {
        Self::build(Arc::new(MockLlmClient::new()), config)
    }

    /// 创建显式使用环境变量 LLM_PROVIDER 的测试服务器
    ///
    /// 用于真实 LLM 可用性门禁，不参与默认发布门禁。
    #[allow(dead_code)]
    pub fn new_with_env_llm_config(config: ServerConfig) -> Self {
        let llm_client: Arc<dyn LlmClient> = {
            use std::env;
            let provider = env::var("LLM_PROVIDER").unwrap_or_else(|_| "mock".to_string());

            match provider.as_str() {
                "ark" => {
                    use cyberclaw_llm::prelude::ArkClient;
                    let api_key =
                        env::var("LLM_API_KEY").unwrap_or_else(|_| "test-key".to_string());
                    let base_url = env::var("LLM_BASE_URL")
                        .unwrap_or_else(|_| "http://localhost:8080".to_string());
                    Arc::new(
                        ArkClient::new(api_key, &base_url).expect("Failed to create ARK client"),
                    )
                }
                "openai" => {
                    use cyberclaw_llm::prelude::OpenAiClient;
                    let api_key =
                        env::var("LLM_API_KEY").unwrap_or_else(|_| "test-key".to_string());
                    let base_url = env::var("LLM_BASE_URL")
                        .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
                    Arc::new(
                        OpenAiClient::new(api_key, &base_url)
                            .expect("Failed to create OpenAI client"),
                    )
                }
                "anthropic" => {
                    use cyberclaw_llm::prelude::AnthropicClient;
                    let api_key =
                        env::var("LLM_API_KEY").unwrap_or_else(|_| "test-key".to_string());
                    let base_url = env::var("LLM_BASE_URL")
                        .unwrap_or_else(|_| "https://api.anthropic.com".to_string());
                    Arc::new(
                        AnthropicClient::new(api_key, &base_url)
                            .expect("Failed to create Anthropic client"),
                    )
                }
                "generic" => {
                    let api_key =
                        env::var("LLM_API_KEY").unwrap_or_else(|_| "test-key".to_string());
                    let base_url = env::var("LLM_BASE_URL")
                        .unwrap_or_else(|_| "http://localhost:8080".to_string());
                    Arc::new(
                        GenericOpenAiClient::new(Some(api_key), &base_url)
                            .expect("Failed to create LLM client"),
                    )
                }
                // "mock" 或任何未知值均使用 MockLlmClient
                _ => Arc::new(MockLlmClient::new()),
            }
        };
        Self::build(llm_client, config)
    }

    /// 创建服务器的内部构建方法
    fn build(llm_client: Arc<dyn LlmClient>, config: ServerConfig) -> Self {
        let jwt_secret = "test-jwt-secret-key".to_string();

        let connector_registry = Arc::new(ConnectorRegistry::new());
        let policy_engine = Arc::new(DefaultPolicyEngine::default());
        let event_recorder = Arc::new(InMemoryEventRecorder::new());

        // 注册 LocalConnector 用于能力分发
        let workspace_path = std::env::temp_dir().join("cyberclaw-server-test");
        std::fs::create_dir_all(&workspace_path).expect("Failed to create test workspace");
        let local_connector = Arc::new(LocalConnector::new(workspace_path));
        connector_registry
            .register(local_connector)
            .expect("Failed to register LocalConnector");

        // 创建 CapabilityDispatcher
        let dispatcher = Arc::new(CapabilityDispatcher::new(connector_registry.clone()));

        // 创建 Control Plane 组件
        let task_manager = Arc::new(InMemoryTaskManager::new());
        let execution_service = Arc::new(
            InMemoryExecutionService::with_event_recorder(event_recorder.clone())
                .with_capability_dispatcher(dispatcher),
        );
        let registry = Arc::new(InMemoryRegistry::new());
        let resolver = Arc::new(InMemoryResolver::new(registry.clone()));
        let review_queue = Arc::new(InMemoryReviewQueue::new(None));
        let gateway = Arc::new(InMemoryGatewayRouter::new());
        let placement_engine = Arc::new(InMemoryPlacementEngine);
        let lease_manager = Arc::new(InMemoryLeaseManager::default());
        let membership_service = Arc::new(InMemoryMembershipService::default());

        // 注册并激活一个测试节点（修复 "no active nodes available" 错误）
        {
            use chrono::Utc;
            use cyberclaw_control_plane::membership_service::MembershipService;
            use cyberclaw_core::cluster::{
                MembershipState, NodeCapacity, NodeHealth, NodeRecord, NodeRole,
            };
            use cyberclaw_core::ids::NodeId;

            let test_node_id =
                NodeId::from_string("test-node-1".to_string()).expect("valid node id");
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

        let event_bus = Arc::new(InMemoryEventBus::default());
        let security_event_store = Arc::new(InMemorySecurityEventStore::new());

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

        use cyberclaw_server::ControlPlaneComponents;
        let cp_components = ControlPlaneComponents {
            control_plane,
            task_manager,
            execution_service,
            review_queue,
        };

        let state = Arc::new(AppState::new(
            llm_client,
            connector_registry,
            policy_engine,
            event_recorder,
            jwt_secret.clone(),
            cp_components,
        ));

        let app = cyberclaw_server::create_router_with_config(state.clone(), config);

        Self {
            app,
            state,
            jwt_secret,
        }
    }

    /// 生成测试用的 JWT token
    pub fn generate_jwt(&self) -> String {
        let now = chrono::Utc::now().timestamp();
        let claims = Claims {
            sub: "test-user-123".to_string(),
            iat: now,
            exp: now + 86400, // 24 hours
        };

        let encoding_key = EncodingKey::from_secret(self.jwt_secret.as_bytes());
        encode(&Header::default(), &claims, &encoding_key).expect("Failed to generate test JWT")
    }

    /// 获取 Authorization header 值
    pub fn auth_header(&self) -> String {
        format!("Bearer {}", self.generate_jwt())
    }

    /// 生成已过期的 JWT token
    #[allow(dead_code)]
    pub fn generate_expired_jwt(&self) -> String {
        let now = chrono::Utc::now().timestamp();
        let claims = Claims {
            sub: "test-user".to_string(),
            iat: now - 1000,
            exp: now - 100, // 已过期
        };

        let encoding_key = EncodingKey::from_secret(self.jwt_secret.as_bytes());
        encode(&Header::default(), &claims, &encoding_key).expect("Failed to generate expired JWT")
    }

    /// 发送 GET 请求
    #[allow(dead_code)]
    pub async fn get(&mut self, uri: &str) -> TestResponse {
        self.get_with_auth(uri, true).await
    }

    /// 发送 GET 请求（可选认证）
    #[allow(dead_code)]
    pub async fn get_with_auth(&mut self, uri: &str, with_auth: bool) -> TestResponse {
        let mut request_builder = Request::builder().uri(uri).method("GET");

        if with_auth {
            request_builder = request_builder.header("authorization", self.auth_header());
        }

        let request = request_builder.body(Body::empty()).unwrap();
        let response = self.app.clone().oneshot(request).await.unwrap();

        TestResponse::from_response(response).await
    }

    /// 发送 POST 请求
    #[allow(dead_code)]
    pub async fn post(&mut self, uri: &str, body: Value) -> TestResponse {
        self.post_with_auth(uri, body, true).await
    }

    /// 发送 POST 请求（可选认证）
    #[allow(dead_code)]
    pub async fn post_with_auth(
        &mut self,
        uri: &str,
        body: Value,
        with_auth: bool,
    ) -> TestResponse {
        let mut request_builder = Request::builder()
            .uri(uri)
            .method("POST")
            .header("content-type", "application/json");

        if with_auth {
            request_builder = request_builder.header("authorization", self.auth_header());
        }

        let request = request_builder.body(Body::from(body.to_string())).unwrap();

        let response = self.app.clone().oneshot(request).await.unwrap();
        TestResponse::from_response(response).await
    }

    /// 发送 DELETE 请求
    #[allow(dead_code)]
    pub async fn delete(&mut self, uri: &str) -> TestResponse {
        let request = Request::builder()
            .uri(uri)
            .method("DELETE")
            .header("authorization", self.auth_header())
            .body(Body::empty())
            .unwrap();

        let response = self.app.clone().oneshot(request).await.unwrap();
        TestResponse::from_response(response).await
    }
}

impl Default for TestServer {
    fn default() -> Self {
        Self::new()
    }
}

/// 测试响应封装
#[allow(dead_code)]
pub struct TestResponse {
    pub status: StatusCode,
    pub body: Option<Value>,
    pub text: Option<String>,
}

impl TestResponse {
    /// 从 Axum Response 创建 TestResponse
    async fn from_response(response: axum::response::Response) -> Self {
        let status = response.status();
        let body = response.into_body();
        let bytes = axum::body::to_bytes(body, usize::MAX)
            .await
            .expect("Failed to collect body");

        let text = String::from_utf8(bytes.to_vec()).ok();
        let body = text.as_ref().and_then(|t| serde_json::from_str(t).ok());

        Self { status, body, text }
    }

    /// 检查状态码是否为成功
    pub fn is_success(&self) -> bool {
        self.status.is_success()
    }

    /// 获取 JSON 响应体
    pub fn json(&self) -> &Value {
        self.body.as_ref().expect("Response body is not JSON")
    }

    /// 获取文本响应体
    #[allow(dead_code)]
    pub fn text(&self) -> &str {
        self.text.as_deref().unwrap_or("")
    }
}

/// 测试数据生成器
pub struct TestDataGenerator;

#[allow(dead_code)]
impl TestDataGenerator {
    /// 生成测试任务请求
    ///
    /// `Priority` 在 `cyberclaw-core/src/enums.rs` 用了
    /// `#[serde(rename_all = "lowercase")]`，所以 JSON 必须送
    /// 小写串（"high"），否则 axum 会以 422 拒掉。
    pub fn create_task_request(title: &str, kind: &str) -> Value {
        serde_json::json!({
            "title": title,
            "summary": format!("Test task: {}", title),
            "kind": kind,
            "priority": "high",
            "labels": ["test"]
        })
    }

    /// 生成 Agent 调用请求
    pub fn invoke_agent_request(input: &str) -> Value {
        serde_json::json!({
            "input": input,
            "context": {}
        })
    }

    /// 生成 Chat completion 请求
    pub fn chat_completion_request(message: &str, stream: bool) -> Value {
        // 从环境变量读取model名称，默认使用gpt-4（用于Mock测试）
        let model = std::env::var("LLM_DEFAULT_MODEL").unwrap_or_else(|_| "gpt-4".to_string());
        serde_json::json!({
            "model": model,
            "messages": [
                {"role": "user", "content": message}
            ],
            "stream": stream
        })
    }
}

/// 断言辅助函数
#[allow(dead_code)]
pub mod assertions {
    use super::*;

    /// 断言响应状态码
    pub fn assert_status(response: &TestResponse, expected: StatusCode) {
        assert_eq!(
            response.status, expected,
            "Expected status {}, got {}",
            expected, response.status
        );
    }

    /// 断言响应为成功
    pub fn assert_success(response: &TestResponse) {
        assert!(
            response.is_success(),
            "Expected success, got status {}",
            response.status
        );
    }

    /// 断言 JSON 字段存在
    pub fn assert_json_field(response: &TestResponse, field: &str) {
        let json = response.json();
        assert!(
            json.get(field).is_some(),
            "Expected field '{}' in JSON response",
            field
        );
    }

    /// 断言 JSON 字段值
    pub fn assert_json_value(response: &TestResponse, field: &str, expected: &Value) {
        let json = response.json();
        let actual = json
            .get(field)
            .unwrap_or_else(|| panic!("Field '{}' not found", field));
        assert_eq!(
            actual, expected,
            "Expected field '{}' to be {:?}, got {:?}",
            field, expected, actual
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_server_creation() {
        let _server = TestServer::new();
        // 只是确保能成功创建服务器
    }

    #[tokio::test]
    async fn test_jwt_generation() {
        let server = TestServer::new();
        let token = server.generate_jwt();
        assert!(!token.is_empty());
    }

    #[tokio::test]
    async fn test_data_generator() {
        let request = TestDataGenerator::create_task_request("Test Task", "Analysis");
        assert_eq!(request["title"], "Test Task");
        assert_eq!(request["kind"], "Analysis");
    }
}
