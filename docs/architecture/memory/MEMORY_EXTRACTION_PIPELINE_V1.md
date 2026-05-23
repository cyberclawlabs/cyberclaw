# CyberClaw Memory Extraction Pipeline v1

## 1. 目标

本文档定义 CyberClaw 如何从执行过程、Artifact、Review 和外部知识结果中提炼长期记忆。

目标：

1. 明确 memory extraction 的输入、输出与边界
2. 防止把低质量 transcript 直接写成长久记忆
3. 将 compaction 和 memory write 分离
4. 为 `cyberclaw-memory` 的 `MemoryExtractor` 提供实现路线

---

## 2. 正式原则

> Memory extraction 不是“把历史总结一下”，而是“把经过筛选的信息提炼成可治理的 memory card”。

因此，提炼 pipeline 必须满足：

1. 输入可追溯
2. 输出结构化
3. 支持 review / confidence / ttl
4. 可区分 draft 与 reviewed memory

---

## 3. 输入来源

memory extraction 的输入只应来自高价值对象：

1. `ExecutionSummary`
2. `CaseSummary`
3. `ArtifactSummary`
4. `ReviewDecision`
5. `SecurityEventSummary`
6. `Knowledge Retrieval Summary`

不建议直接使用：

1. 全量 transcript
2. 临时工具输出
3. 未筛选的模型自由思考
4. 未 externalize 的大文本原文

---

## 4. Pipeline 分层

## 4.1 Stage 1: Candidate Selection

先从候选源中选出可能值得记忆的对象。

筛选规则：

1. 是否包含稳定事实
2. 是否包含长期约束
3. 是否包含可复用结论
4. 是否包含高价值审计信息
5. 是否有明确来源对象

输出：

- `MemoryCandidate[]`

## 4.2 Stage 2: Normalization

将不同来源标准化为统一提取输入。

统一输入建议：

```rust
pub struct MemoryExtractionInput {
    pub scope: MemoryScope,
    pub source_kind: MemorySourceKind,
    pub source_refs: Vec<MemorySourceRef>,
    pub summary_text: Option<String>,
    pub structured_payload: Option<serde_json::Value>,
    pub suggested_tags: Vec<String>,
}
```

## 4.3 Stage 3: Structured Extraction

从标准化输入中提炼 memory draft。

输出目标：

- `MemoryCardDraft[]`

每条 draft 至少包含：

- `scope`
- `kind`
- `summary`
- `content`
- `source_refs`
- `confidence`
- `suggested_ttl`

## 4.4 Stage 4: Validation and Policy Gate

提炼结果不能直接无条件写入主库。

要经过：

1. schema 校验
2. source_refs 非空校验
3. low-value 过滤
4. conflict 检测
5. 高风险结论是否需要 review

输出：

- `accepted draft`
- `rejected draft`
- `review-required draft`

## 4.5 Stage 5: Upsert

通过后的 draft 再写入 `MemoryStore`。

Upsert 要求：

1. 支持同 scope + 同 kind + 同主题合并
2. 不覆盖更高置信度 reviewed memory
3. 保留历史 source refs

---

## 5. 候选来源的具体规则

## 5.1 ExecutionSummary -> Episodic / Conclusion

适合提炼：

- 决策理由
- 阶段结论
- 失败原因
- 稳定观察

不适合提炼：

- 临时命令输出
- 大段工具日志

## 5.2 CaseSummary -> Conclusion / Constraint

适合提炼：

- case 最终结论
- 长期处置策略
- 确认后的风险判断

## 5.3 ArtifactSummary -> Fact / Conclusion

适合提炼：

- 报告中的关键 finding
- 文档比对的稳定结论
- 分析结果的高层摘要

注意：

- 不把整个 artifact 正文复制进 memory

## 5.4 ReviewDecision -> Constraint / Reviewed Conclusion

适合提炼：

- 审批明确确认的结论
- 审批形成的新约束
- 人工纠偏意见

这是高价值来源，因为它天然增强记忆可信度。

## 5.5 Knowledge Retrieval Summary -> Fact / Note

适合提炼：

- 可引用事实
- 经过交叉验证的文档结论
- 长文档知识摘要

不适合提炼：

- 外部检索的原始全文
- 未验证的检索回答

---

## 6. 推荐提炼规则

## 6.1 默认提炼的 kind

第一阶段建议只提炼：

1. `Fact`
2. `Constraint`
3. `Conclusion`
4. `EpisodicSummary`

## 6.2 不建议自动提炼的 kind

默认不自动提炼：

1. `Preference`
2. `Profile`
3. `ProceduralNote`

原因：

- 这些更容易误判
- 更适合人工确认或专门入口写入

---

## 7. Draft 与 Reviewed 的边界

### 7.1 自动提炼默认状态

建议：

- 默认写成 `Draft` 或 `Active`
- 不直接写成 `Reviewed`

### 7.2 什么时候可以升级为 `Reviewed`

1. 来自明确 `ReviewDecision`
2. 来自人工确认的 `CaseSummary`
3. 来自规则可信度极高的系统输入

---

## 8. TTL 与过期建议

建议按 kind 设置默认 TTL：

| kind | 默认 TTL |
|---|---|
| `Fact` | 30d |
| `Constraint` | 90d |
| `Conclusion` | 30d |
| `EpisodicSummary` | 14d |
| `Reviewed Conclusion` | 90d |

注意：

- TTL 不是硬删除时间，而是再验证触发条件

---

## 9. 冲突处理建议

当新提炼结果与旧 memory card 冲突时：

1. 若旧卡是 `Reviewed`，新卡只能作为 draft 并标记 conflict
2. 若旧卡置信度更高，不直接覆盖
3. 若新卡来源更新且更可信，可生成 supersede 候选

建议增加：

```rust
pub enum MemoryConflictResolution {
    KeepExisting,
    CreateNewDraft,
    SupersedeExisting,
    RequireReview,
}
```

---

## 10. 推荐运行时流程

### 10.1 Execution Close Hook

1. execution 结束
2. 生成 execution summary
3. 提取 episodic / conclusion draft
4. 写入 memory store

### 10.2 Case Close Hook

1. case 结束或进入稳定阶段
2. 生成 case summary
3. 提取长期 conclusion / constraint
4. 必要时进入 review

### 10.3 Background Memory Worker

后台周期执行：

1. revalidation
2. stale marking
3. archive
4. conflict review queue

---

## 11. 最终建议

正式结论：

> CyberClaw 的 memory extraction 应采用“候选筛选 -> 标准化 -> 结构化提炼 -> 校验/策略门禁 -> Upsert”的五段式 pipeline。

这样才能保证：

1. 记忆不是随手生成的摘要垃圾
2. 记忆对象始终有 provenance
3. 高价值结论能沉淀
4. 低价值临时上下文不会污染长期记忆
