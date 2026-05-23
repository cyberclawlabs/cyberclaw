# CyberClaw M2 Governance Core 架构设计

- Status: Draft
- Scope: Architecture
- Owner: CyberClaw Platform Team
- Created: 2026-03-21
- Target: M2 Governance Core (Beta)

---

## 执行摘要

本文档定义 CyberClaw Governance Core（`cyberclaw-governance` crate）的完整架构设计，作为 M2 Milestone 的核心交付物。设计遵循 KISS 原则，优先实现最小可用集，确保可测试性和可扩展性。

**核心目标**：
- 从 control-plane 抽离所有治理逻辑
- 建立统一的策略评估引擎
- 支持多维度的决策模型
- 提供清晰的集成接口

**关键设计决策**：
- 采用 trait-based 架构确保可扩展性
- 决策模型支持 allow/deny/review_required 三态
- 策略组合使用优先级和短路规则
- 与现有系统保持向后兼容

---

## 1. 架构概览

### 1.1 组件架构图

```
┌─────────────────────────────────────────────────────────────┐
│                      Control Plane                           │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐      │
│  │ Orchestrator │  │  Execution   │  │   Review     │      │
│  │              │  │   Service    │  │    Queue     │      │
│  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘      │
│         │                  │                  │              │
└─────────┼──────────────────┼──────────────────┼──────────────┘
          │                  │                  │
          ▼                  ▼                  ▼
┌─────────────────────────────────────────────────────────────┐
│                   Governance Core API                        │
│  ┌────────────────────────────────────────────────────┐     │
│  │              PolicyEngine (trait)                   │     │
│  │  - evaluate_capability()                           │     │
│  │  - evaluate_execution()                            │     │
│  │  - evaluate_review()                               │     │
│  └────────────────────────────────────────────────────┘     │
└─────────────────────────────────────────────────────────────┘
          │
          ▼
┌─────────────────────────────────────────────────────────────┐
│                   Governance Core Implementation             │
│                                                              │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐     │
│  │   Default    │  │   Policy     │  │  Decision    │     │
│  │   Engine     │──│    Store     │──│   Builder    │     │
│  └──────────────┘  └──────────────┘  └──────────────┘     │
│                                                              │
│  ┌──────────────────────────────────────────────────┐      │
│  │              Policy Types                        │      │
│  │  ┌──────────┐ ┌──────────┐ ┌──────────┐       │      │
│  │  │Capability│ │  Review  │ │  Tenant  │       │      │
│  │  │  Policy  │ │  Policy  │ │  Policy  │       │      │
│  │  └──────────┘ └──────────┘ └──────────┘       │      │
│  └──────────────────────────────────────────────────┘      │
│                                                              │
│  ┌──────────────────────────────────────────────────┐      │
│  │              Security Features                   │      │
│  │  ┌──────────┐ ┌──────────┐ ┌──────────┐       │      │
│  │  │  Secret  │ │  Prompt  │ │  Command │       │      │
│  │  │  Redact  │ │Injection │ │  Safety  │       │      │
│  │  └──────────┘ └──────────┘ └──────────┘       │      │
│  └──────────────────────────────────────────────────┘      │
└─────────────────────────────────────────────────────────────┘
```

### 1.2 数据流

```
Task Request
     │
     ▼
Orchestrator ──────► PolicyEngine.evaluate_execution()
                            │
                            ▼
                    Load Policies by Context
                    (tenant, workspace, actor)
                            │
                            ▼
                    Evaluate Each Policy
                    (priority order, short-circuit)
                            │
                            ▼
                    Build GovernanceDecision
                    (allow/deny/review_required)
                            │
                            ▼
                    Return to Orchestrator
                            │
     ┌──────────────────────┼──────────────────────┐
     ▼                      ▼                      ▼
  Allowed              Review Required            Denied
  (execute)            (enqueue review)          (reject)
```

---

## 2. 核心接口定义

