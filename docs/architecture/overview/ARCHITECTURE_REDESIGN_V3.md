# CyberClaw 架构重设计方案 v3.0

> **性质**: 基于 14 份竞品分析 + 自省报告 + 战略分析 + 红蓝对抗的最终设计方案
> **日期**: 2026-04-11
> **方法**: 红蓝对抗 PK，证据裁决
> **输入**: 26 份研究报告（15,772+ 行），CyberClaw 代码库 114,719 行
> **竞品覆盖**: IronClaw / Claude Code / OpenClaw / Hermes / DeerFlow / OpenViking / Cline / OpenCode / NanoClaw / AutoResearch / gbrain / cc-connect / OpenClaw-RL / AReaL

---

## 0. 核心决策摘要

| 决策点 | 结论 | 理由 |
|--------|------|------|
| 五对象模型 | **保留不变** | 分离逻辑成立，问题在实现不在模型 |
| Platform Plugin 机制 | **Hook/Middleware 替代动态库** | 14 竞品零采用动态库，Hook 是工业验证方案 |
| Skill 独立执行 | **移除，保留加载+热重载** | CLAUDE.md 约束 "Skill 不直接拥有平台执行权限" |
| AgenticLoop 位置 | **放 agent-runtime** | 语义归属 + 避免 control-plane 继续膨胀 |
| 治理层 | **Governance 大幅升级，不加新对象** | "受控"是核心卖点，治理深度必须匹配定位 |
| 存储 | **sub-trait 拆分 + SQLite + PG 激活** | 全内存不可上生产 |

---

## 1. 不变的部分

### 1.1 五对象模型（生态对象）

```
Agent           — 谁来做（角色主体 + 编排者）
Skill           — 怎么做（方法/知识/模板，兼容 Claude/Codex/OpenClaw）
Connector       — 用什么做（传输无关的统一能力接入面）
Capability      — 最小动作单元（治理粒度 + 放置约束）
Platform Plugin — 平台怎么被增强（事件监听/拦截/审计增强）
```

### 1.2 受控执行骨架

```
Execution → Artifact → Provenance
```

### 1.3 核心约束

1. 不把 Tool 作为一级平台对象
2. 不新增第六类生态对象
3. 底层执行一律走 Connector → Capability
4. Skill 不直接拥有平台执行权限
5. Platform Plugin 不绕过治理、审计和追踪
6. 所有操作必须经过 Orchestrator → PolicyEngine 门禁
7. **接口长于实现** — 核心 trait（`OrchestratorGateway`、`Connector`、`LlmProvider`、`CredentialVault`、`PolicyEngine`）一旦稳定，视为公共契约，实现可替换但接口不随意变更。借鉴: Managed Agents "interfaces outlast implementations" 原则

---

## 2. Agent 推理循环（P0 — Sprint 1）

### 2.1 当前问题

`MinimalAgentRuntime` 只做超时 + 并发控制。Agent 不能调用 LLM、不绑定 Skill、不读写 Memory、不编排 SubAgent。这不是 Agent，是 Dispatcher。Agent manifest 中的 `defaultSkills`、`memory_file`、`spawn_policy` 全部声明但运行时未使用。

### 2.2 竞品方案对比

| 竞品 | 推理循环实现 | 核心机制 | 代码参考 | CyberClaw 借鉴价值 |
|------|-------------|---------|---------|-------------------|
| **IronClaw** | 共享 AgenticLoop + LoopDelegate trait | 三种执行路径（Chat/Job/Container）共享同一 `run_agentic_loop()` 引擎，通过 `LoopDelegate` trait 差异化行为 | `src/agent/agentic_loop.rs` | **★★★ 最高** — 同 Rust，trait 模式直接可复用 |
| **Claude Code** | QueryEngine → query.ts 工具循环 | 用户输入 → LLM 推理 → 工具调用 → 结果反馈 → 循环，直到无 tool_use 或 end_turn | `src/QueryEngine.ts`, `src/query.ts` | ★★★ 流式处理和上下文管理最成熟 |
| **Hermes Agent** | AIAgent.run_conversation() 主循环 | IterationBudget（默认90次）+ 并行工具执行 + 上下文压缩触发 + 模型降级链 | `run_agent.py:170-211` | ★★★ 预算控制和降级机制 |
| **OpenViking** | AgentLoop (ReAct) | SessionManager → ContextBuilder → LLM → 解析响应 → Tool/Reply → 循环 | `bot/vikingbot/agent/loop.py` | ★★ 上下文构建模式 |
| **Cline** | recursivelyMakeClineRequests() | 递归调用，流式 LLM 响应 + XML 工具解析 + 用户审批 | `src/core/task/index.ts:2268` | ★★ 用户审批集成 |
| **OpenCode** | SessionProcessor | 多 Agent 切换（build/plan/general/explore）+ Doom Loop 检测 | `src/session/processor.ts` | ★★ 卡住检测机制 |
| **DeerFlow** | Lead Agent + 中间件 | 单图 Agent + 13 层中间件管道 + SubAgent 委派 | `agents/lead_agent/agent.py:273-350` | ★★ 中间件管道模式 |

### 2.3 设计方案

**核心借鉴**: IronClaw 的 LoopDelegate trait 模式（三路径复用同一引擎）

```rust
// crates/cyberclaw-agent-runtime/src/agentic_loop.rs

/// 推理循环差异化行为 trait
/// 借鉴: IronClaw LoopDelegate (src/agent/agentic_loop.rs)
/// 三种执行路径共享同一循环引擎，通过 trait 差异化
#[async_trait]
pub trait LoopDelegate: Send + Sync {
    /// 循环信号检查（中断、取消、超时）
    async fn check_signals(&self, ctx: &LoopContext) -> LoopSignal;

    /// LLM 调用前的上下文构建
    /// 借鉴: OpenViking ContextBuilder (bot/vikingbot/agent/loop.py)
    /// 注入 Skill prompt、Memory、工具列表、会话历史
    async fn build_context(&self, state: &LoopState) -> Result<LlmRequest>;

    /// 调用 LLM — AgentRuntime 内部行为，不经过 Orchestrator 门禁
    /// LLM 调用路径: AgenticLoop → LlmProvider（装饰器链）→ 外部 API
    /// 与工具调用路径分离: 工具调用 → Orchestrator → PolicyEngine → Connector → Capability
    /// AuditMiddleware 记录所有 LLM 调用（token 用量、模型、耗时）
    async fn call_llm(&self, request: LlmRequest) -> Result<LlmResponse>;

    /// 处理文本响应
    /// 借鉴: Cline 的用户审批集成 (src/core/task/index.ts:2268)
    /// InteractiveDelegate 可在此暂停等待用户确认
    async fn handle_text_response(&self, ctx: &mut LoopState, text: &str) -> Result<LoopAction>;

    /// 执行工具调用（经过 Orchestrator → PolicyEngine 门禁）
    async fn execute_tool_calls(&self, ctx: &mut LoopState, calls: Vec<ToolCall>) -> Result<Vec<ToolResult>>;

    /// 每次迭代后的收尾
    /// 借鉴: Hermes after_iteration — 上下文压缩检查 + Memory 同步
    async fn after_iteration(&self, ctx: &mut LoopState) -> Result<()>;

    /// 判断是否继续循环
    fn should_continue(&self, state: &LoopState) -> ContinueDecision;
}
```

**三种 LoopDelegate 实现**:

| Delegate | 场景 | 借鉴来源 | 差异化行为 |
|----------|------|---------|-----------|
| `InteractiveDelegate` | Chat API 对话 | IronClaw ChatDelegate + Cline 用户审批 | 流式输出，用户可中断，危险操作暂停审批 |
| `AutonomousDelegate` | Autopilot 自治任务 | IronClaw JobDelegate + Hermes IterationBudget | 无人值守，预算严格控制，卡住时策略切换 |
| `SandboxedDelegate` | 隔离执行 | IronClaw ContainerDelegate + NanoClaw Docker 隔离 | 受限工具集，资源限额，容器/WASM 环境 |

### 2.4 迭代预算控制

**当前问题**: Autopilot 无预算安全阀，可能无限循环。

**竞品借鉴**:
- **Hermes Agent**: `IterationBudget`（默认 90 次迭代），线程安全的 `AtomicUsize` 计数器，预算耗尽后自动降级模型 → `run_agent.py:170-211`
- **Claude Code**: `maxTurns` 限制 + `autoCompact` 阈值（`contextWindow - maxOutputTokens - 13K buffer`）→ `services/compact/autoCompact.ts`
- **OpenCode**: Doom Loop 检测（连续 3 次相同工具调用触发策略切换）→ `src/session/processor.ts`

```rust
/// 迭代预算 — 推理循环的安全阀
/// 借鉴: Hermes IterationBudget (run_agent.py:170-211)
pub struct IterationBudget {
    pub max_iterations: u32,       // 默认 50，由 Governance Policy 可覆盖
    pub max_tokens: u64,           // 默认 200K
    pub max_duration: Duration,    // 默认 30 min
    pub max_tool_calls: u32,       // 默认 200
}

/// Doom Loop 检测
/// 借鉴: OpenCode Doom Loop 检测 (src/session/processor.ts)
/// 连续 N 次相同工具调用模式 → 触发策略切换
pub struct DoomLoopDetector {
    pub recent_calls: VecDeque<String>,  // 最近 N 次工具调用 fingerprint
    pub threshold: usize,                 // 默认 3
}

impl DoomLoopDetector {
    pub fn check(&mut self, call: &ToolCall) -> bool {
        let fingerprint = format!("{}:{}", call.name, hash(&call.arguments));
        self.recent_calls.push_back(fingerprint.clone());
        if self.recent_calls.len() > self.threshold {
            self.recent_calls.pop_front();
        }
        // 全部相同 = doom loop
        self.recent_calls.iter().all(|c| c == &fingerprint)
    }
}
```

### 2.5 LLM Provider 适配层

**当前问题**: `cyberclaw-llm` 直接调用 API，无重试、无降级、无熔断。

**竞品借鉴**:
- **IronClaw**: `build_provider_chain()` 装饰器链（Raw → Retry → SmartRouting → Failover → CircuitBreaker → Cache → Recording）→ `src/llm/mod.rs`
- **Hermes Agent**: 模型降级链（主模型 → 降级模型 → 最小模型），瞬态 vs 非瞬态错误分类 → `agent/error_classifier.py`
- **Claude Code**: 流式响应处理，`StreamEvent` 类型化事件 → `src/QueryEngine.ts`

```rust
/// LLM Provider 装饰器链
/// 借鉴: IronClaw build_provider_chain() (src/llm/mod.rs)
pub fn build_provider_chain(
    primary: Box<dyn LlmProvider>,
    fallbacks: Vec<Box<dyn LlmProvider>>,
    config: &ProviderChainConfig,
) -> Box<dyn LlmProvider> {
    let provider = RetryProvider::new(primary, config.retry);      // 指数退避重试
    let provider = FailoverProvider::new(provider, fallbacks);      // 故障转移
    let provider = CircuitBreakerProvider::new(provider, config.cb); // 熔断
    let provider = CacheProvider::new(provider, config.cache);      // 响应缓存
    Box::new(provider)
}

/// 瞬态 vs 非瞬态错误分类
/// 借鉴: IronClaw (src/llm/error.rs) + Hermes (agent/error_classifier.py)
pub enum LlmErrorKind {
    // 瞬态 — 可重试
    RateLimited { retry_after: Option<Duration> },
    Overloaded,
    ConnectionFailed,
    // 非瞬态 — 不重试，触发降级或终止
    AuthFailed,
    ContextLengthExceeded { max: usize, requested: usize },
    ContentFiltered,
}

/// 模型降级链
/// 借鉴: Hermes 三级降级（主模型 → 降级 → 最小）
pub struct ModelDegradationChain {
    pub primary: String,      // e.g. "claude-opus-4-6"
    pub degraded: String,     // e.g. "claude-sonnet-4-6"
    pub minimal: String,      // e.g. "claude-haiku-4-5-20251001"
    pub degrade_after: u32,   // 连续失败 N 次后降级
}
```

