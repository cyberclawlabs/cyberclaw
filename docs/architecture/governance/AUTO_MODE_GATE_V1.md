# Auto Mode Gate 架构设计 v1

- Status: Draft
- Scope: Architecture / Governance
- Owner: CyberClaw Maintainers
- Created: 2026-04-11
- Target: Control Plane + Governance
- References:
  - [Autopilot Architecture v1](../runtime/CYBERCLAW_AUTOPILOT_ARCHITECTURE_V1.md)
  - [M2 Governance Core](M2_GOVERNANCE_ARCHITECTURE.md)
  - [Claude Code Migration Blueprint §3.4](../../implementation/reports/2026-04-01-claude-first-migration-blueprint-for-cyberclaw.md)
  - [Claude Code Benchmark §4.4](../../implementation/reports/2026-04-01-claude-code-benchmark-and-openviking-integration-plan.md)

---

## 执行摘要

Auto Mode Gate 是 CyberClaw 治理层的权限动态收窄机制。当执行模式从 `Normal` 切换到 `Autopilot` 时，Gate 自动剥离危险 Capability 权限；退出时恢复原始权限快照。同时引入熔断器，在连续失败达到阈值时自动退出 Autopilot 模式。

**设计灵感来源**：Claude Code `permissionSetup.ts` 中的 `stripDangerousAllowRulesForAutoMode` / `restoreOriginalRules` 模式。

**核心约束**：
1. 不引入新的生态对象类型
2. 不绕开 `Connector -> Capability` 执行主链
3. 不替代 SecurityGate / ReviewQueue 的治理决策
4. 权限变更全链路可审计

---

## 1. 问题定义

### 1.1 当前状态

CyberClaw 已有：
- `ExecutionMode::Autopilot` 枚举值（`execution_service.rs`），但仅用于循环检测
- `SecurityGate` trait + `DefaultSecurityGate` 实现，做静态安全检查
- `GovernedLoopRuntime` 驱动的 Autopilot 执行循环
- `GovernedAutopilotStepRunner` 在每个 step 调用 SecurityGate

### 1.2 缺失能力

| 能力 | 状态 | 说明 |
|------|------|------|
| 模式切换时权限动态收窄 | 缺失 | 进入 Autopilot 时不会剥离危险权限 |
| 退出时权限恢复 | 缺失 | 无快照/恢复机制 |
| 危险 Capability 分类 | 缺失 | 无标准化的危险等级定义 |
| 连续失败熔断 | 缺失 | Autopilot Loop 无自动退出机制 |
| 模式切换审计 | 缺失 | 进入/退出事件未记录 |

---

## 2. 架构设计

### 2.1 组件总览

```
┌─────────────────────────────────────────────────────────┐
│                    Control Plane                         │
│                                                         │
│  ┌───────────────┐    ┌────────────────────────────┐    │
│  │ ExecutionMode │───▶│     AutoModeGate            │    │
│  │ Normal ↔ Auto │    │  ┌──────────────────────┐   │    │
│  └───────────────┘    │  │ PermissionSnapshot   │   │    │
│                       │  │ (pre-auto baseline)  │   │    │
│                       │  └──────────────────────┘   │    │
│                       │  ┌──────────────────────┐   │    │
│                       │  │ DangerousCapability   │   │    │
│                       │  │ Filter               │   │    │
│                       │  └──────────────────────┘   │    │
│                       │  ┌──────────────────────┐   │    │
│                       │  │ CircuitBreaker        │   │    │
│                       │  │ (failure counting)   │   │    │
│                       │  └──────────────────────┘   │    │
│                       └────────────────────────────┘    │
│                              │                          │
│                              ▼                          │
│  ┌────────────────────────────────────────────────┐     │
│  │           SecurityGate (existing)               │     │
│  │  check_execution_results() 现有治理不变          │     │
│  └────────────────────────────────────────────────┘     │
│                              │                          │
│                              ▼                          │
│  ┌────────────────────────────────────────────────┐     │
│  │        GovernedAutopilotStepRunner              │     │
│  │  run_step() 现有步骤执行不变                     │     │
│  └────────────────────────────────────────────────┘     │
└─────────────────────────────────────────────────────────┘
```