### 2.1 PolicyEngine Trait

```rust
// crates/cyberclaw-governance/src/engine.rs

use cyberclaw_core::prelude::*;
use async_trait::async_trait;
use anyhow::Result;

/// 策略评估引擎的核心 trait
#[async_trait]
pub trait PolicyEngine: Send + Sync {
    /// 评估 capability 是否可以执行
    async fn evaluate_capability(
        &self,
        request: CapabilityEvaluationRequest,
    ) -> Result<GovernanceDecision>;

    /// 评估完整的执行计划
    async fn evaluate_execution(
        &self,
        request: ExecutionEvaluationRequest,
    ) -> Result<GovernanceDecision>;

    /// 评估 review 请求（用于自动审批逻辑）
    async fn evaluate_review(
        &self,
        request: ReviewEvaluationRequest,
    ) -> Result<ReviewDecision>;

    /// 重新加载策略配置
    async fn reload_policies(&self) -> Result<()>;

    /// 获取当前活跃的策略数量（用于监控）
    async fn policy_count(&self) -> usize;
}

/// Capability 评估请求
#[derive(Debug, Clone)]
pub struct CapabilityEvaluationRequest {
    pub capability_id: CapabilityId,
    pub connector_id: ConnectorId,
    pub risk: RiskLevel,
    pub effects: Vec<CapabilityEffect>,
    pub actor: ActorRef,
    pub workspace: Option<WorkspaceRef>,
    pub session: Option<SessionRef>,
    pub input: Option<serde_json::Value>,
}

/// 执行计划评估请求
#[derive(Debug, Clone)]
pub struct ExecutionEvaluationRequest {
    pub execution_id: ExecutionId,
    pub task: Task,
    pub plan: ExecutionPlan,
    pub actor: ActorRef,
    pub workspace: Option<WorkspaceRef>,
    pub session: Option<SessionRef>,
}

/// Review 评估请求
#[derive(Debug, Clone)]
pub struct ReviewEvaluationRequest {
    pub review_id: ReviewId,
    pub execution_id: ExecutionId,
    pub reviewer: ActorRef,
    pub decision: ReviewDecision,
}
```

### 2.2 决策模型

```rust
// crates/cyberclaw-governance/src/decision.rs

use serde::{Deserialize, Serialize};

/// 治理决策结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernanceDecision {
    /// 决策类型
    pub decision: Decision,
    /// 决策原因
    pub reason: String,
    /// 触发此决策的策略
    pub triggered_policies: Vec<PolicyRef>,
    /// 需要的审批者（如果 review_required）
    pub required_reviewers: Vec<ActorRef>,
    /// 安全建议
    pub security_recommendations: Vec<String>,
    /// 是否需要记录审计日志
    pub audit_required: bool,
}

/// 决策类型
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Decision {
    /// 允许执行
    Allow,
    /// 拒绝执行
    Deny,
    /// 需要人工审批
    ReviewRequired,
}

/// Review 决策
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ReviewDecision {
    /// 批准
    Approve,
    /// 拒绝
    Reject,
    /// 批准但修改参数
    ApproveWithModification(serde_json::Value),
    /// 升级到更高级别审批
    Escalate(Vec<ActorRef>),
}

/// 策略引用
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyRef {
    pub id: String,
    pub name: String,
    pub version: String,
}
```

### 2.3 策略类型系统

