# 安全架构

**最后更新:** 2024-03-18
**范围:** 跨层安全设计
**状态:** 🛡️ 深度防御

## 安全设计原则

### 1. 零信任架构 (Zero Trust)
- 所有输入必须验证，无论来源
- 最小权限原则 (Principle of Least Privilege)
- 默认拒绝 (Deny by Default)

### 2. 深度防御 (Defense in Depth)
- 多层安全控制
- 单层失效不导致系统沦陷
- 每层独立验证

### 3. 安全边界明确
- 清晰的信任边界
- 显式的权限转移点
- 可审计的安全决策

### 4. 失败安全 (Fail-Safe)
- 验证失败 → 操作中止
- 资源耗尽 → 优雅降级
- 异常状态 → 安全模式

## 安全层级架构

```
┌─────────────────────────────────────────────────────────────┐
│                    应用层安全 (L1)                           │
│  职责: 输入验证、业务逻辑验证                                 │
│  ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━  │
│  • ExecutionId/NodeId 格式验证                               │
│  • 配置参数范围验证                                          │
│  • 业务规则一致性检查                                        │
│  • 资源配额验证                                              │
└──────────────────┬──────────────────────────────────────────┘
                   │
┌──────────────────▼──────────────────────────────────────────┐
│                    服务层安全 (L2)                           │
│  职责: 访问控制、并发控制、资源隔离                          │
│  ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━  │
│  • 租约管理与冲突检测 (LeaseManager)                         │
│  • 背压处理与流控 (EventBus)                                 │
│  • 状态一致性验证 (SharedState)                              │
│  • 并发安全保证 (RwLock + 原子操作)                          │
└──────────────────┬──────────────────────────────────────────┘
                   │
┌──────────────────▼──────────────────────────────────────────┐
│                    数据层安全 (L3)                           │
│  职责: 文件系统隔离、路径验证、数据完整性                     │
│  ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━  │
│  • 路径遍历防护 (ArtifactStore)                              │
│  • 符号链接检测与拒绝                                        │
│  • 文件系统边界强制                                          │
│  • 原子操作保证数据一致性                                    │
└──────────────────┬──────────────────────────────────────────┘
                   │
┌──────────────────▼──────────────────────────────────────────┐
│                    监控审计层 (L4)                           │
│  职责: 可观测性、异常检测、安全审计                          │
│  ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━  │
│  • 安全事件日志记录                                          │
│  • 攻击模式识别与告警                                        │
│  • 不可变审计跟踪                                            │
│  • 资源使用度量                                              │
└─────────────────────────────────────────────────────────────┘
```

## 威胁模型与防护设计

### T1: 路径遍历攻击 (Path Traversal)

**威胁描述**
攻击者通过构造特殊路径（如 `../../etc/passwd`）尝试访问文件系统边界外的资源。

**攻击面**
- ArtifactStore 文件操作
- 任何接受路径参数的 API

**防护设计**
```rust
// 1. 路径组件清理
fn sanitize_path_component(component: &str) -> Result<()> {
    if component.contains("..") ||
       component.contains("/") ||
       component.contains("\\") {
        return Err(SecurityError::PathTraversal);
    }
    Ok(())
}

// 2. 路径规范化与边界检查
fn enforce_boundary(path: &Path, base: &Path) -> Result<()> {
    let canonical = path.canonicalize()?;
    if !canonical.starts_with(base) {
        return Err(SecurityError::BoundaryViolation);
    }
    Ok(())
}

// 3. 符号链接拒绝
fn reject_symlinks(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_symlink() {
        return Err(SecurityError::SymlinkNotAllowed);
    }
    Ok(())
}
```

**安全保证**
- 所有路径操作前验证
- 规范化后再次边界检查
- 符号链接一律拒绝

---

### T2: 拒绝服务攻击 (DoS)

**威胁描述**
攻击者通过资源耗尽（内存、CPU、文件描述符）导致系统不可用。

**攻击面**
- EventBus 消息洪泛
- SubagentScheduler 递归生成
- 配置参数极值

