# Runtime Blueprint 设计草案 v2.0

## 1. 目标

Runtime Blueprint 用于描述 CyberClaw 的运行时拓扑、策略边界与资源约束，确保运行时部署具备：

1. 可版本化
2. 可验证
3. 可规划
4. 可审计

Runtime Blueprint 不是业务生态对象，不直接对外暴露为 `Agent / Skill / Connector / Platform Plugin` 的替代物。  
它属于平台运行对象层，服务于 `Control Plane -> Runtime` 的落地过程。

---

## 2. 设计边界

### 2.1 Runtime Blueprint 负责

1. Runtime profile
2. Sandbox profile
3. Workspace materialization policy
4. Network egress policy
5. Filesystem mount policy
6. Secret injection policy
7. Inference routing policy
8. Resource limits

### 2.2 Runtime Blueprint 不负责

1. 业务角色定义
2. 业务任务编排
3. 审批状态记录
4. 会话记忆内容

---

## 3. 对象模型

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeBlueprint {
    pub api_version: String,
    pub id: String,
    pub version: String,
    pub summary: String,
    pub runtime_profile: RuntimeProfile,
    pub sandbox_profile: SandboxProfile,
    pub policy_profile: PolicyProfile,
    pub resources: ResourceProfile,
    pub compatibility: BlueprintCompatibility,
}
```

### 3.1 RuntimeProfile

```rust
pub struct RuntimeProfile {
    pub runtime_kind: String,              // native | container | remote
    pub workspace_mode: String,            // shared | isolated | ephemeral
    pub materialization_mode: String,      // local_persistent | shared_remote ...
    pub entrypoint: Option<String>,
}
```

### 3.2 SandboxProfile

```rust
pub struct SandboxProfile {
    pub filesystem_policy: FilesystemPolicy,
    pub process_policy: ProcessPolicy,
    pub network_policy: NetworkPolicy,
    pub inference_policy: InferencePolicy,
}
```

### 3.3 PolicyProfile

```rust
pub struct PolicyProfile {
    pub hot_reloadable: Vec<String>,
    pub creation_locked: Vec<String>,
}
```

### 3.4 ResourceProfile

```rust
pub struct ResourceProfile {
    pub cpu_millis: Option<u64>,
    pub memory_mb: Option<u64>,
    pub disk_mb: Option<u64>,
    pub max_concurrency: Option<u32>,
}
```

---

## 4. 生命周期

Runtime Blueprint 生命周期采用四阶段模型：

1. `Resolve`
2. `Verify`
3. `Plan`
4. `Apply`

### 4.1 Resolve

输入：

- blueprint ref
- environment ref
- tenant ref

输出：

- resolved blueprint artifact

### 4.2 Verify

校验项：

1. schema 校验
2. digest / signature 校验
3. compatibility 校验
4. policy guard 校验

输出：

- verified blueprint

### 4.3 Plan

生成：

1. workspace materialization plan
2. runtime allocation plan
3. policy activation plan
4. review/approval gate plan

输出：

- runtime apply plan

### 4.4 Apply

执行：

1. 创建/绑定 workspace
2. 创建 sandbox runtime
3. 下发策略
4. 注入 secrets
5. 启动 runtime instance

输出：

- runtime instance record
- provenance record

---

## 5. 策略分层

Blueprint 明确区分两类策略。

### 5.1 热更新策略（Hot Reloadable）

1. network egress allowlist
2. inference backend route
3. connector allowlist
4. review threshold (非创建性)

### 5.2 创建期锁定策略（Creation Locked）

1. filesystem root/mount
2. process sandbox level
3. privilege model
4. workspace isolation class

---

## 6. 与现有模块的集成

### 6.1 `cyberclaw-control-plane`

新增职责：

1. blueprint resolve/verify 接口
2. blueprint plan 生成
3. apply orchestration

### 6.2 `cyberclaw-governance`

新增职责：

1. blueprint policy guard 校验
2. apply 前治理检查

### 6.3 `cyberclaw-agent-runtime`

新增职责：

1. runtime instance materialization
2. sandbox profile apply

### 6.4 `cyberclaw-observability`

新增记录：

1. blueprint lifecycle events
2. runtime apply traces
3. blueprint provenance linkage

---

## 7. YAML 草案示例

```yaml
apiVersion: cyberclaw.io/v2
kind: RuntimeBlueprint
id: baseline-secure-runtime
version: 0.1.0
summary: Baseline secure runtime blueprint for controlled execution.