```rust
// crates/cyberclaw-governance/src/policy.rs

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// 策略基础 trait
#[async_trait]
pub trait Policy: Send + Sync {
    /// 策略唯一标识
    fn id(&self) -> &str;

    /// 策略名称
    fn name(&self) -> &str;

    /// 策略优先级（数字越小优先级越高）
    fn priority(&self) -> i32;

    /// 是否启用
    fn is_enabled(&self) -> bool;

    /// 评估策略
    async fn evaluate(&self, context: &PolicyContext) -> PolicyResult;
}

/// 策略上下文
#[derive(Debug, Clone)]
pub struct PolicyContext {
    pub actor: ActorRef,
    pub workspace: Option<WorkspaceRef>,
    pub session: Option<SessionRef>,
    pub capability: Option<CapabilityRef>,
    pub task: Option<Task>,
    pub metadata: serde_json::Value,
}

/// 策略评估结果
#[derive(Debug, Clone)]
pub enum PolicyResult {
    /// 策略不适用
    NotApplicable,
    /// 允许
    Allow,
    /// 拒绝
    Deny { reason: String },
    /// 需要审批
    RequireReview { reviewers: Vec<ActorRef>, reason: String },
}

/// Capability 策略配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityPolicy {
    pub id: String,
    pub name: String,
    pub priority: i32,
    pub enabled: bool,
    /// 匹配规则
    pub matchers: Vec<CapabilityMatcher>,
    /// 决策规则
    pub rules: Vec<CapabilityRule>,
}

/// Capability 匹配器
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityMatcher {
    /// 匹配 capability ID（支持通配符）
    pub capability_id_pattern: Option<String>,
    /// 匹配 connector ID（支持通配符）
    pub connector_id_pattern: Option<String>,
    /// 匹配风险级别
    pub risk_levels: Option<Vec<RiskLevel>>,
    /// 匹配效果
    pub effects: Option<Vec<CapabilityEffect>>,
}

/// Capability 规则
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityRule {
    /// 规则条件
    pub condition: RuleCondition,
    /// 规则动作
    pub action: RuleAction,
}

/// 规则条件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RuleCondition {
    /// 总是匹配
    Always,
    /// Actor 匹配
    ActorMatches { pattern: String },
    /// Workspace 匹配
    WorkspaceMatches { pattern: String },
    /// 输入包含敏感信息
    InputContainsSensitive { patterns: Vec<String> },
    /// 时间范围
    TimeRange { start: String, end: String },
    /// 组合条件
    And(Vec<RuleCondition>),
    Or(Vec<RuleCondition>),
    Not(Box<RuleCondition>),
}

/// 规则动作
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RuleAction {
    /// 允许
    Allow,
    /// 拒绝
    Deny { reason: String },
    /// 需要审批
    RequireReview {
        reviewers: Vec<String>,
        reason: String,
    },
    /// 修改输入（安全脱敏）
    RedactInput {
        patterns: Vec<String>,
        replacement: String,
    },
}

/// Review 策略配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewPolicy {
    pub id: String,
    pub name: String,
    pub priority: i32,
    pub enabled: bool,
    /// 自动批准规则
    pub auto_approve_rules: Vec<AutoApproveRule>,
    /// 升级规则
    pub escalation_rules: Vec<EscalationRule>,
}

/// 自动批准规则
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoApproveRule {
    pub condition: RuleCondition,
    pub max_risk: RiskLevel,
    pub allowed_effects: Vec<CapabilityEffect>,
}

/// 升级规则
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EscalationRule {
    pub condition: RuleCondition,
    pub escalate_to: Vec<String>,
    pub reason: String,
}
```

---

## 3. 实现架构

### 3.1 默认引擎实现

