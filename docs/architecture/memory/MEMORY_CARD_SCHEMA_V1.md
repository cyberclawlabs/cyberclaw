# CyberClaw Memory Card Schema v1

## 1. 目标

本文档定义 CyberClaw 长期记忆对象 `MemoryCard` 的正式 schema 草案。

目标：

1. 为 `cyberclaw-memory` 提供稳定的数据模型
2. 为 compaction、memory extraction、memory retrieval 提供统一对象
3. 确保长期记忆可审计、可追踪、可失效、可治理
4. 避免把 transcript、全文 Artifact、外部知识索引直接塞进 memory card

一句话：

> `MemoryCard` 是 CyberClaw 长期记忆的最小治理单元，不是任意文本片段容器。

---

## 2. 设计原则

1. **结构化优先**
   长期记忆优先保存结构化内容，而不是大段自由文本。

2. **来源必填**
   每张 memory card 必须能追溯到 execution、artifact、review 或其它平台对象。

3. **可失效**
   每张 memory card 必须支持 stale、archive、ttl、revalidation。

4. **按 scope 管理**
   记忆必须明确归属于 user / project / case / tenant 等 scope。

5. **与 Artifact 分离**
   memory card 只保存沉淀、摘要、事实与引用，不存大正文。

---

## 3. 正式对象定义

## 3.1 Rust 草案

```rust
pub struct MemoryCard {
    pub id: MemoryCardId,
    pub scope: MemoryScope,
    pub kind: MemoryKind,
    pub status: MemoryStatus,
    pub title: Option<String>,
    pub summary: Option<String>,
    pub content: serde_json::Value,
    pub tags: Vec<String>,
    pub confidence: Option<f32>,
    pub ttl: Option<chrono::Duration>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub reviewed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_validated_at: Option<chrono::DateTime<chrono::Utc>>,
    pub source_refs: Vec<MemorySourceRef>,
    pub related_memory_ids: Vec<MemoryCardId>,
}
```

---

## 3.2 字段说明

| 字段 | 说明 | 必填 |
|---|---|---|
| `id` | memory card 唯一标识 | 是 |
| `scope` | 记忆作用域 | 是 |
| `kind` | 记忆类型 | 是 |
| `status` | 生命周期状态 | 是 |
| `title` | 简要标题 | 否 |
| `summary` | 可读摘要 | 否 |
| `content` | 结构化主体内容 | 是 |
| `tags` | 检索和分类标签 | 否 |
| `confidence` | 置信度，`0.0-1.0` | 否 |
| `ttl` | 生存期 | 否 |
| `created_at` | 创建时间 | 是 |
| `updated_at` | 更新时间 | 是 |
| `reviewed_at` | 审核时间 | 否 |
| `last_validated_at` | 最近再验证时间 | 否 |
| `source_refs` | 来源引用 | 是 |
| `related_memory_ids` | 相关 memory card | 否 |

---

## 4. Scope 设计

## 4.1 `MemoryScope`

```rust
pub enum MemoryScope {
    User { actor_id: ActorId },
    Project { workspace_id: WorkspaceId },
    Case { case_id: CaseId },
    Tenant { tenant_id: TenantId },
    Global,
}
```

### 含义

- `User`
  - 个人偏好、个人工作习惯、个人助手画像
- `Project`
  - 项目级规则、环境事实、工程约束、项目知识
- `Case`
  - 调查结论、事件上下文、特定工单长期信息
- `Tenant`
  - 组织级策略、团队共识、共享约束
- `Global`
  - 平台级公共记忆，默认应慎用

### 建议

默认优先级：

1. `Case`
2. `Project`
3. `Tenant`
4. `User`
5. `Global`

原因：

- CyberClaw 首先是受控执行平台
- 记忆应优先围绕 `Case / Project / Tenant` 组织，而不是纯个人画像

---

## 5. Kind 设计

## 5.1 `MemoryKind`

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

### 含义

- `Fact`
  - 稳定事实
- `Preference`
  - 用户/团队偏好
- `Constraint`
  - 约束、禁止条件、环境限制
- `Conclusion`
  - 任务/Case 提炼后的结论
