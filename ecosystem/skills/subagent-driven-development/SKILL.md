---
name: subagent-driven-development
description: 并行分派多 Agent 的任务分解方法论 — 任务拆解 → 文件地盘分配（无重叠） → 各 lane 独立接收标准 → 并行执行 → 聚合验收。触发词：dispatch parallel、多 agent 并行、并行分派、swarm、subagent swarm。
source: superpowers/skills/subagent-driven-development/SKILL.md
adapted-for: CyberClaw (Sprint 11/12 wave, 2026-04-22)
level: 3
---

<!--
CyberClaw adaptation notes:

- 本文件是 **方法论文档**（Skill 的本体）。CyberClaw 的 Skill 不直接派遣
  子 Agent；真正的派遣走 `Connector -> Capability`（参考 `CLAUDE.md §2 / §9`）。
- SubAgentOrchestrator 限制：
    * 深度 ≤ 3（不能无限套娃）
    * 每层最多 5 个子 Agent
    * 预算分数 0.5（子 Agent 用掉 50% 的 parent token budget）
    * 源码：`crates/cyberclaw-agent-runtime/src/sub_agent.rs`
- 与 `team` skill 和 `ultrawork` 的关系：
    * `team` 是通用协调层（支持交互式调度）
    * `ultrawork` 是高吞吐并行引擎
    * `subagent-driven-development` 是设计方法论（task 拆解 + lane 边界 + 聚合验收）
-->

# Subagent-Driven Development (SDD)

用并行的 Agent swarm 打一个大任务。

## 核心思想

一个大任务 → 拆成 N 个独立的小任务 → 分配给 N 个 subagent（lane）→
每个 lane 独立接收 acceptance criteria → 并行跑 → 都通过了就 Done。

**适用场景：**
- 任务可以清晰地拆解成不相交的部分（不用频繁合并、不用互相等待）
- 对 execution 速度有要求（希望并行快于串行）
- 有多条 Agent lane 可用（CyberClaw 支持通过 `SubAgentOrchestrator` 并行派遣）

**不适用场景：**
- 子任务高度耦合（A 做完 B 才能做）
- 任务太小或太简单（拆解成本 > 收益）
- 需要频繁合并反馈（并行的意义丧失）

## 流程

### Phase 1: Task Decomposition — 任务拆解

问清楚：

1. **大任务是什么？** 写一份 1-2 句话的需求。
2. **有哪些 **独立的** 子任务？** 关键词是"独立"。
   - 代码示例：功能 A、功能 B、功能 C 可以各自写，只要接口对好，就是独立
   - 文档示例：架构文档、API 文档、用户指南，可以各自写，不互相阻塞
3. **子任务之间的依赖是什么？** 如果有循环依赖或强串行依赖，这个任务不适合 SDD
4. **有多少条 lane？** 通常 2-5 条；太多了反而增加协调成本

**示例拆解：**

```
大任务："实现一个电商平台的支付模块"

子任务拆解：
  Lane A: "Stripe 集成 + 交易日志"
  Lane B: "支付验证 + 错误处理"
  Lane C: "退款流程 + 审计日志"
  Lane D: "前端支付表单 + 错误提示"

依赖分析：
  Lane A 和 B 必须定义好接口（交易事件格式）
  Lane C 依赖 A 和 B（需要交易对象）
  Lane D 依赖 A、B、C（前端调用的 API）

结论：Lane A 和 B 先做（可并行），再做 C（可快速因为只依赖 A/B 的接口），
      最后 Lane D（依赖都齐了）。可以改成两波派遣，或者接受 Lane D 会被阻塞。
```

### Phase 2: File Territory Allocation — 文件地盘分配

**关键原则：没有两个 lane 能修改同一个文件。**

理由：并行工作时如果两条 lane 都改同一个文件，合并会很麻烦、容易冲突。

**分配方法：**

列出所有会被修改的文件，明确指定每个文件由哪条 lane 负责。

**示例：**

```
Lane A (Stripe integration):
  - src/payment/stripe_client.rs       ← Lane A 新文件
  - src/payment/transaction_log.rs     ← Lane A 新文件
  - tests/payment/stripe_integration_test.rs ← Lane A 的测试

Lane B (Validation + error handling):
  - src/payment/validator.rs           ← Lane B 新文件
  - src/payment/error.rs               ← Lane B 新文件
  - tests/payment/validator_test.rs    ← Lane B 的测试

Lane C (Refund + audit):
  - src/payment/refund.rs              ← Lane C 新文件
  - src/payment/audit_log.rs           ← Lane C 新文件
  - tests/payment/refund_test.rs       ← Lane C 的测试

Lane D (Frontend):
  - web/src/components/PaymentForm.jsx      ← Lane D 新文件
  - web/src/pages/CheckoutPage.jsx          ← Lane D 新文件（修改已有）
  - web/tests/PaymentForm.test.jsx          ← Lane D 的测试

共享（不允许修改）：
  - src/payment/mod.rs                 ← 由 Lane A 在 Phase 1 定义接口，其他 lane 只读
  - src/lib.rs                         ← 只有最后聚合时改
```