```rust
// crates/cyberclaw-governance/src/engine/default.rs

use super::*;
use crate::policy::{Policy, PolicyContext};
use std::collections::BTreeMap;
use tokio::sync::RwLock;

/// 默认的策略引擎实现
pub struct DefaultPolicyEngine {
    /// 按优先级排序的策略
    policies: RwLock<BTreeMap<i32, Vec<Box<dyn Policy>>>>,
    /// 策略加载器
    loader: Box<dyn PolicyLoader>,
}

impl DefaultPolicyEngine {
    pub fn new(loader: Box<dyn PolicyLoader>) -> Self {
        Self {
            policies: RwLock::new(BTreeMap::new()),
            loader,
        }
    }

    /// 评估策略集合
    async fn evaluate_policies(
        &self,
        context: &PolicyContext,
    ) -> Result<GovernanceDecision> {
        let policies = self.policies.read().await;

        let mut decision = Decision::Allow;
        let mut reasons = vec![];
        let mut triggered = vec![];
        let mut reviewers = vec![];

        // 按优先级顺序评估策略
        for (_priority, policy_group) in policies.iter() {
            for policy in policy_group {
                if !policy.is_enabled() {
                    continue;
                }

                match policy.evaluate(context).await {
                    PolicyResult::NotApplicable => continue,

                    PolicyResult::Deny { reason } => {
                        // Deny 立即短路返回
                        return Ok(GovernanceDecision {
                            decision: Decision::Deny,
                            reason,
                            triggered_policies: vec![to_policy_ref(policy.as_ref())],
                            required_reviewers: vec![],
                            security_recommendations: vec![],
                            audit_required: true,
                        });
                    }

                    PolicyResult::RequireReview { reviewers: r, reason } => {
                        // Review 优先级高于 Allow
                        if decision == Decision::Allow {
                            decision = Decision::ReviewRequired;
                        }
                        reviewers.extend(r);
                        reasons.push(reason);
                        triggered.push(to_policy_ref(policy.as_ref()));
                    }

                    PolicyResult::Allow => {
                        // Allow 只在没有更高优先级决策时记录
                        if decision == Decision::Allow {
                            triggered.push(to_policy_ref(policy.as_ref()));
                        }
                    }
                }
            }
        }

        Ok(GovernanceDecision {
            decision,
            reason: reasons.join("; "),
            triggered_policies: triggered,
            required_reviewers: reviewers,
            security_recommendations: vec![],
            audit_required: decision != Decision::Allow,
        })
    }
}

#[async_trait]
impl PolicyEngine for DefaultPolicyEngine {
    async fn evaluate_capability(
        &self,
        request: CapabilityEvaluationRequest,
    ) -> Result<GovernanceDecision> {
        let context = PolicyContext {
            actor: request.actor,
            workspace: request.workspace,
            session: request.session,
            capability: Some(CapabilityRef {
                id: request.capability_id,
                connector_id: request.connector_id,
                risk: request.risk,
                effects: request.effects,
                placement: None,
            }),
            task: None,
            metadata: request.input.unwrap_or_default(),
        };

        self.evaluate_policies(&context).await
    }

    async fn evaluate_execution(
        &self,
        request: ExecutionEvaluationRequest,
    ) -> Result<GovernanceDecision> {
        // 评估执行计划中的所有 capabilities
        let mut final_decision = Decision::Allow;
        let mut all_triggered = vec![];
        let mut all_reviewers = vec![];
        let mut all_reasons = vec![];

        for action in &request.plan.actions {
            let context = PolicyContext {
                actor: request.actor.clone(),
                workspace: request.workspace.clone(),
                session: request.session.clone(),
                capability: Some(action.capability.clone()),
                task: Some(request.task.clone()),
                metadata: action.input.clone(),
            };

            let decision = self.evaluate_policies(&context).await?;

            match decision.decision {
                Decision::Deny => {
                    // 任何 Deny 立即返回
                    return Ok(decision);
                }
                Decision::ReviewRequired => {
                    // ReviewRequired 覆盖 Allow
                    final_decision = Decision::ReviewRequired;
                    all_reviewers.extend(decision.required_reviewers);
                    all_reasons.push(decision.reason);
                    all_triggered.extend(decision.triggered_policies);
                }
                Decision::Allow => {
                    // Allow 只记录
                    all_triggered.extend(decision.triggered_policies);
                }
            }
        }

        Ok(GovernanceDecision {
            decision: final_decision,
            reason: all_reasons.join("; "),
            triggered_policies: all_triggered,
            required_reviewers: all_reviewers,
            security_recommendations: vec![],
            audit_required: final_decision != Decision::Allow,
        })
    }

    async fn evaluate_review(
        &self,
        _request: ReviewEvaluationRequest,
    ) -> Result<ReviewDecision> {
        // M2 简化版：暂不实现自动审批
        Ok(ReviewDecision::Approve)
    }

    async fn reload_policies(&self) -> Result<()> {
        let new_policies = self.loader.load_all().await?;

        let mut policies_map = BTreeMap::new();
        for policy in new_policies {
            policies_map
                .entry(policy.priority())
                .or_insert_with(Vec::new)
                .push(policy);
        }

        let mut policies = self.policies.write().await;
        *policies = policies_map;

        Ok(())
    }

    async fn policy_count(&self) -> usize {
        let policies = self.policies.read().await;
        policies.values().map(|v| v.len()).sum()
    }
}
```