### 2.6 流式响应处理

**当前问题**: Chat API 有基础流式支持，但 Agent Runtime 不参与流式处理。

**竞品借鉴**:
- **Claude Code**: `QueryEngine.ts` 的 `StreamEvent` 处理 — 类型化的流事件（Text / ToolCallStart / ToolCallDelta / Stop）→ `src/QueryEngine.ts`
- **Cline**: 流式 XML 解析器，逐 chunk 解析工具调用 → `src/core/task/index.ts`

```rust
/// 流式响应处理器
/// 借鉴: Claude Code QueryEngine.ts StreamEvent 处理
pub struct StreamProcessor {
    token_tracker: TokenTracker,
    content_buffer: String,
    tool_calls_buffer: Vec<ToolCallBuilder>,
}

impl StreamProcessor {
    pub async fn process_stream(
        &mut self,
        stream: impl Stream<Item = Result<ChatChunk>>,
        on_text: impl Fn(&str),        // 实时文本回调（流式输出给用户）
        on_tool_start: impl Fn(&str),  // 工具开始回调（UI 状态更新）
    ) -> Result<LlmResponse> {
        // 逐 chunk 处理，区分文本和工具调用
        // Token 使用量实时追踪
        // 工具调用参数逐步拼接
    }
}
```

### 2.7 工具调用管道

**当前问题**: ExecutionService 直接执行，工具调用不经过推理循环。

**竞品借鉴**:
- **Claude Code**: 工具调用前 10 层安全检查（工具过滤 → 权限模式 → 规则匹配 → 工具自检 → Hook 拦截 → 分类器 → 交互确认 → 沙箱 → 拒绝追踪）→ `src/utils/permissions/` (24文件)
- **Hermes Agent**: 并行工具批处理 `_should_parallelize_tool_batch()` — 检测工具是否可安全并行 → `run_agent.py`
- **DeerFlow**: GuardrailProvider Protocol — 工具调用前置授权，`fail_closed=True` → `guardrails/provider.py`

```rust
/// [P0 修正] 解除 agent-runtime ↔ control-plane 循环依赖
/// 定义窄接口 trait 放在 cyberclaw-core，打破循环:
///   agent-runtime -> core::OrchestratorGateway (trait)
///   control-plane -> impl OrchestratorGateway for ControlPlaneOrchestrator
///   control-plane -> agent-runtime (AgentRuntime trait)
/// 依赖方向: control-plane -> agent-runtime -> core，无循环

// crates/cyberclaw-core/src/gateway.rs
#[async_trait]
pub trait OrchestratorGateway: Send + Sync {
    async fn execute_capability(
        &self,
        capability_name: &str,
        arguments: serde_json::Value,
        actor: &ActorContext,
    ) -> Result<CapabilityResult>;
}

/// 推理循环内的工具调用路径
/// [P0 修正] 通过 OrchestratorGateway trait 调用，不直接依赖 control-plane
/// [P0 修正] 统一走 MiddlewarePipeline（§8.3），不直调 Orchestrator 内部方法
async fn execute_tool_in_loop(
    tool_call: &ToolCall,
    gateway: &dyn OrchestratorGateway,  // 通过 trait 而非具体类型
    ctx: &LoopContext,
) -> Result<ToolResult> {
    // 统一提交给 MiddlewarePipeline（§8.3）
    // Pipeline 内部依次执行: Trace → Policy → Audit → Execution
    // 避免策略重复评估和审计遗漏
    match gateway.execute_capability(&tool_call.name, tool_call.arguments.clone(), &ctx.actor).await {
        Ok(result) => Ok(ToolResult::success(tool_call.id.clone(), result)),
        Err(e) if e.is_review_required() => {
            // 借鉴 Cline 审批集成 — 暂停等待人工审批
            Ok(ToolResult::pending_review(tool_call.id.clone()))
        }
        Err(e) if e.is_denied() => {
            Ok(ToolResult::denied(tool_call.id.clone(), e.reason()))
        }
        Err(e) => Err(e),
    }
}

/// 工具并行安全检查
/// 借鉴: Hermes _should_parallelize_tool_batch() (run_agent.py)
/// 读操作可并行，写操作串行，混合操作按依赖排序
pub fn classify_tool_parallelism(calls: &[ToolCall]) -> ParallelPlan {
    // Read-only tools (fs.read, search.grep, search.glob) → 并行
    // Write tools (fs.write, fs.edit, cmd.exec) → 串行
    // 混合 → 先并行 read，再串行 write
}
```

### 2.8 Skill 绑定机制

**当前问题**: Agent 的 `defaultSkills` 在 manifest 声明但运行时未使用。Skill 加载后只是元数据，不自动注入 Agent 上下文。

**竞品借鉴**:
- **Claude Code**: Skill 作为 `SKILL.md` 文件，通过 `AttachmentMessage` 注入上下文，成为系统提示的一部分 → `utils/claudemd.ts`
- **IronClaw**: `SkillSet` 附属于 Agent，Skill 的工具定义自动加入可用工具列表 → `src/skills/mod.rs`
- **AutoResearch**: Markdown-as-Skill — Skill 就是结构化的 prompt template → `skills/*.md`
- **Hermes Agent**: 自学习 Skill 创建 — Agent 从执行经验中提取 Skill → `agent/skill_creator.py`

```rust
/// Skill 绑定器 — 将 Registry 中的 Skill 绑定到 Agent 的推理循环上下文
/// 借鉴: Claude Code AttachmentMessage 注入 + IronClaw SkillSet 工具注册
pub struct SkillBinder {
    skill_loader: Arc<UnifiedSkillLoader>,
}

impl SkillBinder {
    /// 解析 Agent 的 defaultSkills，注入到 LoopContext
    pub async fn bind(&self, agent_config: &AgentConfig) -> Result<BoundSkills> {
        let mut system_prompt_parts = Vec::new();
        let mut tool_definitions = Vec::new();
        let mut reference_files = Vec::new();

        for skill_id in &agent_config.default_skills {
            let skill = self.skill_loader.load(skill_id)?;

            // 1. Skill prompt_template → 注入 Agent 系统提示
            //    借鉴: Claude Code — SKILL.md 内容注入为系统消息
            if let Some(template) = &skill.prompt_template {
                system_prompt_parts.push(format!(
                    "<skill name=\"{}\">\n{}\n</skill>",
                    skill.id, template
                ));
            }

            // 2. Skill required_capabilities → 注册为可用工具
            //    借鉴: IronClaw — SkillSet 自动暴露工具定义
            for cap_ref in &skill.required_capabilities {
                tool_definitions.push(cap_ref.to_tool_definition());
            }

            // 3. Skill references → 作为可检索上下文资源
            //    借鉴: AutoResearch — Markdown 资源直接可引用
            reference_files.extend(skill.references.iter().cloned());
        }

        Ok(BoundSkills {
            system_prompt_extension: system_prompt_parts.join("\n\n"),
            additional_tools: tool_definitions,
            reference_files,
        })
    }
}
```

### 2.9 SubAgent 编排

**当前问题**: `spawn_policy` 在 manifest 声明但运行时未使用。`SubagentScheduler` 有类型定义但未集成。

**竞品借鉴**:
- **Claude Code**: Coordinator 模式 — 独立系统提示、Worker 工具过滤、多 Agent 并行 → `src/coordinator/coordinatorMode.ts`
- **Hermes Agent**: 父子委托 — `delegate_tool.py` 生成子 AIAgent 实例，独立预算（50次），禁止递归委托 → `delegate_tool.py:30-39`
- **OpenCode**: 子任务 Session — `task` 工具创建子 Session（parentID），子 Agent 在独立上下文中执行 → `src/tool/task.ts`
- **Managed Agents**: 多脑多手扩展 — Brain 无状态化，水平扩展，按需连接 Hands → `REF-managed-agents.md`

```rust
/// SubAgent 编排
/// 借鉴: Claude Code Coordinator + Hermes delegate_tool
pub struct SubAgentOrchestrator {
    spawn_policy: SpawnPolicy,
}

impl SubAgentOrchestrator {
    /// 派生子 Agent
    /// 借鉴: Hermes — 独立预算（父预算的 50%），禁止超过 max_depth 的递归
    pub async fn spawn(
        &self,
        parent_ctx: &LoopContext,
        child_agent_id: &AgentId,
        task: &str,
    ) -> Result<SubAgentHandle> {
        // 1. 检查 spawn_policy.can_spawn
        // 2. 检查当前深度 < spawn_policy.max_depth
        // 3. 检查 child_agent_id 在 spawn_policy.allowed_children 中
        // 4. 为子 Agent 分配独立预算（父预算的 50%，借鉴 Hermes）
        // 5. 创建子 LoopContext，继承 trace_id（借鉴 Managed Agents 的 Session 共享）
        // 6. 启动子 AgenticLoop
    }
}
```

### 2.10 多模态支持（LLM Provider 能力透传）

**设计原则**: CyberClaw 是受控执行平台，不是文档处理引擎。现代 LLM（Claude、GPT-4V、Gemini）原生支持多模态输入。CyberClaw 的职责是**透传**，不是自建解析器。

**竞品参考**: OpenViking 的 VLM Provider 抽象（自动切换 Provider）、Cline 的截图反馈循环

```rust
// crates/cyberclaw-agent-runtime/src/multimodal.rs

/// 多模态内容 — 极简设计，不做语义分类
pub enum ContentPart {
    Text(String),
    /// 二进制内容（图片/文档/音频），由 LLM Provider 原生处理
    Binary {
        data: ContentData,       // Base64 / URL / FilePath
        media_type: String,      // MIME type (image/png, application/pdf, etc.)
    },
}

/// LLM Provider 能力声明
pub struct ProviderCapabilities {
    pub supported_media_types: Vec<String>,  // Provider 支持的 MIME 类型
    pub max_image_size: Option<usize>,       // 图片大小限制
}

/// 多模态透传逻辑:
/// - Provider 支持该 media_type → 直接透传
/// - Provider 不支持 → 返回错误（让用户选择支持该类型的 Provider）
/// 不自建 DocumentParser，不做格式转换

/// 截图 Capability — 通过 Connector → Capability 链路注册
/// 借鉴: Cline 截图 + 视觉反馈循环
// 注册为 Capability: "visual.screenshot"
// is_read_only: true, is_concurrency_safe: true
```

---

## 3. 上下文管理（P1 — Sprint 4）

### 3.1 当前问题

Agent 无上下文窗口管理，无压缩策略。长任务会超出 LLM 上下文窗口限制，导致信息丢失或 API 报错。

### 3.2 竞品方案对比

