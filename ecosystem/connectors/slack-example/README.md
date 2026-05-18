# Slack Connector Example

Slack Connector 的配置和使用示例。

## 目录结构

```
slack-example/
├── README.md                 # 本文件
├── slack-config.yaml         # Slack Connector 配置文件
├── templates/                # 消息模板目录
│   ├── welcome.hbs          # 欢迎消息模板
│   ├── alert.hbs            # 告警消息模板
│   └── report.hbs           # 报告消息模板
└── examples/                 # 使用示例
    ├── send_message.json    # 发送消息示例
    ├── create_channel.json  # 创建频道示例
    ├── upload_file.json     # 上传文件示例
    └── react_emoji.json     # 添加反应示例
```

## 快速开始

### 1. 配置 Slack App

1. 访问 [Slack API](https://api.slack.com/apps)
2. 创建新的 Slack App
3. 添加 Bot Token Scopes:
   - `chat:write` - 发送消息
   - `channels:manage` - 管理频道
   - `files:write` - 上传文件
   - `reactions:write` - 添加反应
4. 安装 App 到工作区
5. 复制 Bot User OAuth Token (xoxb-...)

### 2. 配置 Connector

编辑 `slack-config.yaml`，填入你的 Bot Token:

```yaml
slack:
  bot_token: "xoxb-your-actual-token-here"
```

### 3. 使用示例

#### 发送简单消息

```json
{
  "channel": "#general",
  "text": "Hello from CyberClaw!"
}
```

#### 使用模板发送消息

```json
{
  "channel": "#general",
  "template": "welcome",
  "data": {
    "name": "Alice",
    "channel": "#engineering",
    "onboarding_tasks": [
      {
        "title": "Complete profile",
        "description": "Fill in your Slack profile"
      },
      {
        "title": "Join channels",
        "description": "Join your team channels"
      }
    ],
    "team_members": [
      {
        "name": "Bob",
        "role": "Team Lead"
      },
      {
        "name": "Carol",
        "role": "Senior Engineer"
      }
    ],
    "links": [
      {
        "title": "Team Wiki",
        "url": "https://wiki.example.com"
      },
      {
        "title": "Onboarding Guide",
        "url": "https://guide.example.com"
      }
    ],
    "support_channel": "#team-support"
  }
}
```

#### 创建频道

```json
{
  "name": "project-alpha",
  "is_private": false,
  "description": "Discussion for Project Alpha"
}
```

#### 上传文件

```json
{
  "channel": "#general",
  "content": "SGVsbG8gV29ybGQh",  // Base64 编码的 "Hello World!"
  "filename": "hello.txt",
  "filetype": "text/plain",
  "initial_comment": "Here's a test file"
}
```

#### 添加 Emoji 反应

```json
{
  "channel": "C12345678",
  "timestamp": "1234567890.123456",
  "emoji": "thumbsup"
}
```

## 消息模板

### 模板语法

使用 Handlebars 模板引擎，支持：

- **变量**: `{{variable}}`
- **条件**: `{{#if condition}}...{{/if}}`
- **循环**: `{{#each items}}...{{/each}}`
- **索引**: `{{@index}}`
- **嵌套**: `{{object.property}}`

### 自定义模板

在 `templates/` 目录创建 `.hbs` 文件：

```handlebars
:rocket: *{{title}}*

{{description}}

{{#each items}}
• {{this.name}}: {{this.value}}
{{/each}}
```

在配置文件中注册：

```yaml
templates:
  preload:
    - name: "my_template"
      file: "my_template.hbs"
```

## Capabilities 参考

### slack.send_message

**风险级别**: Low
**需要审批**: 否

**输入参数**:
- `channel` (必需): 频道 ID 或名称
- `text` (可选): 消息文本
- `template` (可选): 模板名称
- `data` (可选): 模板数据
- `blocks` (可选): 消息块 (高级格式)
- `thread_ts` (可选): 线程时间戳 (回复消息)

**输出**:
- `ts`: 消息时间戳
- `channel`: 频道 ID
- `ok`: 是否成功

### slack.create_channel

**风险级别**: Medium
**需要审批**: 是

**输入参数**:
- `name` (必需): 频道名称
- `is_private` (可选): 是否为私有频道 (默认 false)
- `description` (可选): 频道描述

**输出**:
- `id`: 频道 ID
- `name`: 频道名称
- `ok`: 是否成功

### slack.upload_file

**风险级别**: Medium
**需要审批**: 否

**输入参数**:
- `channel` (必需): 频道 ID 或名称
- `content` (必需): 文件内容 (Base64 编码)
- `filename` (必需): 文件名
- `filetype` (可选): MIME 类型
- `initial_comment` (可选): 初始评论

**输出**:
- `file_id`: 文件 ID
- `url`: 文件 URL
- `ok`: 是否成功

### slack.react_emoji

**风险级别**: Low
**需要审批**: 否

**输入参数**:
- `channel` (必需): 频道 ID
- `timestamp` (必需): 消息时间戳
- `emoji` (必需): Emoji 名称 (不含冒号)

**输出**:
- `ok`: 是否成功

## 安全最佳实践

1. **Token 安全**:
   - 不要在代码中硬编码 Bot Token
   - 使用环境变量或密钥管理系统
   - 定期轮换 Token

2. **权限控制**:
   - 仅授予必要的 Bot Scopes
   - 使用频道白名单/黑名单
   - 限制文件上传大小和类型

3. **速率限制**:
   - 遵守 Slack API 速率限制
   - 实现请求重试和退避策略
   - 监控 API 使用情况

4. **审计**:
   - 记录所有 Connector 操作
   - 保留消息发送历史
   - 监控异常活动

## 故障排查

### Bot Token 无效

```
Error: Slack API error: invalid_auth
```

**解决方案**: 检查 Bot Token 是否正确，是否已安装到工作区。

### 权限不足

```
Error: Slack API error: missing_scope
```

**解决方案**: 在 Slack App 管理页面添加缺失的 Bot Scope。

### 频道不存在

```
Error: Slack API error: channel_not_found
```

**解决方案**: 确认频道 ID 或名称正确，Bot 已加入该频道。

### 速率限制

```
Error: Slack API error: rate_limited
```

**解决方案**: 降低请求频率，实现退避重试策略。

## 更多资源

- [Slack API 文档](https://api.slack.com/)
- [Block Kit Builder](https://app.slack.com/block-kit-builder)
- [Handlebars 文档](https://handlebarsjs.com/)
- [CyberClaw 文档](../../docs/)

## 支持

如有问题，请在项目仓库提交 Issue。
