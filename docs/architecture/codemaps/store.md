# 存储层架构

**最后更新:** 2024-03-18
**包路径:** `crates/cyberclaw-store/`
**状态:** 🚧 规划中

## 存储层概览

```
Storage Layer
├── State Store        - 状态存储
├── Artifact Store     - 工件存储
├── Event Store        - 事件存储
├── Audit Store        - 审计存储
└── Cache Layer        - 缓存层
```

## 架构定位

```
┌─────────────────────────────────────────────┐
│       Application Components                 │
│  • Control Plane • Runtime • Governance     │
└────────────────┬────────────────────────────┘
                 │
                 │ 存储接口 (Storage Traits)
                 │
┌────────────────▼────────────────────────────┐
│         Storage Layer (本层)                 │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  │
│  │  State   │  │ Artifact │  │  Event   │  │
│  │  Store   │  │  Store   │  │  Store   │  │
│  └─────┬────┘  └─────┬────┘  └─────┬────┘  │
│        │             │             │        │
│  ┌─────▼────┐  ┌─────▼────┐  ┌─────▼────┐  │
│  │  Audit   │  │  Cache   │  │ Memory   │  │
│  │  Store   │  │  Layer   │  │  Store   │  │
│  └─────┬────┘  └─────┬────┘  └─────┬────┘  │
└────────┼─────────────┼─────────────┼────────┘
         │             │             │
┌────────▼─────────────▼─────────────▼────────┐
│           Backend Implementations            │
│  • File System  • PostgreSQL  • Redis       │
│  • S3/MinIO     • RocksDB     • In-Memory   │
└─────────────────────────────────────────────┘
```

## 1. State Store (状态存储)

### 设计目标

```
功能职责：
├── 元数据存储
│   ├── Task 元数据
│   ├── Case 元数据
│   ├── Agent 状态
│   └── Workflow 状态
│
├── 关系数据
│   ├── 任务 ↔ 案例关联
│   ├── 任务 ↔ Agent 关联
│   ├── 执行树父子关系
│   └── 审批流关联
│
├── ACID 保证
│   ├── 原子性 (Atomicity)
│   ├── 一致性 (Consistency)
│   ├── 隔离性 (Isolation)
│   └── 持久性 (Durability)
│
└── 查询能力
    ├── 按 ID 查询
    ├── 按状态过滤
    ├── 按时间范围查询
    └── 复杂关联查询
```

### 数据模型

```sql
-- Tasks 表
CREATE TABLE tasks (
    id VARCHAR(128) PRIMARY KEY,
    case_id VARCHAR(128),
    agent_id VARCHAR(128) NOT NULL,
    title TEXT NOT NULL,
    description TEXT,
    status VARCHAR(32) NOT NULL,
    priority VARCHAR(32) NOT NULL,
    execution_id VARCHAR(128),
    input JSONB,
    output JSONB,
    error TEXT,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    completed_at TIMESTAMPTZ,
    created_by VARCHAR(255),
    FOREIGN KEY (case_id) REFERENCES cases(id)
);

CREATE INDEX idx_tasks_status ON tasks(status);
CREATE INDEX idx_tasks_case_id ON tasks(case_id);
CREATE INDEX idx_tasks_created_at ON tasks(created_at DESC);

-- Cases 表
CREATE TABLE cases (
    id VARCHAR(128) PRIMARY KEY,
    title TEXT NOT NULL,
    description TEXT,
    status VARCHAR(32) NOT NULL,
    priority VARCHAR(32),
    metadata JSONB,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    closed_at TIMESTAMPTZ,
    created_by VARCHAR(255)
);

-- Executions 表 (执行树)
CREATE TABLE executions (
    id VARCHAR(128) PRIMARY KEY,
    parent_id VARCHAR(128),
    task_id VARCHAR(128) NOT NULL,
    agent_id VARCHAR(128) NOT NULL,
    depth INT NOT NULL,
    status VARCHAR(32) NOT NULL,
    budget JSONB,
    budget_used JSONB,
    artifacts JSONB,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    completed_at TIMESTAMPTZ,
    FOREIGN KEY (parent_id) REFERENCES executions(id),
    FOREIGN KEY (task_id) REFERENCES tasks(id)
);

-- Reviews 表 (审批流)
CREATE TABLE reviews (
    id VARCHAR(128) PRIMARY KEY,
    task_id VARCHAR(128) NOT NULL,
    capability_id VARCHAR(128) NOT NULL,
    risk VARCHAR(32) NOT NULL,
    status VARCHAR(32) NOT NULL,
    requester VARCHAR(255) NOT NULL,
    reason TEXT,
    context JSONB,
    decision VARCHAR(32),
    reviewer VARCHAR(255),
    comment TEXT,
    created_at TIMESTAMPTZ NOT NULL,
    decided_at TIMESTAMPTZ,
    timeout_at TIMESTAMPTZ,
    FOREIGN KEY (task_id) REFERENCES tasks(id)
);

CREATE INDEX idx_reviews_status ON reviews(status);
CREATE INDEX idx_reviews_task_id ON reviews(task_id);
```

