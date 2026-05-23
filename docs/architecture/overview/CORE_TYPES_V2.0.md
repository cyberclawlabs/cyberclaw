# CyberClaw Core Rust 类型草案 v2.0

## 1. 目标

本文档定义 `cyberclaw-core` 的核心 Rust 类型边界。

目标：

1. 给 `control-plane / agent-runtime / governance / connectors / store` 提供统一类型基础
2. 让平台对象、业务对象、执行对象在编译期有稳定边界
3. 保持 `cyberclaw-core` 只承载对象模型、trait 和协议，不掺杂 IO 与具体运行时逻辑

---

## 2. crate 职责

`cyberclaw-core` 负责：

- 核心 ID 类型
- 业务对象类型
- 平台运行对象类型
- 执行协议对象
- 生态对象 manifest 的抽象表示
- 跨 crate 共用 trait
- 错误与结果类型

`cyberclaw-core` 不负责：

- 文件系统扫描
- manifest 解析落盘
- 网络调用
- 审批存储
- trace 上报
- connector 实现
- skill 加载
- runtime 调度

---

## 3. 模块建议

```text
crates/cyberclaw-core/
├── src/
│   ├── lib.rs
│   ├── ids.rs
│   ├── task.rs
│   ├── case.rs
│   ├── workflow.rs
│   ├── execution.rs
│   ├── capability.rs
│   ├── artifact.rs
│   ├── review.rs
│   ├── identity.rs
│   ├── workspace.rs
│   ├── manifests.rs
│   ├── protocol.rs
│   ├── provenance.rs
│   ├── security.rs
│   ├── traits.rs
│   ├── enums.rs
│   ├── errors.rs
│   └── prelude.rs
```

---

## 4. 基础 ID 类型

建议所有核心对象使用显式 ID 新类型，避免字符串混用。

```rust
use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub struct $name(pub String);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4().to_string())
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

id_type!(TaskId);
id_type!(CaseId);
id_type!(WorkflowId);
id_type!(ExecutionId);
id_type!(ArtifactId);
id_type!(ReviewId);
id_type!(ApprovalStepId);
id_type!(AgentId);
id_type!(SkillId);
id_type!(ConnectorId);
id_type!(PlatformPluginId);
id_type!(CapabilityId);
id_type!(TenantId);
id_type!(ActorId);
id_type!(WorkspaceId);
id_type!(SessionId);
id_type!(TraceId);
id_type!(SecurityEventId);
id_type!(PolicyDecisionId);
id_type!(ProvenanceId);
```

### 原则

1. 不直接用裸 `String` 传递核心对象标识
2. `Id` 类型必须 `Serialize / Deserialize / Clone / Eq / Hash`
3. 运行时边界外再决定是否映射成 UUID、ULID 或数据库主键

---

## 5. 业务对象类型

## 5.1 `Task`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub case_id: Option<CaseId>,
    pub title: String,
    pub summary: String,
    pub kind: TaskKind,
    pub priority: Priority,
    pub requested_by: ActorRef,
    pub requested_at: chrono::DateTime<chrono::Utc>,
    pub trigger: TriggerRef,
    pub input: TaskInput,
    pub desired_outputs: Vec<OutputContractRef>,
    pub labels: Vec<String>,
}
```

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskKind {
    Analysis,
    Investigation,
    Review,
    Execution,
    Reporting,
    Automation,
    Custom(String),
}
```

## 5.2 `Case`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Case {
    pub id: CaseId,
    pub title: String,
    pub summary: String,
    pub kind: CaseKind,
    pub status: CaseStatus,
    pub owner_tenant: TenantId,
    pub created_by: ActorRef,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub labels: Vec<String>,
    pub metadata: std::collections::BTreeMap<String, serde_json::Value>,
}
```

## 5.3 `WorkflowRef`

`core` 不持有 workflow 引擎实现，只保留引用。

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowRef {
    pub id: WorkflowId,
    pub version: String,
    pub source: PackageRef,
}
```

---

## 6. 平台运行对象类型

## 6.1 `Identity` / `ActorRef`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActorRef {
    pub id: ActorId,
    pub actor_type: ActorType,
    pub tenant_id: Option<TenantId>,
    pub display_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ActorType {
    Human,
    Agent,
    System,
    Connector,
}
```

## 6.2 `WorkspaceRef`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceRef {
    pub id: WorkspaceId,
    pub mode: WorkspaceMode,
    pub root: String,
    pub writable_roots: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkspaceMode {
    Shared,
    Isolated,
    Ephemeral,
}
```

