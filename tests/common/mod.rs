//! 测试辅助工具模块
//!
//! 提供 E2E 测试所需的 Mock 服务器、临时文件管理和测试数据生成功能。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::RwLock;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use anyhow::{Result, Context};

/// Plugin 测试辅助工具
pub mod plugin {
    use super::*;

    /// 创建临时测试 Plugin
    pub async fn create_test_plugin(name: &str) -> Result<(TempDir, PathBuf)> {
        let temp_dir = TempDir::new()?;
        let plugin_dir = temp_dir.path().join(name);
        tokio::fs::create_dir_all(&plugin_dir).await?;

        // 创建 Plugin manifest
        let manifest = format!(
            r#"[plugin]
id = "{}"
name = "Test Plugin - {}"
version = "0.1.0"
description = "Test plugin for E2E testing"
authors = ["CyberClaw Test"]

[plugin.library]
path = "target/release/lib{}.so"
entry_point = "cyberclaw_plugin_init"

[plugin.hooks]
before_execution = {{ handler = "before_exec_hook", priority = 100, timeout_ms = 5000 }}
after_execution = {{ handler = "after_exec_hook", priority = 100, timeout_ms = 5000 }}

[plugin.capabilities]
required = ["fs.read", "network.http"]

[plugin.resources]
memory = 104857600  # 100 MB
cpu_ms = 10000      # 10 seconds
file_handles = 100
network_connections = 10

[plugin.metadata]
homepage = "https://cyberclaw.test"
repository = "https://github.com/cyberclawlabs/test-plugin"
license = "Apache-2.0"
"#,
            name, name, name
        );

        tokio::fs::write(
            plugin_dir.join("cyberclaw-plugin.toml"),
            manifest,
        )
        .await?;

        // 创建示例 hook 实现文件
        let hook_code = r#"
use cyberclaw_plugin_runtime::{HookContext, HookOutput, Result};

pub fn before_exec_hook(context: &HookContext) -> Result<HookOutput> {
    Ok(HookOutput {
        modified_params: None,
        metadata: vec![("hook".to_string(), "before_exec".to_string())],
    })
}

pub fn after_exec_hook(context: &HookContext) -> Result<HookOutput> {
    Ok(HookOutput {
        modified_params: None,
        metadata: vec![("hook".to_string(), "after_exec".to_string())],
    })
}
"#;

        tokio::fs::write(plugin_dir.join("hooks.rs"), hook_code).await?;

        Ok((temp_dir, plugin_dir))
    }

    /// 创建带有自定义 Hook 的测试 Plugin
    pub async fn create_plugin_with_hooks(
        name: &str,
        hooks: Vec<HookConfig>,
    ) -> Result<(TempDir, PathBuf)> {
        let (temp_dir, plugin_dir) = create_test_plugin(name).await?;

        // 更新 manifest 添加自定义 hooks
        let mut manifest = String::from("[plugin.hooks]\n");
        for hook in hooks {
            manifest.push_str(&format!(
                "{} = {{ handler = \"{}\", priority = {}, timeout_ms = {} }}\n",
                hook.hook_type, hook.handler_name, hook.priority, hook.timeout_ms
            ));
        }

        let manifest_path = plugin_dir.join("cyberclaw-plugin.toml");
        let existing = tokio::fs::read_to_string(&manifest_path).await?;
        let updated = existing.replace("[plugin.hooks]", &manifest);
        tokio::fs::write(&manifest_path, updated).await?;

        Ok((temp_dir, plugin_dir))
    }

    #[derive(Debug, Clone)]
    pub struct HookConfig {
        pub hook_type: String,
        pub handler_name: String,
        pub priority: i32,
        pub timeout_ms: u64,
    }
}

/// MCP Server Mock
pub mod mcp {
    use super::*;
    use axum::{
        Router,
        routing::{get, post},
        Json,
        response::IntoResponse,
        extract::State,
    };
    use std::net::SocketAddr;
    use std::collections::HashMap;