**防护设计**
```rust
// 1. 有界资源
pub struct EventBus {
    tx: broadcast::Sender<Event>,  // bounded channel
    config: EventBusConfig,
}

impl EventBus {
    pub fn new(config: EventBusConfig) -> Result<Self> {
        config.validate()?;  // 强制验证
        let (tx, _) = broadcast::channel(config.subscriber_buffer_size);
        Ok(Self { tx, config })
    }
}

// 2. 资源配额
pub struct SubagentBudget {
    pub max_depth: u32,        // 1-20
    pub max_steps: u32,        // ≤ 100,000
    pub max_duration_ms: u64,  // ≤ 10分钟
    pub max_tokens: u32,       // ≤ 1,000,000
    pub max_children: u32,     // ≤ 100
}

// 3. 背压处理
async fn publish_with_backpressure(&self, event: Event) -> Result<()> {
    match self.tx.send(event) {
        Ok(_) => Ok(()),
        Err(broadcast::error::SendError(_)) => {
            // 通道满载，应用背压
            Err(Error::ChannelFull)
        }
    }
}
```

**安全保证**
- 所有通道有界 (bounded)
- 配置参数强制范围验证
- 递归深度硬限制

---

### T3: 竞态条件 (Race Conditions)

**威胁描述**
并发访问共享资源时，检查与使用之间的时间窗口可被利用（TOCTOU）。

**攻击面**
- LeaseManager 租约分配
- SharedState 乐观锁
- ArtifactStore 文件操作

**防护设计**
```rust
// 1. 原子操作（LeaseManager）
pub async fn try_acquire(&self, lease_id: &str, owner_id: &str) -> Result<()> {
    let mut leases = self.leases.write().await;

    // Entry API 确保原子性：检查与插入在同一锁内
    match leases.entry(lease_id.to_string()) {
        Entry::Vacant(e) => {
            e.insert(Lease::new(owner_id));
            Ok(())
        }
        Entry::Occupied(_) => {
            Err(Error::LeaseConflict)
        }
    }
}

// 2. 乐观锁（SharedState）
pub async fn update<F>(&self, key: &str, f: F) -> Result<()>
where
    F: Fn(&str) -> String,
{
    let mut map = self.map.write().await;
    let entry = map.get_mut(key).ok_or(Error::KeyNotFound)?;

    // checked_add 防止版本溢出
    entry.version = entry.version.checked_add(1)
        .ok_or(Error::VersionOverflow)?;
    entry.value = f(&entry.value);
    Ok(())
}

// 3. TOCTOU 消除（ArtifactStore）
pub async fn delete_artifact(&self, exec_id: &ExecutionId) -> Result<()> {
    exec_id.validate()?;  // 验证在同一事务内

    let path = self.build_path(exec_id)?;
    self.enforce_boundary(&path)?;
    self.reject_symlinks(&path)?;

    // 验证后立即删除，最小化窗口
    fs::remove_file(&path).await?;
    Ok(())
}
```

**安全保证**
- 检查-设置操作原子化
- 版本号溢出检测
- TOCTOU 窗口最小化

---

### T4: 输入注入攻击 (Injection)

**威胁描述**
恶意输入绕过验证，注入到日志、命令、查询等上下文。

**攻击面**
- ExecutionId/NodeId 字段
- 日志记录
- 配置参数

**防护设计**
```rust
// 1. 严格的输入验证
pub struct ExecutionId(String);

impl ExecutionId {
    pub fn validate(&self) -> Result<()> {
        // 长度限制
        if self.0.is_empty() || self.0.len() > 128 {
            return Err(Error::InvalidLength);
        }

        // 字符白名单
        for c in self.0.chars() {
            if !c.is_ascii_alphanumeric() &&
               !matches!(c, '-' | '_' | ':') {
                return Err(Error::InvalidCharacter);
            }
        }

        // 路径遍历检测
        if self.0.contains("..") || self.0.contains("/") {
            return Err(Error::PathTraversal);
        }

        Ok(())
    }
}

// 2. NewType 模式强制验证
impl TryFrom<String> for ExecutionId {
    type Error = Error;

    fn try_from(s: String) -> Result<Self> {
        let id = ExecutionId(s);
        id.validate()?;  // 构造时强制验证
        Ok(id)
    }
}

// 3. 配置验证框架
pub trait ValidatedConfig {
    fn validate(&self) -> Result<()>;

    fn validate_range<T: PartialOrd>(
        value: T, min: T, max: T, name: &str
    ) -> Result<()> {
        if value < min || value > max {
            return Err(Error::ConfigOutOfRange {
                field: name.to_string(),
            });
        }
        Ok(())
    }
}
```

