# CyberClaw Memory Runtime APIs v1

- Status: Active
- Scope: Architecture
- Owner: CyberClaw Maintainers
- Last Updated: 2026-03-26

## 0. Beta 口径

本文档已按当前记忆架构复核结论收敛：

1. Beta 运行时只服务三层热路径：`Working + Episodic + Procedural`
2. 第一阶段不再以 `MemoryStore / MemoryWriter / MemoryExtractor` 为正式前置接口
3. `Semantic Memory` 相关接口全部降级为 Post-Beta 预留能力
4. `Knowledge Retrieval` 统一走 `Connector`，不纳入本文件的 Memory Core API
5. 外接检索结果通过独立的 `ExternalRetrievalProvider` 接口拼接，不伪装成 Memory Core 对象

正式决议见：

1. `docs/architecture/memory/MEMORY_ARCH_REVIEW_DECISION_V1.md`
2. `docs/architecture/retrieval/LETTA_ZEP_PAGEINDEX_CONNECTOR_STRATEGY_V1.md`

---

## 1. 目标

本文档定义 CyberClaw 在 Beta 阶段建议暴露的最小记忆运行时接口。

目标：

1. 给 Control Plane / Runtime / Context Builder 一个稳定的三层记忆入口
2. 避免把“审计骨架”误写成“完整长期记忆系统”
3. 避免第一阶段提前固化过重的语义记忆 API

---

## 2. 当前实现边界

已具备：

1. `ProvenanceRecord` 作为执行血缘主干
2. `SecurityEventStore` 作为审计事件存储
3. `Execution / Review / Artifact` 作为上下文来源对象

尚未具备：

1. 独立的 `cyberclaw-memory` crate
2. 面向 Semantic Memory 的正式 `MemoryStore`
3. 完整的 `MemoryCard` 写入与治理流程
4. 可直接供 Agent 读取的 episodic context projection

注意：

1. 当前 `SecurityEvent` 模型尚未包含一等 `timestamp`
2. 当前 `SecurityEvent` 的 actor 查询仍依赖 `details` 内容约定

因此，Beta API 不应对“时间窗口查询”“主体级精确查询”作过度承诺。

---

## 3. Beta 推荐接口

## 3.1 `WorkingMemory`

负责当前执行热上下文的限长缓存与回滚。

```rust
pub trait WorkingMemory: Send + Sync {
    fn push(&mut self, entry: WorkingMemoryEntry);
    fn list_recent(&self, limit: usize) -> Vec<WorkingMemoryEntry>;
    fn checkpoint(&self) -> WorkingMemoryCheckpoint;
    fn rollback(&mut self, checkpoint: &WorkingMemoryCheckpoint);
    fn clear(&mut self);
}
```

职责：

1. 保存最近的任务目标、阶段状态、关键工具结果
2. 严格控制热上下文体积
3. 为出错回滚保留最小恢复点

## 3.2 `EpisodicContextProvider`

负责把执行、审批、审计、Artifact 引用聚合成可读取上下文。

```rust
#[async_trait]
pub trait EpisodicContextProvider: Send + Sync {
    async fn load(
        &self,
        request: EpisodicContextRequest,
    ) -> anyhow::Result<EpisodicContextProjection>;
}
```

职责：

1. 从 `Execution / Provenance / Review / SecurityEvent / Artifact` 聚合记忆视图
2. 输出面向上下文构建的投影，而不是底层原始存储对象
3. 作为 Beta 阶段的 episodic memory 读取主入口

## 3.3 `ProceduralLoader`

负责加载规则、方法和约束类文件。

```rust
#[async_trait]
pub trait ProceduralLoader: Send + Sync {
    async fn load_global_rules(&self) -> anyhow::Result<Vec<ProceduralDocument>>;
    async fn load_case_rules(&self, case_id: &CaseId) -> anyhow::Result<Vec<ProceduralDocument>>;
    async fn load_skill_rules(
        &self,
        skill_ids: &[SkillId],
    ) -> anyhow::Result<Vec<ProceduralDocument>>;
}
```