| 竞品 | 上下文管理方案 | 核心机制 | 代码参考 | CyberClaw 借鉴价值 |
|------|--------------|---------|---------|-------------------|
| **Hermes Agent** | 四阶段压缩算法 | 裁剪旧工具结果 → 保护头部 → 预算保护尾部(~20K) → LLM 结构化摘要 | `agent/context_compressor.py` | **★★★ 最佳压缩算法** |
| **Claude Code** | 自动+手动压缩 | 阈值触发（contextWindow - maxOutputTokens - 13K buffer）+ 连续失败熔断(3次) + 反应式压缩 | `services/compact/autoCompact.ts` | **★★★ 阈值计算和熔断** |
| **OpenViking** | L0/L1/L2 分级加载 | Abstract → Overview → Detail 按需加载，降低 Token 消耗 | `openviking/core/context.py` | **★★★ 分级摘要理念** |
| **OpenCode** | Prune + Compact 双策略 | prune(): 保留最近 40K token 工具输出; compact(): 用 compaction Agent 生成摘要 | `src/session/compaction.ts` | ★★ 双策略分离 |
| **Cline** | ContextManager | 文件读取优化（替换过时内容）+ 自动压缩（summarize_task/condense 工具） | `src/core/context/ContextManager.ts` | ★★ 过时内容替换 |
| **Managed Agents** | Session 外部化 | 追加式事件日志，Harness 可选择性加载上下文 | `REF-managed-agents.md` | ★★ 选择性加载理念 |

### 3.3 设计方案: 四阶段压缩 + L0/L1/L2 分级

```rust
// crates/cyberclaw-agent-runtime/src/context.rs

/// 上下文窗口守护
/// 借鉴: Claude Code autoCompact.ts 的阈值计算
pub struct ContextWindowGuard {
    context_window_size: usize,    // 总窗口大小 (tokens)
    max_output_tokens: usize,      // 最大输出 tokens
    compact_buffer: usize,         // 压缩缓冲 (借鉴 Claude Code: 13K)
    warning_threshold: usize,      // 警告阈值 (剩余 20K)
}

impl ContextWindowGuard {
    /// 阈值计算公式
    /// 借鉴: Claude Code — available = contextWindow - maxOutputTokens - buffer
    pub fn should_compact(&self, current_tokens: usize) -> CompactDecision {
        let available = self.context_window_size
            .saturating_sub(self.max_output_tokens)
            .saturating_sub(self.compact_buffer);

        if current_tokens > available { CompactDecision::Required }
        else if current_tokens > available - self.warning_threshold { CompactDecision::Warning }
        else { CompactDecision::NotNeeded }
    }
}

/// 四阶段压缩算法
/// 借鉴: Hermes Agent ContextCompressor (agent/context_compressor.py)
pub struct ContextCompressor {
    /// 最大连续压缩失败次数
    /// 借鉴: Claude Code 熔断机制 — 连续 3 次压缩失败后停止重试
    pub max_consecutive_failures: u32,  // 默认 3
}

impl ContextCompressor {
    pub async fn compact(&self, messages: &mut Vec<Message>, llm: &dyn LlmProvider) -> Result<()> {
        // Stage 1: 裁剪旧工具结果
        // 借鉴: Hermes — 保留最近 N 个完整 tool_call/result 对
        // 旧的工具结果替换为 "[结果已裁剪，摘要: ...]"
        self.prune_old_tool_results(messages);

        // Stage 2: 保护头部
        // 借鉴: Hermes — 系统 prompt + 初始用户指令不可压缩
        // 标记前 M 条消息为 protected

        // Stage 3: Token 预算保护尾部
        // 借鉴: Hermes — 最近 ~20K tokens 不压缩（保留最新上下文）
        // 借鉴: OpenCode — prune() 保留最近 40K token 工具输出

        // Stage 4: LLM 结构化摘要
        // 借鉴: Hermes — 中间部分压缩为结构化摘要
        // 借鉴: OpenCode — 用专门的 compaction Agent 生成摘要
        let summary = self.generate_summary(llm, &middle_messages).await?;
        // 替换中间消息为单条摘要消息
    }
}

/// 分级摘要
/// 借鉴: OpenViking L0/L1/L2 分级加载 (openviking/core/context.py)
pub struct SummaryLevels {
    pub l0: String,  // Abstract: 一句话摘要 (<50 tokens) — 用于列表展示
    pub l1: String,  // Overview: 段落级摘要 (<500 tokens) — 用于上下文注入
    pub l2: String,  // Detail: 完整详情 — 用于深入分析时按需加载
}

/// Execution 历史也采用分级摘要
/// 借鉴: Managed Agents — Session 外部化 + 选择性加载
/// Agent 查看历史执行时，先加载 L0 列表，按需加载 L1/L2
```

---

## 4. Memory 系统（P1 — Sprint 1+4）

### 4.1 当前问题

memory 模块有完整类型但 Agent 运行时不使用。Agent 的 `memory_file` 在 manifest 中声明但运行时未读写。

### 4.2 竞品方案对比

| 竞品 | Memory 方案 | 核心机制 | 代码参考 | CyberClaw 借鉴价值 |
|------|-----------|---------|---------|-------------------|
| **OpenViking** | 8 类记忆自动提取+去重 | User 维度（profile/preferences/entities/events）+ Agent 维度（cases/patterns/tools/skills），去重流程（向量相似 + LLM 判断 → CREATE/MERGE/DELETE/SKIP） | `openviking/session/memory_extractor.py:41-52` | **★★★ 分类体系最完善** |
| **Hermes Agent** | 三层记忆架构 | 内置记忆（MEMORY.md + USER.md）+ 会话搜索（FTS5）+ 外部记忆插件（7种 MemoryProvider ABC） | `agent/memory_provider.py`, `agent/memory_manager.py` | **★★★ Provider 抽象** |
| **DeerFlow** | LLM 驱动事实抽取 | 异步更新：MemoryMiddleware → MemoryQueue（30s 防抖）→ LLM 事实抽取 → 原子写入 | `agents/memory/` | ★★ 异步防抖更新 |
| **Claude Code** | memdir 文件存储 | `~/.claude/` 下的持久记忆目录，作为 AttachmentMessage 注入上下文 | `utils/claudemd.ts` | ★★ 文件化记忆 |
| **NanoClaw** | 分层 CLAUDE.md | 全局/群组/文件三层记忆，通过文件系统挂载自然隔离 | `groups/CLAUDE.md` 层次 | ★ 分层隔离 |

### 4.3 设计方案: MemoryProvider trait + 8 类记忆分类

```rust
// crates/cyberclaw-agent-runtime/src/memory.rs

/// Memory Provider 抽象
/// 借鉴: Hermes 7 种 MemoryProvider ABC (agent/memory_provider.py)
#[async_trait]
pub trait MemoryProvider: Send + Sync {
    async fn load(&self, agent_id: &AgentId, config: &MemoryConfig) -> Result<AgentMemory>;
    async fn save(&self, agent_id: &AgentId, memory: &AgentMemory) -> Result<()>;
    async fn search(&self, agent_id: &AgentId, query: &str, limit: usize) -> Result<Vec<MemoryEntry>>;
}

/// 3 类记忆分类（场景适配精简）
/// 参考 OpenViking 8 类体系，根据 CyberClaw "受控执行平台"定位精简:
/// - OpenViking 的 UserProfile/UserPreferences 面向社交 Bot 用户画像，不适用
/// - 保留与执行平台直接相关的分类
pub enum MemoryCategory {
    /// Agent 维度 — Agent 累积的执行经验和行为模式
    AgentMemory,
    /// 会话维度 — 当前会话上下文（跨请求持续）
    SessionMemory,
    /// 项目维度 — 项目级知识（代码库结构、约定、偏好）
    ProjectMemory,
}

/// 记忆去重管道
/// 借鉴: OpenViking MemoryDeduplicator
/// 向量相似度初筛 + LLM 判断精排 → CREATE/MERGE/DELETE/SKIP
pub struct MemoryDeduplicator;

impl MemoryDeduplicator {
    pub async fn deduplicate(
        &self,
        existing: &[MemoryEntry],
        new_entry: &MemoryEntry,
    ) -> DeduplicateAction {
        // Step 1: 向量相似度初筛（阈值 0.85）
        // Step 2: 相似条目送 LLM 判断 → CREATE(新增) / MERGE(合并) / DELETE(删除旧的) / SKIP(跳过)
    }
}

/// 记忆注入到 Agent 上下文
/// 借鉴: Hermes — <memory-context> XML fence 模式
pub fn inject_memory_to_context(memory: &AgentMemory) -> String {
    format!(
        "<memory-context>\n{}\n</memory-context>",
        memory.entries.iter()
            .map(|e| format!("- [{}] {}", e.category, e.content))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

/// 异步记忆更新
/// 借鉴: DeerFlow MemoryMiddleware — 30s 防抖 + 异步 LLM 事实抽取
/// 不阻塞主推理循环，后台异步提取和持久化
pub struct AsyncMemoryUpdater {
    queue: mpsc::Sender<MemoryUpdateRequest>,
    debounce: Duration,  // 默认 30s，借鉴 DeerFlow
}
```

---

## 5. 存储持久化（P0 — Sprint 2）

### 5.1 当前问题

所有状态（Execution/Artifact/Audit/Event）仅在内存中，重启即丢失。`PostgresStateStore` 在 `#[cfg(feature = "postgres")]` 后面，feature 未激活，代码未被编译验证。

### 5.2 竞品方案对比

| 竞品 | 存储方案 | 核心机制 | 代码参考 | CyberClaw 借鉴价值 |
|------|---------|---------|---------|-------------------|
| **IronClaw** | 双后端 DB 抽象 | `Database` supertrait 由 7 个 sub-trait 组合（78 个 async 方法），PostgreSQL + libSQL 双后端 | `src/db/CLAUDE.md` | **★★★ trait 拆分模式** |
| **gbrain** | 全 Postgres | 11 张核心表 + pgvector + pg_trgm + HNSW 索引 + RLS 全表启用 | `src/schema.sql` (274行) | **★★★ Schema 设计** |
| **OpenCode** | SQLite + Drizzle ORM | `bun:sqlite` 原生绑定，WAL 模式 + NORMAL 同步 + 64MB cache | `src/storage/db.ts` | ★★ SQLite 最佳实践 |
| **Hermes Agent** | SQLite WAL + FTS5 | 会话持久化 + 全文搜索虚拟表，Schema v6 增量迁移 | `hermes_state.py` | ★★ FTS5 全文搜索 |
| **Managed Agents** | Session 外部化 | 追加式事件日志（append-only log），故障恢复三步法（wake/getSession/emitEvent） | `REF-managed-agents.md` | ★★ 故障恢复模式 |

### 5.3 设计方案: sub-trait 拆分 + 三后端

```rust
// crates/cyberclaw-store/src/traits.rs

/// 存储层窄接口设计
/// 借鉴: IronClaw 7 sub-trait 组合 (src/db/CLAUDE.md)
/// 消费者只依赖最窄接口（接口隔离原则 ISP）

#[async_trait]
pub trait ExecutionStore: Send + Sync {
    async fn save_execution(&self, record: &ExecutionRecord) -> Result<()>;
    async fn get_execution(&self, id: &ExecutionId) -> Result<Option<ExecutionRecord>>;
    async fn list_executions(&self, filter: &ExecutionFilter) -> Result<Vec<ExecutionRecord>>;
    async fn update_execution_status(&self, id: &ExecutionId, status: ExecutionStatus) -> Result<()>;
}

/// 审计存储 — 追加式，不可删除
/// 借鉴: Managed Agents append-only Session Log
#[async_trait]
pub trait AuditStore: Send + Sync {
    async fn append_event(&self, event: &AuditEvent) -> Result<()>;
    async fn query_events(&self, filter: &AuditFilter) -> Result<Vec<AuditEvent>>;
    // 注意: 无 delete 方法 — 审计日志不可删除
}

/// 会话存储 — 支持故障恢复
/// 借鉴: Managed Agents 故障恢复三步法 (wake/getSession/emitEvent)
#[async_trait]
pub trait SessionStore: Send + Sync {
    async fn save_session(&self, session: &SessionRecord) -> Result<()>;
    async fn get_session(&self, id: &SessionId) -> Result<Option<SessionRecord>>;
    async fn append_turn(&self, session_id: &SessionId, turn: &TurnRecord) -> Result<()>;
    /// 从检查点恢复 — Harness 崩溃后新实例可接管
    async fn get_events_since(&self, session_id: &SessionId, checkpoint: u64) -> Result<Vec<SessionEvent>>;
}

#[async_trait]
pub trait ArtifactStore: Send + Sync { /* ... */ }
#[async_trait]
pub trait PolicyStore: Send + Sync { /* ... */ }
#[async_trait]
pub trait MemoryStore: Send + Sync { /* ... */ }

/// 组合 supertrait
pub trait StateStore: ExecutionStore + ArtifactStore + AuditStore + PolicyStore + SessionStore + MemoryStore {}
```

