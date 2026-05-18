---
name: daily-digest
description: 让 Agent 具备"灵魂+日常记录+反思"的方法论 — 每日从 Execution/Artifact 聚合摘要，反思并提炼可复用的 Procedural Memory 规则（CyberClaw 适配版）
source: hermes-agent-self-evolution (Reflection + Daily Digest cycles)
adapted-for: CyberClaw (Sprint 8, 2026-04-19)
status: scaffold
level: 3
---

<!--
CyberClaw adaptation notes:

- 本文件是 **方法论文档 scaffold**（Skill 的本体，不是 Rust 代码）。
  真正的 Daily Digest 循环实现由 L6 lane 负责，落在
  `crates/cyberclaw-agent-runtime/` 或 `crates/cyberclaw-workflow/`。
  本 skill 描述 **行为契约**，让 L6 和后续迭代有一个权威的参考。
- Daily Digest 不是一个 Skill 本体要执行的动作 —— CyberClaw 的 Skill
  不承担平台执行权限（见 `AGENTS.md §9` 与 `CLAUDE.md §2 / §9`）。
  这里把 Daily Digest 登记成 Skill 是为了：
    1. 让 Agent 在规划阶段能把 "每日反思" 作为一个可引用的方法论。
    2. 让 SkillHub 扫描时把它识别为一个完整的 methodology bundle。
    3. 让下游 WorkflowTrigger::Cron 调度器能按名称引用。
- `status: scaffold` 告诉 SkillRuntime 这是 **方法论骨架**，不要尝试
  把它绑定到任何可执行 SkillHandler。真正的调度通过
  `crates/cyberclaw-workflow/` 的 `WorkflowTrigger::Cron` 注册一个
  定时任务，内部用 Execution/Trace/Artifact 查询聚合，不经过此 SKILL.md。
- 与 hermes-agent-self-evolution 上游的差异：
    1. 上游 Reflection 和 Daily Digest 是 Python 脚本 + 独立 runtime。
       CyberClaw 把等价行为内化为 WorkflowTrigger::Cron + Execution 查询 +
       Semantic Memory 写入，不跑外部进程。
    2. 上游 GEPA 反射式进化（reflection → rule extraction → policy update）
       在 CyberClaw 下拆为两段：
         - Daily Digest 产出 **只写 Semantic Memory**（描述性记忆）
         - Policy/Rule 更新走 EvolutionOrchestrator（已在 Sprint 5-6 落地）
       这条边界避免了 digest 直接修改 governance 规则，保留治理权威。
    3. 数据源严格限定为 **Execution / Artifact / Trace / ProgressJournal**
       这四个已经存在的骨架对象，不引新的 "per-agent daily log" 独立表。
    4. 语言改为中文优先、英文兼容（与 `ecosystem/skills/brainstorm/SKILL.md` 对齐）。
- 本 skill 本身 **不** 写代码、不 spawn 子进程、不调用任何外部 HTTP。
-->

# Daily Digest — 让 Agent 每天自己总结、自己反思

给每个 Agent 一份"每日简报"：把当天的执行结果、工件、失败、轨迹，压缩成一份短摘要，
并从中提炼出 **可被下一次复用的经验规则**。这是 Agent "灵魂"的底层表达 —— 一个 Agent
有没有性格、有没有成长、是不是持续在学习，不靠 prompt 吹，靠它每天能不能从自己的
历史里总结出东西。

## 为什么是一个 Skill 而不是一段 Rust 代码

- **Skill 是方法论**：Daily Digest 的 "怎么写好一份 digest"、"怎么判断一条经验是否值得落库"、
  "什么内容必须进 digest、什么可以丢"，这些是 **方法论决策**，不是运行时逻辑。
- **Rust 代码只管调度和数据聚合**：L6 lane 的 Rust 实现只负责
    * 定时触发（WorkflowTrigger::Cron）
    * 拉取 Execution / Artifact / Trace / ProgressJournal
    * 把聚合结果塞给 Agent
    * 把 Agent 产出的 digest 写回 Semantic Memory
  真正"怎么写 digest"由 Agent 参考本 Skill 完成。

## 五阶段流程

### Stage 1：Collect — 采集原始事实
从四个已存在的骨架对象里拉当天的记录：

| 数据源 | 关键字段 | 查询位置 |
| --- | --- | --- |
| `Execution` | status / started_at / completed_at / execution_mode | `cyberclaw-store` 按 agent_id + date 范围 |
| `Artifact` | kind / size / provenance | 同 Execution 关联 |
| `Trace` | event_type / severity / tags | `cyberclaw-observability` |
| `ProgressJournal` | iteration / verdict | `crates/cyberclaw-control-plane/src/persistent_execution.rs` |

**硬约束**：只读，不修改任何源表。Collect 阶段失败也不能阻塞次日 digest。

### Stage 2：Summarize — 事实压缩
把原始事实压成一份 **三段式** 摘要：

```
## 今日发生了什么（事实层）
- 完成执行 N 条，失败 M 条
- 新增 Artifact K 个（按 kind 分布：{code: a, report: b, trace: c}）
- 触发过 {Autopilot, Persistent, Normal} 三种模式各 X / Y / Z 次

## 今日卡在哪（问题层）
- 最频繁的失败信号：<top-3 error categories from Trace>
- 最长的执行：execution_id=... 耗时 ...
- 反复触发的审批：<top-3 approval categories>

## 今日学到什么（经验层）← 这一段才进 Stage 3
- …（空即空，不编）
```

### Stage 3：Reflect — 反思与因果归因
对"今日学到什么"做三问：