### 2.2 组件职责

#### AutoModeGate

模式切换的核心 Gate，负责：
1. 进入 Autopilot 时快照当前权限并剥离危险 Capability
2. 退出 Autopilot 时从快照恢复权限
3. 协调 CircuitBreaker 判断是否需要强制退出
4. 发射模式切换事件到 ObservabilityEvent

```rust
#[async_trait]
pub trait AutoModeGate: Send + Sync {
    /// 进入 Autopilot 模式：快照权限 + 剥离危险 Capability
    async fn enter_auto_mode(
        &self,
        execution_id: &ExecutionId,
        config: &AutoModeConfig,
    ) -> anyhow::Result<PermissionSnapshot>;

    /// 退出 Autopilot 模式：恢复权限快照
    async fn exit_auto_mode(
        &self,
        execution_id: &ExecutionId,
        snapshot: &PermissionSnapshot,
        reason: ExitReason,
    ) -> anyhow::Result<()>;

    /// 检查当前是否处于 Auto 模式
    fn is_auto_mode(&self, execution_id: &ExecutionId) -> bool;
}
```

#### DangerousCapabilityFilter

分类和过滤危险 Capability，负责：
1. 维护危险 Capability 规则集
2. 在 Auto 模式下拦截匹配的 Capability 调用
3. 支持基于策略的例外（部分危险 Capability 可配置为允许）

```rust
pub struct DangerousCapabilityFilter {
    /// 危险规则集
    rules: Vec<DangerousRule>,
    /// 策略例外列表
    exceptions: Vec<CapabilityException>,
}

pub struct DangerousRule {
    /// 规则标识
    pub id: String,
    /// 匹配的 Capability 模式（支持通配符）
    pub capability_pattern: String,
    /// 危险等级
    pub severity: DangerSeverity,
    /// 说明
    pub reason: String,
}

pub enum DangerSeverity {
    /// 始终拦截，无论配置
    Critical,
    /// Auto 模式默认拦截，可通过策略例外放行
    High,
    /// Auto 模式记录警告，不拦截
    Medium,
}
```

**默认危险规则集**（对应 Claude Code 的 Bash/PowerShell/Agent 危险规则）：

| 规则 ID | 匹配模式 | 等级 | 说明 |
|---------|----------|------|------|
| `D001` | `shell:*:destructive` | Critical | 破坏性 Shell 命令（rm -rf、drop table 等） |
| `D002` | `shell:*:network` | High | 网络操作（curl POST、wget 等外部调用） |
| `D003` | `connector:*:deploy` | Critical | 部署类 Connector 操作 |
| `D004` | `connector:*:delete` | Critical | 删除类 Connector 操作 |
| `D005` | `agent:*:spawn` | High | 自主 spawn 子 Agent |
| `D006` | `plugin:*:install` | High | 运行时安装 Plugin |
| `D007` | `capability:*:credential` | Critical | 凭证操作（读取/写入/轮转） |

#### CircuitBreaker

连续失败熔断器，负责：
1. 追踪 Autopilot Loop 中连续失败的步骤数
2. 达到阈值时触发强制退出 Auto 模式
3. 支持 half-open 探测（冷却后允许单次重试）

```rust
pub struct CircuitBreaker {
    /// 连续失败触发熔断的阈值（默认 3）
    pub failure_threshold: u32,
    /// 熔断冷却时间
    pub cooldown: Duration,
    /// 当前状态
    state: CircuitState,
}

pub enum CircuitState {
    /// 正常运行
    Closed { consecutive_failures: u32 },
    /// 熔断，拒绝所有执行
    Open { opened_at: Instant },
    /// 冷却后允许单次探测
    HalfOpen,
}
```

---

## 3. 执行流程

### 3.1 进入 Auto 模式