    /// Mock MCP Server
    pub struct MockMcpServer {
        addr: SocketAddr,
        tools: Arc<RwLock<Vec<McpTool>>>,
        resources: Arc<RwLock<Vec<McpResource>>>,
        shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct McpTool {
        pub name: String,
        pub description: String,
        pub input_schema: serde_json::Value,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct McpResource {
        pub uri: String,
        pub name: String,
        pub mime_type: Option<String>,
    }

    impl MockMcpServer {
        /// 启动 Mock MCP Server
        pub async fn start() -> Result<Self> {
            Self::start_with_port(0).await // 0 = 随机端口
        }

        /// 启动指定端口的 Mock MCP Server
        pub async fn start_with_port(port: u16) -> Result<Self> {
            let tools = Arc::new(RwLock::new(vec![
                McpTool {
                    name: "test_tool".to_string(),
                    description: "A test tool".to_string(),
                    input_schema: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "message": { "type": "string" }
                        }
                    }),
                },
            ]));

            let resources = Arc::new(RwLock::new(vec![
                McpResource {
                    uri: "test://resource".to_string(),
                    name: "Test Resource".to_string(),
                    mime_type: Some("text/plain".to_string()),
                },
            ]));

            let app = Router::new()
                .route("/rpc", post(handle_rpc))
                .route("/health", get(|| async { "OK" }))
                .with_state(ServerState {
                    tools: tools.clone(),
                    resources: resources.clone(),
                });

            let addr = SocketAddr::from(([127, 0, 0, 1], port));
            let listener = tokio::net::TcpListener::bind(&addr).await?;
            let actual_addr = listener.local_addr()?;

            let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();

            // 启动服务器
            tokio::spawn(async move {
                let server = axum::serve(listener, app)
                    .with_graceful_shutdown(async {
                        shutdown_rx.await.ok();
                    });
                server.await.ok();
            });

            Ok(MockMcpServer {
                addr: actual_addr,
                tools,
                resources,
                shutdown_tx: Some(shutdown_tx),
            })
        }

        pub fn addr(&self) -> SocketAddr {
            self.addr
        }

        pub fn url(&self) -> String {
            format!("http://{}/rpc", self.addr)
        }

        pub async fn add_tool(&self, tool: McpTool) {
            self.tools.write().await.push(tool);
        }

        pub async fn add_resource(&self, resource: McpResource) {
            self.resources.write().await.push(resource);
        }

        pub async fn shutdown(mut self) {
            if let Some(tx) = self.shutdown_tx.take() {
                tx.send(()).ok();
            }
        }
    }

    #[derive(Clone)]
    struct ServerState {
        tools: Arc<RwLock<Vec<McpTool>>>,
        resources: Arc<RwLock<Vec<McpResource>>>,
    }

    async fn handle_rpc(
        State(state): State<ServerState>,
        Json(request): Json<serde_json::Value>,
    ) -> impl IntoResponse {
        let method = request["method"].as_str().unwrap_or("");

        let response = match method {
            "tools/list" => {
                let tools = state.tools.read().await;
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": request["id"],
                    "result": tools.clone(),
                })
            }
            "resources/list" => {
                let resources = state.resources.read().await;
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": request["id"],
                    "result": resources.clone(),
                })
            }
            "tools/call" => {
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": request["id"],
                    "result": {
                        "output": "Tool executed successfully",
                        "metadata": {}
                    }
                })
            }
            "resources/read" => {
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": request["id"],
                    "result": {
                        "content": "Test resource content",
                        "metadata": {}
                    }
                })
            }
            _ => {
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": request["id"],
                    "error": {
                        "code": -32601,
                        "message": "Method not found"
                    }
                })
            }
        };

        Json(response)
    }
}

/// GitHub Mock Server
pub mod github {
    use super::*;
    use axum::{
        Router,
        routing::{get, post},
        Json,
        response::IntoResponse,
        extract::{Path as AxumPath, State},
    };
    use std::net::SocketAddr;

