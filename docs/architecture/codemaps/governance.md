# 治理层架构

**最后更新:** 2024-03-18
**包路径:** `crates/cyberclaw-governance/`
**状态:** 🚧 规划中

## 治理层概览

```
Governance Layer (治理门禁)
├── Permission Check    - 权限验证
├── Policy Evaluation   - 策略评估
├── Risk Assessment     - 风险评估
├── Approval Workflow   - 审批流程
├── Audit Logging       - 审计日志
└── Provenance Tracking - 溯源追踪
```

## 架构定位

```
┌─────────────────────────────────────────────┐
│            Trigger / Event Layer             │
└────────────────┬────────────────────────────┘
                 │
┌────────────────▼────────────────────────────┐
│           Control Plane                      │
│  • Resolver → 选择 Agent + Capabilities     │
└────────────────┬────────────────────────────┘
                 │
┌────────────────▼────────────────────────────┐
│          Execution Service                   │
│  • 任务执行 • 状态管理                      │
└────────────────┬────────────────────────────┘
                 │
┌────────────────▼────────────────────────────┐
│       ★ Governance Gate ★                   │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  │
│  │Permission│  │  Policy  │  │   Risk   │  │
│  │  Check   │→ │Evaluation│→ │Assessment│  │
│  └──────────┘  └──────────┘  └──────────┘  │
│       │              │              │        │
│       └──────────────┼──────────────┘        │
│                      │                       │
│              ┌───────▼───────┐               │
│              │ Approval Flow │               │
│              └───────┬───────┘               │
│                      │                       │
│       ┌──────────────┼──────────────┐        │
│       │              │              │        │
│   ┌───▼───┐    ┌────▼────┐    ┌───▼────┐   │
│   │ Audit │    │Provenance│   │ Budget │   │
│   │  Log  │    │ Tracking │   │ Check  │   │
│   └───────┘    └─────────┘    └────────┘   │
└────────────────┬────────────────────────────┘
                 │
┌────────────────▼────────────────────────────┐
│         Capability Execution                 │
│  (External Systems, Tools, Models)          │
└─────────────────────────────────────────────┘
```

## 1. Permission Check (权限验证)

### 设计目标

```
功能职责：
├── 身份验证
│   ├── 用户身份
│   ├── Agent 身份
│   └── Service 身份
│
├── 权限模型
│   ├── RBAC (基于角色)
│   ├── ABAC (基于属性)
│   └── Capability-based (基于能力)
│
└── 权限检查
    ├── 操作权限 (CRUD)
    ├── 资源权限 (文件, 数据库)
    └── Capability 权限 (外部调用)
```

### 权限模型（规划）

```rust
pub struct Permission {
    pub resource: String,      // tasks, agents, workflows
    pub action: String,        // create, read, update, delete
    pub scope: Scope,          // own, team, org, global
}

pub enum Scope {
    Own,      // 仅自己创建的资源
    Team,     // 团队资源
    Org,      // 组织资源
    Global,   // 全局资源
}

pub struct Role {
    pub id: String,
    pub name: String,
    pub permissions: Vec<Permission>,
}

pub struct Identity {
    pub id: String,
    pub kind: IdentityKind,   // User, Agent, Service
    pub roles: Vec<String>,
    pub attributes: HashMap<String, Value>,
}

pub trait PermissionChecker {
    async fn check(&self, identity: &Identity, permission: &Permission) -> Result<bool>;
    async fn list_permissions(&self, identity: &Identity) -> Result<Vec<Permission>>;
}
```

### 权限检查流程

```
请求 → 提取 Identity
         │
         ├→ 验证身份有效性
         │   ├→ Token 验证
         │   └→ 证书验证
         │
         ├→ 加载角色
         │   ├→ 从用户配置
         │   └→ 从 Agent manifest
         │
         ├→ 检查权限
         │   ├→ RBAC: 角色是否包含权限
         │   ├→ ABAC: 属性是否满足策略
         │   └→ Capability: 能力是否被授予
         │
         └→ 决策
             ├→ Allow → 继续
             └→ Deny → 返回错误
```

## 2. Policy Evaluation (策略评估)

### 设计目标

