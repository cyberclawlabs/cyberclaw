# CyberClaw Skill/Tool 兼容性架构设计 v1.0

- Status: Draft
- Scope: Architecture
- Owner: CyberClaw Maintainers
- Created: 2026-04-14
- Last Updated: 2026-04-14

---

## 1. 设计目标

CyberClaw 兼容主流 Skill 包格式与工具声明语义，并将其归一到平台现有的 `SkillContext -> CapabilityFacade -> Connector -> Capability` 受控执行链。

### 1.1 核心原则

- **输入侧尽量宽容，执行侧必须严格**
- 不新建一级平台对象（Tool 不是第六类生态对象）
- 不新建平行 crate 或平行类型体系
- 只基于现有类型（SkillContext、CapabilityFacade、FormatLoader）收口
- 兼容格式和声明语义，不兼容绕过治理的私有运行时行为

### 1.2 设计边界

**兼容**：
- Skill 的目录结构、元数据、prompt/instruction
- Skill 的 tool schema / tool declarations
- Skill 的 references / assets / scripts 声明
- MCP / OpenAI / Anthropic 的工具声明标准

**不兼容**：
- Skill 依赖的宿主私有 API
- Skill 假定的直接 shell/fs/network 执行权
- Skill 中的隐藏全局变量或私有 hook
- 任何绕过 Connector -> Capability 治理链的执行路径

---

## 2. 架构方向

### 2.1 双视角模型

```
外部开发者视角（接入面）          平台维护者视角（治理面）
┌──────────────────────┐      ┌──────────────────────────────┐
│  Skill: 怎么做        │      │  Agent: 谁来做                │
│  Tool:  做什么        │      │  Skill: 怎么做                │
│                      │      │  Connector: 用什么接入         │
│  (只需理解 2 个概念)   │      │  Capability: 最小动作单元      │
│                      │      │  Platform Plugin: 平台增强     │
└──────────┬───────────┘      └──────────────┬───────────────┘
           │                                  │
           └──────────── 同一个平台 ────────────┘
```

- **外部开发者**只需要理解 Skill（知识/方法包）和 Tool（能力声明）
- **平台内部**保留完整 5 层对象模型，治理、审计、安全不变
- 桥接层由现有组件承担：SkillContext + CapabilityFacade

### 2.2 执行链（不变）

```
Skill（声明上下文）
  ↓ SkillBinder
AgenticLoop（LLM 推理）
  ↓ tool call
OrchestratorGateway
  ↓
PolicyEngine → CapabilityDispatcher
  ↓
Connector::execute() → CapabilityExecutionResult
```

---

## 3. Skill 兼容设计

### 3.1 兼容等级

| 等级 | 含义 | 具体能力 |
|------|------|---------|
| **C1 结构兼容** | 能识别包格式和元数据 | `FormatLoader::can_load()` 返回 true，SkillMetadata 解析成功 |
| **C2 声明兼容** | 能解析 tools/scripts/assets/trust | SkillContext 完整填充，tool_declarations 可用 |
| **C3 可执行兼容** | 工具声明映射到 Connector→Capability | tool call 可通过 OrchestratorGateway 执行，走完整治理链 |

### 3.2 兼容矩阵

| 格式 | 标识文件 | Loader | C1 | C2 | C3 | 改动 |
|------|---------|--------|:--:|:--:|:--:|------|
| Claude Code Skill | `SKILL.md` | ClaudeCodeSkillLoader | Y | 待补 | 待补 | 扩展 SkillMetadata + 补 handler |
| Hermes Skill | `SKILL.md` | **复用 ClaudeCodeSkillLoader** | Y | 待补 | 待补 | 同上，`#[serde(default)]` 自动兼容 |
| Codex Skill | `manifest.yaml` | CodexSkillLoader | Y | 待补 | 待补 | 补 tools 解析 |
| OpenClaw Skill | `skill.toml` | OpenClawSkillLoader | Y | 待补 | 待补 | 补 tools 解析 |
| Hermes Plugin | `plugin.yaml` + `__init__.py` | 新增 HermesPluginLoader | 待建 | 待建 | 待建 | P2，1 个文件 |
| 未来 SKILL.md 格式 | `SKILL.md` | 复用现有 Loader | Y | Y | Y | `extra: Value` 兜底 |
| 未来新标识文件 | 自定义 | 新增 `impl FormatLoader` | 按需 | 按需 | 按需 | 1 个文件 |

### 3.3 Hermes Skill 兼容细节

Hermes Skill 与 Claude Code Skill 使用相同的 `SKILL.md` 格式（Hermes 文档明确声明 "Inspired by Anthropic's Claude Skills system"）。差异仅在 frontmatter 扩展字段：

