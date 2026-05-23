# CyberClaw 上下文工程与长期记忆架构方案 v1

## 0. Beta 执行口径（2026-03-21）

为避免“逻辑模型”和“运行时实现”混淆，当前版本补充如下执行口径：

1. 本文 5 层结构用于逻辑建模与长期规划。
2. Beta 运行时仅采用 3 层热路径：`Working + Episodic + Procedural`。
3. `Semantic Memory` 延后到 Post-Beta（P2）。
4. `Knowledge Retrieval` 统一作为 `Connector` 能力接入，不纳入 Memory Core。
5. 性能数字以基准测试为准，估算值仅用于方案讨论。

规范决议见：
- `docs/architecture/memory/MEMORY_ARCH_REVIEW_DECISION_V1.md`
- `docs/architecture/retrieval/LETTA_ZEP_PAGEINDEX_CONNECTOR_STRATEGY_V1.md`

---

## 1. 结论

CyberClaw 最适合的长期记忆方案，不是单一产品或单一存储技术，而是一套**分层的上下文工程架构**：

1. `Working Memory`
2. `Episodic Memory`
3. `Semantic Memory`
4. `Procedural Memory`
5. `Knowledge Retrieval`

正式建议：

> CyberClaw 应采用“Execution / Artifact 驱动的记忆骨架 + 结构化语义记忆主库 + 文件化程序性记忆 + 外部知识检索连接器 + 自动上下文压缩”的分层方案。

这比纯 `RAG`、纯 `MEMORY.md`、纯向量库、纯图谱都更适合 CyberClaw。

---

## 2. 为什么 CyberClaw 不能采用单一记忆方案

CyberClaw 是受控执行平台，不是单一对话机器人。

平台记忆必须同时服务于：

1. 持续任务执行
2. 多 Agent / Subagent 协作
3. 审批与治理
4. 可观察与审计
5. Artifact 血缘与证据链
6. 多租户和 Workspace 边界
7. 长期知识沉淀

因此，长期记忆不能只回答“记住什么”，还必须回答：

- 这条信息来自哪里
- 它归属哪个 Case / Session / Workspace / Tenant
- 是否经过 review
- 是否还能继续信任
- 何时应该失效、压缩或外部化

这决定了 CyberClaw 的 Memory 不能简化成：

- 一个长 `MEMORY.md`
- 一个向量库
- 一个图数据库
- 一份无限扩展的会话摘要

---

## 3. CyberClaw 最适合的记忆分层

## 3.1 `Working Memory`

定义：

- 当前 Session / Thread / Execution 的热上下文

主要内容：

- 当前任务目标
- 当前 plan
- 最近工具结果
- 当前阶段状态
- 子任务 handoff 信息
- 当前审批状态

要求：

- 严格有大小上限
- 面向当前执行，不负责长期保存
- 先删过时工具结果，再删低价值上下文
- 默认不直接进入长期 Memory 主库

在 CyberClaw 中的推荐归属：

- `Session`
- `Execution`
- `ExecutionTree`
- `Workspace` 上下文缓存

这是平台的“热内存层”。

---

## 3.2 `Episodic Memory`

定义：

- 记录“发生过什么”的执行型记忆

主要内容：

- 关键执行事件
- 失败原因
- 决策理由
- 审批结论
- 子代理结果摘要
- 关键人工干预
- 阶段总结

要求：

- 以事件和 Artifact 为主，不以自由文本为主
- 必须保留 provenance
- 摘要可以压缩，但原始事件不能丢失
- 必须能回溯到 `Execution / Case / Artifact`

在 CyberClaw 中的推荐归属：

- `Execution`
- `Artifact`
- `Provenance`
- `SecurityEvent`
- `Review`

这是 CyberClaw 的**主记忆骨架**。

对于受控平台，episodic memory 比“用户偏好记忆”更重要。

---

## 3.3 `Semantic Memory`

