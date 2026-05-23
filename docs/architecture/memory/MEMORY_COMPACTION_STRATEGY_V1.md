# CyberClaw 记忆压缩与上下文收敛策略 v1

## 0. Beta 执行口径（2026-03-21）

本文档保留完整压缩梯度作为长期规划，但当前执行口径调整为：

1. Beta 热路径只启用两段式压缩：
   - 确定性裁剪
   - Artifact 外部化
2. `阶段摘要` 与 `结构化记忆提炼` 不进入同步请求链路
3. 涉及 LLM 的压缩必须作为后台任务或 Post-Beta 增强能力

---

## 1. 结论

对 CyberClaw 而言，压缩不是“可选优化”，而是长期执行、多 Agent 协作和可控成本的基础设施。

正式建议：

> CyberClaw 应采用“Beta 热路径两段式压缩 + 后台四段式增强”的策略，而不是把完整四段式压缩直接放进同步执行链路。

---

## 2. 为什么压缩是必需的

CyberClaw 面向的不是一次性聊天，而是：

1. 长时执行
2. 多阶段任务
3. 多 Agent / Subagent 协作
4. 工具调用和大结果返回
5. 审批与回流
6. 证据和 Artifact 累积

如果不做压缩，必然出现：

- 上下文窗口膨胀
- 成本不可控
- 决策噪音增大
- Agent 重复读取历史
- 子代理结果淹没核心目标
- 审批与治理信息被稀释

因此，压缩目标不只是“更短”，而是：

1. 保留关键决策
2. 保留来源和引用
3. 丢弃低价值瞬时上下文
4. 把大内容从 prompt 移出到 Artifact
5. 形成后续可检索、可审计的 memory 结构

---

## 3. CyberClaw 最适合的压缩顺序

说明：

1. 以下 4 层是完整压缩阶梯
2. 其中第 1、2 层属于 Beta 热路径
3. 第 3、4 层属于后台增强或 Post-Beta 能力

## 3.1 第一层：确定性裁剪

先删什么：

1. 过时工具结果
2. 已消费的中间观察
3. 重复输出
4. 可重建的临时上下文
5. 已结束阶段的低价值消息

这一步不依赖 LLM，总是应该优先做。

原则：

- 优先删临时结果
- 不删决策理由
- 不删 review / security / provenance 关键点
- 不删仍被 plan 引用的内容

这是最便宜、最稳定的一层压缩。

---

## 3.2 第二层：Artifact 外部化

这是 CyberClaw 最关键的一层。

原则：

- 大文本、大 JSON、大 diff、大报告，不应长期留在主上下文
- 应写入 `ArtifactStore`
- 主上下文只保留：
  - `artifact_ref`
  - 简要摘要
  - provenance

典型外部化对象：

1. 大型搜索结果
2. 代码扫描结果
3. 安全分析报告
4. 文档检索结果全文
5. 子代理完整产出
6. 对比分析长表格

建议上下文中只保留：

```yaml
artifact_id: art_xxx
kind: report
summary: >
  对 3 个告警样本完成聚类和 root cause 分析，结论为同一配置缺陷。
source_execution: exec_xxx
```

---

## 3.3 第三层：阶段摘要

状态：

- 不进入 Beta 同步热路径
- 仅建议作为后台任务或阶段结束后的异步任务

压缩单位不应是“整个会话”，而应是“阶段”。

推荐摘要层级：

1. `turn summary`
2. `phase summary`
3. `execution summary`
4. `case summary`

每一级摘要都必须保留：

- `source_refs`
- `artifact_refs`
- `execution_refs`
- `timestamp`
- `confidence`

要求：

- 摘要是增量的，不是每次重写全历史
- 摘要必须能回链原始 evidence
- 摘要不能替代原始 provenance

---

## 3.4 第四层：结构化记忆提炼

状态：

- 不进入 Beta 同步热路径
- 与 `Semantic Memory` 一起放入 Post-Beta

当一个阶段结束后，不是把摘要直接塞回 prompt，而是要从中提炼 memory card。

推荐提炼出的类型：

1. `fact`
2. `constraint`
3. `preference`
4. `conclusion`
5. `playbook-note`

建议结构：

```yaml
id: mem_xxx
kind: conclusion
scope: case
content:
  finding: 某资产段存在统一弱口令配置问题
  impact: 高
confidence: 0.91
source_refs:
  - exec_001
  - art_002
review_status: reviewed
```

这样压缩的结果就进入长期 memory，而不是变成另一段不可管理的自由文本。

---

## 4. 最适合 CyberClaw 的压缩策略

## 4.1 Artifact-first，而不是 transcript-first

不推荐：

- 先积累大量 transcript
- 再让模型总结 transcript

推荐：

- 子代理产出直接写 Artifact
- 主代理只拿摘要和引用
- transcript 只保留关键协商信息

原因：