## 6.3 `SessionRef`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRef {
    pub id: SessionId,
    pub case_id: Option<CaseId>,
    pub workspace_id: Option<WorkspaceId>,
}
```

---

## 7. 执行对象类型

## 7.1 `Execution`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Execution {
    pub id: ExecutionId,
    pub root_execution_id: ExecutionId,
    pub parent_execution_id: Option<ExecutionId>,
    pub case_id: Option<CaseId>,
    pub task_id: Option<TaskId>,
    pub agent: AgentRef,
    pub status: ExecutionStatus,
    pub join_strategy: Option<JoinStrategy>,
    pub budget: ExecutionBudget,
    pub workspace: Option<WorkspaceRef>,
    pub trace_id: TraceId,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
}
```

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ExecutionStatus {
    Pending,
    Running,
    WaitingReview,
    WaitingApproval,
    Completed,
    Failed,
    Cancelled,
    TimedOut,
}
```

## 7.2 `ExecutionBudget`

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExecutionBudget {
    pub max_steps: Option<u32>,
    pub max_duration_ms: Option<u64>,
    pub max_tokens: Option<u32>,
    pub max_children: Option<u32>,
}
```

## 7.3 `ExecutionTreeNode`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionTreeNode {
    pub execution: Execution,
    pub children: Vec<ExecutionId>,
}
```

## 7.4 `JoinStrategy`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum JoinStrategy {
    JoinAll,
    JoinAny,
    FanOutFanIn,
    MapReduce,
}
```

---

## 8. 能力与动作类型

## 8.1 `CapabilityRef`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityRef {
    pub id: CapabilityId,
    pub connector_id: ConnectorId,
    pub risk: RiskLevel,
    pub effects: Vec<CapabilityEffect>,
}
```

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CapabilityEffect {
    Read,
    Write,
    Execute,
    Network,
    Ticket,
    Notification,
    Custom(String),
}
```

## 8.2 `ActionRequest`

`ActionRequest` 表示某次执行想要调用某个 capability。

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionRequest {
    pub execution_id: ExecutionId,
    pub requested_by: ActorRef,
    pub capability: CapabilityId,
    pub input: serde_json::Value,
    pub reason: String,
}
```

---

## 9. Review / Governance 类型

## 9.1 `ReviewRequest`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewRequest {
    pub id: ReviewId,
    pub execution_id: ExecutionId,
    pub case_id: Option<CaseId>,
    pub title: String,
    pub summary: String,
    pub requested_by: ActorRef,
    pub status: ReviewStatus,
    pub review_kind: ReviewKind,
    pub trace_id: TraceId,
}
```

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ReviewStatus {
    Pending,
    Approved,
    Rejected,
    Cancelled,
    Expired,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ReviewKind {
    HumanReview,
    Approval,
    Escalation,
}
```

## 9.2 `PolicyDecision`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyDecision {
    pub id: PolicyDecisionId,
    pub execution_id: ExecutionId,
    pub capability_id: CapabilityId,
    pub actor: ActorRef,
    pub decision: Decision,
    pub risk: RiskLevel,
    pub reasons: Vec<String>,
    pub approval_step_id: Option<ApprovalStepId>,
    pub trace_id: TraceId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Decision {
    Allow,
    Review,
    ApprovalRequired,
    Deny,
}
```

---

## 10. Artifact / Provenance 类型

## 10.1 `ArtifactRef`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRef {
    pub id: ArtifactId,
    pub kind: ArtifactKind,
    pub title: String,
    pub uri: String,
    pub content_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ArtifactKind {
    Report,
    Evidence,
    Patch,
    Summary,
    Memory,
    Log,
    Custom(String),
}
```

## 10.2 `ProvenanceRecord`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvenanceRecord {
    pub id: ProvenanceId,
    pub artifact_id: ArtifactId,
    pub execution_id: ExecutionId,
    pub parent_execution_id: Option<ExecutionId>,
    pub case_id: Option<CaseId>,
    pub agent_id: AgentId,
    pub skill_refs: Vec<SkillId>,
    pub connector_refs: Vec<ConnectorId>,
    pub capability_refs: Vec<CapabilityId>,
    pub trace_id: TraceId,
}
```

---

## 11. Security 相关类型

## 11.1 `SecurityEvent`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityEvent {
    pub id: SecurityEventId,
    pub execution_id: Option<ExecutionId>,
    pub case_id: Option<CaseId>,
    pub source: SecurityEventSource,
    pub event_type: SecurityEventType,
    pub severity: Severity,
    pub summary: String,
    pub details: serde_json::Value,
    pub trace_id: TraceId,
}
```

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SecurityEventSource {
    PromptScanner,
    PackageTrustScanner,
    RuntimeDetection,
    PermissionEngine,
    PolicyEngine,
    PlatformPlugin,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SecurityEventType {
    PromptInjectionDetected,
    SkillPoisoningSuspected,
    RuntimeAnomalyDetected,
    PermissionViolation,
    PolicyDenied,
    Custom(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Severity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}
```

---

## 12. 执行协议对象

## 12.1 `SpawnRequest`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnRequest {
    pub parent_execution_id: ExecutionId,
    pub requesting_agent_id: AgentId,
    pub target_agent_id: AgentId,
    pub task: Task,
    pub context: ContextPack,
    pub budget: ExecutionBudget,
    pub workspace_mode: WorkspaceMode,
    pub priority: Priority,
}
```

## 12.2 `ContextPack`

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ContextPack {
    pub artifact_refs: Vec<ArtifactRef>,
    pub memory_refs: Vec<ArtifactRef>,
    pub policy_refs: Vec<String>,
    pub workflow_ref: Option<WorkflowRef>,
    pub session: Option<SessionRef>,
    pub workspace: Option<WorkspaceRef>,
}
```

