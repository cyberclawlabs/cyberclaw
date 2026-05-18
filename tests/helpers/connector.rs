// tests/helpers/connector.rs
// Connector 测试辅助工具

use anyhow::Result;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Mock MCP Server
pub struct MockMcpServer {
    tools: Arc<RwLock<HashMap<String, ToolDefinition>>>,
    resources: Arc<RwLock<HashMap<String, ResourceDefinition>>>,
    port: u16,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

#[derive(Clone, Debug)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub handler: Arc<dyn Fn(Value) -> Value + Send + Sync>,
}

#[derive(Clone, Debug)]
pub struct ResourceDefinition {
    pub uri: String,
    pub name: String,
    pub mime_type: Option<String>,
    pub content: String,
}

impl MockMcpServer {
    /// 创建新的 Mock MCP Server
    pub async fn new() -> Result<Self> {
        Ok(Self {
            tools: Arc::new(RwLock::new(HashMap::new())),
            resources: Arc::new(RwLock::new(HashMap::new())),
            port: 0,
            shutdown_tx: None,
        })
    }

    /// 添加 Tool
    pub async fn add_tool<F>(
        &self,
        name: &str,
        description: &str,
        input_schema: Value,
        handler: F,
    ) -> Result<()>
    where
        F: Fn(Value) -> Value + Send + Sync + 'static,
    {
        let tool = ToolDefinition {
            name: name.to_string(),
            description: description.to_string(),
            input_schema,
            handler: Arc::new(handler),
        };

        self.tools.write().await.insert(name.to_string(), tool);
        Ok(())
    }

    /// 添加 Resource
    pub async fn add_resource(
        &self,
        uri: &str,
        name: &str,
        mime_type: Option<String>,
        content: &str,
    ) -> Result<()> {
        let resource = ResourceDefinition {
            uri: uri.to_string(),
            name: name.to_string(),
            mime_type,
            content: content.to_string(),
        };

        self.resources.write().await.insert(uri.to_string(), resource);
        Ok(())
    }

    /// 启动服务器
    pub async fn start(&mut self) -> Result<String> {
        use axum::{
            extract::State,
            routing::post,
            Router,
            Json,
        };

        let tools = self.tools.clone();
        let resources = self.resources.clone();

        let app = Router::new()
            .route("/rpc", post(handle_rpc))
            .with_state((tools, resources));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        self.port = listener.local_addr()?.port();
        let url = format!("http://127.0.0.1:{}", self.port);

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        self.shutdown_tx = Some(shutdown_tx);

        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    shutdown_rx.await.ok();
                })
                .await
                .unwrap();
        });

        Ok(url)
    }

    /// 停止服务器
    pub async fn stop(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

async fn handle_rpc(
    State((tools, resources)): State<(
        Arc<RwLock<HashMap<String, ToolDefinition>>>,
        Arc<RwLock<HashMap<String, ResourceDefinition>>>,
    )>,
    Json(request): Json<Value>,
) -> Json<Value> {
    let method = request["method"].as_str().unwrap_or("");
    let id = request["id"].clone();

    let result = match method {
        "tools/list" => {
            let tools = tools.read().await;
            let tool_list: Vec<Value> = tools
                .values()
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.input_schema,
                    })
                })
                .collect();
            json!(tool_list)
        }
        "tools/call" => {
            let tool_name = request["params"]["name"].as_str().unwrap_or("");
            let args = request["params"]["arguments"].clone();

            let tools = tools.read().await;
            if let Some(tool) = tools.get(tool_name) {
                (tool.handler)(args)
            } else {
                json!({"error": "Tool not found"})
            }
        }
        "resources/list" => {
            let resources = resources.read().await;
            let resource_list: Vec<Value> = resources
                .values()
                .map(|r| {
                    json!({
                        "uri": r.uri,
                        "name": r.name,
                        "mime_type": r.mime_type,
                    })
                })
                .collect();
            json!(resource_list)
        }
        "resources/read" => {
            let uri = request["params"]["uri"].as_str().unwrap_or("");

            let resources = resources.read().await;
            if let Some(resource) = resources.get(uri) {
                json!({
                    "content": resource.content,
                    "mime_type": resource.mime_type,
                })
            } else {
                json!({"error": "Resource not found"})
            }
        }
        _ => json!({"error": "Method not found"}),
    };

    Json(json!({
        "jsonrpc": "2.0",
        "result": result,
        "id": id,
    }))
}

/// Mock GitHub API
pub struct MockGitHubApi {
    issues: Arc<RwLock<Vec<Issue>>>,
    repos: Arc<RwLock<Vec<Repository>>>,
}