- `EpisodicSummary`
  - execution 或 case 阶段总结
- `ProceduralNote`
  - 对 procedural memory 的提炼性索引，而不是正文
- `Profile`
  - 用户或团队画像

---

## 6. Status 设计

## 6.1 `MemoryStatus`

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

### 生命周期说明

- `Draft`
  - 刚从 extraction pipeline 生成，尚未确认
- `Active`
  - 已可用，但未人工审阅
- `Reviewed`
  - 已经 review 或被可信规则确认
- `Stale`
  - 可能过期，不应默认注入上下文
- `Archived`
  - 保留但不再参与常规检索
- `Rejected`
  - 明确判定为无效，不应再使用

### 建议写入策略

- 自动提炼默认写成 `Draft` 或 `Active`
- 高风险结论应升到 `Reviewed` 后再作为强记忆使用

---

## 7. Source 引用设计

## 7.1 `MemorySourceRef`

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

### 设计要求

1. `source_refs` 不能为空
2. 一张 card 至少指向一个平台对象来源
3. 如果来自外部知识检索，必须尽量带：
   - `connector_id`
   - `capability_id`
   - `artifact_id`
4. 如果来自执行总结，必须尽量带：
   - `execution_id`
   - `case_id`

---

## 8. Content 设计规范

`content` 必须是结构化 JSON/YAML 可映射对象。

不建议：

```yaml
content: "一大段没有结构的总结文本"
```

推荐：

```yaml
content:
  finding: 某资产段存在统一弱口令配置
  severity: high
  affected_scope:
    - subnet-a
    - subnet-b
  recommended_action:
    - rotate-credentials
    - enforce-mfa
```

### 规则

1. `summary` 用于人类快速阅读
2. `content` 用于程序化使用
3. 长正文不放 `content`，应放 Artifact 后只保留 `artifact_ref`

---

## 9. 样例

## 9.1 `Conclusion`

```yaml
id: mem_case_001
scope:
  case:
    case_id: case_001
kind: conclusion
status: reviewed
title: 弱口令问题已确认
summary: 某资产组存在统一弱口令配置风险。
content:
  finding: 某资产组存在统一弱口令配置风险
  severity: high
  impact: lateral_movement_possible
  recommendation:
    - rotate_all_passwords
    - enable_mfa
confidence: 0.93
tags:
  - secops
  - credential
  - high-risk
source_refs:
  - execution_id: exec_101
    artifact_id: art_201
    case_id: case_001
    review_id: review_001
```

## 9.2 `Constraint`

```yaml
id: mem_project_007
scope:
  project:
    workspace_id: ws_main
kind: constraint
status: active
title: 生产环境禁止直接执行高风险变更
summary: 涉及生产环境变更必须先 review。
content:
  environment: prod
  require_review: true
  restricted_capabilities:
    - cmd.exec
    - fs.write
confidence: 1.0
tags:
  - governance
  - prod
  - review
source_refs:
  - artifact_id: art_policy_001
```

---

## 10. 检索与注入建议

### 10.1 检索优先级

上下文构建时建议按顺序读取：

1. `Case` scope
2. `Project` scope
3. `Tenant` scope
4. `User` scope
5. `Global` scope

### 10.2 注入过滤条件

默认注入时应过滤掉：

- `Rejected`
- `Archived`
- 明显过期 TTL
- `Stale` 且无再验证
- 低置信度且无 review 的高风险结论

---

## 11. 第一阶段实现建议

第一阶段建议仅支持：

1. `Fact`
2. `Constraint`
3. `Conclusion`
4. `EpisodicSummary`

先不要一开始把所有 kind 做满。

原因：

- 这四类最贴近 CyberClaw 的平台需求
- 对 R&D / Security / GRC 都适用
- 最容易和 Execution / Artifact / Review 闭环结合

---

## 12. 最终建议

正式结论：

> `MemoryCard` 应被定义为“带 scope、status、provenance、ttl 和结构化 content 的长期记忆对象”，而不是任意摘要文本容器。

这能保证 CyberClaw 的长期记忆同时满足：

1. 可读
2. 可治理
3. 可审计
4. 可过期
5. 可程序化使用