**三种后端**:

| 后端 | 用途 | 借鉴来源 | 关键特性 |
|------|------|---------|---------|
| `InMemoryStateStore` | 测试 | 现有 | 适配新 trait |
| `SqliteStateStore` | 开发/单机 | OpenCode `bun:sqlite` + Hermes SQLite WAL | WAL 模式 + FTS5 全文搜索 + 零配置 |
| `PostgresStateStore` | 生产 | gbrain Schema 设计 + IronClaw 双后端 | 连接池(deadpool) + RLS 租户隔离 + 版本化迁移 |

```rust
/// 迁移系统
/// 借鉴: gbrain 版本化迁移 (src/schema.sql)
/// 借鉴: Hermes Schema v6 增量迁移 (hermes_state.py)
pub struct MigrationRunner {
    pub migrations: Vec<Migration>,
}

pub struct Migration {
    pub version: u32,
    pub name: String,
    pub up: String,   // SQL
    pub down: String,  // SQL
}
```

---

## 6. 治理与安全升级（P1 — Sprint 3）

### 6.1 当前问题

`DefaultPolicyEngine` 只看 RiskLevel 阈值。没有 RBAC/ABAC。`TenantBoundaryPolicy` 代码存在但未集成到 Orchestrator。CyberClaw 以"受控"为核心卖点，但治理深度仅一层。

### 6.2 竞品方案对比

| 竞品 | 安全/治理方案 | 核心机制 | 代码参考 | CyberClaw 借鉴价值 |
|------|-------------|---------|---------|-------------------|
| **Claude Code** | 10 层安全纵深 | 工具过滤 → 权限模式 → 规则匹配 → 工具自检 → Hook 拦截 → 分类器 → 交互确认 → 沙箱 → 拒绝追踪 | `src/utils/permissions/` (24文件) | **★★★ 纵深理念** |
| **OpenCode** | 规则引擎 | Wildcard 模式匹配 + allow/deny/ask 三态 + Effect Deferred 交互确认 | `src/permission/index.ts` | **★★★ 规则引擎实现** |
| **Hermes Agent** | 危险命令审批 | 30+ 正则模式 + Unicode 归一化 + Smart Approval（LLM 辅助判断是否危险） | `tools/approval.py` | ★★ Smart Approval |
| **IronClaw** | Skill 信任模型 | `Installed < Trusted` 排序，最低信任级别决定有效工具上限 | `src/skills/mod.rs:58-71` | ★★ 信任分级 |
| **DeerFlow** | GuardrailProvider Protocol | 工具调用前置授权，`fail_closed=True` 策略，支持 OAP 标准 | `guardrails/provider.py` | ★★ fail_closed 原则 |
| **OpenViking** | Account/User/Agent 三级租户 | URI 空间隔离（`viking://user/{account}_{user}/`），RBAC（ROOT/ADMIN/USER） | `openviking/server/auth.py` | ★★ 多租户 RBAC |
| **Trustworthy Agents** | 分级权限框架 | always-allowed / requiring approval / blocked 三级 + Plan Mode | `REF-trustworthy-agents.md` | ★★★ 权限分级理念 |
| **cc-connect** | 用户角色系统 | UserRoleManager + per-role disabled_commands + rate_limit | `core/user_roles.go` | ★ 角色级命令限制 |
| **NanoClaw** | 凭据代理 | HTTP 代理注入真实密钥，容器收到 placeholder，.env 被 /dev/null 遮蔽 | `src/credential-proxy.ts:65-79` | ★★★ 凭据隔离 |

### 6.3 设计方案

#### 6.3.1 规则引擎

```rust
// crates/cyberclaw-governance/src/rules.rs

/// 统一规则模型
/// 借鉴: Claude Code allow/deny/ask 三态 + OpenCode wildcard 匹配
/// 借鉴: Trustworthy Agents — always-allowed / requiring approval / blocked
pub struct PolicyRule {
    pub id: String,
    pub pattern: CapabilityPattern,       // glob 匹配 (e.g. "fs.*", "cmd.exec", "*.read")
    pub action: PolicyAction,
    pub conditions: Vec<PolicyCondition>,
    pub priority: i32,                    // 高优先级覆盖低优先级
    pub source: PolicySource,
}

/// 借鉴: Trustworthy Agents 三级权限
pub enum PolicyAction {
    Allow,                                          // always-allowed
    Deny { reason: String },                         // blocked
    RequireApproval { approver: ApproverSpec },       // requiring approval
}

/// 规则来源分层（4 层，高层覆盖低层）
/// 借鉴: Claude Code 多层规则来源（system → tenant → project → session）
pub enum PolicySource {
    SystemDefault,     // 平台内置（最低优先级）
    TenantPolicy,      // 租户级策略
    ProjectPolicy,     // 项目级策略
    SessionOverride,   // 会话级覆盖（最高优先级）
}

/// 触发条件
/// 借鉴: OpenViking RBAC 三级角色（ROOT/ADMIN/USER）
pub enum PolicyCondition {
    RiskLevel(RiskLevel),                  // 现有能力
    ActorRole(String),                     // 借鉴: OpenViking ROOT/ADMIN/USER
    TimeWindow { start: Time, end: Time }, // 工作时间限制
    ResourceQuota { metric: String, limit: u64 },
    TenantId(TenantId),
}

/// Skill 信任分级
/// 借鉴: IronClaw Skill 信任模型 (src/skills/trust.rs) — 按来源区分信任级别
/// 信任级别决定 Skill 可声明的 Capability 范围
pub enum SkillTrustLevel {
    /// 用户本地安装，未经验证 — 仅可使用 Allow 类 Capability
    /// 注: Installed + Allow 组合保持 Allow（不降级），仅 Installed + RequireApproval 降级为 Deny
    Installed,
    /// 平台审核通过 — 可使用 RequireApproval 类 Capability
    Verified,
    /// 组织管理员信任 — 可使用全部 Capability（仍受 PolicyRule 门禁）
    Trusted,
}

impl SkillTrustLevel {
    /// 信任级别与 PolicyAction 联动:
    /// Installed Skill 声明的 Capability 若命中 RequireApproval 规则 → 自动提升为 Deny
    /// Trusted Skill 声明的 Capability 若命中 RequireApproval 规则 → 保持 RequireApproval
    pub fn effective_action(&self, base_action: &PolicyAction) -> PolicyAction {
        match (self, base_action) {
            (SkillTrustLevel::Installed, PolicyAction::RequireApproval { .. }) => {
                PolicyAction::Deny { reason: "untrusted skill".into() }
            }
            _ => base_action.clone(),
        }
    }
}
```

#### 6.3.2 Capability 自描述增强

```rust
/// Capability 元数据增强
/// 借鉴: Claude Code 工具自描述 — isReadOnly, isDestructive 等属性
/// 让 PolicyEngine 可以基于工具属性而非仅名称做决策
pub struct CapabilityContract {
    // ... 现有字段 ...

    /// 借鉴: Claude Code 工具自检属性
    pub is_read_only: bool,           // 只读操作，低风险
    pub is_destructive: bool,         // 破坏性操作，需审批
    pub is_concurrency_safe: bool,    // 可并行执行
    pub max_result_size_chars: usize, // 结果最大长度（借鉴 Claude Code maxResultSizeChars）

    /// 借鉴: gbrain Contract-first — 运行时可验证的 JSON Schema
    pub input_json_schema: Option<serde_json::Value>,
    pub output_json_schema: Option<serde_json::Value>,
}
```

#### 6.3.3 凭据隔离

```rust
/// 凭据代理
/// 借鉴: NanoClaw credential-proxy (src/credential-proxy.ts:65-79)
/// 借鉴: Managed Agents — Sandbox 无凭据，通过 MCP Proxy 持有凭据
/// 借鉴: IronClaw — AES-256-GCM + OS Keychain

/// Connector 执行环境不直接持有敏感凭据
/// 凭据通过 CredentialProxy 在传输层透明注入
pub struct CredentialProxy {
    vault: Box<dyn CredentialVault>,
}

#[async_trait]
pub trait CredentialVault: Send + Sync {
    /// 获取凭据（不返回明文给调用者，直接注入到请求中）
    async fn inject(&self, connector_id: &ConnectorId, request: &mut HttpRequest) -> Result<()>;
}

/// 三种 Vault 实现
/// 借鉴: OpenViking 多 KMS Provider (openviking/crypto/encryptor.py:70-98)
pub enum VaultBackend {
    EnvVar,           // 环境变量（开发）
    OsKeychain,       // OS 级密钥存储（借鉴 IronClaw）
    ExternalVault,    // HashiCorp Vault / AWS KMS（生产）
}
```

#### 6.3.4 Hook 系统（Platform Plugin 运行时）

```rust
/// Hook 注册机制 — Platform Plugin 的运行时实现
/// 借鉴: Claude Code 5 种 Hook 类型 (command/prompt/agent/http/function)
/// 借鉴: DeerFlow 13 层中间件管道 (agents/middlewares/)
pub struct HookRegistry {
    hooks: Vec<RegisteredHook>,
}

pub struct RegisteredHook {
    pub plugin_id: PlatformPluginId,
    pub point: HookPoint,
    pub target: HookTarget,
    pub handler: Arc<dyn HookHandler>,
    /// 借鉴: DeerFlow GuardrailProvider — fail_closed 原则
    pub failure_policy: FailurePolicy,
}

/// Hook 执行点
/// 借鉴: Claude Code — PreToolUse / PostToolUse / Notification / Agent / HTTP
pub enum HookTarget {
    BeforeToolCall,     // 工具调用前拦截
    AfterToolCall,      // 工具调用后增强
    BeforeLlmCall,      // LLM 调用前（上下文修改）
    AfterLlmCall,       // LLM 调用后（输出过滤）
    OnExecutionComplete, // 执行完成（审计增强）
    OnError,            // 错误处理
}

/// Plugin manifest 的 hooks 字段自动加载
/// 替代 libloading 动态库机制
pub struct PluginHookLoader;

impl PluginHookLoader {
    /// 扫描 ecosystem/platform-plugins/ 目录
    /// 读取每个 Plugin 的 manifest.yaml hooks 字段
    /// 自动注册到 HookRegistry
    pub fn load_from_ecosystem(&self, ecosystem_dir: &Path) -> Result<Vec<RegisteredHook>> {
        // ...
    }
}
```

#### 6.3.5 Smart Approval

```rust
/// Smart Approval — LLM 辅助的危险操作判断
/// 借鉴: Hermes Agent Smart Approval (tools/approval.py)
/// 当 PolicyRule 匹配结果为 RequireApproval 且上下文足够时，
/// 用 LLM 判断操作是否真的危险，减少审批疲劳
///
/// 借鉴: Trustworthy Agents "Plan Mode" — 一次性展示完整行动计划
pub struct SmartApproval {
    llm: Box<dyn LlmProvider>,
    /// 30+ 危险模式正则（借鉴 Hermes）
    danger_patterns: Vec<Regex>,
}
```

