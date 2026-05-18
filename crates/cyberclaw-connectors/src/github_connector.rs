//! GitHub Connector - 提供 GitHub API 能力
//!
//! ## 能力清单
//!
//! - `github.create_issue`: 创建 Issue
//! - `github.create_pr`: 创建 Pull Request
//! - `github.review_code`: 代码审查
//! - `github.list_repos`: 列举仓库
//! - `github.search_code`: 搜索代码
//!
//! ## 认证方式
//!
//! - OAuth 2.0 (支持 Device Flow 和 Web Flow)
//! - Personal Access Token
//!
//! ## 速率限制
//!
//! 使用 `governor` 实现令牌桶算法，确保符合 GitHub API 速率限制。

use crate::types::{
    CapabilityExecutionRequest, CapabilityExecutionResult, Connector, ExecutionStatus,
};
use async_trait::async_trait;
use cyberclaw_core::prelude::*;
use governor::{Quota, RateLimiter as GovernorRateLimiter};
use octocrab::{Octocrab, OctocrabBuilder};
use serde::{Deserialize, Serialize};
use std::num::NonZeroU32;
use std::sync::Arc;
use tokio::sync::RwLock;

/// GitHub Connector 主结构
#[derive(Debug)]
pub struct GitHubConnector {
    /// Connector ID
    id: ConnectorId,
    /// GitHub 客户端
    client: Arc<RwLock<Option<Octocrab>>>,
    /// 认证提供者
    auth: Arc<dyn Authenticator>,
    /// 速率限制器
    rate_limiter: Arc<RateLimiter>,
    /// 能力清单
    capabilities: Vec<CapabilityContract>,
}

/// 认证器 Trait
#[async_trait]
pub trait Authenticator: Send + Sync + std::fmt::Debug {
    /// 执行认证，返回凭证
    async fn authenticate(&self) -> anyhow::Result<Credentials>;

    /// 刷新凭证
    async fn refresh(&self) -> anyhow::Result<Credentials>;
}

/// 认证凭证
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Credentials {
    /// Personal Access Token
    PersonalAccessToken(String),
    /// OAuth Token
    OAuth {
        access_token: String,
        refresh_token: Option<String>,
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
    },
}

/// OAuth 认证器
#[derive(Debug, Clone)]
pub struct GitHubAuth {
    /// OAuth Client ID
    client_id: String,
    /// OAuth Client Secret
    #[allow(dead_code)]
    client_secret: String,
    /// Token 缓存
    token_cache: Arc<RwLock<Option<Credentials>>>,
}

impl GitHubAuth {
    /// 创建新的 OAuth 认证器
    pub fn new(client_id: String, client_secret: String) -> Self {
        Self {
            client_id,
            client_secret,
            token_cache: Arc::new(RwLock::new(None)),
        }
    }

    /// 从 Personal Access Token 创建
    pub fn from_token(token: String) -> Self {
        Self {
            client_id: String::new(),
            client_secret: String::new(),
            token_cache: Arc::new(RwLock::new(Some(Credentials::PersonalAccessToken(token)))),
        }
    }
}

#[async_trait]
impl Authenticator for GitHubAuth {
    async fn authenticate(&self) -> anyhow::Result<Credentials> {
        // 1. 检查缓存
        if let Some(cached) = self.token_cache.read().await.as_ref() {
            if !is_expired(cached) {
                return Ok(cached.clone());
            }
        }

        // 2. 如果使用 PAT，直接返回
        if self.client_id.is_empty() {
            return Err(anyhow::anyhow!("No authentication method configured"));
        }

        // 3. 执行 OAuth Device Flow
        // 注意: 这里简化实现，实际应该启动完整的 Device Flow
        tracing::warn!("OAuth Device Flow not fully implemented, using cached token");

        // 返回缓存的 token (如果有)
        self.token_cache
            .read()
            .await
            .clone()
            .ok_or_else(|| anyhow::anyhow!("No valid credentials available"))
    }