定义：

- 记录稳定事实、偏好、长期约束和已确认结论

主要内容：

- 用户偏好
- 团队约束
- 项目常识
- 环境事实
- 已确认结论
- 稳定实体属性
- Case 级长期研判结论

要求：

- 结构化存储，不能只是自然语言堆叠
- 每条记忆必须带：
  - `scope`
  - `source_refs`
  - `confidence`
  - `updated_at`
  - `ttl`
  - `review_status`
- 允许增量更新
- 允许过期和再验证

推荐数据形态：

```yaml
id: mem_xxx
scope: project | case | tenant | user
kind: fact | preference | constraint | conclusion
content: {}
tags: []
confidence: 0.92
ttl: 30d
source_refs: []
updated_at: 2026-03-20T00:00:00Z
review_status: reviewed | unreviewed
```

这是 CyberClaw 长期记忆主库的核心形态。

---

## 3.4 `Procedural Memory`

定义：

- 记录“怎么做”的程序性规则和方法记忆

主要内容：

- Agent 行为边界
- Skill 方法论
- 安全规则
- 开发规范
- 审批规则
- Runbook
- 平台操作约束

要求：

- 优先文件化
- 人类可读
- 易 review
- 与平台运行时边界清晰

推荐载体：

- `SKILL.md`
- `CLAUDE.md` 风格项目规则文件
- `POLICY.md`
- `RUNBOOK.md`
- Agent manifest 中的角色规则

在 CyberClaw 中，procedural memory 不应混入 semantic memory。

它本质上属于：

- `Skill`
- `Policy`
- `Platform governance docs`

---

## 3.5 `Knowledge Retrieval`

定义：

- 面向外部知识源的检索能力层

典型来源：

- PageIndex
- 向量库
- GraphRAG / 图谱
- 文档库
- 工单系统
- CMDB
- Wiki / Ticket / SOP

原则：

- `Knowledge != Memory`
- 检索结果可以沉淀为 Artifact 和 Memory Summary
- 外部知识系统不能替代 CyberClaw 的 Memory Core

在 CyberClaw 中的推荐归属：

- `Connector`
- `Capability`

---

## 4. 为什么这是 CyberClaw 最合适的方案

### 4.1 与平台对象模型一致

CyberClaw 已有核心对象：

- `Task`
- `Case`
- `Session`
- `Workspace`
- `Execution`
- `Artifact`
- `Review`
- `Provenance`

这些对象天然适合承载 episodic memory 和执行型上下文。

因此，CyberClaw 不需要从外部引入一个“全能记忆系统”覆盖内核。

### 4.2 与治理模型一致

长期记忆在 CyberClaw 中必须支持：

- 信任边界
- 审批边界
- 租户边界
- 来源边界
- 失效和清理策略

只有结构化 memory + provenance 才能支撑这一点。

### 4.3 与业务场景一致

CyberClaw 的典型业务场景包括：

- `R&D`
- `DevOps`
- `Security`
- `GRC`
- `Audit`

这些场景最需要的是：

- 记住发生过什么
- 记住结论依据是什么
- 记住当前策略和规范是什么
- 记住哪些外部知识可被引用

而不是“无边界聊天记忆”。

---

## 5. 推荐的内核职责划分

## 5.1 Memory Core 应保留在 CyberClaw 内核

推荐由平台内核负责：

1. `SessionStore`
2. `ExecutionStore`
3. `ArtifactStore`
4. `ProvenanceStore`
5. `StructuredMemoryStore`

建议对应 crate：

- `cyberclaw-memory`
- `cyberclaw-store`
- `cyberclaw-observability`

### 5.2 外部知识引擎只作为 Connector

外部知识系统应通过 Connector 进入平台，例如：

- `PageIndexConnector`
- `VectorRetrievalConnector`
- `GraphConnector`

这些连接器只提供检索能力，不直接成为 Memory Core。

---