#### 6.4 自学习治理（策略自动演进）

**当前问题**: 治理规则完全静态，管理员手动维护。随着 Agent 执行量增长，规则维护成本线性增加。

**竞品借鉴**:
- **Hermes Agent**: 自学习 Skill 创建 — Agent 从执行经验中自动提取可复用模板 → `agent/skill_creator.py`
- **OpenViking**: AgentPatterns 行为模式积累 — Agent 自动从执行历史中提取行为模式 → `openviking/session/memory_extractor.py`
- **Trustworthy Agents**: 五原则中的"可审计性" — 执行结果自动评估，反馈到规则调整 → `REF-trustworthy-agents.md`

```rust
// crates/cyberclaw-governance/src/adaptive.rs

/// 治理信号收集器
/// 借鉴: OpenClaw-RL 透明数据拦截 — 在不修改执行链的情况下收集治理信号
/// 借鉴: AReaL stats_tracker 全局收集 (areal/utils/stats_tracker.py)
pub struct GovernanceSignalCollector {
    /// 审批决策历史（allow/deny/ask 的实际结果）
    decision_history: Vec<DecisionRecord>,
    /// 规则命中统计
    rule_hit_stats: HashMap<String, RuleHitStats>,
}

/// 规则命中统计
pub struct RuleHitStats {
    pub rule_id: String,
    pub total_hits: u64,
    pub allow_count: u64,
    pub deny_count: u64,
    pub approval_count: u64,
    pub avg_approval_latency: Duration,
    /// 用户覆盖率（用户手动否决 Smart Approval 建议的比率）
    pub user_override_rate: f64,
}

/// 策略演进引擎
/// 借鉴: Hermes skill_creator — 从执行经验中提取可复用规则
/// 借鉴: OpenViking AgentPatterns — 行为模式积累驱动规则优化
pub struct PolicyEvolutionEngine {
    signal_collector: GovernanceSignalCollector,
    llm: Box<dyn LlmProvider>,
}

impl PolicyEvolutionEngine {
    /// 周期性分析治理信号，生成规则建议
    /// 输出不自动生效 — 通过元治理规则审批
    pub async fn analyze_and_suggest(&self) -> Result<Vec<PolicySuggestion>> {
        // 1. 识别高频审批规则（审批疲劳信号）
        //    → 建议: 如果连续 N 次都被批准，建议升级为 Allow
        // 2. 识别从未命中的规则（死规则）
        //    → 建议: 标记为 deprecated，建议清理
        // 3. 识别用户高频覆盖的 Smart Approval 建议
        //    → 建议: 调整该规则的风险评估逻辑
        // 4. 融合执行结果信号(成功/失败) + 审批信号(快速批准/犹豫/拒绝)
        //    → 生成规则置信度评分
        todo!()
    }
}

/// [P0 修正] 元治理规则 — 不可被 PolicyEvolutionEngine 修改的固化规则
/// 解决"谁审批审批规则变更"的自引用循环问题
pub struct MetaGovernancePolicy;

impl MetaGovernancePolicy {
    /// 元治理规则（hardcoded，不受 PolicyEvolutionEngine 影响）:
    /// 1. 策略变更必须经过 TenantAdmin 角色审批
    /// 2. CreateNewRule 只能创建 RequireApproval 或 Deny 类型（不能自动创建 Allow）
    /// 3. PromoteToAllow 变更需 72h 冷却期 + 二次确认
    /// 4. 置信度低于 min_confidence_threshold (0.7) 的建议不进入审批流
    /// 5. DeleteRule 操作被禁止 — PolicyEvolutionEngine 只能 Promote/Demote/Reduce，不能删除规则
    pub const MIN_CONFIDENCE_THRESHOLD: f64 = 0.7;
    pub const PROMOTE_COOLDOWN: Duration = Duration::from_secs(72 * 3600);
}

/// 策略建议（需管理员审批后生效）
pub struct PolicySuggestion {
    pub suggestion_type: SuggestionType,
    pub target_rule_id: String,
    pub rationale: String,           // LLM 生成的变更理由
    pub evidence: Vec<DecisionRecord>, // 支撑数据
    pub confidence: f64,             // 0.0-1.0 置信度
}

pub enum SuggestionType {
    PromoteToAllow,       // RequireApproval → Allow
    DemoteToBlock,        // Allow → Deny (检测到风险)
    ReduceApprovalScope,  // 缩小审批范围（减少疲劳）
    DeprecateRule,        // 标记死规则
    CreateNewRule,        // 从行为模式中提取新规则
}
```

---

## 7. 可观测性升级（P1 — Sprint 4）

### 7.1 当前问题

指标为内存 `AtomicUsize` 计数器，无 Prometheus/OpenTelemetry 导出。distributed tracing 有类型定义但无实际跨节点传播。

### 7.2 竞品方案对比

| 竞品 | 可观测性方案 | 核心机制 | 代码参考 | CyberClaw 借鉴价值 |
|------|-----------|---------|---------|-------------------|
| **OpenViking** | OpenTelemetry + Prometheus | 分布式追踪 + 指标采集，~1,700 行遥测模块 | `openviking/telemetry/` | **★★★ 最完整实现** |
| **Claude Code** | OpenTelemetry + gRPC | 延迟加载（~400KB OTEL + ~700KB gRPC），动态 `import()` | `services/api/` + OTel 集成 | ★★ 延迟加载模式 |
| **AReaL** | WandB / TensorBoard / SwanLab | `stats_tracker` 全局收集 + `@trace_perf` 装饰器 | `areal/utils/stats_tracker.py` | ★ 装饰器模式 |
| **Cline** | PostHog + OpenTelemetry | 双通道遥测收集 | `src/services/telemetry/` | ★ 双通道模式 |

### 7.3 设计方案

```rust
// crates/cyberclaw-observability/src/otel.rs

/// OpenTelemetry 集成
/// 借鉴: OpenViking telemetry 模块 (openviking/telemetry/, ~1,700 行)
/// 借鉴: Claude Code 延迟加载模式（避免启动性能影响）

/// 延迟初始化
/// 借鉴: Claude Code — ~400KB OTEL + ~700KB gRPC 动态加载
/// CyberClaw: 首次使用时初始化，不影响启动时间
pub struct LazyOtelInit {
    initialized: AtomicBool,
    tracer: OnceCell<Tracer>,
    meter: OnceCell<Meter>,
}

/// 桥接现有 tracing crate
/// 使用 tracing-opentelemetry 将现有 tracing span 导出为 OTEL span
/// 零侵入: 现有 #[instrument] 宏和 tracing::info! 自动导出

/// Prometheus 指标导出
/// 借鉴: OpenViking Prometheus 集成
/// 在 /metrics 端点暴露，Grafana 可直接采集
pub fn setup_prometheus_exporter(app: Router) -> Router {
    // 注册自定义指标:
    // - cyberclaw_executions_total (Counter)
    // - cyberclaw_execution_duration_seconds (Histogram)
    // - cyberclaw_active_agents (Gauge)
    // - cyberclaw_tool_calls_total (Counter, labels: tool_name, status)
    // - cyberclaw_policy_decisions_total (Counter, labels: decision)
    // - cyberclaw_llm_tokens_total (Counter, labels: provider, direction)
}
```

### 7.4 分布式 Trace 传播

**当前问题**: Trace 仅单节点 span，无跨节点传播，多节点部署时无法追踪完整请求链路。

**竞品借鉴**:
- **OpenViking**: OpenTelemetry + Prometheus 完整集成，TraceID 注入日志 → `openviking/telemetry/` (~1,700 行)
- **DeerFlow**: 13 层中间件管道中的 Telemetry 层 — 中间件模式自动注入追踪 → `agents/middlewares/`
- **Managed Agents**: Session 级追踪 — 跨 Brain/Hands/Sandbox 的统一 Session Log → `REF-managed-agents.md`

```rust
// crates/cyberclaw-observability/src/distributed.rs

/// 分布式 Trace 传播
/// 借鉴: OpenViking telemetry 模块 — W3C TraceContext 标准
/// 借鉴: Managed Agents — 跨 Session/Harness/Sandbox 的统一追踪
pub struct DistributedTraceContext {
    /// W3C traceparent: version-trace_id-parent_id-flags
    pub trace_id: TraceId,
    pub parent_span_id: SpanId,
    pub trace_flags: u8,
    /// W3C baggage: 跨服务传递的业务上下文
    pub baggage: HashMap<String, String>,
}

impl DistributedTraceContext {
    /// 从 HTTP 请求头中提取 W3C TraceContext
    /// 用于集群节点间 RPC 调用
    pub fn from_headers(headers: &HeaderMap) -> Option<Self> {
        let traceparent = headers.get("traceparent")?.to_str().ok()?;
        Self::parse_traceparent(traceparent)
    }

    /// 注入到 HTTP 请求头中
    pub fn inject_headers(&self, headers: &mut HeaderMap) {
        headers.insert("traceparent", self.to_traceparent().parse().unwrap());
        if !self.baggage.is_empty() {
            headers.insert("baggage", self.to_baggage().parse().unwrap());
        }
    }
}

/// 跨节点 Span 传播中间件
/// 借鉴: DeerFlow 中间件管道 — 自动为关键路径注入追踪
/// 借鉴: OpenViking TraceID 注入日志 (openviking/telemetry/)
pub struct TracePropagationMiddleware;

impl TracePropagationMiddleware {
    /// 作为 axum 中间件注入到集群 RPC 路由
    /// 自动提取入站 traceparent → 创建子 span → 注入出站请求
    pub async fn propagate(
        req: axum::extract::Request,
        next: axum::middleware::Next,
    ) -> axum::response::Response {
        // 1. 从入站请求提取 DistributedTraceContext
        // 2. 创建新的子 span，关联到父 trace
        // 3. 将 trace context 存入 task-local
        // 4. 执行后续处理
        // 5. 出站 RPC 自动从 task-local 注入 traceparent
        todo!()
    }
}

/// Agent 执行链路追踪
/// 借鉴: Managed Agents Session Log — 完整记录 Agent → SubAgent → Tool 调用链
/// 每个 AgenticLoop 迭代自动生成 span:
///   root_span (execution)
///     └── loop_iteration_span
///           ├── llm_call_span (provider, model, tokens)
///           ├── tool_call_span (capability_id, connector_id, duration)
///           │     └── policy_eval_span (decision, rule_id)
///           └── memory_update_span
pub struct AgentTraceInstrumentation;

/// SubAgent 跨进程追踪
/// 当 SubAgentOrchestrator::spawn() 创建子 Agent 时，
/// 自动传递 trace_id，子 Agent 的所有 span 关联到父链路
/// 借鉴: Managed Agents — Brain spawn Hands 时传递 Session context
```

---

## 8. Workflow 集成

### 8.1 当前问题

WorkflowEngine 独立存在（完整的 DAG 执行引擎），但 Orchestrator 的执行模型是扁平的 `ExecutionPlan`（PlannedAction 列表），不走 WorkflowEngine。

### 8.2 竞品方案对比

