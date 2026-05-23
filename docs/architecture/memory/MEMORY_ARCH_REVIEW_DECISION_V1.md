# CyberClaw 记忆架构复核决议 v1

- Status: Active
- Scope: Architecture
- Owner: CyberClaw Maintainers
- Last Updated: 2026-03-21

## 1. 决议结论

本决议基于当前仓库实现与评审材料交叉复核后定稿：

1. Beta 运行时采用三层记忆热路径：`Working + Episodic + Procedural`
2. `Semantic Memory` 延后至 Post-Beta（P2）实现
3. `Knowledge Retrieval` 不作为 Memory Core 层，统一通过 `Connector` 接入
4. Letta / Zep / PageIndex 均按 Connector 集成，不进入 Memory Core

一句话定稿：

> CyberClaw 当前采用“5 层逻辑模型 + 3 层运行时实现”的策略，先保性能与可交付，再逐步增强语义层能力。

---

## 2. 证据化复核结果

## 2.1 已确认事实

1. 记忆 V1 架构文档定义为 5 层（逻辑模型）
2. 仓库中不存在 `crates/cyberclaw-memory`（语义层尚未落地）
3. `ProvenanceRecord`、`SecurityEventStore`、`SensitiveString`、`SecretsManager` 已实现
4. Connector-only 策略已定稿（Letta / Zep / PageIndex）

## 2.2 需谨慎使用的结论

评审文档中的性能数字（如 350-1100ms、46s）当前属于估算，不可作为最终性能事实。  
项目后续必须以基准测试结果作为唯一验收依据。

## 2.3 当前实现缺口

虽然 `ProvenanceRecord` 和 `SecurityEventStore` 已存在，但这并不等于完整的 episodic memory 已完成。

当前仍缺：

1. 面向上下文构建的 `EpisodicContextProjection`
2. `SecurityEvent` 的一等时间语义
3. `SecurityEvent` 的一等主体语义
4. Beta 可用的 `WorkingMemory` 与热路径 compaction 实现

---

## 3. 统一口径（面向开发团队）

## 3.1 逻辑模型

逻辑上保留 5 类概念：

1. Working
2. Episodic
3. Semantic
4. Procedural
5. Knowledge Retrieval

## 3.2 运行时实现（Beta）

Beta 仅实现 3 层热路径：

1. Working（会话热上下文）
2. Episodic（执行历史与审计主干）
3. Procedural（规则/方法文件）

延后项：

1. Semantic（Post-Beta P2）
2. Knowledge Retrieval（通过 Connector 接入，不纳入 Memory Core）

---

## 4. 性能与验收门禁

在未形成基准数据前，不允许将估算数字写入对外结论。  
以下门禁用于后续实现验收：

1. `memory_context_query p95 < 50ms`
2. `sync_compaction p95 < 100ms`
3. `100K executions memory footprint < 150MB`
4. 关键路径压缩不得依赖同步 LLM 调用

---

## 5. 开发约束

1. 不新增“第六类”记忆对象
2. 不把 Connector 能力混入 Memory Core
3. 不在请求热路径引入阻塞型 LLM 压缩
4. 不以“层数完整”替代“治理可控 + 可观测 + 可交付”

## 5.1 当前优先补齐项

1. 将 `MEMORY_RUNTIME_APIS_V1.md` 收敛为 Beta 三层接口
2. 将 `MEMORY_COMPACTION_STRATEGY_V1.md` 收敛为 Beta 两段式热路径
3. 后续代码实现中补齐 `EpisodicContextProjection`
4. 后续代码实现中补齐 `SecurityEvent.timestamp / actor`

---

## 6. 文档关系

本决议与以下文档共同生效：

1. `docs/architecture/memory/CONTEXT_ENGINEERING_MEMORY_ARCHITECTURE_V1.md`
2. `docs/architecture/retrieval/LETTA_ZEP_PAGEINDEX_CONNECTOR_STRATEGY_V1.md`
3. `tmp/claw-research/MEMORY_ARCHITECTURE_EVALUATION_REPORT.md`（评审输入，不作为最终规范）