```
用户/系统触发 Autopilot
       │
       ▼
AutoModeGate::enter_auto_mode()
       │
       ├── 1. 快照当前 Capability 权限 → PermissionSnapshot
       ├── 2. DangerousCapabilityFilter 评估所有活跃 Capability
       ├── 3. 剥离 Critical/High 危险 Capability 的执行权限
       ├── 4. 发射 SecurityEvent::AutoModeEntered
       └── 5. 返回 PermissionSnapshot（用于退出时恢复）
```

### 3.2 Auto 模式执行中

```
GovernedLoopRuntime 驱动每轮迭代
       │
       ▼
GovernedAutopilotStepRunner::run_step()
       │
       ├── SecurityGate::check_execution_results() ← 现有治理不变
       │
       ├── DangerousCapabilityFilter::check()
       │   ├── 如果 Capability 匹配 Critical 规则 → 直接拒绝
       │   ├── 如果 Capability 匹配 High 规则且无例外 → 拒绝 + 记录
       │   └── 如果 Capability 匹配 Medium 规则 → 警告 + 放行
       │
       └── CircuitBreaker::record_result()
           ├── 成功 → 重置 consecutive_failures
           └── 失败 → consecutive_failures += 1
               └── if consecutive_failures >= threshold
                   → 触发 AutoModeGate::exit_auto_mode(Reason::CircuitBreak)
```

### 3.3 退出 Auto 模式

```
触发条件（任意一项）：
  - 用户主动退出
  - 任务完成（GoalMet）
  - CircuitBreaker 熔断
  - 预算耗尽
       │
       ▼
AutoModeGate::exit_auto_mode()
       │
       ├── 1. 从 PermissionSnapshot 恢复原始权限
       ├── 2. 重置 CircuitBreaker 状态
       ├── 3. 发射 SecurityEvent::AutoModeExited { reason }
       └── 4. ExecutionMode 切回 Normal
```

---

## 4. 数据模型

### 4.1 配置

```rust
pub struct AutoModeConfig {
    /// 是否启用 Auto 模式（全局开关）
    pub enabled: bool,
    /// 熔断阈值（默认 3 次连续失败）
    pub circuit_breaker_threshold: u32,
    /// 熔断冷却时间（默认 60 秒）
    pub circuit_breaker_cooldown: Duration,
    /// 危险规则例外列表
    pub capability_exceptions: Vec<CapabilityException>,
    /// 最大自动迭代次数（默认 50）
    pub max_auto_iterations: u32,
}
```

### 4.2 权限快照

```rust
pub struct PermissionSnapshot {
    /// 快照创建时间
    pub created_at: Instant,
    /// 被剥离的 Capability 权限列表
    pub stripped_capabilities: Vec<StrippedCapability>,
    /// 原始权限配置的序列化副本
    pub original_config: serde_json::Value,
}

pub struct StrippedCapability {
    /// Capability 标识
    pub capability_id: String,
    /// 匹配的危险规则
    pub matched_rule: String,
    /// 危险等级
    pub severity: DangerSeverity,
}
```

### 4.3 退出原因

```rust
pub enum ExitReason {
    /// 用户主动退出
    UserRequested,
    /// 任务目标达成
    GoalMet,
    /// 熔断器触发
    CircuitBreak { consecutive_failures: u32 },
    /// 迭代次数用尽
    MaxIterationsReached { iterations: u32 },
    /// 预算耗尽
    BudgetExhausted,
    /// 安全门拦截
    SecurityGateBlock { reason: String },
}
```

---

## 5. 与现有模块的集成点

### 5.1 代码落地位置

| 组件 | 落地 crate | 文件 |
|------|-----------|------|
| `AutoModeGate` trait + 默认实现 | `cyberclaw-control-plane` | `auto_mode_gate.rs` (新增) |
| `DangerousCapabilityFilter` | `cyberclaw-governance` | `dangerous_capability_filter.rs` (新增) |
| `CircuitBreaker` | `cyberclaw-control-plane` | `circuit_breaker.rs` (新增) |
| `AutoModeConfig` | `cyberclaw-control-plane` | `autopilot_types.rs` (扩展) |
| `ExitReason` | `cyberclaw-control-plane` | `autopilot_types.rs` (扩展) |
| `PermissionSnapshot` | `cyberclaw-control-plane` | `auto_mode_gate.rs` (新增) |