**安全保证**
- 所有外部输入强制验证
- NewType 模式防止未验证值传递
- 字符白名单而非黑名单

---

### T5: 整数溢出 (Integer Overflow)

**威胁描述**
整数运算溢出导致版本回绕、资源计数错误等安全问题。

**攻击面**
- SharedState 版本号
- 资源计数器
- 时间戳计算

**防护设计**
```rust
// 1. 检查算术运算
pub fn increment_version(&mut self) -> Result<()> {
    self.version = self.version.checked_add(1)
        .ok_or(Error::VersionOverflow)?;
    Ok(())
}

pub fn add_tokens(&mut self, tokens: u32) -> Result<()> {
    self.tokens_used = self.tokens_used.checked_add(tokens)
        .ok_or(Error::TokenOverflow)?;
    Ok(())
}

// 2. 饱和算术（非安全关键场景）
pub fn increment_metric(&mut self) {
    self.count = self.count.saturating_add(1);
}

// 3. 溢出检测测试
#[test]
fn test_version_overflow() {
    let mut state = SharedState::new();
    state.version = u64::MAX;
    assert!(state.increment_version().is_err());
}
```

**安全保证**
- 所有安全关键运算使用 `checked_*`
- 显式溢出错误处理
- 溢出场景测试覆盖

## 配置安全规范

所有配置结构必须实现 `validate()` 方法，强制执行安全边界：

### 时间相关配置
```rust
pub struct MembershipConfig {
    pub heartbeat_timeout_secs: i64,   // 5-300秒
    pub suspect_timeout_secs: i64,     // 10-600秒
}

impl ValidatedConfig for MembershipConfig {
    fn validate(&self) -> Result<()> {
        Self::validate_range(
            self.heartbeat_timeout_secs, 5, 300,
            "heartbeat_timeout_secs"
        )?;

        // 逻辑约束
        if self.suspect_timeout_secs <= self.heartbeat_timeout_secs {
            return Err(Error::InvalidConfigLogic {
                msg: "suspect_timeout must > heartbeat_timeout".into()
            });
        }

        Ok(())
    }
}
```

### 资源限制配置
```rust
pub struct SubagentConfig {
    pub max_depth: u32,         // 1-20
    pub max_steps: u32,         // ≤ 100,000
    pub max_duration_ms: u64,   // ≤ 600,000 (10分钟)
    pub max_tokens: u32,        // ≤ 1,000,000
    pub max_children: u32,      // ≤ 100
}

pub struct EventBusConfig {
    pub subscriber_buffer_size: usize,  // 10-100,000
}

pub struct LeaseConfig {
    pub default_ttl_secs: i64,  // 10-3600秒
}
```

**验证时机**
- 配置加载时验证
- 配置更新时验证
- 构造函数强制调用 `validate()`

## 安全边界与信任模型

```
┌─────────────────────────────────────────────────────────────┐
│                      信任边界 T0                             │
│                  (外部输入 - 零信任)                         │
└──────────────────┬──────────────────────────────────────────┘
                   │ 严格验证
                   ▼
┌─────────────────────────────────────────────────────────────┐
│                      信任边界 T1                             │
│              (验证后的内部类型 - 部分信任)                   │
│  • ExecutionId (已验证)                                      │
│  • ValidatedConfig (已验证)                                  │
└──────────────────┬──────────────────────────────────────────┘
                   │ 业务逻辑验证
                   ▼
┌─────────────────────────────────────────────────────────────┐
│                      信任边界 T2                             │
│                (内部服务 - 有限信任)                         │
│  • Control Plane Services                                   │
│  • Runtime Layers                                           │
└──────────────────┬──────────────────────────────────────────┘
                   │ 访问控制
                   ▼
┌─────────────────────────────────────────────────────────────┐
│                      信任边界 T3                             │
│              (持久化层 - 可信存储)                           │
│  • PostgreSQL (ACID保证)                                    │
│  • File System (边界强制)                                   │
└─────────────────────────────────────────────────────────────┘
```