---

## 4. 集成方案

### 4.1 Control Plane 集成

```rust
// 修改 crates/cyberclaw-control-plane/src/orchestrator.rs

use cyberclaw_governance::prelude::*;

impl ControlPlaneOrchestrator {
    // 新增 governance 字段
    governance: Arc<dyn PolicyEngine>,

    /// 替换原有的 evaluate_risk 方法
    async fn evaluate_governance(
        &self,
        plan: &ExecutionPlan,
        task: &Task,
        actor: &ActorRef,
        workspace: Option<WorkspaceRef>,
    ) -> anyhow::Result<GovernanceDecision> {
        let request = ExecutionEvaluationRequest {
            execution_id: ExecutionId::new(),
            task: task.clone(),
            plan: plan.clone(),
            actor: actor.clone(),
            workspace,
            session: None,
        };

        self.governance.evaluate_execution(request).await
    }

    /// 修改 process_ingress 方法
    pub async fn process_ingress(
        &self,
        request: IngressRequest,
    ) -> anyhow::Result<SubmitExecutionResult> {
        // ... 前面步骤不变 ...

        // Step 5: 使用 governance 评估
        let decision = self.evaluate_governance(
            &plan,
            &task,
            &normalized.actor,
            normalized.workspace.clone()
        ).await?;

        match decision.decision {
            Decision::Deny => {
                // 直接拒绝
                return Err(anyhow::anyhow!("Execution denied: {}", decision.reason));
            }
            Decision::ReviewRequired => {
                // 加入审批队列
                let review_id = self.enqueue_for_review(
                    execution_id.clone(),
                    trace_id.clone(),
                    &plan,
                    &task,
                    &normalized.actor,
                    normalized.workspace.clone(),
                    decision.required_reviewers,
                ).await?;
                return Ok(SubmitExecutionResult {
                    execution_id,
                    review_id: Some(review_id),
                    submitted: false,
                });
            }
            Decision::Allow => {
                // 直接执行
                // ... submit_execution ...
            }
        }
    }
}
```

### 4.2 ExecutionService 集成

```rust
// 修改 crates/cyberclaw-control-plane/src/execution_service.rs

impl ExecutionService {
    // 新增 governance 字段
    governance: Arc<dyn PolicyEngine>,

    /// 在执行前进行 capability 级别的检查
    async fn check_capability_permission(
        &self,
        capability: &CapabilityRef,
        actor: &ActorRef,
        workspace: Option<WorkspaceRef>,
    ) -> anyhow::Result<()> {
        let request = CapabilityEvaluationRequest {
            capability_id: capability.id.clone(),
            connector_id: capability.connector_id.clone(),
            risk: capability.risk.clone(),
            effects: capability.effects.clone(),
            actor: actor.clone(),
            workspace,
            session: None,
            input: None,
        };

        let decision = self.governance.evaluate_capability(request).await?;

        match decision.decision {
            Decision::Allow => Ok(()),
            Decision::Deny => Err(anyhow::anyhow!("Capability denied: {}", decision.reason)),
            Decision::ReviewRequired => {
                Err(anyhow::anyhow!("Capability requires review: {}", decision.reason))
            }
        }
    }
}
```