**冲突检查：**
逐一检查是否有文件出现两次。如果有，要么：
1. **重新拆任务** — 把这个文件作为一个独立的 lane（"API 设计" lane）
2. **改编制** — 两个 lane 合并成一个
3. **抽出共享层** — 把会冲突的部分拆成一个独立的 shared module，两个 lane 都使用但不修改

如果无法避免冲突，**不要用 SDD**；改用串行或两波派遣。

### Phase 3: Acceptance Criteria — 每条 Lane 的验收标准

对每条 lane，写出 **明确的、可验证的** 验收标准。

**示例（Lane A）：**

```
Lane A Acceptance Criteria:
  1. Stripe client 实现
     [ ] StripeClient::charge(amount, token) 成功返回 TransactionId
     [ ] StripeClient::refund(transaction_id) 成功返回确认
     [ ] 错误情况（invalid token、insufficient funds）返回合适的错误码
  
  2. 事务日志
     [ ] 每笔交易都记录到 transaction_log.rs
     [ ] 日志包含：时间戳、金额、用户 ID、状态、stripe 返回值
     [ ] 日志能被外部查询接口读取
  
  3. 测试覆盖
     [ ] cargo test --lib payment::stripe_client 通过
     [ ] cargo test --lib payment::transaction_log 通过
  
  4. 接口定义文档
     [ ] src/payment/mod.rs 中定义 TransactionLog trait
     [ ] 定义 ChargeRequest / ChargeResponse / RefundRequest / RefundResponse 数据结构
     [ ] 在 trait 中留出给其他 lane（B/C）的扩展点（比如 audit_id 字段）
```

**示例（Lane D，前端）：**

```
Lane D Acceptance Criteria:
  1. 支付表单组件
     [ ] web/src/components/PaymentForm.jsx 实现
     [ ] 包含字段：cardholder name, card number, expiry, CVC
     [ ] 表单验证（卡号格式、过期日期）
  
  2. 结账页面集成
     [ ] web/src/pages/CheckoutPage.jsx 中嵌入 PaymentForm
     [ ] 点击 "Pay" 按钮后调用 backend API /api/payment/charge
     [ ] 显示成功/失败提示
  
  3. 错误处理
     [ ] 网络错误显示 "Please try again"
     [ ] 支付失败显示后端返回的错误信息
  
  4. 测试覆盖
     [ ] npm test PaymentForm 通过
     [ ] npm test CheckoutPage 通过
  
  5. 与其他 lane 的依赖
     [ ] 假设 Lane A/B/C 提供的 /api/payment/charge API
     [ ] API 返回格式：{ success: bool, transaction_id: string, error?: string }
```

**关键：**
- 每条 lane 的 criteria 要 **独立可验证**（不依赖其他 lane 的实现细节）
- 但可以 **依赖接口合约**（比如 Lane D 依赖 Lane A/B/C 提供的 API）

### Phase 4: Lane Briefing — 给每条 Lane 的 Briefing Document

为每条 lane 写一份简洁的 briefing，包括：

**示例 Briefing for Lane A:**

```markdown
# Lane A Briefing: Stripe Integration + Transaction Log

## Your File Territory
  - src/payment/stripe_client.rs (新)
  - src/payment/transaction_log.rs (新)
  - tests/payment/stripe_integration_test.rs (新)

## Your Acceptance Criteria
  [参考上面列出的 4 项]

## Dependencies You Provide
  - StripeClient trait + impl
  - TransactionLog trait (defined in src/payment/mod.rs)
  - API 约定：ChargeRequest / ChargeResponse 数据结构

## Dependencies You Consume
  - 假设 Stripe API 客户端库已在 Cargo.toml 中（stripe crate）
  - 假设数据库连接池已有（database_pool from cyberclaw-store）

## Interface Contract (DON'T CHANGE)
  This will be defined collaboratively and frozen before you start.
  See src/payment/mod.rs for the canonical interface.

## Confluence/Context Links
  - Architecture: docs/architecture/payment/README.md
  - Stripe API docs: https://stripe.com/docs/api/charges/create

## Questions Before You Start?
  Reply in the Execution before beginning work.
```

### Phase 5: Parallel Dispatch — 并行派遣 Subagents

使用 `SubAgentOrchestrator::spawn_child()` 派遣各 lane：

```rust
// Pseudocode (在 control-plane 里)
let lane_a = SubAgentOrchestrator::spawn_child(AgentId::new("executor"))
    .with_briefing(lane_a_briefing_doc)
    .with_acceptance_criteria(lane_a_criteria);

let lane_b = SubAgentOrchestrator::spawn_child(AgentId::new("executor"))
    .with_briefing(lane_b_briefing_doc)
    .with_acceptance_criteria(lane_b_criteria);

let lane_c = SubAgentOrchestrator::spawn_child(AgentId::new("executor"))
    .with_briefing(lane_c_briefing_doc)
    .with_acceptance_criteria(lane_c_criteria);

let lane_d = SubAgentOrchestrator::spawn_child(AgentId::new("executor"))
    .with_briefing(lane_d_briefing_doc)
    .with_acceptance_criteria(lane_d_criteria);

// 等所有 lane 完成
let results = futures::future::try_join_all(vec![
    lane_a.execute(),
    lane_b.execute(),
    lane_c.execute(),
    lane_d.execute(),
]).await?;
```