| 字段 | Claude Code | Hermes | 处理方式 |
|------|------------|--------|---------|
| name, description | Y | Y | 共用 |
| version, author, tags | Y | Y | 共用 |
| platforms | N | Y | `#[serde(default)]` |
| required_environment_variables | N | Y | `#[serde(default)]` |
| required_credential_files | N | Y | `#[serde(default)]` |
| triggers | N | Y | `#[serde(default)]` |
| metadata.hermes.* | N | Y | 落入 `extra: Value` |
| toolsets | N | Y | 映射到 required_toolsets |

**不需要新 Loader**。扩展 `SkillMetadata` 的 `#[serde(default)]` 字段即可同时兼容。

### 3.4 SkillMetadata 扩展

在现有 `SkillMetadata`（`skill-runtime/src/loaders/mod.rs:36-52`）上增加字段：

```rust
pub struct SkillMetadata {
    // --- 现有字段（不变）---
    pub name: String,
    pub description: String,
    pub version: String,
    pub author: Option<String>,
    pub homepage: Option<String>,
    pub tags: Vec<String>,

    // --- 扩展字段（全部 #[serde(default)]）---
    #[serde(default)]
    pub tools: Vec<SkillToolDeclaration>,       // C2: 工具声明
    #[serde(default)]
    pub scripts: Vec<String>,                    // C2: 脚本路径
    #[serde(default)]
    pub assets: Vec<String>,                     // C2: 资源路径
    #[serde(default)]
    pub platforms: Vec<String>,                  // Hermes: 平台限制
    #[serde(default)]
    pub required_env_vars: Vec<EnvVarRequirement>, // Hermes: 环境变量需求
    #[serde(default)]
    pub required_credentials: Vec<CredentialRequirement>, // Hermes: 凭证需求
    #[serde(default)]
    pub triggers: Vec<String>,                   // Hermes: 触发词
    #[serde(default)]
    pub required_toolsets: Vec<String>,           // Hermes: 工具集依赖
    #[serde(default)]
    pub trust_level: Option<String>,             // 信任等级
    #[serde(default, flatten)]
    pub extra: serde_json::Value,                // 未来格式兜底
}
```

### 3.5 SkillContext 扩展

在现有 `SkillContext`（`skill-runtime/src/context.rs:15`）上增加字段：

```rust
pub struct SkillContext {
    // --- 现有字段（不变）---
    pub metadata: SkillMetadata,
    pub prompt_extension: String,
    pub tool_declarations: Vec<ToolDeclaration>,
    pub references: Vec<SkillReference>,

    // --- 扩展字段 ---
    #[serde(default)]
    pub scripts: Vec<SkillScript>,               // 脚本声明
    #[serde(default)]
    pub assets: Vec<SkillAsset>,                  // 资源声明
    #[serde(default)]
    pub trust_manifest: Option<TrustManifest>,    // 信任声明
    #[serde(default)]
    pub origin_format: SkillOriginFormat,         // 来源格式标记
}

pub enum SkillOriginFormat {
    ClaudeCode,
    Codex,
    OpenClaw,
    Hermes,
    Unknown,
}
```

### 3.6 FormatLoader 扩展策略

新增 Skill 格式的成本 = 1 个 `impl FormatLoader` 文件：

```rust
// FormatLoader trait（已有，不变）
pub trait FormatLoader: Send + Sync {
    fn can_load(&self, path: &Path) -> bool;
    async fn load(&self, path: &Path) -> Result<LoadedSkill, SkillRuntimeError>;
    fn name(&self) -> &str;
}
```

当新格式使用已有标识文件（如 `SKILL.md`）时，不需要新 Loader，只需扩展 `SkillMetadata` 字段。

当新格式使用全新标识文件时，新增一个 FormatLoader 实现文件并注册到 `UnifiedSkillLoader.loaders`。

### 3.7 多 Skill 同名 Tool 冲突解决

当多个 Skill 声明了同名工具时，`SkillBinder` 使用命名空间前缀避免冲突：

- 工具名格式：`{skill_name}.{tool_name}`（见 `skill_binder.rs:175`）
- LLM 看到的是 `research.web_search` 而非裸 `web_search`
- 如果前缀后仍然冲突（两个 Skill 同名），后绑定的覆盖先绑定的，并记录警告日志
- Agent 编排者可通过 `default_skills` 配置控制 Skill 加载顺序和优先级

---

## 4. Tool 兼容设计

### 4.1 核心约束

**Tool 不是一级平台对象**。Tool 是 `Connector + Capability` 的只读外部投影，由 `CapabilityFacade` 承担。