| 竞品 | 编排方案 | 核心机制 | 代码参考 | CyberClaw 借鉴价值 |
|------|---------|---------|---------|-------------------|
| **DeerFlow** | 中间件管道 | 13 层可组合中间件，`@Next`/`@Prev` 装饰器控制顺序 | `agents/middlewares/` | **★★★ 中间件模式** |
| **Hermes Agent** | 父子委托 | delegate_tool 生成子 AIAgent，独立预算 | `delegate_tool.py:30-39` | ★★ 委托模式 |
| **OpenCode** | 子任务 Session | task 工具创建子 Session | `src/tool/task.ts` | ★★ 子任务隔离 |
| **AReaL** | 分布式异步 RL | Master → Workers 异步分发，独立 GPU 资源 | `areal/trainer/` | ★ 异步分发 |

### 8.3 设计方案

**核心借鉴**: DeerFlow 13 层中间件管道 (`agents/middlewares/`) + OpenCode 子任务隔离

```rust
// crates/cyberclaw-control-plane/src/middleware.rs

/// 执行管道中间件 trait
/// 借鉴: DeerFlow @Next/@Prev 装饰器 — 可组合的中间件链
/// 借鉴: axum/tower 中间件模式 — Rust 生态验证方案
#[async_trait]
pub trait ExecutionMiddleware: Send + Sync {
    async fn handle(
        &self,
        request: ExecutionRequest,
        next: &dyn MiddlewareNext,
    ) -> Result<ExecutionResponse>;
}

#[async_trait]
pub trait MiddlewareNext: Send + Sync {
    async fn run(&self, request: ExecutionRequest) -> Result<ExecutionResponse>;
}

/// 中间件管道构建器
/// 借鉴: DeerFlow 13 层可组合中间件 — 声明式注册，运行时排序
pub struct MiddlewarePipeline {
    middlewares: Vec<(i32, Arc<dyn ExecutionMiddleware>)>, // (priority, middleware)
}

impl MiddlewarePipeline {
    pub fn builder() -> MiddlewarePipelineBuilder {
        MiddlewarePipelineBuilder::new()
    }

    /// 按 priority 排序后依次执行
    pub async fn execute(&self, request: ExecutionRequest) -> Result<ExecutionResponse> {
        let chain = MiddlewareChain::new(&self.middlewares, 0);
        chain.run(request).await
    }
}

/// 内置中间件
/// GovernedStepRunner 拆分为 5 个独立中间件:

/// 1. 策略评估中间件（最高优先级）
pub struct PolicyMiddleware {
    engine: Arc<PolicyEngine>,
}

/// 2. 审计日志中间件（写入 AuditStore）
/// 借鉴: Managed Agents append-only Session Log
pub struct AuditMiddleware {
    store: Arc<dyn AuditStore>,
}

/// 3. 工作流编排中间件
/// 当 ExecutionPlan 包含 DAG 依赖时自动激活 WorkflowEngine
pub struct WorkflowMiddleware {
    engine: Arc<WorkflowEngine>,
}

/// 4. 执行中间件（实际调用 Connector → Capability）
pub struct ExecutionMiddleware {
    dispatcher: Arc<CapabilityDispatcher>,
}

/// 5. 追踪中间件（OpenTelemetry span 注入）
pub struct TraceMiddleware {
    instrumentation: Arc<AgentTraceInstrumentation>,
}

/// 默认管道组装
pub fn build_default_pipeline(deps: &PipelineDependencies) -> MiddlewarePipeline {
    MiddlewarePipeline::builder()
        // [P1 修正] TraceMiddleware 放最外层（priority 0），捕获完整链路耗时
        .add(0,   Arc::new(TraceMiddleware::new(deps.trace.clone())))
        .add(100, Arc::new(PolicyMiddleware::new(deps.policy_engine.clone())))
        .add(200, Arc::new(AuditMiddleware::new(deps.audit_store.clone())))
        .add(300, Arc::new(WorkflowMiddleware::new(deps.workflow_engine.clone())))
        .add(400, Arc::new(ExecutionMiddleware::new(deps.dispatcher.clone())))
        // Platform Plugin Hook 中间件在此注入（见 §6.3.4）
        .build()
}
```

### 8.4 Workflow 触发器扩展

**设计原则**: CI/CD 管线是 Workflow 的一种使用模式，不需要独立领域模型。通过扩展 WorkflowEngine 的触发器即可支持。

```rust
// crates/cyberclaw-workflow/src/trigger.rs

/// Workflow 触发器 — 扩展现有 WorkflowEngine
/// 借鉴: cc-connect Cron + Webhook (core/cron.go)
pub enum WorkflowTrigger {
    /// Agent 执行完成后自动触发
    OnExecutionComplete { agent_id: AgentId, status_filter: Vec<ExecutionStatus> },
    /// Webhook 事件（GitHub push / PR / issue）
    OnWebhook { event_type: String, filter: serde_json::Value },
    /// Cron 定时（复用现有 cyberclaw-scheduler）
    OnSchedule { cron_expr: String },
    /// 手动触发
    Manual,
}
// CI/CD 语义直接用 WorkflowDefinition DAG 表达，不引入独立类型系统
```

---

## 9. 平台接入扩展

### 9.1 竞品方案对比

| 竞品 | 平台接入方案 | 核心机制 | 代码参考 | CyberClaw 借鉴价值 |
|------|-----------|---------|---------|-------------------|
| **cc-connect** | Bridge WebSocket + 20+ 接口 | 能力协商机制，Go interface type assertion 能力探测 | `core/interfaces.go:214-238`, `core/bridge.go` | **★★★ 能力探测模式** |
| **Hermes Agent** | 8 种消息网关 | Telegram/Discord/Slack/WhatsApp/Signal/Email/HomeAssistant/Web | `hermes_cli/gateways/` | ★★ 多渠道适配 |
| **OpenClaw** | 110+ 扩展类型 | 声明式 Plugin SDK + Provider 注册 | `src/plugins/registry.ts` | ★★ 扩展生态规模 |
| **gbrain** | Contract-first Operation | 一处定义，CLI/MCP/tools-json 全自动派生 | `src/core/operations.ts` | **★★★ 接口漂移消除** |

### 9.2 设计方案: 能力探测 + Contract-first 派生

```rust
// crates/cyberclaw-connectors/src/probe.rs

/// Connector 能力探测
/// 借鉴: cc-connect 接口能力探测 (core/interfaces.go:214-238)
/// 在 Rust 中用 trait object downcast 替代 Go 的 type assertion
pub trait ConnectorCapabilityProbe {
    fn supports_streaming(&self) -> bool { false }
    fn supports_batch(&self) -> bool { false }
    fn supports_webhook(&self) -> bool { false }
    fn supports_bidirectional(&self) -> bool { false }
}

/// 运行时能力协商
/// 借鉴: cc-connect — Bridge 连接时双向协商支持的能力集
pub struct CapabilityNegotiator;

impl CapabilityNegotiator {
    /// Connector 注册时执行能力探测，记录到 Registry
    pub fn negotiate(&self, connector: &dyn Connector) -> NegotiatedCapabilities {
        let probe = connector.as_capability_probe();
        NegotiatedCapabilities {
            streaming: probe.map_or(false, |p| p.supports_streaming()),
            batch: probe.map_or(false, |p| p.supports_batch()),
            webhook: probe.map_or(false, |p| p.supports_webhook()),
            bidirectional: probe.map_or(false, |p| p.supports_bidirectional()),
        }
    }
}

/// Connector 执行错误分类
/// 借鉴: IronClaw (src/llm/error.rs) + Hermes (agent/error_classifier.py)
/// 将 §2.5 LLM Provider 的瞬态/非瞬态分类泛化到所有 Connector
pub enum ConnectorErrorKind {
    // 瞬态 — 可重试（指数退避）
    Timeout,
    RateLimited { retry_after: Option<Duration> },
    ConnectionReset,
    ServiceUnavailable,
    // 非瞬态 — 不重试，直接上报
    AuthenticationFailed,
    PermissionDenied,
    InvalidInput { reason: String },
    ResourceNotFound,
    /// 未知错误默认为非瞬态（fail-safe）
    Unknown { source: Box<dyn std::error::Error + Send + Sync> },
}

impl ConnectorErrorKind {
    pub fn is_transient(&self) -> bool {
        matches!(self,
            Self::Timeout | Self::RateLimited { .. } |
            Self::ConnectionReset | Self::ServiceUnavailable
        )
    }
}

/// Connector trait 要求实现 classify_error，使 Orchestrator 可统一重试决策
/// 不同 Connector 实现各自的错误映射（HTTP status → ConnectorErrorKind 等）
pub trait ConnectorErrorClassifier {
    fn classify_error(&self, err: &anyhow::Error) -> ConnectorErrorKind;
}

/// Contract-first Capability 定义
/// 借鉴: gbrain operations.ts — 一处定义，多处派生
/// 消除接口漂移: Capability 定义是唯一真相源
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct CapabilityDefinition {
    pub id: String,
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,   // JSON Schema
    pub output_schema: serde_json::Value,  // JSON Schema
    pub contract: CapabilityContract,       // §6.3.2 自描述属性
}

impl CapabilityDefinition {
    /// 自动派生 LLM tool_definition（用于 AgenticLoop）
    pub fn to_llm_tool(&self) -> LlmToolDefinition {
        LlmToolDefinition {
            name: self.id.clone(),
            description: self.description.clone(),
            input_schema: self.input_schema.clone(),
        }
    }

    /// 自动派生 OpenAPI endpoint schema
    pub fn to_openapi_operation(&self) -> openapi::Operation {
        // 从 JSON Schema 自动生成 request/response body
        todo!()
    }

    /// 自动派生 MCP tool schema
    pub fn to_mcp_tool(&self) -> mcp::ToolDefinition {
        mcp::ToolDefinition {
            name: self.id.clone(),
            description: self.description.clone(),
            input_schema: self.input_schema.clone(),
        }
    }

    /// 自动派生 CLI 参数定义
    pub fn to_cli_args(&self) -> Vec<clap::Arg> {
        // 从 JSON Schema properties 自动生成 CLI 参数
        todo!()
    }
}
```

### 9.3 RAG / 向量检索接入

**设计原则**: 遵循 CLAUDE.md "外部知识检索通过 Connector 接入"。CyberClaw 是执行平台，不是搜索引擎。检索能力通过 Connector → Capability 链路对接外部服务。

**竞品参考**: gbrain RRF 混合搜索管道、OpenViking 层次化检索器、Hermes FTS5

```rust
// crates/cyberclaw-connectors/src/retrieval.rs

/// 检索 Connector — 通过 Connector → Capability 接入外部向量数据库
///
/// 注册为 Capability:
///   "retrieval.search"  (is_read_only: true)
///   "retrieval.ingest"  (is_read_only: false)
///   "retrieval.delete"  (is_destructive: true)

/// 检索后端适配 — 通过 Connector 对接，不内建搜索引擎
#[async_trait]
pub trait RetrievalConnector: Connector {
    /// 语义搜索（向量 + 全文，由后端实现混合检索）
    async fn search(&self, query: &str, top_k: usize) -> Result<Vec<SearchResult>>;
    /// 索引内容
    async fn ingest(&self, id: &str, content: &str, metadata: serde_json::Value) -> Result<()>;
}

/// 内置后端适配:
/// - pgvector（复用 PostgresStateStore 连接）
/// - Pinecone / Qdrant / Milvus（通过 MCP Connector）
/// - SQLite FTS5（开发场景，借鉴 Hermes）
/// RRF 融合、去重等高级检索逻辑由后端服务负责，CyberClaw 只做透传
```

### 9.4 多通道接入

**设计原则**: CyberClaw 是 API-first 平台。IM 平台（Slack/飞书/Discord/Telegram）对接通过 Connector 模型统一接入 — 每个 IM 平台是一个 `MessageGatewayConnector`，遵循 Connector → Capability 链路，不在核心架构中引入独立的 Gateway 框架。