```
功能职责：
├── 策略定义
│   ├── 时间窗口策略 (工作时间)
│   ├── 资源配额策略 (预算限制)
│   ├── 风险等级策略 (高风险审批)
│   └── 合规性策略 (监管要求)
│
├── 策略语言
│   ├── OPA Rego (Open Policy Agent)
│   ├── CEL (Common Expression Language)
│   └── 自定义 DSL
│
└── 策略执行
    ├── 评估上下文
    ├── 规则匹配
    └── 决策输出
```

### 策略定义示例（OPA Rego）

```rego
package cyberclaw.policies

# 策略 1: 高风险操作必须审批
require_approval[msg] {
    input.capability.risk == "high"
    msg := "High risk capability requires approval"
}

require_approval[msg] {
    input.capability.risk == "critical"
    msg := "Critical risk capability requires approval"
}

# 策略 2: 工作时间限制
deny[msg] {
    is_production_write(input.capability)
    not is_business_hours()
    msg := "Production writes only allowed during business hours"
}

is_business_hours() {
    hour := time.now_hour()
    day := time.weekday(time.now_ns())
    hour >= 9
    hour < 18
    day != "Saturday"
    day != "Sunday"
}

# 策略 3: 预算限制
deny[msg] {
    input.budget.tokens_used + input.capability.estimated_tokens > input.budget.max_tokens
    msg := sprintf("Token budget exceeded: %d + %d > %d",
        [input.budget.tokens_used, input.capability.estimated_tokens, input.budget.max_tokens])
}

# 策略 4: 子代理深度限制
deny[msg] {
    input.execution.depth >= input.agent.spawn_policy.max_depth
    msg := sprintf("Max spawn depth reached: %d >= %d",
        [input.execution.depth, input.agent.spawn_policy.max_depth])
}
```

### 策略引擎（规划）

```rust
pub struct PolicyContext {
    pub identity: Identity,
    pub agent: AgentSpec,
    pub capability: CapabilityContract,
    pub execution: ExecutionContext,
    pub budget: ExecutionBudget,
}

pub struct PolicyDecision {
    pub allow: bool,
    pub reasons: Vec<String>,
    pub require_approval: bool,
    pub approvers: Vec<String>,
}

pub trait PolicyEngine {
    async fn evaluate(&self, context: &PolicyContext) -> Result<PolicyDecision>;
    async fn load_policies(&mut self, policies: Vec<Policy>) -> Result<()>;
}
```

## 3. Risk Assessment (风险评估)

### 风险等级定义

```rust
pub enum RiskLevel {
    Low,       // 只读操作
    Medium,    // 受控写操作
    High,      // 敏感操作
    Critical,  // 破坏性操作
}

pub enum CapabilityEffect {
    Read,                // 读取数据
    Write,               // 写入数据
    ExternalRead,        // 外部系统读取
    ExternalWrite,       // 外部系统写入
    NetworkAccess,       // 网络访问
    ProcessSpawn,        // 进程创建
    FileSystemWrite,     // 文件系统写入
    DatabaseWrite,       // 数据库写入
    Destructive,         // 破坏性操作 (删除)
}
```

### 风险评估矩阵

| Capability Effect | 默认风险 | 需要审批 |
|-------------------|----------|----------|
| Read | Low | ❌ |
| ExternalRead | Low | ❌ |
| Write | Medium | ⚠️ (策略决定) |
| ExternalWrite | Medium | ⚠️ (策略决定) |
| NetworkAccess | Medium | ⚠️ (策略决定) |
| ProcessSpawn | High | ✅ |
| FileSystemWrite | High | ✅ |
| DatabaseWrite | High | ✅ |
| Destructive | Critical | ✅ |

### 风险评估流程

```
Capability 请求 → 提取 Effects
                    │
                    ├→ 计算基础风险
                    │   └→ 根据 RiskLevel + Effects
                    │
                    ├→ 应用上下文因子
                    │   ├→ 生产环境 (+1 级)
                    │   ├→ 敏感数据 (+1 级)
                    │   └→ 离线时间 (-1 级)
                    │
                    ├→ 最终风险等级
                    │
                    └→ 审批决策
                        ├→ Low/Medium → 自动通过
                        └→ High/Critical → 需要审批
```

## 4. Approval Workflow (审批流程)

### 审批模型