**边界转移规则**
- T0→T1: 显式验证（`validate()` + `TryFrom`）
- T1→T2: 业务规则检查（权限、配额、状态）
- T2→T3: 访问控制（路径验证、并发控制）
- 反向传递: 禁止（单向信任）

## 并发安全保证

### 锁策略

```rust
// 1. RwLock 读写分离
pub struct SharedState {
    map: Arc<RwLock<HashMap<String, Entry>>>,  // 细粒度锁
}

// 读操作：允许并发
pub async fn get(&self, key: &str) -> Option<String> {
    let map = self.map.read().await;
    map.get(key).map(|e| e.value.clone())
}

// 写操作：独占锁
pub async fn set(&self, key: &str, value: String) -> Result<()> {
    let mut map = self.map.write().await;
    // 原子操作
    Ok(())
}

// 2. 无锁数据结构 (DashMap)
pub struct Registry {
    agents: DashMap<String, AgentManifest>,  // 并发哈希表
}

// 3. 通道通信代替共享内存
pub struct EventBus {
    tx: broadcast::Sender<Event>,  // MPMC channel
}
```

### 死锁预防

**锁顺序规则**
1. 全局锁 → 局部锁
2. 外层服务锁 → 内层服务锁
3. 同级锁：按字典序

**示例**
```rust
// ❌ 错误：可能死锁
async fn bad_pattern() {
    let a = service_a.lock().await;
    let b = service_b.lock().await;  // Thread 2 可能持有 b 等待 a
}

// ✅ 正确：固定顺序
async fn good_pattern() {
    // 按字典序：先 service_a 后 service_b
    let a = service_a.lock().await;
    let b = service_b.lock().await;
}
```

## 符合性对齐

### OWASP Top 10 2021 对齐

| OWASP 风险 | CyberClaw 控制措施 |
|-----------|-------------------|
| **A01 权限控制失效** | LeaseManager 租约机制 + 治理层权限检查 |
| **A03 注入** | ExecutionId/NodeId 严格验证 + 字符白名单 |
| **A04 不安全设计** | 威胁模型驱动设计 + 深度防御架构 |
| **A05 安全配置错误** | ValidatedConfig + 默认安全配置 |
| **A08 软件数据完整性失效** | 原子操作 + 乐观锁版本控制 |

### CWE Top 25 对齐

| CWE | 名称 | 控制措施 |
|-----|------|---------|
| **CWE-20** | 输入验证不当 | `validate()` 方法 + TryFrom 模式 |
| **CWE-22** | 路径遍历 | `sanitize_path()` + 边界检查 |
| **CWE-78** | OS 命令注入 | 字符白名单（未来 Connector 层） |
| **CWE-190** | 整数溢出 | `checked_add()` + 显式检测 |
| **CWE-362** | 竞态条件 | Entry API + 原子操作 |
| **CWE-400** | 资源耗尽 | 有界通道 + 配额验证 |
| **CWE-502** | 不可信数据反序列化 | Schema 验证（Manifest 加载） |

### Rust 安全保证

| 安全属性 | Rust 机制 | CyberClaw 应用 |
|---------|----------|---------------|
| **内存安全** | 所有权系统 + 借用检查 | 无 `unsafe` 代码（除必要 FFI） |
| **类型安全** | 强类型系统 | NewType 模式 (ExecutionId, NodeId) |
| **并发安全** | Send + Sync trait | 所有共享类型实现 Send+Sync |
| **错误处理** | Result 类型 | `anyhow` + `thiserror` |