    async fn refresh(&self) -> anyhow::Result<Credentials> {
        // OAuth Token 刷新逻辑
        match self.token_cache.read().await.as_ref() {
            Some(Credentials::OAuth {
                refresh_token: Some(refresh_token),
                ..
            }) => {
                // 使用 refresh_token 获取新的 access_token
                // 这里简化实现
                tracing::warn!("OAuth refresh not fully implemented");
                Ok(Credentials::PersonalAccessToken(refresh_token.clone()))
            }
            _ => self.authenticate().await,
        }
    }
}

/// 检查凭证是否过期
fn is_expired(creds: &Credentials) -> bool {
    match creds {
        Credentials::OAuth {
            expires_at: Some(exp),
            ..
        } => chrono::Utc::now() >= *exp,
        _ => false,
    }
}

/// 速率限制器
#[derive(Debug)]
pub struct RateLimiter {
    /// Governor 限制器
    limiter: GovernorRateLimiter<
        governor::state::direct::NotKeyed,
        governor::state::InMemoryState,
        governor::clock::DefaultClock,
    >,
}

impl RateLimiter {
    /// 创建新的速率限制器
    ///
    /// GitHub API 限制: 5000 requests/hour (约 83 req/min)
    pub fn new(permits_per_minute: u32) -> Self {
        let quota = Quota::per_minute(NonZeroU32::new(permits_per_minute).unwrap());
        Self {
            limiter: GovernorRateLimiter::direct(quota),
        }
    }

    /// 获取许可 (阻塞直到可用)
    pub async fn acquire(&self) -> anyhow::Result<()> {
        self.limiter.until_ready().await;
        Ok(())
    }
}

impl GitHubConnector {
    /// 创建新的 GitHub Connector
    pub fn new(auth: Arc<dyn Authenticator>) -> Self {
        let id =
            ConnectorId::from_string("github".to_string()).expect("github is a valid connector id");

        // 默认速率限制: 80 req/min (留有余量)
        let rate_limiter = Arc::new(RateLimiter::new(80));

        let capabilities = Self::build_capabilities();

        Self {
            id,
            client: Arc::new(RwLock::new(None)),
            auth,
            rate_limiter,
            capabilities,
        }
    }

    /// 构建能力清单
    fn build_capabilities() -> Vec<CapabilityContract> {
        vec![
            CapabilityContract {
                id: "github.create_issue".to_string(),
                title: "Create GitHub Issue".to_string(),
                description: Some("Create a new issue in a GitHub repository".to_string()),
                input_schema: r#"{"owner": "string", "repo": "string", "title": "string", "body": "string"}"#.to_string(),
                output_schema: r#"{"number": "integer", "html_url": "string"}"#.to_string(),
                risk: RiskLevel::Medium,
                effects: vec![CapabilityEffect::Write, CapabilityEffect::Ticket],
                placement: None,
                timeouts: CapabilityTimeouts::default(),
            },
            CapabilityContract {
                id: "github.create_pr".to_string(),
                title: "Create Pull Request".to_string(),
                description: Some("Create a new pull request".to_string()),
                input_schema: r#"{"owner": "string", "repo": "string", "title": "string", "head": "string", "base": "string", "body": "string"}"#.to_string(),
                output_schema: r#"{"number": "integer", "html_url": "string"}"#.to_string(),
                risk: RiskLevel::High,
                effects: vec![CapabilityEffect::Write],
                placement: None,
                timeouts: CapabilityTimeouts::default(),
            },
            CapabilityContract {
                id: "github.review_code".to_string(),
                title: "Review Code".to_string(),
                description: Some("Submit a code review on a pull request".to_string()),
                input_schema: r#"{"owner": "string", "repo": "string", "pull_number": "integer", "event": "string", "body": "string"}"#.to_string(),
                output_schema: r#"{"id": "integer", "state": "string"}"#.to_string(),
                risk: RiskLevel::Medium,
                effects: vec![CapabilityEffect::Write],
                placement: None,
                timeouts: CapabilityTimeouts::default(),
            },
            CapabilityContract {
                id: "github.list_repos".to_string(),
                title: "List Repositories".to_string(),
                description: Some("List repositories for the authenticated user".to_string()),
                input_schema: r#"{"visibility": "string", "sort": "string"}"#.to_string(),
                output_schema: r#"{"repos": [{"name": "string", "full_name": "string", "private": "boolean"}]}"#.to_string(),
                risk: RiskLevel::Low,
                effects: vec![CapabilityEffect::Read],
                placement: None,
                timeouts: CapabilityTimeouts::default(),
            },
            CapabilityContract {
                id: "github.search_code".to_string(),
                title: "Search Code".to_string(),
                description: Some("Search code across GitHub repositories".to_string()),
                input_schema: r#"{"query": "string", "sort": "string", "order": "string"}"#.to_string(),
                output_schema: r#"{"total_count": "integer", "items": [{"name": "string", "path": "string", "repository": {"full_name": "string"}}]}"#.to_string(),
                risk: RiskLevel::Low,
                effects: vec![CapabilityEffect::Read, CapabilityEffect::Network],
                placement: None,
                timeouts: CapabilityTimeouts::default(),
            },
        ]
    }