### 4.3 配置加载

```rust
// crates/cyberclaw-governance/src/loader.rs

use async_trait::async_trait;
use std::path::Path;

/// 策略加载器 trait
#[async_trait]
pub trait PolicyLoader: Send + Sync {
    /// 加载所有策略
    async fn load_all(&self) -> Result<Vec<Box<dyn Policy>>>;
}

/// 基于文件的策略加载器
pub struct FilePolicyLoader {
    config_dir: PathBuf,
}

impl FilePolicyLoader {
    pub fn new<P: AsRef<Path>>(config_dir: P) -> Self {
        Self {
            config_dir: config_dir.as_ref().to_path_buf(),
        }
    }

    async fn load_capability_policies(&self) -> Result<Vec<Box<dyn Policy>>> {
        let path = self.config_dir.join("capability_policies.yaml");
        if !path.exists() {
            return Ok(vec![]);
        }

        let content = tokio::fs::read_to_string(&path).await?;
        let policies: Vec<CapabilityPolicy> = serde_yaml::from_str(&content)?;

        Ok(policies.into_iter()
            .map(|p| Box::new(p) as Box<dyn Policy>)
            .collect())
    }

    async fn load_review_policies(&self) -> Result<Vec<Box<dyn Policy>>> {
        let path = self.config_dir.join("review_policies.yaml");
        if !path.exists() {
            return Ok(vec![]);
        }

        let content = tokio::fs::read_to_string(&path).await?;
        let policies: Vec<ReviewPolicy> = serde_yaml::from_str(&content)?;

        Ok(policies.into_iter()
            .map(|p| Box::new(p) as Box<dyn Policy>)
            .collect())
    }
}

#[async_trait]
impl PolicyLoader for FilePolicyLoader {
    async fn load_all(&self) -> Result<Vec<Box<dyn Policy>>> {
        let mut all_policies = vec![];

        all_policies.extend(self.load_capability_policies().await?);
        all_policies.extend(self.load_review_policies().await?);

        Ok(all_policies)
    }
}
```

---

## 5. 测试策略

### 5.1 单元测试计划

```rust
// crates/cyberclaw-governance/src/tests/mod.rs

#[cfg(test)]
mod tests {
    use super::*;

    // 1. PolicyEngine 基础功能测试（5 个测试）
    mod engine_tests {
        #[tokio::test]
        async fn test_engine_allow_decision() { /* ... */ }

        #[tokio::test]
        async fn test_engine_deny_decision() { /* ... */ }

        #[tokio::test]
        async fn test_engine_review_required_decision() { /* ... */ }

        #[tokio::test]
        async fn test_engine_policy_priority() { /* ... */ }

        #[tokio::test]
        async fn test_engine_reload_policies() { /* ... */ }
    }

    // 2. CapabilityPolicy 测试（5 个测试）
    mod capability_policy_tests {
        #[tokio::test]
        async fn test_capability_risk_based_decision() { /* ... */ }

        #[tokio::test]
        async fn test_capability_effect_matching() { /* ... */ }

        #[tokio::test]
        async fn test_capability_actor_restriction() { /* ... */ }

        #[tokio::test]
        async fn test_capability_workspace_isolation() { /* ... */ }

        #[tokio::test]
        async fn test_capability_pattern_matching() { /* ... */ }
    }

    // 3. ReviewPolicy 测试（5 个测试）
    mod review_policy_tests {
        #[tokio::test]
        async fn test_review_auto_approve() { /* ... */ }

        #[tokio::test]
        async fn test_review_escalation() { /* ... */ }

        #[tokio::test]
        async fn test_review_multi_reviewer() { /* ... */ }

        #[tokio::test]
        async fn test_review_tenant_isolation() { /* ... */ }

        #[tokio::test]
        async fn test_review_condition_evaluation() { /* ... */ }
    }

    // 4. 决策组合测试（5 个测试）
    mod decision_combination_tests {
        #[tokio::test]
        async fn test_deny_overrides_all() { /* ... */ }

        #[tokio::test]
        async fn test_review_overrides_allow() { /* ... */ }

        #[tokio::test]
        async fn test_multiple_policies_aggregation() { /* ... */ }

        #[tokio::test]
        async fn test_short_circuit_on_deny() { /* ... */ }

        #[tokio::test]
        async fn test_collect_all_reviewers() { /* ... */ }
    }
}
```