```rust
pub struct ApprovalRequest {
    pub id: String,
    pub task_id: String,
    pub capability: CapabilityContract,
    pub risk: RiskLevel,
    pub requester: Identity,
    pub reason: String,
    pub context: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

pub struct Approval {
    pub request_id: String,
    pub reviewer: Identity,
    pub decision: ApprovalDecision,
    pub comment: Option<String>,
    pub timestamp: DateTime<Utc>,
}

pub enum ApprovalDecision {
    Approved,
    Rejected,
    RequestMoreInfo,
}

pub struct ApprovalPolicy {
    pub min_approvers: u32,
    pub required_roles: Vec<String>,
    pub timeout_secs: i64,
    pub auto_approve_low_risk: bool,
}
```

### 审批流程

```
Capability 调用 → 风险评估 → 需要审批?
                                  │
                                  ├→ 否 → 直接执行
                                  │
                                  └→ 是 → 创建 ApprovalRequest
                                          │
                                          ├→ 通知审批人
                                          │   ├→ Slack 通知
                                          │   ├→ Email 通知
                                          │   └→ WebHook 通知
                                          │
                                          ├→ 等待审批
                                          │   ├→ 审批人查看
                                          │   ├→ 审批人决策
                                          │   └→ 超时检查
                                          │
                                          └→ 审批结果
                                              ├→ Approved → 执行
                                              └→ Rejected → 拒绝
```

### 审批人选择策略

```
策略 1: 基于角色
  └→ High Risk → security-team
  └→ Critical Risk → security-lead + engineering-lead

策略 2: 基于资源
  └→ 生产数据库 → database-admin
  └→ 客户数据 → privacy-officer

策略 3: 基于预算
  └→ > $100 成本 → finance-team

策略 4: 自动审批
  └→ Low Risk + 非生产 → Auto Approve
```

## 5. Audit Logging (审计日志)

### 审计事件

```rust
pub struct AuditEvent {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub event_type: AuditEventType,
    pub actor: Identity,
    pub resource: String,
    pub action: String,
    pub result: AuditResult,
    pub metadata: serde_json::Value,
    pub trace_id: String,
}

pub enum AuditEventType {
    Authentication,      // 认证事件
    Authorization,       // 授权检查
    PolicyEvaluation,    // 策略评估
    ApprovalRequest,     // 审批请求
    ApprovalDecision,    // 审批决策
    CapabilityInvoke,    // 能力调用
    ResourceAccess,      // 资源访问
    SecurityIncident,    // 安全事件
}

pub enum AuditResult {
    Success,
    Failure(String),
    Blocked(String),
}
```

### 审计日志格式

```json
{
  "id": "audit-789",
  "timestamp": "2024-03-18T10:00:00Z",
  "eventType": "capability-invoke",
  "actor": {
    "id": "agent-123",
    "kind": "agent",
    "name": "security-scanner"
  },
  "resource": "github-connector",
  "action": "create-issue",
  "result": "success",
  "metadata": {
    "capability": "github-connector:create-issue",
    "risk": "medium",
    "approved": false,
    "autoApproved": true,
    "executionId": "exec-456"
  },
  "traceId": "trace-abc"
}
```

### 审计存储

```
审计日志流向：
├── 实时流
│   ├→ Event Bus (内部监控)
│   └→ WebSocket (实时监控)
│
├── 短期存储 (7-30 天)
│   └→ 时序数据库 (InfluxDB/TimescaleDB)
│
└── 长期归档 (1-7 年)
    └→ 对象存储 (S3/MinIO) + 加密
```

## 6. Provenance Tracking (溯源追踪)

### 执行溯源

```rust
pub struct ProvenanceRecord {
    pub execution_id: String,
    pub parent_id: Option<String>,
    pub agent: String,
    pub task: Task,
    pub capabilities_invoked: Vec<CapabilityInvocation>,
    pub artifacts_produced: Vec<ArtifactRef>,
    pub decisions: Vec<DecisionRecord>,
    pub timeline: Vec<ProvenanceEvent>,
}

pub struct CapabilityInvocation {
    pub capability: String,
    pub timestamp: DateTime<Utc>,
    pub input: serde_json::Value,
    pub output: serde_json::Value,
    pub approved_by: Option<Vec<Identity>>,
}

pub struct DecisionRecord {
    pub decision_point: String,
    pub policy: String,
    pub result: bool,
    pub reason: String,
}
```

### 溯源链