职责：

1. 读取 `AGENTS.md`、`SKILL.md`、案例规则文件
2. 明确 procedural memory 是文件化规则，不是数据库记忆

## 3.4 `MemoryContextProvider`

负责统一组装 Beta 可用的记忆上下文。

```rust
#[async_trait]
pub trait MemoryContextProvider: Send + Sync {
    async fn build_context(
        &self,
        request: MemoryContextRequest,
    ) -> anyhow::Result<MemoryContextEnvelope>;
}
```

职责：

1. 并行读取 `Working / Episodic / Procedural`
2. 输出供 Agent / Skill / Runtime 使用的最小上下文包
3. 不直接承担语义记忆 CRUD

## 3.4.1 `ExternalRetrievalProvider`

负责在 `MemoryContextProvider` 之后，为 Context Builder 提供外接检索结果。

```rust
#[async_trait]
pub trait ExternalRetrievalProvider: Send + Sync {
    async fn retrieve(
        &self,
        request: ExternalRetrievalRequest,
    ) -> anyhow::Result<ExternalRetrievalEnvelope>;
}
```

职责：

1. 调用 `Connector` 侧的外接检索系统，如 `OpenViking / Zep / PageIndex`
2. 返回可追踪的外接检索结果，而不是底层平台事实对象
3. 与 `MemoryContextProvider` 解耦，避免把外接检索生命周期混入 Memory Core

约束：

1. 失败默认允许回退到 `core-only`
2. 不得阻塞主执行链
3. 结果必须带来源与 trace 元数据

## 3.5 `CompactionService`

负责热路径上下文收敛，但 Beta 只支持确定性裁剪与 Artifact 外部化。

```rust
#[async_trait]
pub trait CompactionService: Send + Sync {
    async fn compact_working_context(
        &self,
        input: WorkingContextCompactionInput,
    ) -> anyhow::Result<CompactionResult>;
}
```

职责：

1. 裁剪低价值上下文
2. 外部化大结果到 Artifact
3. 不在热路径做 LLM 摘要或结构化记忆提炼

---

## 4. Beta 输入输出草案

## 4.1 `WorkingMemoryEntry`

```rust
pub struct WorkingMemoryEntry {
    pub execution_id: Option<ExecutionId>,
    pub kind: WorkingEntryKind,
    pub summary: String,
    pub artifact_refs: Vec<ArtifactRef>,
    pub trace_id: Option<TraceId>,
}
```

## 4.2 `EpisodicContextRequest`

```rust
pub struct EpisodicContextRequest {
    pub case_id: Option<CaseId>,
    pub execution_id: Option<ExecutionId>,
    pub trace_id: Option<TraceId>,
    pub max_events: usize,
}
```

## 4.3 `EpisodicContextProjection`

```rust
pub struct EpisodicContextProjection {
    pub executions: Vec<Execution>,
    pub review_records: Vec<ReviewRecordRef>,
    pub provenance_records: Vec<ProvenanceRecord>,
    pub security_events: Vec<SecurityEvent>,
    pub artifact_refs: Vec<ArtifactRef>,
    pub summary_notes: Vec<String>,
}
```

说明：

1. 这里的 projection 是“读取模型”
2. 它不等于底层存储 schema
3. 它的目标是供上下文组装使用

## 4.4 `WorkingContextCompactionInput`

```rust
pub struct WorkingContextCompactionInput {
    pub session: Option<SessionRef>,
    pub execution_id: Option<ExecutionId>,
    pub messages: Vec<ContextMessage>,
    pub artifact_refs: Vec<ArtifactRef>,
    pub token_budget: usize,
}
```

## 4.5 `CompactionResult`

```rust
pub struct CompactionResult {
    pub kept_messages: Vec<ContextMessage>,
    pub dropped_message_ids: Vec<String>,
    pub externalized_artifacts: Vec<ArtifactRef>,
    pub compaction_notes: Vec<String>,
}
```