### 5.2 集成测试场景

```rust
// crates/cyberclaw-governance/tests/integration.rs

#[tokio::test]
async fn test_high_risk_capability_triggers_review() {
    let engine = create_test_engine().await;

    let request = CapabilityEvaluationRequest {
        capability_id: CapabilityId::from_string("fs.delete").unwrap(),
        risk: RiskLevel::High,
        // ...
    };

    let decision = engine.evaluate_capability(request).await.unwrap();
    assert_eq!(decision.decision, Decision::ReviewRequired);
}

#[tokio::test]
async fn test_low_risk_capability_auto_allowed() {
    let engine = create_test_engine().await;

    let request = CapabilityEvaluationRequest {
        capability_id: CapabilityId::from_string("fs.read").unwrap(),
        risk: RiskLevel::Low,
        // ...
    };

    let decision = engine.evaluate_capability(request).await.unwrap();
    assert_eq!(decision.decision, Decision::Allow);
}

#[tokio::test]
async fn test_workspace_isolation() {
    // 测试不同 workspace 的隔离性
}

#[tokio::test]
async fn test_policy_reload_takes_effect() {
    // 测试策略重载后立即生效
}
```

---

## 6. 迁移路径

### 6.1 阶段一：并行运行（Day 1-2）

1. 部署 governance crate，但不修改 control-plane
2. 添加监控，记录两种决策的差异
3. 验证 governance 决策的正确性

### 6.2 阶段二：影子模式（Day 3-4）

1. Control-plane 同时调用新旧逻辑
2. 使用旧逻辑的决策，但记录新逻辑的决策
3. 分析决策差异，调整策略配置

### 6.3 阶段三：切换（Day 5）

1. 修改 control-plane 使用新的 governance 决策
2. 保留旧代码作为 fallback
3. 监控关键指标

### 6.4 阶段四：清理（Day 6）

1. 移除 control-plane 中的旧治理逻辑
2. 更新文档
3. 发布 Beta 版本

---

## 7. 风险评估

### 7.1 技术风险

| 风险 | 可能性 | 影响 | 缓解措施 |
|------|--------|------|---------|
| 策略配置错误导致服务中断 | 中 | 高 | 1. 配置验证<br/>2. 灰度发布<br/>3. 快速回滚机制 |
| 性能下降 | 低 | 中 | 1. 策略缓存<br/>2. 短路评估<br/>3. 异步加载 |
| 向后兼容性问题 | 中 | 高 | 1. 影子模式验证<br/>2. 完整的迁移测试<br/>3. 保留 fallback |
| 策略冲突 | 中 | 中 | 1. 优先级机制<br/>2. 冲突检测工具<br/>3. 清晰的文档 |

### 7.2 安全风险

| 风险 | 可能性 | 影响 | 缓解措施 |
|------|--------|------|---------|
| 策略绕过 | 低 | 高 | 1. Deny 优先原则<br/>2. 默认拒绝<br/>3. 审计日志 |
| 权限提升 | 低 | 高 | 1. Actor 验证<br/>2. Workspace 隔离<br/>3. 最小权限原则 |
| 配置泄露 | 低 | 中 | 1. 敏感信息加密<br/>2. 访问控制<br/>3. 配置审计 |

---

## 8. 性能考虑

### 8.1 优化策略

1. **策略缓存**
   - 内存缓存编译后的策略
   - TTL 机制避免过期策略

2. **短路评估**
   - Deny 立即返回
   - 优先级排序减少评估次数