runtimeProfile:
  runtimeKind: container
  workspaceMode: isolated
  materializationMode: local_persistent

sandboxProfile:
  filesystemPolicy:
    allowedRoots:
      - /sandbox
      - /tmp
  processPolicy:
    profile: restricted
  networkPolicy:
    mode: allowlist
    egressHosts:
      - api.github.com
      - api.openai.com
  inferencePolicy:
    route: controlled-gateway

policyProfile:
  hotReloadable:
    - sandboxProfile.networkPolicy.egressHosts
    - sandboxProfile.inferencePolicy.route
  creationLocked:
    - runtimeProfile.workspaceMode
    - sandboxProfile.filesystemPolicy
    - sandboxProfile.processPolicy

resources:
  cpuMillis: 1000
  memoryMb: 2048
  diskMb: 4096
  maxConcurrency: 4

compatibility:
  platform: cyberclaw
  os:
    - linux
```

---

## 8. 演进建议

### 8.1 v2

1. 完成 Blueprint 对象建模与加载
2. 接入 `resolve -> verify -> plan -> apply`
3. 记录完整 lifecycle trace

### 8.2 v3

1. 支持多 blueprint profile（dev/staging/prod）
2. 支持节点标签和 placement 约束
3. 支持 blueprint migration

### 8.3 v4

1. 支持 blueprint registry 和签名分发
2. 支持 blueprint policy pack 复用

---

## 9. 多节点 Blueprint 扩展（Cluster-aware v1）

当 CyberClaw 从单节点演进到多节点时，Runtime Blueprint 需要显式表达以下策略面：

1. 节点放置约束（Placement）
2. 执行租约策略（Lease）
3. 共享状态策略（Shared State）
4. 事件总线策略（Event Bus）
5. 产物存储策略（Artifact Store）

### 9.1 扩展对象草案

```rust
pub struct ClusterProfile {
    pub enabled: bool,
    pub role: String, // control-plane | worker | hybrid
    pub placement: PlacementPolicy,
    pub lease: LeasePolicy,
    pub shared_state: SharedStatePolicy,
    pub event_bus: EventBusPolicy,
    pub artifact_store: ArtifactStorePolicy,
}

pub struct PlacementPolicy {
    pub allowed_node_labels: Vec<String>,
    pub required_runtime: Vec<String>,
    pub network_zone: Option<String>,
    pub strategy: String, // least-loaded | round-robin
}

pub struct LeasePolicy {
    pub ttl_ms: u64,
    pub renew_interval_ms: u64,
    pub max_handoff: u32,
}

pub struct SharedStatePolicy {
    pub mode: String, // in-memory | distributed
    pub cas_required: bool,
}

pub struct EventBusPolicy {
    pub mode: String, // in-memory | external
    pub ack_required: bool,
}

pub struct ArtifactStorePolicy {
    pub mode: String, // local-fs | object-store
    pub digest_required: bool,
    pub retention_days: Option<u32>,
}
```

---

## 10. 多节点生命周期补充（在 resolve/verify/plan/apply 之上）

### 10.1 Plan 阶段新增步骤

1. 读取 active membership
2. 计算 placement 目标节点
3. 预生成 lease plan
4. 生成 artifact store binding plan

### 10.2 Apply 阶段新增步骤

1. Acquire execution lease
2. Publish `execution.assigned` 事件
3. Worker 消费事件并启动 runtime
4. Worker 周期 renew lease
5. 完成后写 artifact + publish completion

### 10.3 Recover 阶段（新增）

当 lease 过期或 worker 超时时：

1. 标记原租约失效
2. placement 重算
3. 创建新租约并增加 handoff 计数
4. 发布 `execution.reassigned`

---

## 11. 多节点 YAML 草案（在现有示例基础上追加）

```yaml
clusterProfile:
  enabled: true
  role: control-plane
  placement:
    allowedNodeLabels:
      - linux
      - sec-zone-a
    requiredRuntime:
      - container
    networkZone: zone-a
    strategy: least-loaded
  lease:
    ttlMs: 30000
    renewIntervalMs: 10000
    maxHandoff: 3
  sharedState:
    mode: in-memory
    casRequired: true
  eventBus:
    mode: in-memory
    ackRequired: true
  artifactStore:
    mode: local-fs
    digestRequired: true
    retentionDays: 30
```

说明：v1 可以全部使用 in-memory 实现验证控制流；后续再替换为分布式实现，不改变对象模型。