### 存储接口（规划）

```rust
#[async_trait]
pub trait StateStore: Send + Sync {
    // Task 操作
    async fn create_task(&self, task: Task) -> Result<()>;
    async fn get_task(&self, id: &TaskId) -> Result<Option<Task>>;
    async fn update_task(&self, id: &TaskId, updates: TaskUpdate) -> Result<()>;
    async fn list_tasks(&self, filter: TaskFilter) -> Result<Vec<Task>>;

    // Case 操作
    async fn create_case(&self, case: Case) -> Result<()>;
    async fn get_case(&self, id: &CaseId) -> Result<Option<Case>>;
    async fn update_case(&self, id: &CaseId, updates: CaseUpdate) -> Result<()>;
    async fn list_cases(&self, filter: CaseFilter) -> Result<Vec<Case>>;

    // Execution 操作
    async fn create_execution(&self, exec: Execution) -> Result<()>;
    async fn get_execution(&self, id: &ExecutionId) -> Result<Option<Execution>>;
    async fn get_execution_tree(&self, root_id: &ExecutionId) -> Result<ExecutionTree>;

    // Review 操作
    async fn create_review(&self, review: Review) -> Result<()>;
    async fn get_review(&self, id: &ReviewId) -> Result<Option<Review>>;
    async fn update_review(&self, id: &ReviewId, decision: ReviewDecision) -> Result<()>;
    async fn list_pending_reviews(&self) -> Result<Vec<Review>>;
}
```

## 2. Artifact Store (工件存储)