## 监控与告警设计

### 安全事件指标

```rust
// Prometheus 指标定义
lazy_static! {
    // 攻击尝试计数
    pub static ref SECURITY_EVENTS: IntCounterVec = register_int_counter_vec!(
        "cyberclaw_security_events_total",
        "Security events by type",
        &["event_type", "severity"]
    ).unwrap();

    // 验证失败率
    pub static ref VALIDATION_FAILURES: IntCounterVec = register_int_counter_vec!(
        "cyberclaw_validation_failures_total",
        "Validation failures by component",
        &["component", "reason"]
    ).unwrap();

    // 资源配额使用率
    pub static ref RESOURCE_QUOTA_USAGE: GaugeVec = register_gauge_vec!(
        "cyberclaw_resource_quota_usage_ratio",
        "Resource quota usage (0.0-1.0)",
        &["resource_type"]
    ).unwrap();
}
```

### 告警规则（Prometheus AlertManager）

```yaml
groups:
  - name: cyberclaw_security
    interval: 30s
    rules:
      # CRITICAL: 路径遍历攻击
      - alert: PathTraversalAttack
        expr: |
          rate(cyberclaw_security_events_total{
            event_type="path_traversal",
            severity="critical"
          }[5m]) > 0.1
        for: 1m
        labels:
          severity: critical
        annotations:
          summary: "Path traversal attack detected"

      # HIGH: 输入验证失败率异常
      - alert: HighValidationFailureRate
        expr: |
          rate(cyberclaw_validation_failures_total[5m]) > 1.0
        for: 5m
        labels:
          severity: high
        annotations:
          summary: "Abnormal validation failure rate"

      # MEDIUM: 资源配额接近上限
      - alert: ResourceQuotaNearLimit
        expr: |
          cyberclaw_resource_quota_usage_ratio > 0.9
        for: 10m
        labels:
          severity: medium
        annotations:
          summary: "Resource quota usage > 90%"
```

### 审计日志规范

```rust
// 安全审计事件
#[derive(Debug, Serialize)]
pub struct SecurityAuditEvent {
    pub timestamp: DateTime<Utc>,
    pub event_id: String,
    pub event_type: SecurityEventType,
    pub actor: Identity,
    pub resource: String,
    pub action: String,
    pub result: AuditResult,
    pub threat_indicators: Vec<ThreatIndicator>,
    pub trace_id: String,
}

#[derive(Debug, Serialize)]
pub enum SecurityEventType {
    ValidationFailure,
    PathTraversal,
    LeaseConflict,
    RateLimitExceeded,
    UnauthorizedAccess,
    ResourceExhaustion,
}

#[derive(Debug, Serialize)]
pub struct ThreatIndicator {
    pub indicator_type: String,  // "malformed_input", "boundary_violation"
    pub confidence: f64,          // 0.0-1.0
    pub evidence: serde_json::Value,
}
```

## 安全演化路线图

### 当前架构 (v2.0)
- ✅ 输入验证框架
- ✅ 路径遍历防护
- ✅ DoS 防护（资源限制）
- ✅ 并发安全保证
- ✅ 深度防御架构

### 未来增强 (v2.1+)

**治理层集成**
- 基于能力的权限模型 (Capability-Based Security)
- 动态策略评估 (OPA Rego)
- 风险评分与审批工作流

**沙箱隔离**
- Agent 进程隔离（容器/WASM）
- 文件系统命名空间
- 网络隔离

**密码学签名**
- 包签名验证
- 审计日志签名
- 端到端加密（敏感数据）

**威胁情报集成**
- IP 信誉检查
- 异常行为检测（ML）
- 自动化响应

## 相关文档

- [治理层架构](./governance.md) - 权限与策略系统
- [控制平面架构](./control-plane.md) - 服务安全设计
- [核心引擎](./core.md) - 安全原语定义
- [Observability](./observability.md) - 安全监控集成

---

**安全联系:** security@cyberclawlabs.com
**更新频率:** 每次架构变更或新威胁识别后
