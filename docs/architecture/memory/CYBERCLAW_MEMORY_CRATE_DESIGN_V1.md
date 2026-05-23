# cyberclaw-memory Crate 设计草案 v1

## 1. 目标

本文档定义 `cyberclaw-memory` crate 的职责边界、核心类型、存储接口、压缩流程和与平台其它模块的协作关系。

目标不是把所有知识系统塞进一个 crate，而是为 CyberClaw 提供一个**轻量、稳定、可治理、可审计**的 Memory Core。

`cyberclaw-memory` 的职责是：

1. 管理长期与短期记忆边界
2. 管理结构化记忆条目
3. 承接阶段摘要与记忆提炼
4. 对接 Artifact / Provenance / Session / Execution
5. 为上下文装配提供可控记忆读取接口

不负责：

1. 外部知识检索本身
2. 向量数据库托管
3. 图谱系统实现
4. Prompt 运行时本身
5. Review / Policy 决策本身

---

## 2. 正式定位

`cyberclaw-memory` 在整体架构中的位置是：

- 属于平台内核 crate
- 是 `Memory / Compaction Service` 的主要实现位置
- 与 `cyberclaw-store`、`cyberclaw-observability`、`cyberclaw-control-plane` 协作

正式边界：

- `Memory` 负责沉淀和提炼
- `Artifact` 负责保存大结果和原始产物
- `Provenance` 负责血缘关系
- `Connector` 负责外部知识获取

一句话：

> `cyberclaw-memory` 负责“记忆对象”，不负责“所有信息”。

---

## 3. 设计原则

1. **结构化优先**
   记忆优先以结构化 card 表示，而不是整段自由文本。

2. **Artifact-first**
   大内容优先进入 Artifact，memory 只保留摘要和引用。

3. **分层记忆**
   Working / Episodic / Semantic / Procedural 各自有边界。

4. **可审计**
   每条长期记忆必须带来源和更新时间。

5. **可失效**
   记忆必须支持 TTL、再验证和清理。

6. **轻量实现**
   第一阶段不引入重型依赖，不要求内建向量库或图数据库。

---

## 4. 记忆分层在 crate 中的落位

## 4.1 Working Memory

不建议由 `cyberclaw-memory` 主存。

Working Memory 主要归属：

- `Session`
- `Execution`
- `ExecutionTree`
- runtime context builder

`cyberclaw-memory` 对其只提供：

- 裁剪规则
- 摘要提炼接口

## 4.2 Episodic Memory

建议由 `cyberclaw-memory` 与 `cyberclaw-store` 协作管理。

数据来源：

- execution 事件
- artifact 元数据
- 阶段总结
- review 结论
- security event

输出形态：

- execution summary
- case summary
- episodic memory card

## 4.3 Semantic Memory

这是 `cyberclaw-memory` 的主存对象。

数据来源：

- 被确认的事实
- 提炼后的结论
- 用户 / 团队偏好
- 项目约束
- reviewed conclusion

输出形态：

- structured memory card

## 4.4 Procedural Memory

不建议由 `cyberclaw-memory` 自己存正文。

应由：

- `Skill`
- `Policy`
- 文件规则系统

来承载。

`cyberclaw-memory` 只保存：

- procedural refs
- procedural note
- 适用范围索引

---

## 5. 核心对象模型

## 5.1 `MemoryCard`

建议作为核心长期记忆对象：

```rust
pub struct MemoryCard {
    pub id: MemoryCardId,
    pub scope: MemoryScope,
    pub kind: MemoryKind,
    pub status: MemoryStatus,
    pub content: serde_json::Value,
    pub summary: Option<String>,
    pub tags: Vec<String>,
    pub confidence: Option<f32>,
    pub ttl: Option<chrono::Duration>,
    pub source_refs: Vec<MemorySourceRef>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub reviewed_at: Option<chrono::DateTime<chrono::Utc>>,
}
```

## 5.2 `MemoryScope`

```rust
pub enum MemoryScope {
    User { actor_id: ActorId },
    Project { workspace_id: WorkspaceId },
    Case { case_id: CaseId },
    Tenant { tenant_id: TenantId },
    Global,
}
```

说明：

- 默认避免 `Global`
- `Case` 和 `Project` 是第一阶段最重要的 scope

## 5.3 `MemoryKind`

```rust
pub enum MemoryKind {
    Fact,
    Preference,
    Constraint,
    Conclusion,
    EpisodicSummary,
    ProceduralNote,
    Profile,
}
```

## 5.4 `MemoryStatus`

```rust
pub enum MemoryStatus {
    Draft,
    Active,
    Reviewed,
    Stale,
    Archived,
    Rejected,
}
```

## 5.5 `MemorySourceRef`

```rust
pub struct MemorySourceRef {
    pub execution_id: Option<ExecutionId>,
    pub artifact_id: Option<ArtifactId>,
    pub case_id: Option<CaseId>,
    pub review_id: Option<ReviewId>,
    pub connector_id: Option<ConnectorId>,
    pub capability_id: Option<CapabilityId>,
}
```

这保证记忆始终可回链。

---

## 6. 关键接口设计

## 6.1 `MemoryStore`

```rust
#[async_trait]
pub trait MemoryStore: Send + Sync {
    async fn upsert(&self, card: MemoryCard) -> anyhow::Result<()>;
    async fn get(&self, id: &MemoryCardId) -> anyhow::Result<Option<MemoryCard>>;
    async fn list_by_scope(&self, scope: &MemoryScope) -> anyhow::Result<Vec<MemoryCard>>;
    async fn search(&self, query: MemoryQuery) -> anyhow::Result<Vec<MemoryCard>>;
    async fn mark_stale(&self, id: &MemoryCardId) -> anyhow::Result<()>;
    async fn archive(&self, id: &MemoryCardId) -> anyhow::Result<()>;
    async fn delete_expired(&self, now: chrono::DateTime<chrono::Utc>) -> anyhow::Result<u64>;
}
```