### 4.2 兼容的工具声明标准

| 标准 | 桥接组件 | 状态 |
|------|---------|------|
| MCP Tool | McpToolBridge → CapabilityFacade | 已有，需修复路由 |
| OpenAI Function Calling | ToolDescriptionBridge.format_openai() | 已有输出，需补输入解析 |
| Anthropic Tool Schema | ToolDescriptionBridge.format_anthropic() | 已有输出，需补输入解析 |
| Skill ToolDeclaration | SkillBinder → CapabilityFacade | 已有，需补 tools 解析 |

### 4.3 CapabilityFacade 增强

在现有 `CapabilityFacade`（`agent-runtime/src/tool_description.rs:46-60`）上修复和增强：

```rust
#[derive(Debug, Clone)]
pub struct CapabilityFacade {
    // --- 外部可见（对 LLM 和开发者）---
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub risk_level: RiskLevel,
    pub effects: Vec<CapabilityEffect>,          // 新增
    pub read_only: bool,                          // 新增
    pub destructive: bool,                        // 新增

    // --- 内部路由（不序列化）---
    #[serde(skip_serializing)]
    pub connector_id: String,
    #[serde(skip_serializing)]
    pub capability_id: String,
    #[serde(skip_serializing)]
    pub runtime: Option<ConnectorRuntime>,         // 新增
}
```

关键修改：
1. 移除整体 `#[derive(Serialize, Deserialize)]`，改为手动实现 `Serialize`（只暴露外部字段）
2. `connector_id` / `capability_id` 加 `#[serde(skip_serializing)]`
3. 新增 `effects` / `read_only` / `destructive` 字段

---

## 5. 安全修复清单

### 5.1 P0 安全修复

| # | 问题 | 位置 | 修复方案 | 改动量 |
|---|------|------|---------|--------|
| S1 | PolicyMiddleware 硬编码 `RiskLevel::Medium` | `middleware_pipeline.rs:262` | 从 ConnectorRegistry 查找真实 risk/effects | ~20 行 |
| S2 | CapabilityFacade 通过 Serialize 泄漏 connector_id/capability_id | `tool_description.rs:46` | `#[serde(skip_serializing)]` | ~2 行 |
| S3 | MCP 默认 risk 为 Low | `tool_bridge.rs` BridgeConfig::default() | 默认改为 High，显式 allowlist 降级 | ~10 行 |
| S4 | facade 注册路径绕过治理 | `register_facade()` 等 | 注册时调用 DangerousCapabilityFilter + ToolPermissionMatcher | ~50 行 |
| S5 | `from_capability_definition()` 硬编码 Medium | `tool_description.rs:244` | 从 CapabilityDefinition.behavior 提取真实 risk | ~5 行 |

### 5.2 P1 安全修复

| # | 问题 | 位置 | 修复方案 | 改动量 |
|---|------|------|---------|--------|
| S6 | Skill prompt_extension 无注入扫描 | `skill_binder.rs:165` | bind() 前调用 PromptInjectionGuard | ~15 行 |
| S7 | risk 与 is_destructive 无交叉验证 | `capability_contract.rs:42` 定义，`dispatcher.rs` Native 分支缺校验 | 注册时和 dispatch 时校验 is_destructive+low risk 不一致则拒绝 | ~10 行 |
| S8 | ToolPermissionMatcher deny 规则仅按名称 | `tool_permission_matcher.rs:246` | 增加基于 CapabilityEffect::Execute 的 deny | ~20 行 |

---

## 6. 功能补完清单

### 6.1 P1 功能补完

| # | 功能 | 位置 | 方案 | 改动量 |
|---|------|------|------|--------|
| F1 | SkillMetadata 加 tools 字段 | `loaders/mod.rs:36-52` | 增加 `tools: Vec<SkillToolDeclaration>` | ~20 行 |
| F2 | ClaudeCodeSkillLoader 解析 tools | `claude_code.rs` | 从 frontmatter 提取 tools 字段 | ~30 行 |
| F3 | CodexSkillLoader 解析 tools | `codex.rs` | 从 manifest.yaml 提取 tools | ~30 行 |
| F4 | OpenClawSkillLoader 解析 tools | `openclaw.rs` | 从 skill.toml 提取 [[skill.tools]] | ~30 行 |
| F5 | SkillMetadata Hermes 扩展字段 | `loaders/mod.rs` | 增加 platforms/required_env_vars/triggers 等 | ~40 行 |
| F6 | SkillContext 扩展字段 | `context.rs` | 增加 scripts/assets/trust_manifest/origin_format | ~30 行 |
| F7 | MCP 工具调用路由打通 | `McpToolBridge → ToolCallMapper` | 自动注册命名空间化工具名 | ~40 行 |
| F8 | 三个 Loader 补完 stub handler | `claude_code.rs/codex.rs/openclaw.rs` | 实现真实 handler 逻辑 | ~150 行 |