**竞品参考**: cc-connect Bridge 协议、Hermes 8 种消息网关

```rust
// crates/cyberclaw-connectors/src/gateway.rs

/// 消息网关 Connector — 通过 Connector 模型统一多通道接入
/// 注册为 Capability:
///   "gateway.receive"   (webhook 接收外部消息)
///   "gateway.send"      (推送消息到外部平台)

/// 消息网关 Connector trait
/// 每个 IM 平台实现此 trait 作为独立 Connector
#[async_trait]
pub trait MessageGatewayConnector: Connector {
    /// 将外部消息转换为 ExecutionRequest
    fn to_execution_request(&self, raw: &serde_json::Value) -> Result<ExecutionRequest>;
    /// 将执行结果转换为外部平台消息格式
    fn to_platform_message(&self, result: &ExecutionResponse) -> Result<serde_json::Value>;
}

/// Webhook 入口 — 复用现有 HTTP API
/// 外部 IM 平台通过 Webhook 推送消息到 /api/webhook/{connector_id}
/// Server 路由到对应 MessageGatewayConnector → 转换 → Orchestrator 执行
/// 借鉴: cc-connect Token 认证 + constant-time 比较 (core/bridge.go:1048-1064)
```

### 9.5 分布式多节点扩展

**当前问题**: 现有 Raft 共识和集群 API 已搭建骨架，但 Brain（推理节点）无法水平扩展，RPC client 为 placeholder。

**竞品借鉴**:
- **Managed Agents**: Brain 无状态水平扩展 — Session 外部化，任意 Brain 实例可接管任意 Session → `REF-managed-agents.md`
- **AReaL**: Master-Worker 异步分发架构 — Coordinator 集中调度，Worker 无状态执行 → `areal/infra/controller/train_controller.py`
- **OpenClaw-RL**: Ray Actor 模式 — 独立生命周期管理 + 故障自动恢复 → `slime/slime/ray/rollout.py:44-48`

```rust
// crates/cyberclaw-control-plane/src/distributed.rs

/// 节点角色
/// 借鉴: Managed Agents Brain/Hands/Sandbox 分离
pub enum NodeRole {
    /// 推理节点 — 运行 AgenticLoop，无状态
    /// 借鉴: Managed Agents Brain — 无状态，可水平扩展
    Brain,
    /// 执行节点 — 运行 Connector，可隔离
    /// 借鉴: Managed Agents Hands — 按需连接工具
    Hands,
    /// 协调节点 — 运行 Raft 共识 + 任务调度
    Coordinator,
}

/// 无状态 Brain 设计
/// 借鉴: Managed Agents — Session 外部化到 SessionStore
/// Brain 节点不持有任何会话状态，所有状态读写通过 StateStore
/// 任意 Brain 实例崩溃后，Coordinator 将 Session 重新分配给其他 Brain
pub struct StatelessBrain {
    node_id: NodeId,
    /// 所有状态通过 StateStore 读写（§5 存储持久化）
    store: Arc<dyn StateStore>,
    /// AgenticLoop 实例池
    loop_pool: AgenticLoopPool,
}

/// 任务分配器
/// 借鉴: Managed Agents — Coordinator 按 Brain 负载分配 Session
/// 策略: least-loaded（当前活跃 Session 数最少的 Brain 优先）
pub struct TaskDistributor {
    /// 节点健康状态
    node_registry: Arc<NodeRegistry>,
    /// 分配策略
    strategy: DistributionStrategy,
}

pub enum DistributionStrategy {
    /// 贪心负载均衡（借鉴 AReaL）
    /// 将任务分配给当前负载最低的节点
    BalancedGreedy,
    /// 亲和性分配
    /// 相同 Agent 的任务优先分配给同一节点（缓存亲和）
    AffinityBased { affinity_key: String },
    /// 资源感知（借鉴 OpenClaw-RL Placement Group）
    /// 根据任务资源需求和节点可用资源匹配
    ResourceAware,
}

impl TaskDistributor {
    /// 分配执行任务到 Brain 节点
    pub async fn distribute(&self, request: ExecutionRequest) -> Result<NodeAssignment> {
        // 1. 获取所有健康的 Brain 节点
        let nodes = self.node_registry.healthy_brains().await?;
        // 2. 按策略选择目标节点
        let target = match &self.strategy {
            DistributionStrategy::BalancedGreedy => {
                // 选择当前活跃 Session 数最少的节点
                nodes.iter().min_by_key(|n| n.active_sessions).unwrap()
            }
            DistributionStrategy::AffinityBased { affinity_key } => {
                // 优先选择已有该 Agent 缓存的节点
                nodes.iter()
                    .find(|n| n.cached_agents.contains(affinity_key))
                    .unwrap_or(&nodes[0])
            }
            DistributionStrategy::ResourceAware => {
                // 根据任务预估资源需求匹配
                nodes.iter()
                    .filter(|n| n.available_memory > request.estimated_memory)
                    .min_by_key(|n| n.active_sessions)
                    .unwrap()
            }
        };
        // 3. 通过集群 RPC 发送任务（复用现有 Raft RPC 通道）
        Ok(NodeAssignment { node_id: target.id.clone(), rpc_addr: target.rpc_addr.clone() })
    }
}

/// 故障恢复
/// 借鉴: Managed Agents 故障恢复三步法 (wake/getSession/emitEvent)
pub struct FailoverHandler {
    store: Arc<dyn SessionStore>,
    distributor: Arc<TaskDistributor>,
}

impl FailoverHandler {
    /// 节点故障时自动迁移 Session
    pub async fn handle_node_failure(&self, failed_node: &NodeId) -> Result<()> {
        // 1. 从 SessionStore 获取该节点所有活跃 Session
        // 2. 标记为 Recovering 状态
        // 3. 重新分配到健康节点
        // 4. 新节点通过 get_events_since(checkpoint) 恢复执行上下文
        // 5. 借鉴 Managed Agents — 从最后一个检查点继续，不丢失进度
        todo!()
    }
}
```

### 9.6 RL 训练接入

**设计原则**: CyberClaw 不是训练框架。RL 训练能力通过 Connector → Capability 链路对接外部训练框架（OpenClaw-RL / AReaL 等）。CyberClaw 的价值是提供训练信号（Execution Trace）和受控的权重部署流程。

**竞品参考**: OpenClaw-RL 异步 4 组件架构、AReaL Actor-Learner 分离

```rust
// crates/cyberclaw-connectors/src/rl_training.rs

/// RL 训练 Connector — 通过 Connector → Capability 接入外部训练框架
///
/// 注册为 Capability:
///   "rl.export_traces"    (is_read_only: true)    — 导出 Execution Trace 作为训练数据
///   "rl.deploy_weights"   (is_destructive: true)  — 部署新权重（需 PolicyEngine 审批）

#[async_trait]
pub trait RlTrainingConnector: Connector {
    /// 导出 Execution Trace 为训练框架可消费的格式
    /// 复用 AuditStore 中的执行历史，不额外收集
    async fn export_traces(&self, filter: &TraceFilter) -> Result<Vec<serde_json::Value>>;

    /// 部署新权重（必须经过 PolicyEngine 审批流程）
    async fn deploy_weights(&self, checkpoint_url: &str) -> Result<()>;
}

/// 训练信号类型系统、奖励函数设计、PRM 评估等
/// 均由外部训练框架负责，CyberClaw 只做数据导出和受控部署
```

---

## 10. Skill Runtime 精简（P1 — Sprint 3）

### 10.1 当前问题

`MinimalSkillRuntime::invoke()` 直接执行 Skill，违反 "Skill 不直接拥有平台执行权限" 和 "底层执行一律走 Connector → Capability" 两条约束。

### 10.2 竞品验证

14 个竞品中 Skill 的定位（全部为 Agent 子资源或上下文注入，无独立运行时）:

| 竞品 | Skill 定位 | 执行方式 |
|------|-----------|---------|
| Claude Code | SKILL.md 文件 | 注入 Agent 上下文，通过 Agent 的工具循环执行 |
| IronClaw | SkillSet（工具集合） | 附属于 Agent，通过 LoopDelegate 执行 |
| AutoResearch | Markdown-as-Skill | 结构化 prompt template，Agent 加载后执行 |
| Hermes Agent | 自学习 Skill | Agent 从经验中提取，存储为可复用模板 |
| OpenClaw | Provider 注册 | 通过 Plugin SDK 注册，Gateway 调度执行 |
| Codex | Skill 文件 | Agent 启动时加载，注入系统提示 |

### 10.3 设计方案

```
保留:
  - UnifiedSkillLoader (三格式加载: ClaudeCode/Codex/OpenClaw)
  - HotReloadWatcher (文件系统监听热重载)
  - SkillId / SkillSpec / SkillFormat 类型定义
  - Registry 中 Skill 的独立注册和发现

删除:
  - MinimalSkillRuntime::invoke() (独立执行能力)

新增:
  - SkillBinder: 见 §2.8
```

---

## 11. Plugin Runtime 机制

### 11.1 竞品验证

| 竞品 | Plugin/扩展机制 | 是否使用动态库 |
|------|---------------|--------------|
| Claude Code | Hook 系统（5 种类型） | 否 — Shell 命令 + JS 函数 |
| IronClaw | WASM Component Model | 否 — WASM 沙箱 |
| OpenClaw | 声明式 Plugin SDK | 否 — npm 包 |
| DeerFlow | 中间件管道 | 否 — Python 装饰器 |
| Hermes Agent | 内置插件 | 否 — Python 类 |

**结论: 14 个竞品零采用动态库加载。**

### 11.2 设计方案

Plugin 运行时采用 **WASM Component Model + Hook 注册** 双轨机制：

```
保留:
  - PlatformPluginId / PluginManifest 类型定义
  - ecosystem/platform-plugins/ 目录结构
  - manifest.yaml 格式（hooks 字段）
  - Plugin 作为五对象之一的生态地位

删除:
  - libloading 动态库加载（替换为 WASM）
  - SandboxManager 逻辑沙箱（替换为 WASM 沙箱）

Plugin 运行时:
  - PluginHookLoader: 从 manifest 自动注册 Hook（见 §6.3.4）
  - WASM Component Model 沙箱执行
    借鉴: IronClaw WIT 接口 + Fuel metering + Epoch 中断
    (wasmtime 作为运行时，WIT 定义 Plugin 接口，Fuel 控制执行配额)
  - 内置 Plugin 实现:
    1. audit-enricher: 执行完成后丰富审计记录
    2. metrics-exporter: Prometheus/OTEL 指标导出
    3. secret-scanner: 输入/输出敏感信息扫描
```

---

## 12. 实施路线

### 12.1 Sprint 规划

| Sprint | 周期 | 优先级 | 目标 | 核心交付 | 主要借鉴竞品 |
|--------|------|--------|------|---------|-------------|
| **S1** | W1-W2 | P0 | Agent 推理循环 | AgenticLoop trait + InteractiveDelegate + Skill 绑定 + Memory 集成 + Chat API 端到端闭环 | IronClaw, Claude Code, Hermes |
| **S2** | W3-W4 | P0 | 存储持久化 | StateStore sub-trait 拆分 + SqliteStateStore + PostgresStateStore 激活验证 + 迁移系统 | IronClaw, gbrain, OpenCode |
| **S3** | W5-W6 | P1 | 治理 + Skill/Plugin 精简 | PolicyRule 规则引擎 + Hook 集成 + TenantBoundary 集成 + 凭据代理 + Skill invoke 移除 + Plugin 机制切换 | Claude Code, OpenCode, NanoClaw, DeerFlow |
| **S4** | W7-W8 | P1 | 上下文管理 + 可观测性 | ContextCompressor 四阶段 + L0/L1/L2 分级摘要 + OTEL 导出 + 分布式 Trace 传播 + Autopilot 对接 AgenticLoop | Hermes, OpenViking, Claude Code |
| **S5** | W9-W10 | P2 | 平台接入 | Contract-first 派生 + 检索 Connector + Webhook 网关 + Workflow 触发器 | gbrain, cc-connect, Hermes |
| **S6** | W11-W13 | P2 | 分布式扩展 | 多节点 Brain 水平扩展 + 故障迁移 + RL 训练 Connector + 自学习治理 | Managed Agents, AReaL, OpenClaw-RL |