- 降低主上下文体积
- 减少重复 token 消耗
- 更适合审计和溯源
- 更符合多 Agent 平台架构

---

## 4.2 结构化压缩优先于自由摘要

自由摘要容易丢掉：

- 条件
- 引用
- 否定信息
- 审批上下文
- 安全判断边界

所以 CyberClaw 应优先压缩成：

1. `decision record`
2. `artifact ref`
3. `memory card`
4. `phase summary`

自由自然语言摘要只能作为附加层，不应作为唯一输出。

---

## 4.3 后台 compaction 优先于热路径 compaction

建议区分：

### 热路径压缩

只做：

1. 删旧工具结果
2. 大结果 externalize
3. 关键状态裁剪

### 后台压缩

做：

1. execution summary
2. case summary
3. memory extraction
4. 过期清理
5. 低置信度记忆再验证

原因：

- 降低主执行延迟
- 避免 Agent 在关键路径上频繁自我总结
- 更适合稳定输出

---

## 4.4 Cache-friendly context layout

压缩不仅服务记忆，也服务成本。

CyberClaw 应使 prompt 结构尽可能缓存友好：

1. 固定的 system prefix
2. 固定的 policy prefix
3. 固定的 tool schema prefix
4. 高频变动内容放在末尾

这样更容易利用：

- OpenAI Prompt Caching
- 其他模型厂商的前缀缓存能力

这属于上下文工程，不是单独的 memory feature，但必须一起设计。

---

## 5. 业务场景下的压缩建议

## 5.1 R&D / DevOps

推荐压缩策略：

1. 删除旧工具输出
2. 将大文件 diff / scan 结果写 Artifact
3. 保留当前变更计划和关键决策
4. 提炼项目级 semantic memory

## 5.2 SecOps / Incident Response

推荐压缩策略：

1. 保留证据链引用
2. 保留审批与人工介入结论
3. 子代理长报告全部 Artifact 化
4. 提炼 case conclusion memory

重点：

- 不能只留摘要不留证据

## 5.3 GRC / Audit

推荐压缩策略：

1. 文档引用必须保留
2. 压缩后仍能定位原始条款和证据
3. 摘要必须标明来源和生效范围
4. 高风险结论进入 reviewed memory

---

## 6. 不建议的压缩方式

### 6.1 整段会话自由总结

问题：

- 丢条件
- 丢引用
- 丢 provenance
- 易发生语义漂移

### 6.2 摘要替代原始记录

问题：

- 不可审计
- 不可验证
- 不可争议处理

### 6.3 无限递归摘要

问题：

- 摘要的摘要会持续损失信息
- 最终只剩“看起来正确”的低保真文本

### 6.4 将所有历史都做 embedding

问题：

- 成本高
- 无法解决治理和 provenance 问题
- 不适合作为唯一压缩路径

---

## 7. CyberClaw 的推荐实现顺序

### Phase 1

1. 实现 `Working Memory` 限长策略
2. 实现 `Artifact externalization`
3. 不在热路径引入 LLM 摘要

### Phase 2

1. 实现 `memory extraction`
2. 实现 `phase summary`
3. 实现 background compaction worker

### Phase 3

1. 实现 structured memory card
2. 实现跨 case compaction
3. 实现 stale memory pruning
4. 实现 memory revalidation

---

## 8. 最终建议

正式结论：

> CyberClaw 最适合采用“热路径两段式（裁剪 -> 外部化）+ 后台四段式（裁剪 -> 外部化 -> 阶段摘要 -> 结构化提炼）”的压缩方案。

这条路线最符合 CyberClaw 的平台属性，因为它同时满足：

1. 可执行
2. 可审计
3. 可追踪
4. 可压缩
5. 可扩展
6. 可与多 Agent 协作结合

一句话定稿：

> 对 CyberClaw，最好的压缩不是“把历史在热路径上总结短一点”，而是“把信息分层迁移到正确的位置，并把昂贵处理移出关键路径”。

---

## 9. 参考来源

评估日期：`2026-03-20`

参考资料：

1. [Anthropic Context Management](https://claude.com/blog/context-management)
2. [Anthropic Claude Code Memory](https://code.claude.com/docs/en/memory)
3. [Anthropic Multi-Agent Research System](https://www.anthropic.com/engineering/multi-agent-research-system)
4. [OpenAI Prompt Caching Docs](https://developers.openai.com/api/docs/guides/prompt-caching)
5. [OpenAI API Prompt Caching](https://openai.com/index/api-prompt-caching/)
6. [LangGraph Memory Overview](https://docs.langchain.com/oss/javascript/langgraph/memory)
7. [LangGraph Long-Term Memory](https://blog.langchain.com/launching-long-term-memory-support-in-langgraph/)
8. [MemGPT](https://arxiv.org/abs/2310.08560)
9. [MemInsight](https://arxiv.org/abs/2503.21760)
10. [CoALA](https://arxiv.org/abs/2309.02427)