## 4.6 `MemoryContextRequest`

```rust
pub struct MemoryContextRequest {
    pub session: Option<SessionRef>,
    pub workspace: Option<WorkspaceRef>,
    pub case_id: Option<CaseId>,
    pub execution_id: Option<ExecutionId>,
    pub actor: ActorRef,
    pub max_items: usize,
}
```

## 4.7 `MemoryContextEnvelope`

```rust
pub struct MemoryContextEnvelope {
    pub working_items: Vec<WorkingMemoryEntry>,
    pub episodic: EpisodicContextProjection,
    pub procedural_docs: Vec<ProceduralDocument>,
    pub policy_notes: Vec<String>,
}
```

说明：

1. `MemoryContextEnvelope` 只代表 Memory Core 结果
2. 外接检索结果不直接塞进该结构，以免混淆内核事实和外部上下文

## 4.8 `ExternalRetrievalRequest`

```rust
pub struct ExternalRetrievalRequest {
    pub session: Option<SessionRef>,
    pub workspace: Option<WorkspaceRef>,
    pub case_id: Option<CaseId>,
    pub execution_id: Option<ExecutionId>,
    pub actor: ActorRef,
    pub query_hints: Vec<String>,
    pub max_items: usize,
    pub timeout_ms: u64,
}
```

## 4.9 `ExternalRetrievalItem`

```rust
pub struct ExternalRetrievalItem {
    pub connector_id: ConnectorId,
    pub capability_id: CapabilityId,
    pub title: String,
    pub summary: String,
    pub source_uri: String,
    pub source_refs: Vec<String>,
    pub trace_id: Option<TraceId>,
}
```

## 4.10 `ExternalRetrievalEnvelope`

```rust
pub struct ExternalRetrievalEnvelope {
    pub items: Vec<ExternalRetrievalItem>,
    pub provider_notes: Vec<String>,
    pub degraded: bool,
}
```

---

## 5. Beta 推荐调用关系

## 5.1 Agent 构建上下文时

由 Agent Runtime / Context Builder 调：

1. `WorkingMemory::list_recent`
2. `EpisodicContextProvider::load`
3. `ProceduralLoader::*`
4. `MemoryContextProvider::build_context`
5. `ExternalRetrievalProvider::retrieve`（按策略可选调用）
6. `Context Builder` 合并 `MemoryContextEnvelope + ExternalRetrievalEnvelope`

## 5.2 热路径上下文超限时

由 Control Plane / Runtime 调：

1. `CompactionService::compact_working_context`

---

## 6. Post-Beta 预留接口

以下能力保留到 Post-Beta，再单独设计和实现：

1. `MemoryStore`
2. `MemoryExtractor`
3. `MemoryWriter`
4. `MemoryCard`
5. `Semantic Memory` 的全文 / 结构化检索

这些能力属于增强层，不是 Beta 的最小前置条件。

---

## 7. 第一阶段不建议暴露的 API

1. 向量检索 API
2. 图谱查询 API
3. 跨租户全局 memory merge API
4. 自动修改 procedural file 的 API
5. 大而全的“memory super service”接口
6. 把 `ProvenanceRecord` 直接当作 Agent 读取接口的 API

原因：

1. 会把接口做胖
2. 会破坏边界
3. 会把审计模型和读取模型混在一起
4. 不利于按需演进

---

## 8. 最终建议

Beta 阶段推荐正式暴露 5 类接口，但它们属于三层热路径而非五层长期记忆系统：

1. `WorkingMemory`
2. `EpisodicContextProvider`
3. `ProceduralLoader`
4. `MemoryContextProvider`
5. `CompactionService`

一句话定稿：

> CyberClaw 的 Beta memory runtime API 应围绕“热缓存、事件投影、规则加载、上下文组装、热路径压缩”五个动作展开，而不是过早固化完整长期语义记忆系统。
