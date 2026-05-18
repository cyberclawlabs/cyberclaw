# cyberclaw-governance

- Status: Active
- Scope: Crate
- Owner: CyberClaw Governance Maintainers
- Last Updated: 2026-04-14

`cyberclaw-governance` 是 CyberClaw 的治理引擎 crate，提供基于策略的能力评估、风险评级和访问控制决策机制。

## 核心职责

### 策略引擎

- `engine.rs`：PolicyEngine trait 和 DefaultPolicyEngine 实现
  - 异步能力评估
  - 基于风险级别的决策
  - 可插拔的策略引擎架构

### 决策类型

- `decision.rs`：GovernanceDecision 和 ReviewType 定义
  - Allow：允许执行
  - Deny：拒绝执行
  - ReviewRequired：需要审查（Human/Approval/Escalation/Security）

### 策略定义

- `policy.rs`：Policy 和 PolicyCondition 实现
  - RiskLevel 映射到决策
  - 条件评估（ActorType、Capability、Tenant 等）
  - 策略组合和优先级

### 类型系统

- `types.rs`：CapabilityRef 和评估上下文
  - CapabilityRef：能力引用（id、name、risk_level、tags）
  - EvaluationContext：评估上下文（capability、actor、execution_id、reason）

### 安全策略引擎 (新增 2026-03-21)

- `security_policy_engine.rs`：统一安全策略引擎
  - 集成 SecurityScanner（秘密扫描、Prompt 注入、命令安全、包信任）
  - 策略违规检测和阻断
  - 安全事件记录
  - 12 个安全扫描测试通过

### 持久化策略引擎 (新增 2026-03-29)

- `persistent_engine.rs`：支持策略持久化的 PolicyEngine 实现
  - 集成 `cyberclaw-store::StateStore` 后端（PostgreSQL 或内存存储）
  - 双层架构：内存缓存（快速评估）+ 持久化存储（持久保存）
  - 设计模式：
    * Load-on-Init：创建时从 StateStore 加载所有活跃策略到内存
    * Write-Through Caching：新策略先写 StateStore，再更新缓存
    * Read-Optimized：策略评估使用只读锁，支持高并发
  - 完整的策略生命周期管理：
    * `add_policy(rule)`: 添加新策略
    * `deactivate_policy(id)`: 停用策略
    * `activate_policy(id)`: 激活策略
    * `delete_policy(id)`: 删除策略
    * `reload_policies()`: 运行时重新加载策略
  - 测试覆盖：29 个测试用例，100% 通过率
  - 文档：200+ 行模块文档，包含完整使用示例和架构图

### 危险能力过滤器 (新增 2026-04-11)

- `dangerous_capability_filter.rs`：Autopilot 模式下的能力过滤
  - 7 条默认危险规则（D001-D007），Critical/High/Medium 三级分类
  - 通配符模式匹配
  - Critical 规则无例外，High 规则可豁免
  - 9 个测试用例通过

## 风险级别决策矩阵

| RiskLevel | 默认决策 | ReviewType |
|-----------|---------|------------|
| Low | Allow | - |
| Medium | ReviewRequired | Human |
| High | ReviewRequired | Approval |
| Critical | ReviewRequired | Security |

## 使用示例

```rust
use cyberclaw_governance::engine::{DefaultPolicyEngine, PolicyEngine};
use cyberclaw_governance::decision::GovernanceDecision;
use cyberclaw_core::capability::RiskLevel;

#[tokio::main]
async fn main() {
    // 创建策略引擎
    let engine = DefaultPolicyEngine::new();

    // 评估能力
    let capability = CapabilityRef {
        id: CapabilityId::from_string("fs.write".to_string()).unwrap(),
        name: "fs.write".to_string(),
        risk_level: RiskLevel::Medium,
        tags: vec!["filesystem".to_string()],
    };

    let actor = ActorRef {
        id: ActorId::new(),
        actor_type: ActorType::Agent,
        tenant_id: None,
    };

    let context = EvaluationContext {
        capability,
        actor,
        execution_id: ExecutionId::new(),
        reason: "Write config file".to_string(),
    };

    // 执行评估
    let result = engine.evaluate(&context).await.unwrap();

    match result.decision {
        GovernanceDecision::Allow { .. } => println!("允许执行"),
        GovernanceDecision::Deny { reason } => println!("拒绝执行: {}", reason),
        GovernanceDecision::ReviewRequired { reason, review_type } => {
            println!("需要审查 ({:?}): {}", review_type, reason);
        }
    }
}
```

## 集成到控制平面

`cyberclaw-control-plane` 的 `orchestrator.rs` 使用本 crate 进行治理决策：

```rust
// orchestrator.rs 中的集成
use cyberclaw_governance::engine::PolicyEngine;

pub struct ControlPlaneOrchestrator {
    policy_engine: Arc<dyn PolicyEngine>,
    // ... 其他字段
}

// 评估执行计划中的每个能力
async fn evaluate_governance(&self, plan: &ExecutionPlan, task: &Task)
    -> anyhow::Result<(GovernanceDecision, RiskLevel)>
{
    for action in &plan.actions {
        let capability = self.registry.get_capability(&action.capability_id)?;
        let context = EvaluationContext { /* ... */ };
        let result = self.policy_engine.evaluate(&context).await?;
        // 聚合决策（最严格的决策获胜）
    }
}
```

## 测试与验证

```bash
# 运行所有测试
cargo test -p cyberclaw-governance

# 运行 clippy 检查
cargo clippy -p cyberclaw-governance --all-targets -- -D warnings

# 运行集成测试
cargo test -p cyberclaw-governance --test integration_test

# 运行文档测试
cargo test -p cyberclaw-governance --doc
```

## 安全特性

- **失败安全原则**：默认拒绝未知或空能力列表
- **防止自我批准**：审查流程需要不同的 Actor 批准
- **风险级别升级**：可疑行为（如空计划）自动提升风险级别
- **审计追踪**：所有决策包含原因和上下文信息

## 相关文档

- [仓库根 README](../../README.md)
- [治理架构](../../docs/architecture/governance/M2_GOVERNANCE_ARCHITECTURE.md)
- [集成和迁移计划](../../docs/architecture/governance/M2_INTEGRATION_AND_MIGRATION_PLAN.md)
- [仓库级 Changelog](../../CHANGELOG.md)

## 开发路线图

- **M2 (已完成)**：PolicyEngine 基础实现和 control-plane 集成
- **M3 (计划中)**：多租户 RBAC 支持
- **M4 (计划中)**：审计日志和合规报告
- **M5 (计划中)**：自定义策略语言和动态策略更新

### 命令权限注册表 (新增 2026-04-14)

- `command_rewrite_registry.rs`：Shell 命令级权限门禁
  - Deny > Ask > Allow > Default 优先级模型
  - 引号感知的复合命令拆分 (`&&`, `||`, `;`, `|`)
  - 透明前缀递归剥离 (`sudo`, `env KEY=VAL`, `nohup`)
  - 32 条默认规则：8 Deny (rm -rf /, DROP TABLE, fork bomb 等) + 16 Ask (force push, chmod 777 等) + 8 Allow (ls, cat, git status 等)
  - 支持运行时动态追加规则
  - 20 个测试用例通过

## 维护规则

1. 本文件说明 crate 局部职责，不重复仓库级路线图全文。
2. 显著变更记录写入仓库级 `CHANGELOG.md` 的相关章节。
3. 如果 crate 边界变化，需同步更新本文件和相关 `docs/` 文档。