### 6.2 P2 功能

| # | 功能 | 位置 | 方案 |
|---|------|------|------|
| F9 | HermesPluginLoader | 新建 `loaders/hermes_plugin.rs` | 解析 plugin.yaml → PluginDeclaration |
| F10 | MCP sampling 支持 | `mcp/client.rs` | server-to-client 请求处理 |
| F11 | MCP notifications/SSE | `mcp/transport.rs` | Transport 双向化 |
| F12 | OpenAI strict mode 适配 | `tool_description.rs` | format_openai() 注入 additionalProperties:false |

---

## 7. 优先级路径

```
P0 安全修复（S1-S5）
  ↓
P1 功能补完（F1-F8）+ 安全修复（S6-S8）
  ↓
P2 高级特性（F9-F12）
  ↓
文档分层（开发者指南 / 平台维护者指南）
```

**P0 预估**：~90 行代码改动，4 个定向 PR
**P1 预估**：~350 行代码改动，可拆为 3-4 个 PR
**P2 预估**：另行评估

---

## 8. 与项目约束的一致性检查

| CLAUDE.md 约束 | 本方案是否合规 | 说明 |
|----------------|:------------:|------|
| 不把 Tool 作为一级平台对象 | Y | Tool 由 CapabilityFacade 承担，只是投影 |
| 不新增第五类生态对象 | Y | 不新建 crate，不新建生态对象 |
| 底层执行一律走 Connector -> Capability | Y | 所有 tool call 经 OrchestratorGateway |
| Skill 不直接拥有平台执行权限 | Y | Skill 提供 context，不执行 |
| Platform Plugin 不绕过治理 | Y | 不变 |
| 不要新增重型依赖 | Y | 仅扩展现有结构体字段 |
| 不做未明确要求的未来预留 | Y | extra: Value 是最小兜底 |
| 优先复用现有对象模型 | Y | 基于 SkillContext + CapabilityFacade |

---

## 9. 文档分层规划

| 受众 | 看到什么 | 文档 |
|------|---------|------|
| 生态开发者 | Tool + Skill（2 个概念） | `docs/guides/skill-development.md`（待建） |
| 平台集成者 | Tool Surface + MCP 互操作 | `docs/guides/integration.md`（待建） |
| 平台维护者 | 完整 5 层模型 | `docs/architecture/`（现有） |
| 安全审计者 | Capability 粒度 + 治理链 | `docs/architecture/security/`（现有） |

---

## 10. 参考文件索引

| 文件 | 职责 |
|------|------|
| `crates/cyberclaw-skill-runtime/src/loaders/mod.rs` | UnifiedSkillLoader + FormatLoader trait + SkillMetadata |
| `crates/cyberclaw-skill-runtime/src/loaders/claude_code.rs` | Claude Code / Hermes Skill 加载器 |
| `crates/cyberclaw-skill-runtime/src/loaders/codex.rs` | Codex Skill 加载器 |
| `crates/cyberclaw-skill-runtime/src/loaders/openclaw.rs` | OpenClaw Skill 加载器 |
| `crates/cyberclaw-skill-runtime/src/context.rs` | SkillContext + ToolDeclaration |
| `crates/cyberclaw-agent-runtime/src/tool_description.rs` | CapabilityFacade + ToolDescriptionBridge |
| `crates/cyberclaw-agent-runtime/src/builtin_tools.rs` | BuiltinToolRegistry |
| `crates/cyberclaw-agent-runtime/src/skill_binder.rs` | SkillBinder + SkillToolDescriptor |
| `crates/cyberclaw-connectors/src/mcp/tool_bridge.rs` | McpToolBridge + BridgedTool |
| `crates/cyberclaw-connectors/src/contract.rs` | CapabilityDefinition |
| `crates/cyberclaw-connectors/src/dispatcher.rs` | CapabilityDispatcher |
| `crates/cyberclaw-control-plane/src/middleware_pipeline.rs` | PolicyMiddleware |
| `crates/cyberclaw-governance/src/dangerous_capability_filter.rs` | DangerousCapabilityFilter |
| `crates/cyberclaw-governance/src/tool_permission_matcher.rs` | ToolPermissionMatcher |
| `crates/cyberclaw-governance/src/prompt_injection_guard.rs` | PromptInjectionGuard |
| `crates/cyberclaw-governance/src/tool_output_sanitizer.rs` | ToolOutputSanitizer |