    pub struct MockGitHubServer {
        addr: SocketAddr,
        repos: Arc<RwLock<Vec<Repository>>>,
        issues: Arc<RwLock<Vec<Issue>>>,
        shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Repository {
        pub owner: String,
        pub name: String,
        pub description: Option<String>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Issue {
        pub id: u64,
        pub title: String,
        pub body: String,
        pub state: String,
    }

    impl MockGitHubServer {
        pub async fn start() -> Result<Self> {
            let repos = Arc::new(RwLock::new(vec![]));
            let issues = Arc::new(RwLock::new(vec![]));

            let app = Router::new()
                .route("/repos/:owner/:repo", get(get_repo))
                .route("/repos/:owner/:repo/issues", get(list_issues).post(create_issue))
                .route("/health", get(|| async { "OK" }))
                .with_state(ServerState {
                    repos: repos.clone(),
                    issues: issues.clone(),
                });

            let addr = SocketAddr::from(([127, 0, 0, 1], 0));
            let listener = tokio::net::TcpListener::bind(&addr).await?;
            let actual_addr = listener.local_addr()?;

            let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();

            tokio::spawn(async move {
                let server = axum::serve(listener, app)
                    .with_graceful_shutdown(async {
                        shutdown_rx.await.ok();
                    });
                server.await.ok();
            });

            Ok(MockGitHubServer {
                addr: actual_addr,
                repos,
                issues,
                shutdown_tx: Some(shutdown_tx),
            })
        }

        pub fn addr(&self) -> SocketAddr {
            self.addr
        }

        pub fn url(&self) -> String {
            format!("http://{}", self.addr)
        }

        pub async fn add_repo(&self, repo: Repository) {
            self.repos.write().await.push(repo);
        }

        pub async fn add_issue(&self, issue: Issue) {
            self.issues.write().await.push(issue);
        }

        pub async fn shutdown(mut self) {
            if let Some(tx) = self.shutdown_tx.take() {
                tx.send(()).ok();
            }
        }
    }

    #[derive(Clone)]
    struct ServerState {
        repos: Arc<RwLock<Vec<Repository>>>,
        issues: Arc<RwLock<Vec<Issue>>>,
    }

    async fn get_repo(
        AxumPath((owner, repo)): AxumPath<(String, String)>,
        State(state): State<ServerState>,
    ) -> impl IntoResponse {
        let repos = state.repos.read().await;
        for r in repos.iter() {
            if r.owner == owner && r.name == repo {
                return Json(serde_json::json!(r));
            }
        }
        Json(serde_json::json!({
            "message": "Not Found",
            "documentation_url": "https://docs.github.com/rest"
        }))
    }

    async fn list_issues(
        State(state): State<ServerState>,
    ) -> impl IntoResponse {
        let issues = state.issues.read().await;
        Json(serde_json::json!(issues.clone()))
    }

    async fn create_issue(
        State(state): State<ServerState>,
        Json(payload): Json<serde_json::Value>,
    ) -> impl IntoResponse {
        let issue = Issue {
            id: Uuid::new_v4().as_u128() as u64,
            title: payload["title"].as_str().unwrap_or("").to_string(),
            body: payload["body"].as_str().unwrap_or("").to_string(),
            state: "open".to_string(),
        };

        state.issues.write().await.push(issue.clone());

        Json(serde_json::json!(issue))
    }
}

/// Slack Mock Server
pub mod slack {
    use super::*;
    use axum::{
        Router,
        routing::post,
        Json,
        response::IntoResponse,
        extract::State,
    };
    use std::net::SocketAddr;

    pub struct MockSlackServer {
        addr: SocketAddr,
        messages: Arc<RwLock<Vec<Message>>>,
        shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Message {
        pub channel: String,
        pub text: String,
        pub timestamp: String,
    }

    impl MockSlackServer {
        pub async fn start() -> Result<Self> {
            let messages = Arc::new(RwLock::new(vec![]));

            let app = Router::new()
                .route("/api/chat.postMessage", post(post_message))
                .route("/api/channels.create", post(create_channel))
                .with_state(ServerState {
                    messages: messages.clone(),
                });

            let addr = SocketAddr::from(([127, 0, 0, 1], 0));
            let listener = tokio::net::TcpListener::bind(&addr).await?;
            let actual_addr = listener.local_addr()?;

            let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();

            tokio::spawn(async move {
                let server = axum::serve(listener, app)
                    .with_graceful_shutdown(async {
                        shutdown_rx.await.ok();
                    });
                server.await.ok();
            });

            Ok(MockSlackServer {
                addr: actual_addr,
                messages,
                shutdown_tx: Some(shutdown_tx),
            })
        }