1. **为什么这件事成了 / 没成？** —— 不要"运气好"、"环境问题"这种廉价归因
2. **换一个上下文会不会复现？** —— 是单点事件还是可泛化模式
3. **如果没有这条经验，下次会重犯吗？** —— 值得沉淀的阈值

只有三问全过的条目才进 Stage 4。

### Stage 4：Extract Rules — 提炼 Procedural Memory 规则
把通过反思的经验转成 **可检索的短规则**（每条 ≤ 100 字符），例如：

- `在 RiskLevel Critical 的 Connector 前，必须先跑 ToolPermissionMatcher 预演`
- `Autopilot 连续失败 3 次后切 Persistent，优于切 Normal`
- `fs.write 大文件（>1MB）要分片，否则 Artifact 存储爆表`

规则格式与 `docs/architecture/memory/README.md` 里 Procedural Memory 的 schema 对齐。

### Stage 5：Persist — 写回 Semantic Memory
把 Summarize 的三段式摘要 + Extract Rules 的规则列表，
作为一条 `SemanticMemoryEntry` 写入：

```rust
SemanticMemoryEntry {
    scope: MemoryScope::Agent(agent_id),
    kind: MemoryKind::DailyDigest,
    content: MarkdownBody(...),
    rules: Vec<ProceduralRule>,
    created_at: chrono::Utc::now(),
    ttl: None,  // 永久
}
```

**硬约束**：
- Semantic Memory 写入不能修改任何 governance 规则（`DangerousCapabilityFilter` / `ToolPermissionMatcher`）
- 如果 digest 反思出 "需要新增一条 filter 规则"，应走 EvolutionOrchestrator 提案流程，不能直接落库

## 调度约定

- 默认时区：UTC 日界
- 默认触发：每日 00:10 UTC（给日界事件留 10 分钟缓冲）
- 每个 Agent 一份 digest，不做跨 Agent 聚合
- 若当天无任何 Execution，跳过，不生成空 digest

## 失败模式

| 场景 | 处理 |
| --- | --- |
| Collect 阶段查询超时 | 跳过今天，记一条 Trace warning，次日重试时**不补齐**过去日期 |
| Agent 自己拒绝写 digest | 写一条占位条目 `{status: refused, reason: ...}`，保留空位 |
| Extract Rules 产出 > 10 条 | 截断到 Top 10（按反思强度排序），其余打到 Trace debug |
| Semantic Memory 写入失败 | 落入 `cyberclaw-store` dead-letter，不影响调度继续 |

## 与其他 Skill 的关系

- **brainstorm**：brainstorm 在 "设计前" 使用；daily-digest 在 "执行后" 使用。两头管着 Agent 的日常循环。
- **skill-creator**：digest 里提炼出的规则如果足够稳定（连续 N 天重复），
  可以作为 skill-creator 的输入种子，进入 EvolutionOrchestrator 流程，
  产出新 Skill 或新 policy 变体。
- **plan**：plan 消费 brainstorm 的设计，digest 消费 plan 执行后的结果，形成闭环。

## 反模式（HARD-AVOID）

- **编造事实**：Collect 阶段拿不到的数据，不要用 LLM 补全"大概发生了什么"
- **廉价归因**：把失败归到 "环境问题" / "网络抖动" / "运气不好"，不走三问
- **规则膨胀**：Extract Rules 一天出 20 条 —— 说明没有经过反思筛选
- **跨 Agent 偷看**：Agent A 的 digest 不能读 Agent B 的 Execution（隔离边界）
- **改 governance 规则**：digest 只写 Semantic Memory，不写 policy

## 可追踪性

每条 daily digest 在 Semantic Memory 里带：

- `source_executions: Vec<ExecutionId>` — 可反查原始事实
- `source_artifacts: Vec<ArtifactId>` — 工件指针
- `reflection_trace_id: TraceId` — 反思阶段的完整 Trace，审计用

这样未来的 Agent 或 reviewer 可以从一条规则反溯到它是哪天从哪些 Execution 里提炼出来的，
保证"学习过程"本身也是可审计的。

## 实施指引（给 L6 lane 参考）

> **本段不是方法论的一部分**，是给后续 Rust 实现者的 checklist。

- [ ] 在 `crates/cyberclaw-workflow/src/triggers/` 下注册 Cron trigger
- [ ] 在 `crates/cyberclaw-agent-runtime/` 下加 `DailyDigestCoordinator`
- [ ] 复用现有 `cyberclaw-store::SemanticMemory` API，不要新开表
- [ ] 复用现有 Execution / Artifact / Trace / ProgressJournal 查询接口
- [ ] 对每个 agent_id 单独调度，使用 agent 的 workspace（WorkspaceRef）做隔离
- [ ] 把本 SKILL.md 作为系统提示注入 Agent 的 digest 请求，保证方法论一致性
- [ ] 测试覆盖：空日 / 单条 Execution / 多 Execution / 反思被拒 / Semantic 写入失败

## 参考

- 上游：`tmp/claw-research/hermes-agent-self-evolution/` 相关 Reflection 与 Daily Digest 设计
- 上游：`tmp/claw-research/hermes-agent/` 的长期记忆与反思循环
- CyberClaw：`docs/architecture/memory/README.md` — Semantic Memory schema
- CyberClaw：`crates/cyberclaw-control-plane/src/persistent_execution.rs` — ProgressJournal
- CyberClaw：`ecosystem/skills/brainstorm/SKILL.md` — 设计前方法论，与本 skill 对偶
- CyberClaw：`ecosystem/skills/skill-creator/SKILL.md` — 规则沉淀后的进化入口