#[derive(Clone, Debug)]
pub struct Issue {
    pub id: u64,
    pub title: String,
    pub body: String,
    pub state: String,
}

#[derive(Clone, Debug)]
pub struct Repository {
    pub id: u64,
    pub name: String,
    pub owner: String,
    pub description: Option<String>,
}

impl MockGitHubApi {
    pub async fn new() -> Self {
        Self {
            issues: Arc::new(RwLock::new(Vec::new())),
            repos: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub async fn add_repo(&self, name: &str, owner: &str) -> Result<u64> {
        let mut repos = self.repos.write().await;
        let id = repos.len() as u64 + 1;
        repos.push(Repository {
            id,
            name: name.to_string(),
            owner: owner.to_string(),
            description: None,
        });
        Ok(id)
    }

    pub async fn create_issue(&self, title: &str, body: &str) -> Result<u64> {
        let mut issues = self.issues.write().await;
        let id = issues.len() as u64 + 1;
        issues.push(Issue {
            id,
            title: title.to_string(),
            body: body.to_string(),
            state: "open".to_string(),
        });
        Ok(id)
    }

    pub async fn get_issues(&self) -> Vec<Issue> {
        self.issues.read().await.clone()
    }
}

/// Mock Database Connection
pub struct MockDatabase {
    data: Arc<RwLock<HashMap<String, Vec<HashMap<String, Value>>>>>,
}

impl MockDatabase {
    pub async fn new() -> Self {
        Self {
            data: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn create_table(&self, table_name: &str) -> Result<()> {
        let mut data = self.data.write().await;
        data.insert(table_name.to_string(), Vec::new());
        Ok(())
    }

    pub async fn insert(&self, table_name: &str, row: HashMap<String, Value>) -> Result<()> {
        let mut data = self.data.write().await;
        if let Some(table) = data.get_mut(table_name) {
            table.push(row);
            Ok(())
        } else {
            Err(anyhow::anyhow!("Table not found"))
        }
    }

    pub async fn query(&self, table_name: &str) -> Result<Vec<HashMap<String, Value>>> {
        let data = self.data.read().await;
        Ok(data.get(table_name).cloned().unwrap_or_default())
    }
}

/// Mock Slack API
pub struct MockSlackApi {
    messages: Arc<RwLock<Vec<SlackMessage>>>,
    channels: Arc<RwLock<Vec<String>>>,
}

#[derive(Clone, Debug)]
pub struct SlackMessage {
    pub channel: String,
    pub text: String,
    pub timestamp: String,
}

impl MockSlackApi {
    pub async fn new() -> Self {
        Self {
            messages: Arc::new(RwLock::new(Vec::new())),
            channels: Arc::new(RwLock::new(vec!["general".to_string()])),
        }
    }

    pub async fn send_message(&self, channel: &str, text: &str) -> Result<String> {
        let timestamp = chrono::Utc::now().to_rfc3339();

        let mut messages = self.messages.write().await;
        messages.push(SlackMessage {
            channel: channel.to_string(),
            text: text.to_string(),
            timestamp: timestamp.clone(),
        });

        Ok(timestamp)
    }

    pub async fn get_messages(&self, channel: &str) -> Vec<SlackMessage> {
        let messages = self.messages.read().await;
        messages
            .iter()
            .filter(|m| m.channel == channel)
            .cloned()
            .collect()
    }

    pub async fn create_channel(&self, name: &str) -> Result<()> {
        let mut channels = self.channels.write().await;
        if !channels.contains(&name.to_string()) {
            channels.push(name.to_string());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_mcp_server() {
        let mut server = MockMcpServer::new().await.unwrap();

        // 添加测试 Tool
        server.add_tool(
            "test_tool",
            "Test tool",
            json!({"type": "object"}),
            |input| json!({"result": input}),
        ).await.unwrap();

        // 添加测试 Resource
        server.add_resource(
            "test://resource",
            "Test Resource",
            Some("text/plain".to_string()),
            "Test content",
        ).await.unwrap();

        let url = server.start().await.unwrap();
        assert!(url.starts_with("http://127.0.0.1:"));

        server.stop().await;
    }

    #[tokio::test]
    async fn test_mock_database() {
        let db = MockDatabase::new().await;

        db.create_table("users").await.unwrap();

        let mut user = HashMap::new();
        user.insert("id".to_string(), json!(1));
        user.insert("name".to_string(), json!("Test User"));

        db.insert("users", user).await.unwrap();

        let results = db.query("users").await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["name"], json!("Test User"));
    }
}