## 12.3 `ResultEnvelope`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultEnvelope {
    pub execution_id: ExecutionId,
    pub status: ExecutionStatus,
    pub summary: String,
    pub artifacts: Vec<ArtifactRef>,
    pub evidence_refs: Vec<ArtifactRef>,
    pub output: serde_json::Value,
    pub metrics: ExecutionMetrics,
    pub failure_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExecutionMetrics {
    pub duration_ms: Option<u64>,
    pub steps: Option<u32>,
    pub tokens_in: Option<u32>,
    pub tokens_out: Option<u32>,
}
```

---

## 13. 生态对象 Manifest 抽象表示

`core` 可以保留**解析后的抽象类型**，但不负责文件扫描。

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageManifest {
    pub api_version: String,
    pub kind: PackageKind,
    pub id: String,
    pub version: String,
    pub name: String,
    pub display_name: Option<String>,
    pub summary: String,
    pub owner: Option<String>,
    pub tags: Vec<String>,
    pub compatibility: Compatibility,
    pub dependencies: Dependencies,
    pub artifacts: Artifacts,
    pub config_schema: Option<String>,
    pub spec: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PackageKind {
    Agent,
    Skill,
    Connector,
    PlatformPlugin,
}
```

这样：

- `core` 认识 manifest 的抽象形状
- `registry/loader` 负责从文件系统把它解析出来
- `resolver` 负责根据任务和上下文做选择

---

## 14. 通用 trait 草案

## 14.1 标识 trait

```rust
pub trait Identified {
    type Id;
    fn id(&self) -> &Self::Id;
}
```

## 14.2 可引用 trait

```rust
pub trait Referencable {
    type Ref;
    fn to_ref(&self) -> Self::Ref;
}
```

## 14.3 包对象 trait

```rust
pub trait PackageObject: Identified {
    fn kind(&self) -> PackageKind;
    fn version(&self) -> &str;
    fn summary(&self) -> &str;
}
```

## 14.4 可治理动作 trait

```rust
pub trait GovernedAction {
    fn capability_id(&self) -> &CapabilityId;
    fn execution_id(&self) -> &ExecutionId;
    fn actor(&self) -> &ActorRef;
}
```

---

## 15. 错误类型草案

```rust
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("invalid state transition: {0}")]
    InvalidStateTransition(String),

    #[error("invalid reference: {0}")]
    InvalidReference(String),

    #[error("schema violation: {0}")]
    SchemaViolation(String),

    #[error("governance denied: {0}")]
    GovernanceDenied(String),
}

pub type CoreResult<T> = Result<T, CoreError>;
```

---

## 16. `lib.rs` 导出建议

```rust
pub mod artifact;
pub mod capability;
pub mod case;
pub mod enums;
pub mod errors;
pub mod execution;
pub mod identity;
pub mod ids;
pub mod manifests;
pub mod protocol;
pub mod provenance;
pub mod review;
pub mod security;
pub mod task;
pub mod traits;
pub mod workflow;
pub mod workspace;

pub mod prelude {
    pub use crate::artifact::*;
    pub use crate::capability::*;
    pub use crate::case::*;
    pub use crate::errors::*;
    pub use crate::execution::*;
    pub use crate::identity::*;
    pub use crate::ids::*;
    pub use crate::manifests::*;
    pub use crate::protocol::*;
    pub use crate::provenance::*;
    pub use crate::review::*;
    pub use crate::security::*;
    pub use crate::task::*;
    pub use crate::traits::*;
    pub use crate::workflow::*;
    pub use crate::workspace::*;
}
```

---

## 17. 辅助类型说明

上文示例中有意省略了部分辅助类型的完整定义，例如：

- `Priority`
- `TriggerRef`
- `TaskInput`
- `OutputContractRef`
- `PackageRef`
- `AgentRef`
- `CaseKind`
- `CaseStatus`
- `Compatibility`
- `Dependencies`
- `Artifacts`

这些类型仍然属于 `cyberclaw-core`，但建议拆分到对应模块中实现，避免核心文档被样板代码淹没。

---

## 18. 边界总结

`cyberclaw-core` 应稳定承载三类东西：

1. **对象模型**
Task、Case、Execution、Capability、Artifact、Review、SecurityEvent、Provenance

2. **协议模型**
SpawnRequest、ContextPack、ResultEnvelope、PackageManifest

3. **抽象契约**
trait、错误类型、ID 类型、状态枚举

不应承载：

1. 文件系统 Loader
2. 数据库存取
3. HTTP API
4. Workflow 执行逻辑
5. 审批存储与通知
6. Connector 具体实现

---

## 19. 一句话结论

> **`cyberclaw-core` 是 CyberClaw 的编译期对象模型中心：它负责定义平台的稳定语言，而不负责执行平台的动态行为。**