        pub fn addr(&self) -> SocketAddr {
            self.addr
        }

        pub fn url(&self) -> String {
            format!("http://{}/api", self.addr)
        }

        pub async fn get_messages(&self) -> Vec<Message> {
            self.messages.read().await.clone()
        }

        pub async fn shutdown(mut self) {
            if let Some(tx) = self.shutdown_tx.take() {
                tx.send(()).ok();
            }
        }
    }

    #[derive(Clone)]
    struct ServerState {
        messages: Arc<RwLock<Vec<Message>>>,
    }

    async fn post_message(
        State(state): State<ServerState>,
        Json(payload): Json<serde_json::Value>,
    ) -> impl IntoResponse {
        let message = Message {
            channel: payload["channel"].as_str().unwrap_or("").to_string(),
            text: payload["text"].as_str().unwrap_or("").to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
        };

        state.messages.write().await.push(message.clone());

        Json(serde_json::json!({
            "ok": true,
            "channel": message.channel,
            "ts": message.timestamp,
            "message": {
                "text": message.text,
                "ts": message.timestamp,
            }
        }))
    }

    async fn create_channel(
        Json(payload): Json<serde_json::Value>,
    ) -> impl IntoResponse {
        Json(serde_json::json!({
            "ok": true,
            "channel": {
                "id": format!("C{}", Uuid::new_v4().as_u128() as u64),
                "name": payload["name"].as_str().unwrap_or("test-channel"),
            }
        }))
    }
}

/// 数据库 Mock
pub mod database {
    use super::*;

    pub struct MockDatabase {
        data: Arc<RwLock<HashMap<String, Vec<serde_json::Value>>>>,
    }

    impl MockDatabase {
        pub fn new() -> Self {
            Self {
                data: Arc::new(RwLock::new(HashMap::new())),
            }
        }

        pub async fn execute(&self, query: &str) -> Result<Vec<serde_json::Value>> {
            // 简单的 SQL 解析模拟
            if query.starts_with("SELECT") {
                let table = self.extract_table_name(query);
                let data = self.data.read().await;
                Ok(data.get(&table).cloned().unwrap_or_default())
            } else if query.starts_with("INSERT") {
                let table = self.extract_table_name(query);
                let values = self.extract_values(query);
                self.data.write().await
                    .entry(table)
                    .or_insert_with(Vec::new)
                    .push(values);
                Ok(vec![serde_json::json!({"affected": 1})])
            } else {
                Ok(vec![])
            }
        }

        fn extract_table_name(&self, query: &str) -> String {
            // 简化的表名提取
            if let Some(from_pos) = query.find("FROM") {
                let after_from = &query[from_pos + 4..];
                after_from.split_whitespace()
                    .next()
                    .unwrap_or("unknown")
                    .to_string()
            } else if let Some(into_pos) = query.find("INTO") {
                let after_into = &query[into_pos + 4..];
                after_into.split_whitespace()
                    .next()
                    .unwrap_or("unknown")
                    .to_string()
            } else {
                "unknown".to_string()
            }
        }

        fn extract_values(&self, query: &str) -> serde_json::Value {
            // 简化的值提取
            serde_json::json!({
                "id": Uuid::new_v4().to_string(),
                "data": "test"
            })
        }
    }
}

/// 测试配置
pub struct TestConfig {
    pub enable_logging: bool,
    pub timeout: std::time::Duration,
}

impl Default for TestConfig {
    fn default() -> Self {
        Self {
            enable_logging: true,
            timeout: std::time::Duration::from_secs(30),
        }
    }
}

/// 初始化测试环境
pub fn init_test_env() {
    // 设置测试日志
    let _ = tracing_subscriber::fmt()
        .with_env_filter("cyberclaw=debug")
        .with_test_writer()
        .try_init();
}

/// 创建测试运行时
pub fn test_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to create test runtime")
}