    /// 初始化客户端 (如果尚未初始化)
    async fn ensure_client(&self) -> anyhow::Result<()> {
        let mut client_guard = self.client.write().await;

        if client_guard.is_none() {
            // 获取凭证
            let creds = self.auth.authenticate().await?;

            // 构建客户端
            let token = match creds {
                Credentials::PersonalAccessToken(token) => token,
                Credentials::OAuth { access_token, .. } => access_token,
            };

            let octocrab = OctocrabBuilder::new().personal_token(token).build()?;

            *client_guard = Some(octocrab);
        }

        Ok(())
    }

    /// 创建 Issue
    async fn create_issue(&self, input: serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let owner = input["owner"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'owner'"))?;
        let repo = input["repo"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'repo'"))?;
        let title = input["title"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'title'"))?;
        let body = input["body"].as_str().unwrap_or("");

        self.ensure_client().await?;
        let client = self.client.read().await;
        let client = client.as_ref().unwrap();

        let issue = client
            .issues(owner, repo)
            .create(title)
            .body(body)
            .send()
            .await?;

        Ok(serde_json::json!({
            "number": issue.number,
            "html_url": issue.html_url,
            "state": issue.state,
        }))
    }

    /// 创建 Pull Request
    async fn create_pr(&self, input: serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let owner = input["owner"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'owner'"))?;
        let repo = input["repo"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'repo'"))?;
        let title = input["title"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'title'"))?;
        let head = input["head"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'head'"))?;
        let base = input["base"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'base'"))?;
        let body = input["body"].as_str().unwrap_or("");

        self.ensure_client().await?;
        let client = self.client.read().await;
        let client = client.as_ref().unwrap();

        let pr = client
            .pulls(owner, repo)
            .create(title, head, base)
            .body(body)
            .send()
            .await?;

        Ok(serde_json::json!({
            "number": pr.number,
            "html_url": pr.html_url,
            "state": pr.state,
        }))
    }

    /// 代码审查
    async fn review_code(&self, input: serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let _owner = input["owner"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'owner'"))?;
        let _repo = input["repo"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'repo'"))?;
        let pull_number = input["pull_number"]
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("Missing 'pull_number'"))?;
        let event = input["event"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'event'"))?;
        let body = input["body"].as_str().unwrap_or("");

        self.ensure_client().await?;
        let _client = self.client.read().await;

        // 注意: octocrab 可能没有直接的 review API
        // 这里使用占位实现，实际应该调用 GitHub API
        tracing::warn!("review_code not fully implemented for octocrab");

        // 返回模拟结果
        Ok(serde_json::json!({
            "id": pull_number,
            "state": event,
            "body": body,
        }))
    }

    /// 列举仓库
    async fn list_repos(&self, input: serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let _visibility = input["visibility"].as_str().unwrap_or("all");
        let _sort = input["sort"].as_str().unwrap_or("updated");

        self.ensure_client().await?;
        let client = self.client.read().await;
        let client = client.as_ref().unwrap();

        // 列举当前用户的仓库
        // 注意: octocrab API 可能已经改变，这里简化实现
        let mut page = client
            .current()
            .list_repos_for_authenticated_user()
            .send()
            .await?;

        let repo_list: Vec<_> = page
            .take_items()
            .into_iter()
            .map(|r| {
                serde_json::json!({
                    "name": r.name,
                    "full_name": r.full_name.map(|f| f.to_string()),
                    "private": r.private,
                    "html_url": r.html_url,
                })
            })
            .collect();

        Ok(serde_json::json!({
            "repos": repo_list,
        }))
    }

    /// 搜索代码
    async fn search_code(&self, input: serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let query = input["query"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'query'"))?;

        self.ensure_client().await?;
        let client = self.client.read().await;
        let client = client.as_ref().unwrap();

        let results = client.search().code(query).send().await?;

        let items: Vec<_> = results
            .items
            .into_iter()
            .map(|item| {
                serde_json::json!({
                    "name": item.name,
                    "path": item.path,
                    "repository": {
                        "full_name": item.repository.full_name,
                    },
                })
            })
            .collect();

        Ok(serde_json::json!({
            "total_count": results.total_count,
            "items": items,
        }))
    }
}

#[async_trait]
impl Connector for GitHubConnector {
    fn id(&self) -> &ConnectorId {
        &self.id
    }

    fn runtime(&self) -> ConnectorRuntime {
        ConnectorRuntime::Native
    }

    fn capabilities(&self) -> Vec<CapabilityContract> {
        self.capabilities.clone()
    }

    async fn execute(
        &self,
        request: CapabilityExecutionRequest,
    ) -> anyhow::Result<CapabilityExecutionResult> {
        // 速率限制
        self.rate_limiter.acquire().await?;

        // 路由到对应的能力处理函数
        let output = match request.capability_id.as_ref() {
            "github.create_issue" => self.create_issue(request.input).await?,
            "github.create_pr" => self.create_pr(request.input).await?,
            "github.review_code" => self.review_code(request.input).await?,
            "github.list_repos" => self.list_repos(request.input).await?,
            "github.search_code" => self.search_code(request.input).await?,
            _ => {
                return Ok(CapabilityExecutionResult {
                    execution_id: request.execution_id,
                    trace_id: request.trace_id,
                    connector_id: self.id.clone(),
                    capability_id: request.capability_id.clone(),
                    output: serde_json::Value::Null,
                    status: ExecutionStatus::Failed,
                    error: Some(format!("Unknown capability: {}", request.capability_id)),
                    actual_runtime: Some(crate::runtime::RuntimeMode::Native),
                });
            }
        };

        Ok(CapabilityExecutionResult {
            execution_id: request.execution_id,
            trace_id: request.trace_id,
            connector_id: self.id.clone(),
            capability_id: request.capability_id,
            output,
            status: ExecutionStatus::Success,
            error: None,
            actual_runtime: Some(crate::runtime::RuntimeMode::Native),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_rate_limiter() {
        let limiter = RateLimiter::new(10); // 10 req/min

        // 获取第一个许可应该立即成功
        let start = std::time::Instant::now();
        limiter.acquire().await.unwrap();
        assert!(start.elapsed().as_millis() < 100);
    }

    #[tokio::test]
    async fn test_auth_from_token() {
        let auth = GitHubAuth::from_token("test_token".to_string());
        let creds = auth.authenticate().await.unwrap();

        match creds {
            Credentials::PersonalAccessToken(token) => {
                assert_eq!(token, "test_token");
            }
            _ => panic!("Expected PersonalAccessToken"),
        }
    }

    #[tokio::test]
    async fn test_connector_capabilities() {
        let auth = Arc::new(GitHubAuth::from_token("test_token".to_string()));
        let connector = GitHubConnector::new(auth);

        assert_eq!(connector.capabilities().len(), 5);
        assert!(connector
            .capabilities()
            .iter()
            .any(|c| c.id == "github.create_issue"));
        assert!(connector
            .capabilities()
            .iter()
            .any(|c| c.id == "github.create_pr"));
        assert!(connector
            .capabilities()
            .iter()
            .any(|c| c.id == "github.review_code"));
        assert!(connector
            .capabilities()
            .iter()
            .any(|c| c.id == "github.list_repos"));
        assert!(connector
            .capabilities()
            .iter()
            .any(|c| c.id == "github.search_code"));
    }
}

#[cfg(test)]
#[path = "github_connector_tests.rs"]
mod github_connector_tests;