## 6. 业务场景映射

## 6.1 R&D / DevOps

最适合的记忆组合：

1. `Procedural Memory`
2. `Project Semantic Memory`
3. `Execution / Artifact Memory`
4. `Prompt / Prefix Cache-friendly Layout`

不建议默认上图谱。

## 6.2 SecOps / Incident Response

最适合的记忆组合：

1. `Episodic Memory`
2. `Case Memory`
3. `Artifact / Evidence Provenance`
4. `Structured Semantic Memory`
5. `Optional Graph Memory`

这里最重要的不是“聊天连续性”，而是：

- 哪次执行得出了什么结论
- 哪个证据支持哪个判断
- 哪个审批改变了执行路径

## 6.3 GRC / Audit / Compliance

最适合的记忆组合：

1. `Procedural Memory`
2. `Citation-preserving Semantic Memory`
3. `Document Knowledge Retrieval`
4. `Immutable Audit Trail`

不能把 lossy 摘要当作权威来源。

## 6.4 个人助手 / 团队助手

最适合的记忆组合：

1. `Profile Semantic Memory`
2. `Session Continuity`
3. `Procedural Memory`
4. `Lightweight Retrieval`

这时才更接近聊天产品的长期记忆模型。

---

## 7. 不建议采用的单一方案

### 7.1 纯 `MEMORY.md` 膨胀方案

问题：

- 适合原型
- 不适合平台
- 缺乏结构化治理
- 不利于过期和审计

### 7.2 纯向量库长期记忆

问题：

- 适合召回
- 不适合承载平台长期事实与 provenance
- 可解释性不足

### 7.3 纯图谱长期记忆

问题：

- 太重
- 维护成本高
- 第一阶段收益不匹配成本

### 7.4 只保留摘要，不保留原始事件

问题：

- 不可审计
- 不可回放
- 不适合受控平台

### 7.5 知识库直接替代记忆库

问题：

- 知识和记忆职责不同
- 外部文档检索不能替代执行态记忆

---

## 8. 最终建议

CyberClaw 的长期记忆主线应当是：

1. 以 `Execution / Artifact / Provenance` 作为骨架
2. 以 `Structured Semantic Memory` 作为长期主库
3. 以 `Procedural File Memory` 作为规则层
4. 以 `Connector` 形式接入外部知识检索
5. 以自动 compaction 维持上下文窗口稳定

一句话定稿：

> CyberClaw 最适合采用“执行驱动 + 结构化长期记忆 + 文件化程序性记忆 + 外部检索连接器”的上下文工程架构，而不是单一 memory 产品方案。

---

## 9. 参考来源

评估日期：`2026-03-20`

参考资料：

1. [Anthropic Claude Code Memory](https://code.claude.com/docs/en/memory)
2. [Anthropic Memory Tool](https://platform.claude.com/docs/en/agents-and-tools/tool-use/memory-tool)
3. [Anthropic Context Management](https://claude.com/blog/context-management)
4. [Anthropic Multi-Agent Research System](https://www.anthropic.com/engineering/multi-agent-research-system)
5. [OpenAI Prompt Caching Docs](https://developers.openai.com/api/docs/guides/prompt-caching)
6. [OpenAI API Prompt Caching](https://openai.com/index/api-prompt-caching/)
7. [OpenAI GPT-5.2-Codex](https://openai.com/index/introducing-gpt-5-2-codex/)
8. [LangGraph Memory Overview](https://docs.langchain.com/oss/javascript/langgraph/memory)
9. [LangGraph Long-Term Memory](https://blog.langchain.com/launching-long-term-memory-support-in-langgraph/)
10. [MemGPT](https://arxiv.org/abs/2310.08560)
11. [Zep](https://arxiv.org/abs/2501.13956)
12. [MemInsight](https://arxiv.org/abs/2503.21760)
13. [CoALA](https://arxiv.org/abs/2309.02427)