### 5.2 现有模块改动

| 现有模块 | 改动 | 说明 |
|---------|------|------|
| `execution_service.rs` | 小改 | `execute_autopilot_iteration` 在启动前调用 `AutoModeGate::enter_auto_mode` |
| `governed_step_runner.rs` | 小改 | `run_step` 中插入 `DangerousCapabilityFilter::check` |
| `autopilot_runtime.rs` | 小改 | `GovernedLoopRuntime` 持有 `Arc<dyn AutoModeGate>` |
| `autopilot_types.rs` | 扩展 | 新增 `AutoModeConfig`、`ExitReason` 类型 |
| `lib.rs` | 扩展 | pub mod + re-export 新模块 |

### 5.3 事件与可观察性

所有模式切换和拦截事件应通过现有 `cyberclaw-observability` 发射：

```rust
// 新增事件类型
SecurityEvent::AutoModeEntered {
    execution_id: ExecutionId,
    stripped_count: usize,
    config: AutoModeConfig,
}

SecurityEvent::AutoModeExited {
    execution_id: ExecutionId,
    reason: ExitReason,
    duration: Duration,
    iterations_completed: u32,
}

SecurityEvent::DangerousCapabilityBlocked {
    execution_id: ExecutionId,
    capability_id: String,
    rule_id: String,
    severity: DangerSeverity,
}

SecurityEvent::CircuitBreakerTripped {
    execution_id: ExecutionId,
    consecutive_failures: u32,
    threshold: u32,
}
```

---

## 6. 安全约束

1. **权限剥离不可绕过**：Auto 模式下 DangerousCapabilityFilter 在 SecurityGate 之前执行，Critical 等级无例外机制
2. **快照不可篡改**：PermissionSnapshot 创建后为不可变值，仅 `exit_auto_mode` 消费
3. **熔断不可禁用**：CircuitBreaker 始终启用，仅阈值可配置（最小值为 1）
4. **审计不可跳过**：所有模式切换和拦截事件必须经 ObservabilityEvent 记录
5. **恢复保证**：即使进程崩溃，下次启动时若检测到未恢复的 PermissionSnapshot，自动恢复权限

---

## 7. 非目标

1. 不实现细粒度 per-Agent 权限（留给后续 RBAC 设计）
2. 不实现远程权限同步（单节点范围内）
3. 不替代 ReviewQueue 的人工审批（Gate 拦截后可选转审批）
4. 不实现 UI 层面的模式切换交互（由 CLI/API 层处理）
5. 不与 `Skill` 的 capability 声明耦合（Skill 无执行权限这一约束不变）

---

## 8. 测试策略

| 测试类型 | 覆盖内容 |
|---------|---------|
| 单元测试 | AutoModeGate enter/exit 快照一致性 |
| 单元测试 | DangerousCapabilityFilter 规则匹配（通配符、等级、例外） |
| 单元测试 | CircuitBreaker 状态机转换（Closed→Open→HalfOpen→Closed） |
| 集成测试 | GovernedAutopilotStepRunner + DangerousCapabilityFilter 联合拦截 |
| 集成测试 | CircuitBreaker 触发 → 自动退出 Auto 模式 → 权限恢复 |
| 属性测试 | 任意 enter/exit 序列后权限状态一致 |

---

## 9. 实施阶段

| 阶段 | 内容 | 依赖 |
|------|------|------|
| Phase 1 | `AutoModeGate` trait + 默认实现 + `PermissionSnapshot` | 无 |
| Phase 2 | `DangerousCapabilityFilter` + 默认危险规则集 | Phase 1 |
| Phase 3 | `CircuitBreaker` 状态机 | 无（可与 Phase 2 并行） |
| Phase 4 | 集成到 `GovernedLoopRuntime` + `GovernedAutopilotStepRunner` | Phase 1-3 |
| Phase 5 | ObservabilityEvent 集成 + 审计测试 | Phase 4 |