### 12.2 关键路径

```
S1 (AgenticLoop) ──→ S4 (上下文管理 + 可观测性) ──→ S6 (分布式扩展)
                          ↑                              ↑
S2 (存储持久化) ──→ S3 (治理升级) ──→ S5 (平台接入) ──┘
                                         可并行
```

### 12.3 验收标准

| Sprint | 验收标准 |
|--------|---------|
| S1 | Agent 通过 Chat API 接收自然语言 → 自主调 LLM → 选择工具 → 走 PolicyEngine 门禁 → Connector 执行 → 返回结果。Skill prompt 注入生效。Memory 跨请求可读写。 |
| S2 | Server 重启后 Execution/Artifact/Audit 数据不丢失。SQLite 和 PostgreSQL 双后端通过相同测试集。迁移系统可执行升级。 |
| S3 | PolicyRule 支持 glob 匹配 + allow/deny/ask 三态。Plugin hooks 从 manifest 自动加载。凭据代理对 Connector 透明。TenantBoundary 在 Orchestrator 中生效。 |
| S4 | 长任务自动触发上下文压缩（四阶段）。Memory 支持 L0/L1/L2 分级。Prometheus /metrics 端点暴露。分布式 Trace 跨节点传播。Autopilot Execute 步骤调用 AgenticLoop。 |
| S5 | Capability 定义一处定义自动派生 LLM/MCP 格式。检索 Connector 对接至少一种向量数据库。Webhook 网关接收外部消息并路由到 Agent。Workflow 触发器支持 OnExecutionComplete + OnWebhook + Cron。 |
| S6 | Brain 节点无状态水平扩展，故障自动迁移 Session。自学习治理引擎生成策略建议（受元治理规则约束）。RL 训练 Connector 可导出 Execution Trace。 |

---

## 13. 竞品借鉴全景索引

### 13.1 按竞品分类

| 竞品 | 借鉴设计 | 应用章节 |
|------|---------|---------|
| **IronClaw** | LoopDelegate trait 三路径复用 | §2.3 推理循环 |
| | 7 sub-trait DB 抽象 | §5.3 存储 |
| | LLM Provider 装饰器链 | §2.5 LLM 适配 |
| | Skill 信任分级（Installed/Verified/Trusted） | §6.3.1 规则引擎 |
| | 瞬态/非瞬态错误分类 | §2.5 LLM 适配, §9.2 Connector 错误分类 |
| | WASM Component Model | §11.2 Plugin 沙箱执行 |
| | AES-256-GCM + OS Keychain | §6.3.3 凭据隔离 |
| **Claude Code** | 10 层安全纵深 | §6 治理升级 |
| | Hook 系统（5 种类型） | §6.3.4 Hook |
| | allow/deny/ask 三态规则 | §6.3.1 规则引擎 |
| | autoCompact 阈值计算 | §3.3 上下文管理 |
| | 连续失败熔断(3次) | §3.3 压缩熔断 |
| | StreamEvent 流式处理 | §2.6 流式响应 |
| | 工具自描述属性 | §6.3.2 Capability 增强 |
| | Skill 上下文注入 | §2.8 Skill 绑定 |
| **Hermes Agent** | 四阶段压缩算法 | §3.3 上下文管理 |
| | IterationBudget（90 次） | §2.4 预算控制 |
| | 模型降级链 | §2.5 LLM 适配 |
| | MemoryProvider ABC（7 种） | §4.3 Memory |
| | Smart Approval | §6.3.5 审批 |
| | 并行工具安全检查 | §2.7 工具管道 |
| | 自学习 Skill 创建 | §2.8 Skill, §6.4 自学习治理 |
| | 8 种消息网关 | §9.4 多通道接入 |
| **OpenViking** | L0/L1/L2 分级摘要 | §3.3 上下文管理 |
| | 8 类记忆分类 | §4.3 Memory |
| | 记忆去重管道 | §4.3 去重 |
| | Account/User/Agent 三级租户 | §6.3.1 条件 |
| | OpenTelemetry + Prometheus | §7.3 可观测性 |
| | 信封加密 + 多 KMS Provider | §6.3.3 凭据 |
| | VLM Provider 多模态理解 | §2.10 多模态 Agent |
| | AgentPatterns 行为模式积累 | §6.4 自学习治理 |
| **DeerFlow** | 13 层中间件管道 | §6.3.4 Hook, §8 Workflow |
| | GuardrailProvider fail_closed | §6.3.4 Hook |
| | MemoryMiddleware 30s 防抖 | §4.3 异步更新 |
| | 中间件 CI/CD 钩子 | §8.4 CI/CD |
| **NanoClaw** | 凭据代理（容器不接触真实密钥） | §6.3.3 凭据 |
| | 外部安全配置 | §6.3.3 Vault |
| **Cline** | 用户审批集成 | §2.3 InteractiveDelegate |
| | ContextManager 过时内容替换 | §3.3 补充 |
| | 截图 + 视觉反馈循环 | §2.10 多模态 Agent |
| **OpenCode** | Doom Loop 检测 | §2.4 卡住检测 |
| | Wildcard 规则引擎 | §6.3.1 规则 |
| | Prune + Compact 双策略 | §3.3 压缩 |
| | SQLite WAL 最佳实践 | §5.3 SQLite |
| **gbrain** | Contract-first Operation | §9.2 接口派生 |
| | 混合搜索管道（RRF + 4 层去重） | §9.3 RAG 检索 |
| | PostgreSQL Schema 设计 | §5.3 PG |
| **cc-connect** | 接口能力探测（type assertion） | §9.2 能力探测 |
| | Bridge WebSocket 协议 | §9.4 多通道接入 |
| **AutoResearch** | Markdown-as-Skill | §2.8 Skill |
| **OpenClaw** | 多通道入口 | §9.4 多通道接入 |
| | Plugin SDK 声明式注册 | §11 Plugin |
| **Managed Agents** | Session/Harness/Sandbox 解耦 | 整体架构验证 |
| | 凭据隔离架构 | §6.3.3 凭据 |
| | append-only Session Log | §5.3 AuditStore |
| | 故障恢复三步法 | §5.3 SessionStore |
| | Brain 无状态水平扩展 | §9.5 分布式多节点 |
| | "interfaces outlast implementations" 原则 | §1.3 核心约束 |
| **Trustworthy Agents** | always-allowed/approval/blocked | §6.3.1 规则 |
| | Plan Mode 审批去疲劳 | §6.3.5 审批 |
| **OpenClaw-RL** | 异步 RL 训练循环 | §9.6 RL 训练接入 |
| | PRM 多票投票 | §9.6 RL 训练接入 |
| | Proxy 透明数据采集 | §9.6 训练信号收集 |
| | Combined Advantage 多信号融合 | §9.6 奖励信号 |
| **AReaL** | Master-Worker 异步分发 | §9.5 分布式多节点 |

### 13.2 按设计领域分类

| 设计领域 | Top 3 借鉴竞品 | 覆盖章节 |
|---------|---------------|---------|
| 推理循环 | IronClaw → Claude Code → Hermes | §2 |
| 上下文管理 | Hermes → Claude Code → OpenViking | §3 |
| Memory 系统 | OpenViking → Hermes → DeerFlow | §4 |
| 持久化存储 | IronClaw → gbrain → OpenCode | §5 |
| 治理安全 | Claude Code → IronClaw → NanoClaw | §6 |
| 可观测性 | OpenViking → Claude Code → DeerFlow | §7 |
| Workflow | DeerFlow → Hermes → OpenCode | §8 |
| 平台接入 | cc-connect → gbrain → OpenClaw | §9.2 |
| RAG 检索 | gbrain → OpenViking → Hermes | §9.3 |
| 多通道接入 | cc-connect → Hermes → OpenClaw | §9.4 |
| 分布式多节点 | Managed Agents → AReaL → OpenClaw-RL | §9.5 |
| RL 训练 | OpenClaw-RL → AReaL → Hermes | §9.6 |
| 多模态 | OpenViking → Cline → Claude Code | §2.10 |
| 自学习治理 | Hermes → OpenClaw-RL → OpenViking | §6.4 |
| 分布式 Trace | OpenViking → DeerFlow → Managed Agents | §7.4 |
| CI/CD | DeerFlow → cc-connect → OpenClaw | §8.4 |

---

## 14. 风险矩阵

| 风险 | 可能性 | 影响 | 缓解措施 |
|------|--------|------|---------|
| AgenticLoop 引入后破坏现有执行链 | 中 | 高 | 工具执行仍走 Orchestrator → PolicyEngine，不绕过治理 |
| 移除 Skill invoke 导致回归 | 低 | 低 | Skill Runtime 零测试，无生产依赖 |
| Plugin 机制切换影响现有 manifest | 低 | 低 | manifest 格式不变，只变运行时加载方式 |
| SQLite/PG 持久化后性能回退 | 低 | 中 | InMemory 保留用于测试，生产用连接池 + WAL |
| 治理规则引擎增加审批延迟 | 中 | 中 | 规则匹配优化 + Smart Approval 减少疲劳 |
| control-plane 与 agent-runtime 循环依赖 | 中 | 高 | AgenticLoop 通过 trait 调用 Orchestrator，不直接依赖 |
| 多节点 Brain 扩展引入网络分区风险 | 中 | 高 | Session 外部化 + 故障恢复三步法 + Raft 共识保障一致性 |
| RAG 检索延迟影响推理循环响应时间 | 中 | 中 | 异步检索 + 结果缓存 + 超时降级（跳过检索直接推理） |
| RL 训练权重部署导致 Agent 行为突变 | 低 | 高 | 权重部署必须经过 PolicyEngine 审批 + 灰度发布 |
| 多通道网关安全面扩大 | 中 | 高 | 所有网关消息走完整治理链 + Token constant-time 比较 |

---

## 附录 A: 不在本方案范围的能力

1. 端到端模型训练基础设施（GPU 集群调度、Megatron 并行策略）— 由 RL 训练 Connector 对接外部框架
2. 自有 LLM 推理引擎（SGLang / vLLM）— 通过 Connector 接入现有推理服务

## 附录 B: CLAUDE.md 兼容性检查

| CLAUDE.md 约束 | 本方案是否兼容 | 说明 |
|---------------|---------------|------|
| 不把 Tool 作为一级平台对象 | 兼容 | 未引入 Tool 对象 |
| 不新增第五类生态对象 | 兼容 | 五对象保持不变 |
| 底层执行一律走 Connector → Capability | 兼容 | AgenticLoop 工具执行走 Orchestrator → Connector → Capability |
| Skill 不直接拥有平台执行权限 | 兼容 | 移除了 Skill 独立执行 |
| Platform Plugin 不绕过治理、审计和追踪 | 兼容 | Hook + WASM 沙箱在 Orchestrator 管道中执行 |
| 优先复用现有对象模型 | 兼容 | 五对象不变，运行时全面升级 |