3. **异步加载**
   - 后台加载策略更新
   - 不阻塞主流程

### 8.2 性能目标

- 单次 capability 评估 < 10ms
- 执行计划评估 < 50ms
- 策略重载 < 1s
- 内存占用 < 100MB（1000 个策略）

---

## 9. 监控与可观测性

### 9.1 关键指标

```rust
// crates/cyberclaw-governance/src/metrics.rs

pub struct GovernanceMetrics {
    /// 决策计数器
    pub decisions_total: Counter<Decision>,
    /// 评估延迟
    pub evaluation_duration: Histogram,
    /// 策略命中率
    pub policy_hit_rate: Gauge,
    /// 活跃策略数量
    pub active_policies: Gauge,
}
```

### 9.2 审计日志

```rust
// crates/cyberclaw-governance/src/audit.rs

#[derive(Debug, Serialize)]
pub struct AuditEntry {
    pub timestamp: DateTime<Utc>,
    pub execution_id: ExecutionId,
    pub actor: ActorRef,
    pub decision: Decision,
    pub triggered_policies: Vec<PolicyRef>,
    pub capability: Option<CapabilityId>,
    pub input_hash: String,
}
```

---

## 10. 配置示例

### 10.1 Capability Policy 示例

```yaml
# config/capability_policies.yaml
- id: "high-risk-review"
  name: "High Risk Capability Review"
  priority: 100
  enabled: true
  matchers:
    - risk_levels: [High, Critical]
  rules:
    - condition:
        always: true
      action:
        require_review:
          reviewers: ["security-team"]
          reason: "High risk capability requires security review"

- id: "fs-write-restriction"
  name: "File System Write Restriction"
  priority: 200
  enabled: true
  matchers:
    - capability_id_pattern: "fs.write|fs.delete"
  rules:
    - condition:
        workspace_matches:
          pattern: "production/*"
      action:
        deny:
          reason: "Write operations not allowed in production workspace"
```

### 10.2 Review Policy 示例

```yaml
# config/review_policies.yaml
- id: "auto-approve-low-risk"
  name: "Auto Approve Low Risk"
  priority: 100
  enabled: true
  auto_approve_rules:
    - condition:
        always: true
      max_risk: Low
      allowed_effects: [Read]

- id: "escalate-critical"
  name: "Escalate Critical Operations"
  priority: 50
  enabled: true
  escalation_rules:
    - condition:
        and:
          - risk_level: Critical
          - workspace_matches:
              pattern: "production/*"
      escalate_to: ["cto", "security-lead"]
      reason: "Critical operation in production requires executive approval"
```

---

## 11. 下一步行动

### 立即行动（Day 1）

1. 创建 `cyberclaw-governance` crate 基础结构
2. 实现 PolicyEngine trait 和 DefaultPolicyEngine
3. 编写基础单元测试

### 短期目标（Week 1）

1. 完成所有核心策略类型实现
2. 集成到 control-plane
3. 达到 20+ 测试覆盖

### 中期目标（Week 2）

1. 完成迁移和向后兼容性测试
2. 部署到测试环境
3. 性能优化和监控集成

---

## 12. 验收标准

### 功能验收

- [ ] PolicyEngine 可独立运行
- [ ] 支持 Allow/Deny/ReviewRequired 三种决策
- [ ] 策略可通过配置文件加载
- [ ] Control-plane 完全使用 governance 决策
- [ ] 向后兼容性得到验证

### 质量验收

- [ ] 20+ 单元测试全部通过
- [ ] 集成测试覆盖主要场景
- [ ] 无 clippy 警告
- [ ] 文档完整

### 性能验收

- [ ] 单次评估 < 10ms
- [ ] 策略加载 < 1s
- [ ] 内存占用合理

---

**结论**：本架构设计提供了一个清晰、可扩展、可测试的 Governance Core 实现方案，完全满足 M2 Milestone 的要求，并为未来的功能扩展预留了空间。