**限制检查：**
- 当前派遣是否 ≤ 5 个 subagent（`max_children`）？✓ Lane A-D = 4
- 当前嵌套深度 ≤ 3？（假设这是 depth 2）✓
- 预算分配合理吗？（4 lane 分享 50% parent budget）✓

### Phase 6: Monitoring + Heartbeat

在 lane 执行过程中：

- **每个 lane 每 30 分钟报告一次进度**（如果执行较长）
- **立刻发现阻塞**（"我需要 API 定义，但 Lane A 还没给"）
- **风险浮出** — 如果某条 lane 发现无法按 criteria 完成，立刻通知

如果阻塞无法快速解决，考虑：
1. 换个 lane 先做（减轻 critical path）
2. 抽出卡住的部分做成独立 lane（比如"API 接口定义"）

### Phase 7: Acceptance + Merge — 聚合验收

所有 lane 都报告 Done 后：

1. **逐条验收**：检查每条 lane 是否满足自己的 criteria
   - Lane A 的测试通过了？✓ 
   - Lane B 的测试通过了？✓
   - Lane C 的测试通过了？✓
   - Lane D 的测试通过了？✓

2. **集成验收**：做个集成测试
   ```
   cargo test --workspace        ← 全工作区编译和单元测试通过
   npm test                       ← 前端测试通过
   cargo test --doc              ← 文档测试通过
   ```

3. **合并**：各 lane 的代码已经互不冲突（因为地盘分开了），
   可以直接合并或 fast-forward 到主线

4. **追溯验证**：
   - 如果分支 workflow，确保没有文件冲突
   - 如果 CyberClaw Artifact，各 lane 结果聚合到一个 FinalVerdict Artifact

## Subagent-Driven Development 反模式

| 反模式 | 修正 |
|--------|------|
| Lane 之间有依赖但没明确定义接口 | 提前冻结接口合约（trait 定义、数据结构、API schema）|
| 两条 lane 都能修改同一个文件 | 重新拆任务或两条 lane 合并 |
| Lane 没有明确的 acceptance criteria | 回到 Phase 3，写清楚每条 lane 要达成什么 |
| 派遣了 8 条 lane（超过限制） | `SubAgentOrchestrator` max_children = 5；改成两波派遣 |
| Lane 之间需要频繁同步 | 不适合 SDD；改用串行或强耦合任务 |
| 没监测进度，突然发现一条 lane 卡住了 | 定期 heartbeat（每 30 分钟报告进度）|

## 与 CyberClaw 对象模型的关系

- **任务拆解** → 这个 skill 做
- **Lane briefing 写作** → Agent 做
- **Subagent 派遣** → `SubAgentOrchestrator` 做（`crates/cyberclaw-agent-runtime/src/sub_agent.rs`）
- **接口合约** → 持久化到 Artifact（或放在 `src/payment/mod.rs` 等代码注释里）
- **acceptance criteria** → 编码成 `AcceptanceCriterion` struct（来自 `persistent_execution.rs`）
- **最终聚合** → `FinalVerdict` Artifact 记录各 lane 的完成状态

**不要：**
- 让 skill 本体去拆任务（那是 Agent 的工作，skill 只提供方法论）
- 跳过接口定义就派遣 lane（肯定会冲突）

## 关键原则（重复以强化）

1. **拆解的关键是找"独立性"** — 如果两条 lane 要频繁合并，不是真正独立
2. **文件地盘要互不重叠** — 这是 SDD 能工作的前提
3. **接口要提前冻结** — lane 开始工作前，lane 之间的数据合约必须确定
4. **heartbeat 很关键** — 分散的 lane 需要同步心跳，发现问题要快
5. **不是所有任务都适合 SDD** — 如果有强依赖关系，改用串行或分成两波

## 何时使用 SDD

**强推荐：**
- 实现一个有多个清晰模块的大功能（支付、身份验证、通知系统）
- 多个团队/lane 平行工作（各 lane 拥有明确的文件地盘）
- 追求快速交付（并行比串行快）

**可选：**
- 有一些独立的 Bug fix（可以拆成 lane，但收益不大）

**不适合：**
- 任务太小或太简单
- 子任务高度耦合（需要频繁合并、交互）
- 只有 1-2 条可用 lane（串行可能更简单）

---

**Source acknowledgement**: 原方法论来自 Superpowers 项目的
`skills/subagent-driven-development/SKILL.md`。本适配版在 CyberClaw 语境下重写：
增加了对 `SubAgentOrchestrator`（含深度/子代/预算限制）的引用、
定义了"文件地盘无重叠"原则、
并说明了与 `AcceptanceCriterion` 和 Artifact 的集成。