```
用户请求
  │
  ├→ Task-123 (执行树根)
  │   │
  │   ├→ Agent: security-scanner
  │   │   │
  │   │   ├→ Skill: static-analysis
  │   │   │   └→ Capability: file:read (Low Risk)
  │   │   │
  │   │   ├→ Skill: dependency-audit
  │   │   │   └→ Capability: network:fetch (Medium Risk)
  │   │   │
  │   │   └→ Capability: github:create-issue (High Risk)
  │   │       ├→ Risk Assessment: High
  │   │       ├→ Approval Request: review-101
  │   │       ├→ Approved By: [security-lead]
  │   │       └→ Executed: Success
  │   │
  │   └→ Subagent-456 (子代理)
  │       └→ Agent: report-generator
  │           └→ Skill: report-summary
  │
  └→ Artifacts
      ├→ scan-results.json
      ├→ report.pdf
      └→ github-issue-789 (外部引用)
```

## 治理集成流程

```
┌─────────────────────────────────────────────┐
│           Execution Request                  │
│  (Agent invokes Capability)                  │
└────────────────┬────────────────────────────┘
                 │
┌────────────────▼────────────────────────────┐
│      Step 1: Permission Check                │
│  • 验证 Identity                            │
│  • 检查 RBAC/ABAC 权限                      │
│  • 记录审计日志                             │
└────────────────┬────────────────────────────┘
                 │ Pass
┌────────────────▼────────────────────────────┐
│      Step 2: Policy Evaluation               │
│  • 加载适用策略                             │
│  • 评估策略规则                             │
│  • 生成决策建议                             │
└────────────────┬────────────────────────────┘
                 │ Pass
┌────────────────▼────────────────────────────┐
│      Step 3: Risk Assessment                 │
│  • 计算风险等级                             │
│  • 应用上下文因子                           │
│  • 决定是否需要审批                         │
└────────────────┬────────────────────────────┘
                 │
         ┌───────┴───────┐
         │               │
   Low/Medium Risk   High/Critical Risk
         │               │
         │          ┌────▼────────────────────┐
         │          │ Step 4: Approval Flow   │
         │          │  • 创建审批请求         │
         │          │  • 通知审批人           │
         │          │  • 等待审批决策         │
         │          └────┬────────────────────┘
         │               │ Approved
         └───────┬───────┘
                 │
┌────────────────▼────────────────────────────┐
│      Step 5: Budget Check                    │
│  • 验证 tokens, steps, duration            │
│  • 验证子代理深度                           │
│  • 更新预算使用                             │
└────────────────┬────────────────────────────┘
                 │ Pass
┌────────────────▼────────────────────────────┐
│      Step 6: Execution                       │
│  • 调用 Capability                          │
│  • 记录溯源信息                             │
│  • 返回结果                                 │
└────────────────┬────────────────────────────┘
                 │
┌────────────────▼────────────────────────────┐
│      Step 7: Audit & Provenance              │
│  • 记录完整审计日志                         │
│  • 更新溯源链                               │
│  • 生成合规报告                             │
└─────────────────────────────────────────────┘
```

## 合规性框架

### 支持的合规标准

```
├── SOC 2 Type II
│   ├── 访问控制审计
│   ├── 变更管理日志
│   └── 安全监控
│
├── GDPR
│   ├── 数据访问记录
│   ├── 数据处理溯源
│   └── 用户权限管理
│
├── HIPAA
│   ├── 敏感数据审计
│   ├── 访问日志保留
│   └── 加密传输
│
└── PCI-DSS
    ├── 特权访问控制
    ├── 审计日志存储
    └── 定期审查流程
```

## 未来扩展

### v2.1 规划
- [ ] Permission Check MVP
- [ ] 简单策略引擎 (YAML 配置)
- [ ] 审计日志基础设施

### v2.2 规划
- [ ] OPA Rego 策略支持
- [ ] 审批工作流
- [ ] 溯源链可视化

### v2.3 规划
- [ ] 高级 ABAC
- [ ] 合规报告自动生成
- [ ] 异常检测

## 相关文档

- [控制平面](./control-plane.md) - ReviewQueue, Orchestrator
- [核心引擎](./core.md) - Identity, Capability 定义
- [可观测层](./observability.md) - 日志和追踪
- [安全架构](./security.md) - 安全防护

---

**维护说明:** 治理层目前处于脚手架阶段，本文档描述设计目标和治理流程。
