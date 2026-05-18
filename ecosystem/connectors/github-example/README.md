# GitHub Connector 使用指南

GitHub Connector 提供对 GitHub API 的访问能力，支持 Issue 管理、Pull Request 操作、代码审查等功能。

## 快速开始

### 1. 获取 GitHub Token

访问 [GitHub Personal Access Tokens](https://github.com/settings/tokens) 创建新 token。

**需要的权限**:
- `repo` - 完整仓库访问权限
- `read:user` - 读取用户信息
- `read:org` - 读取组织信息

### 2. 配置环境变量

```bash
export GITHUB_TOKEN="ghp_xxxxxxxxxxxxxxxxxxxxx"
```

### 3. 使用 Connector

```rust
use cyberclaw_connectors::{GitHubConnector, GitHubAuth};
use std::sync::Arc;

// 创建认证器
let auth = Arc::new(GitHubAuth::from_token(
    std::env::var("GITHUB_TOKEN").unwrap()
));

// 创建 Connector
let connector = GitHubConnector::new(auth);

// 注册到 Registry
use cyberclaw_connectors::ConnectorRegistry;
let registry = ConnectorRegistry::global();
registry.register(Arc::new(connector))?;
```

## 能力清单

### github.create_issue

创建 GitHub Issue。

**输入参数**:
```json
{
  "owner": "string",    // 仓库所有者
  "repo": "string",     // 仓库名称
  "title": "string",    // Issue 标题
  "body": "string"      // Issue 内容 (可选)
}
```

**输出**:
```json
{
  "number": 123,
  "html_url": "https://github.com/owner/repo/issues/123",
  "state": "open"
}
```

**风险级别**: Medium

**示例**:
```rust
use cyberclaw_connectors::types::CapabilityExecutionRequest;
use serde_json::json;

let request = CapabilityExecutionRequest {
    execution_id: ExecutionId::new(),
    trace_id: "trace-123".to_string(),
    actor: ActorRef::system(),
    workspace: workspace_ref,
    connector_id: ConnectorId::new("github"),
    capability_id: CapabilityId::new("github.create_issue"),
    input: json!({
        "owner": "cyberclawlabs",
        "repo": "cyberclaw",
        "title": "Bug: Authentication fails",
        "body": "Detailed description of the bug..."
    }),
};

let result = connector.execute(request).await?;
```

---

### github.create_pr

创建 Pull Request。

**输入参数**:
```json
{
  "owner": "string",    // 仓库所有者
  "repo": "string",     // 仓库名称
  "title": "string",    // PR 标题
  "head": "string",     // 源分支
  "base": "string",     // 目标分支
  "body": "string"      // PR 描述 (可选)
}
```

**输出**:
```json
{
  "number": 456,
  "html_url": "https://github.com/owner/repo/pull/456",
  "state": "open"
}
```

**风险级别**: High

**示例**:
```rust
let request = CapabilityExecutionRequest {
    // ... (同上)
    capability_id: CapabilityId::new("github.create_pr"),
    input: json!({
        "owner": "cyberclawlabs",
        "repo": "cyberclaw",
        "title": "feat: Add new feature",
        "head": "feature/new-feature",
        "base": "main",
        "body": "This PR adds a new feature..."
    }),
};
```

---

### github.review_code

提交代码审查。

**输入参数**:
```json
{
  "owner": "string",       // 仓库所有者
  "repo": "string",        // 仓库名称
  "pull_number": 123,      // PR 编号
  "event": "string",       // APPROVE | REQUEST_CHANGES | COMMENT
  "body": "string"         // 审查意见
}
```

**输出**:
```json
{
  "id": 789,
  "state": "APPROVED"
}
```

**风险级别**: Medium

---

### github.list_repos

列举仓库。

**输入参数**:
```json
{
  "visibility": "string",  // all | public | private (默认: all)
  "sort": "string"         // updated | created | pushed | full_name (默认: updated)
}
```

**输出**:
```json
{
  "repos": [
    {
      "name": "cyberclaw",
      "full_name": "cyberclawlabs/cyberclaw",
      "private": false,
      "html_url": "https://github.com/cyberclawlabs/cyberclaw"
    }
  ]
}
```

**风险级别**: Low

---

### github.search_code

搜索代码。

**输入参数**:
```json
{
  "query": "string",      // 搜索查询 (GitHub 搜索语法)
  "sort": "string",       // indexed (可选)
  "order": "string"       // desc | asc (可选)
}
```

**输出**:
```json
{
  "total_count": 42,
  "items": [
    {
      "name": "main.rs",
      "path": "src/main.rs",
      "repository": {
        "full_name": "cyberclawlabs/cyberclaw"
      }
    }
  ]
}
```

**风险级别**: Low

## OAuth 集成 (高级)

### 1. 注册 OAuth App

访问 [GitHub OAuth Apps](https://github.com/settings/developers) 注册新应用。

**Callback URL**: `http://localhost:8080/auth/callback`

### 2. 配置 OAuth

```rust
let auth = Arc::new(GitHubAuth::new(
    "your_client_id".to_string(),
    "your_client_secret".to_string(),
));

let connector = GitHubConnector::new(auth);
```

### 3. 实现 OAuth Flow

```rust
// 1. 获取授权 URL
let auth_url = format!(
    "https://github.com/login/oauth/authorize?client_id={}&scope=repo",
    client_id
);

// 2. 用户授权后，使用 code 换取 token
// (详见 GitHub OAuth 文档)
```

## 速率限制

GitHub API 有以下限制：

- **已认证**: 5000 requests/hour
- **未认证**: 60 requests/hour

Connector 内置速率限制器，默认配置为 **80 requests/minute**，确保不会超出限制。

### 自定义速率限制

```rust
use cyberclaw_connectors::RateLimiter;

let custom_limiter = RateLimiter::new(100); // 100 req/min
```

## 错误处理

Connector 会自动处理以下错误：

1. **认证失败**: 返回 `ExecutionStatus::Failed`
2. **速率限制**: 自动等待直到可用
3. **网络错误**: 返回错误信息
4. **参数缺失**: 返回验证错误

## 最佳实践

1. **使用环境变量**: 不要硬编码 token
2. **限制权限范围**: 只申请必需的权限
3. **处理速率限制**: 监控 rate limit 使用情况
4. **启用日志**: 使用 `tracing` 记录 API 调用
5. **错误重试**: 网络错误时自动重试

## 安全注意事项

- ⚠️ **永远不要提交 token 到代码库**
- ✅ 使用环境变量或密钥管理系统
- ✅ 定期轮换 token
- ✅ 限制 token 权限范围
- ✅ 监控异常 API 调用

## 故障排查

### Token 无效

```
Error: Invalid credentials
```

**解决方案**: 检查 token 是否正确，权限是否足够。

### 速率限制超出

```
Error: API rate limit exceeded
```

**解决方案**: 等待一小时或调整 `permits_per_minute` 配置。

### 仓库不存在

```
Error: Not Found
```

**解决方案**: 确认仓库名称和所有者正确，token 有访问权限。

## 示例项目

完整示例代码请参考:
- [examples/github-issue-bot/](../../../examples/github-issue-bot/)
- [examples/github-pr-automation/](../../../examples/github-pr-automation/)

## 相关资源

- [GitHub API 文档](https://docs.github.com/en/rest)
- [Octocrab 文档](https://docs.rs/octocrab)
- [OAuth 指南](https://docs.github.com/en/developers/apps/building-oauth-apps)
- [速率限制说明](https://docs.github.com/en/rest/overview/resources-in-the-rest-api#rate-limiting)