**注意:** 已在 Control Plane 实现，见 [控制平面文档](./control-plane.md#1-artifactstore-工件存储)

### 现有实现

```
功能职责：
├── 工件存储 (已实现)
│   ├── 文件系统存储
│   ├── 路径遍历防护 [C-1]
│   ├── 符号链接检测 [H-3]
│   └── 原子操作
│
└── 清理策略 (已实现)
    ├── 基于年龄清理
    ├── 基于大小清理
    └── 手动清理
```

### 未来增强（规划）

```
扩展存储后端：
├── 本地文件系统 (已实现)
├── S3 / MinIO
├── Azure Blob Storage
└── Google Cloud Storage

工件类型：
├── 代码扫描结果 (JSON)
├── 生成的报告 (PDF, HTML)
├── 日志文件 (TXT)
├── 截图 (PNG, JPG)
└── 二进制文件 (ZIP, TAR.GZ)

安全增强：
├── 加密存储 (AES-256)
├── 访问控制 (签名 URL)
├── 版本控制
└── 去重存储 (Content-Addressed)
```

## 3. Event Store (事件存储)

### 设计目标

```
功能职责：
├── 事件溯源 (Event Sourcing)
│   ├── 记录所有状态变更
│   ├── 事件不可变
│   ├── 时间顺序保证
│   └── 重放能力
│
├── 事件类型
│   ├── TaskCreated
│   ├── TaskStatusChanged
│   ├── AgentStarted
│   ├── CapabilityInvoked
│   ├── ApprovalRequested
│   └── ApprovalDecided
│
└── 查询能力
    ├── 按聚合根查询 (task_id)
    ├── 按事件类型过滤
    ├── 时间范围查询
    └── 流式订阅
```

### 事件模型

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredEvent {
    pub id: String,
    pub aggregate_id: String,      // task_id, case_id, execution_id
    pub aggregate_type: String,    // Task, Case, Execution
    pub event_type: String,        // TaskCreated, StatusChanged
    pub event_data: serde_json::Value,
    pub metadata: EventMetadata,
    pub sequence: i64,             // 聚合根内的序列号
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventMetadata {
    pub user_id: Option<String>,
    pub agent_id: Option<String>,
    pub correlation_id: String,
    pub causation_id: Option<String>,
}
```

### 事件存储示例

```sql
CREATE TABLE events (
    id VARCHAR(128) PRIMARY KEY,
    aggregate_id VARCHAR(128) NOT NULL,
    aggregate_type VARCHAR(64) NOT NULL,
    event_type VARCHAR(128) NOT NULL,
    event_data JSONB NOT NULL,
    metadata JSONB NOT NULL,
    sequence BIGINT NOT NULL,
    timestamp TIMESTAMPTZ NOT NULL,
    UNIQUE (aggregate_id, sequence)
);

CREATE INDEX idx_events_aggregate ON events(aggregate_id, sequence);
CREATE INDEX idx_events_type ON events(event_type);
CREATE INDEX idx_events_timestamp ON events(timestamp DESC);
```

### 存储接口（规划）

```rust
#[async_trait]
pub trait EventStore: Send + Sync {
    // 追加事件
    async fn append(&self, events: Vec<StoredEvent>) -> Result<()>;

    // 读取事件流
    async fn read_stream(&self, aggregate_id: &str) -> Result<Vec<StoredEvent>>;
    async fn read_from_sequence(&self, aggregate_id: &str, from: i64) -> Result<Vec<StoredEvent>>;

    // 查询事件
    async fn query_by_type(&self, event_type: &str) -> Result<Vec<StoredEvent>>;
    async fn query_range(&self, start: DateTime<Utc>, end: DateTime<Utc>) -> Result<Vec<StoredEvent>>;

    // 订阅事件流
    async fn subscribe(&self) -> Result<EventStream>;
}
```

## 4. Audit Store (审计存储)

### 设计目标

```
功能职责：
├── 审计日志存储
│   ├── 不可变记录
│   ├── 防篡改
│   ├── 长期保留 (1-7 年)
│   └── 合规性报告
│
├── 审计事件类型
│   ├── 认证 (Authentication)
│   ├── 授权 (Authorization)
│   ├── 策略评估 (Policy)
│   ├── 审批决策 (Approval)
│   └── 能力调用 (Capability)
│
└── 查询能力
    ├── 按用户查询
    ├── 按资源查询
    ├── 按时间范围查询
    └── 全文搜索
```

### 审计模型

```sql
CREATE TABLE audit_logs (
    id VARCHAR(128) PRIMARY KEY,
    timestamp TIMESTAMPTZ NOT NULL,
    event_type VARCHAR(64) NOT NULL,
    actor_id VARCHAR(255) NOT NULL,
    actor_type VARCHAR(32) NOT NULL,  -- user, agent, service
    resource VARCHAR(255) NOT NULL,
    action VARCHAR(128) NOT NULL,
    result VARCHAR(32) NOT NULL,      -- success, failure, blocked
    metadata JSONB NOT NULL,
    trace_id VARCHAR(128),
    ip_address INET,
    user_agent TEXT
);

CREATE INDEX idx_audit_timestamp ON audit_logs(timestamp DESC);
CREATE INDEX idx_audit_actor ON audit_logs(actor_id);
CREATE INDEX idx_audit_resource ON audit_logs(resource);
CREATE INDEX idx_audit_event_type ON audit_logs(event_type);

-- 防篡改：审计日志不允许 UPDATE/DELETE
-- 通过数据库权限或触发器强制执行
```

### 存储接口（规划）

```rust
#[async_trait]
pub trait AuditStore: Send + Sync {
    // 追加审计日志 (只追加,不允许修改)
    async fn append(&self, log: AuditLog) -> Result<()>;

    // 查询审计日志
    async fn query(&self, filter: AuditFilter) -> Result<Vec<AuditLog>>;

    // 生成合规报告
    async fn generate_report(&self, period: DateRange) -> Result<ComplianceReport>;

    // 导出审计日志 (用于归档)
    async fn export(&self, period: DateRange, format: ExportFormat) -> Result<Vec<u8>>;
}
```

## 5. Cache Layer (缓存层)

### 设计目标

```
功能职责：
├── 热点数据缓存
│   ├── Agent Spec (manifest)
│   ├── Skill 内容
│   ├── Policy 规则
│   └── 用户权限
│
├── 分布式缓存
│   ├── Redis (推荐)
│   ├── Memcached
│   └── In-Memory (单机)
│
└── 缓存策略
    ├── TTL (Time-To-Live)
    ├── LRU (Least Recently Used)
    └── 主动失效
```

### 缓存接口（规划）

```rust
#[async_trait]
pub trait CacheLayer: Send + Sync {
    // 基础操作
    async fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>>;
    async fn set<T: Serialize>(&self, key: &str, value: &T, ttl: Option<Duration>) -> Result<()>;
    async fn delete(&self, key: &str) -> Result<()>;

    // 批量操作
    async fn get_many<T: DeserializeOwned>(&self, keys: &[String]) -> Result<Vec<Option<T>>>;
    async fn set_many<T: Serialize>(&self, items: &[(String, T)], ttl: Option<Duration>) -> Result<()>;

    // 高级操作
    async fn exists(&self, key: &str) -> Result<bool>;
    async fn expire(&self, key: &str, ttl: Duration) -> Result<()>;
    async fn invalidate_pattern(&self, pattern: &str) -> Result<()>;
}
```

### 缓存键命名规范

```
缓存键格式: {namespace}:{resource}:{id}

示例:
agent:spec:security-scanner
skill:content:static-analysis
policy:rule:high-risk-approval
user:permissions:user@example.com
task:metadata:task-123
```

### 缓存策略

| 数据类型 | TTL | 失效策略 |
|----------|-----|----------|
| Agent Spec | 1 hour | 包更新时失效 |
| Skill 内容 | 1 hour | 包更新时失效 |
| Policy 规则 | 5 min | 策略变更时失效 |
| 用户权限 | 15 min | 角色变更时失效 |
| Task 元数据 | 1 min | 状态变更时失效 |

## 6. Memory Store (内存存储)

### 设计目标

```
功能职责：
├── Agent 会话内存
│   ├── 对话历史
│   ├── 上下文状态
│   └── 临时变量
│
├── 工作流状态
│   ├── 步骤状态机
│   ├── 中间结果
│   └── 等待队列
│
└── 实现方式
    ├── In-Memory (短期)
    ├── Redis (持久化)
    └── 混合模式
```

### 内存模型（规划）

```rust
pub struct SessionMemory {
    pub session_id: String,
    pub agent_id: String,
    pub conversation_history: Vec<Message>,
    pub context: HashMap<String, Value>,
    pub artifacts: Vec<ArtifactRef>,
    pub created_at: DateTime<Utc>,
    pub last_accessed: DateTime<Utc>,
}

pub struct WorkflowState {
    pub workflow_id: String,
    pub current_step: String,
    pub completed_steps: Vec<String>,
    pub step_outputs: HashMap<String, Value>,
    pub waiting_for: Option<WaitCondition>,
}
```

## 存储后端实现

### 1. PostgreSQL (推荐用于生产)

```
优势：
├── ACID 事务保证
├── 复杂查询能力
├── JSONB 支持
└── 成熟生态

使用场景：
├── State Store (主存储)
├── Event Store (事件溯源)
└── Audit Store (审计日志)
```

### 2. File System (本地开发)

```
优势：
├── 零依赖
├── 简单快速
└── 易于调试

使用场景：
├── Artifact Store (已实现)
├── 开发环境
└── 小规模部署
```

### 3. Redis (分布式场景)

```
优势：
├── 高性能
├── 分布式支持
├── 数据结构丰富
└── 发布订阅

使用场景：
├── Cache Layer
├── Memory Store (会话)
└── Event Bus (消息队列)
```

### 4. S3/MinIO (对象存储)

```
优势：
├── 无限扩展
├── 低成本
├── HTTP API
└── 多云兼容

使用场景：
├── Artifact Store (大文件)
├── Audit Store (归档)
└── 日志归档
```

## 存储配置

```yaml
storage:
  # State Store
  state:
    backend: postgresql
    connection: postgres://user:pass@localhost/cyberclaw
    pool_size: 20

  # Artifact Store
  artifacts:
    backend: filesystem
    path: /var/lib/cyberclaw/artifacts
    max_file_size_mb: 100
    retention_days: 90

  # Event Store
  events:
    backend: postgresql
    table: events
    batch_size: 100

  # Audit Store
  audit:
    backend: postgresql
    table: audit_logs
    retention_years: 7
    export_enabled: true

  # Cache Layer
  cache:
    backend: redis
    url: redis://localhost:6379
    ttl_seconds: 3600
    max_memory_mb: 1024

  # Memory Store
  memory:
    backend: redis
    url: redis://localhost:6379
    session_ttl_minutes: 60
```

## 数据迁移

```bash
# 数据库迁移工具 (使用 sqlx/diesel)
cyberclaw-migrate up         # 应用所有迁移
cyberclaw-migrate down       # 回滚最后一次迁移
cyberclaw-migrate status     # 查看迁移状态
cyberclaw-migrate create <name>  # 创建新迁移
```

## 备份与恢复

```bash
# 数据库备份
pg_dump -h localhost -U user cyberclaw > backup.sql

# 工件备份
tar -czf artifacts-backup.tar.gz /var/lib/cyberclaw/artifacts/

# S3 同步备份
aws s3 sync /var/lib/cyberclaw/artifacts/ s3://cyberclaw-backup/artifacts/

# 恢复
psql -h localhost -U user cyberclaw < backup.sql
```

## 性能优化

### 数据库优化

```sql
-- 添加索引
CREATE INDEX CONCURRENTLY idx_tasks_user ON tasks(created_by);

-- 分区表 (按时间分区)
CREATE TABLE tasks_2024_03 PARTITION OF tasks
FOR VALUES FROM ('2024-03-01') TO ('2024-04-01');

-- 物化视图 (报表查询)
CREATE MATERIALIZED VIEW task_stats AS
SELECT
    date_trunc('day', created_at) as day,
    status,
    count(*) as count
FROM tasks
GROUP BY day, status;
```

### 缓存优化

```rust
// 缓存穿透: 空值也缓存
if let Some(task) = cache.get(&task_id).await? {
    return Ok(task);
}

let task = db.get_task(&task_id).await?;

// 即使为 None 也缓存 (防止缓存穿透)
cache.set(&task_id, &task, Some(Duration::from_secs(60))).await?;
```

## 未来扩展

### v2.1 规划
- [ ] PostgreSQL State Store MVP
- [ ] File System Artifact Store (已实现)
- [ ] 基础缓存层 (In-Memory)

### v2.2 规划
- [ ] Redis Cache Layer
- [ ] Event Store (PostgreSQL)
- [ ] 数据库迁移工具

### v2.3 规划
- [ ] S3/MinIO Artifact Store
- [ ] 分布式 Memory Store
- [ ] 审计日志归档

## 相关文档

- [控制平面](./control-plane.md) - ArtifactStore, SharedState
- [治理层](./governance.md) - 审计日志
- [可观测层](./observability.md) - 日志存储

---

**维护说明:** 存储层目前处于脚手架阶段，ArtifactStore 已在控制平面实现。