第一阶段建议提供：

- `InMemoryMemoryStore`
- `FileBackedMemoryStore` 或基于 `cyberclaw-store` 的轻量持久层

## 6.2 `MemoryExtractor`

```rust
#[async_trait]
pub trait MemoryExtractor: Send + Sync {
    async fn extract(
        &self,
        input: MemoryExtractionInput,
    ) -> anyhow::Result<Vec<MemoryCardDraft>>;
}
```

职责：

- 从 execution summary / artifact summary / review notes 中提炼 memory draft

## 6.3 `CompactionService`

```rust
#[async_trait]
pub trait CompactionService: Send + Sync {
    async fn compact_working_context(
        &self,
        input: WorkingContextCompactionInput,
    ) -> anyhow::Result<CompactionResult>;

    async fn summarize_execution(
        &self,
        execution_id: &ExecutionId,
    ) -> anyhow::Result<ExecutionSummaryRecord>;

    async fn summarize_case(
        &self,
        case_id: &CaseId,
    ) -> anyhow::Result<CaseSummaryRecord>;

    async fn extract_memory(
        &self,
        input: MemoryExtractionInput,
    ) -> anyhow::Result<Vec<MemoryCard>>;
}
```

---

## 7. 推荐模块划分

建议目录：

```text
crates/cyberclaw-memory/
├── src/
│   ├── lib.rs
│   ├── types.rs
│   ├── store.rs
│   ├── extractor.rs
│   ├── compaction.rs
│   ├── episodic.rs
│   ├── semantic.rs
│   ├── procedural.rs
│   ├── retention.rs
│   └── tests.rs
```

### `types.rs`

- `MemoryCard`
- `MemoryScope`
- `MemoryKind`
- `MemoryStatus`
- `MemoryQuery`
- `MemorySourceRef`

### `store.rs`

- `MemoryStore`
- `InMemoryMemoryStore`
- 可选 file/store-backed 实现

### `extractor.rs`

- 记忆提炼 trait
- 从 Artifact / Summary 提取 memory draft

### `compaction.rs`

- 工作上下文裁剪
- execution/case 摘要接口
- Artifact externalization 协调接口

### `episodic.rs`

- execution summary / case summary 结构
- episodic memory card 构建

### `semantic.rs`

- semantic memory card 的增量合并策略
- 冲突更新策略

### `procedural.rs`

- procedural refs / policy refs 管理
- 不存正文，只存引用和适用范围

### `retention.rs`

- TTL
- stale detection
- archive / cleanup
- revalidation 入口

---

## 8. 与其它 crate 的协作关系

## 8.1 `cyberclaw-control-plane`

提供：

- `Task`
- `Case`
- `Execution`
- `Review`

使用：

- 在 execution 结束后触发 summary / extraction
- 在 case 阶段收敛时触发 case summary

## 8.2 `cyberclaw-observability`

提供：

- `EventRecorder`
- provenance / trace

使用：

- 为 memory card 附加 source refs
- 为 compaction 过程写 audit 事件

## 8.3 `cyberclaw-store`

负责：

- 低层持久化

`cyberclaw-memory` 负责：

- memory 语义和策略

## 8.4 `cyberclaw-connectors`

作用：

- 外部知识检索

边界：

- connector 结果进入 Artifact
- memory 只接收提炼后的 summary / fact / conclusion

---

## 9. 推荐的运行流程

### 9.1 Execution 结束后

1. Execution 写入最终状态
2. 生成 execution summary
3. 大结果已 Artifact 化
4. 调用 MemoryExtractor 提炼 memory draft
5. 通过规则或 review 决定是否写入 MemoryStore

### 9.2 Case 阶段完成后

1. 汇总 case 下关键 execution
2. 生成 case summary
3. 提炼长期结论型 memory
4. 标记低价值 episodic 条目为 stale 或 archive

### 9.3 Session 构建上下文时

1. 读取 working context
2. 查询 case / workspace 对应 semantic memory
3. 查询适用 procedural refs
4. 读取必要 artifact summary
5. 组合为可控上下文，而不是把所有 memory 全塞进去

---

## 10. 第一阶段不建议做的内容

1. 内建向量索引
2. 内建图谱引擎
3. 复杂 memory ranking model
4. 跨租户全局共享记忆
5. 自动改写 procedural memory 正文
6. 无审计的自我学习写入

这些都偏重，不适合第一阶段。

---

## 11. 最终建议

`cyberclaw-memory` 第一阶段的最佳定位是：

> 轻量结构化 Memory Core + Compaction Coordinator

它的价值不在于“成为一个很大的知识系统”，而在于：

1. 给 CyberClaw 提供稳定的长期记忆对象模型
2. 把压缩和提炼流程收敛到统一入口
3. 让 Artifact、Execution、Review、Provenance 和 Memory 形成闭环

一句话定稿：

> `cyberclaw-memory` 应该负责“把平台运行中产生的信息沉淀成可治理的记忆对象”，而不是负责承载所有知识系统本身。

---

## 12. 下一步建议

建议后续继续补三份文档：

1. `MEMORY_CARD_SCHEMA_V1.md`
2. `MEMORY_EXTRACTION_PIPELINE_V1.md`
3. `MEMORY_RUNTIME_APIS_V1.md`
