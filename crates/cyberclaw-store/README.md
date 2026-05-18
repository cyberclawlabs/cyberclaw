# cyberclaw-store

CyberClaw 平台的持久化存储层，提供统一的状态存储抽象，支持多种后端实现。

## 特性

- **统一抽象**: 通过 `StateStore` trait 提供一致的 API
- **多后端支持**:
  - PostgreSQL（生产环境，feature: `postgres`）
  - 内存存储（测试和开发，默认包含）
- **完整 CRUD**: 支持 Execution、Artifact、AuditLog、Policy 的完整生命周期管理
- **异步优先**: 基于 Tokio 的完全异步 API
- **类型安全**: 强类型记录结构和错误处理

## 架构设计

### 数据模型

```rust
ExecutionRecord     // 执行记录（Agent 任务执行状态）
ArtifactRecord      // 产物记录（执行生成的文件、日志等）
AuditLogRecord      // 审计日志（操作追踪）
PolicyRecord        // 策略记录（治理规则）
```

### 存储实现

1. **PostgresStateStore** (`feature = "postgres"`)
   - 基于 `sqlx` 的 PostgreSQL 实现
   - 支持连接池和事务
   - 自动 Schema 迁移（refinery）

2. **InMemoryStateStore** (默认包含)
   - 基于 `HashMap` 的内存实现
   - 适用于测试和开发
   - 零依赖启动

## 使用示例

### 内存存储（测试/开发）

```rust
use cyberclaw_store::{StateStore, InMemoryStateStore, ExecutionRecord};
use uuid::Uuid;
use chrono::Utc;
use serde_json::json;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 创建内存存储
    let store = InMemoryStateStore::new();

    // 保存执行记录
    let record = ExecutionRecord {
        id: Uuid::new_v4(),
        agent_id: "agent-1".to_string(),
        skill_id: Some("skill-a".to_string()),
        status: "running".to_string(),
        input: json!({"task": "example"}),
        output: None,
        error: None,
        started_at: Utc::now(),
        completed_at: None,
    };

    store.save_execution(record).await?;

    // 查询执行记录
    let executions = store.list_executions(None, 10, 0).await?;
    println!("Found {} executions", executions.len());

    Ok(())
}
```

### PostgreSQL 存储（生产环境）

```rust
use cyberclaw_store::{StateStore, PostgresStateStore};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 连接 PostgreSQL
    let database_url = "postgresql://user:password@localhost/cyberclaw";
    let store = PostgresStateStore::new(database_url).await?;

    // 执行数据库迁移
    store.run_migrations().await?;

    // 使用相同的 StateStore API
    let executions = store.list_executions(None, 10, 0).await?;
    println!("Found {} executions", executions.len());

    Ok(())
}
```

### 完整工作流示例

```rust
use cyberclaw_store::*;
use uuid::Uuid;
use chrono::Utc;
use serde_json::json;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let store = InMemoryStateStore::new();

    // 1. 创建执行记录
    let exec_id = Uuid::new_v4();
    let execution = ExecutionRecord {
        id: exec_id,
        agent_id: "agent-1".to_string(),
        skill_id: Some("data-processing".to_string()),
        status: "running".to_string(),
        input: json!({"file": "data.csv"}),
        output: None,
        error: None,
        started_at: Utc::now(),
        completed_at: None,
    };
    store.save_execution(execution).await?;

    // 2. 记录审计日志
    let audit_log = AuditLogRecord {
        id: Uuid::new_v4(),
        execution_id: Some(exec_id),
        event_type: "execution.started".to_string(),
        actor: Some("system".to_string()),
        action: "create".to_string(),
        resource: Some(format!("execution:{}", exec_id)),
        details: Some(json!({"agent": "agent-1"})),
        timestamp: Utc::now(),
    };
    store.save_audit_log(audit_log).await?;

    // 3. 保存产物
    let artifact = ArtifactRecord {
        id: Uuid::new_v4(),
        execution_id: exec_id,
        artifact_type: "output_file".to_string(),
        data: json!({"path": "/tmp/result.json", "size": 1024}),
        metadata: Some(json!({"format": "json"})),
    };
    store.save_artifact(artifact).await?;

    // 4. 更新执行状态
    store.update_execution(
        exec_id,
        "completed".to_string(),
        Some(json!({"status": "success", "records_processed": 1000})),
        None,
    ).await?;

    // 5. 查询完整结果
    let final_execution = store.get_execution(exec_id).await?;
    let artifacts = store.list_artifacts(exec_id).await?;
    let logs = store.list_audit_logs(Some(exec_id), 100, 0).await?;

    println!("Execution: {:?}", final_execution);
    println!("Artifacts: {}", artifacts.len());
    println!("Audit logs: {}", logs.len());

    Ok(())
}
```

## 数据库 Schema

### Executions 表

```sql
CREATE TABLE executions (
    id UUID PRIMARY KEY,
    agent_id VARCHAR(255) NOT NULL,
    skill_id VARCHAR(255),
    status VARCHAR(50) NOT NULL,
    input JSONB NOT NULL,
    output JSONB,
    error TEXT,
    started_at TIMESTAMPTZ NOT NULL,
    completed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
```

### Artifacts 表

```sql
CREATE TABLE artifacts (
    id UUID PRIMARY KEY,
    execution_id UUID NOT NULL REFERENCES executions(id) ON DELETE CASCADE,
    artifact_type VARCHAR(100) NOT NULL,
    data JSONB NOT NULL,
    metadata JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
```

### Audit Logs 表

```sql
CREATE TABLE audit_logs (
    id UUID PRIMARY KEY,
    execution_id UUID REFERENCES executions(id) ON DELETE SET NULL,
    event_type VARCHAR(100) NOT NULL,
    actor VARCHAR(255),
    action VARCHAR(255) NOT NULL,
    resource VARCHAR(255),
    details JSONB,
    timestamp TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
```

### Policies 表

```sql
CREATE TABLE policies (
    id UUID PRIMARY KEY,
    name VARCHAR(255) NOT NULL UNIQUE,
    effect VARCHAR(10) NOT NULL CHECK (effect IN ('allow', 'deny')),
    conditions JSONB NOT NULL,
    active BOOLEAN NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
```

## 配置

### Feature Flags

- `default = ["postgres"]`: 默认启用 PostgreSQL 支持
- `postgres`: 启用 PostgreSQL 后端（依赖 sqlx + refinery）
- `memory-only`: 仅使用内存存储（移除 PostgreSQL 依赖）

### 环境变量

```bash
# PostgreSQL 连接 URL
DATABASE_URL=postgresql://user:password@localhost:5432/cyberclaw

# 连接池配置
DATABASE_MAX_CONNECTIONS=20
DATABASE_MIN_CONNECTIONS=5
```

## 测试

```bash
# 运行所有测试（包括内存存储）
cargo test -p cyberclaw-store

# 仅测试内存存储实现
cargo test -p cyberclaw-store --no-default-features

# 测试 PostgreSQL 实现（需要运行的 PostgreSQL 实例）
DATABASE_URL=postgresql://localhost/cyberclaw_test cargo test -p cyberclaw-store --features postgres
```

## 迁移管理

Schema 迁移文件位于 `migrations/` 目录：

```
migrations/
├── V1__initial_schema.sql       # 初始表结构
├── V2__add_indexes.sql          # 索引优化（未来）
└── V3__add_metadata.sql         # 扩展字段（未来）
```

执行迁移：

```rust
let store = PostgresStateStore::new(&database_url).await?;
store.run_migrations().await?;
```

## 性能考虑

1. **连接池**: PostgreSQL 实现使用 sqlx 连接池，默认最大 20 个连接
2. **索引**: 关键字段已建立索引（agent_id, status, created_at, timestamp）
3. **JSONB**: input/output/details 使用 JSONB 类型，支持高效查询和索引
4. **分页**: list_* 方法支持 limit/offset 分页，避免大量数据查询
5. **级联删除**: artifacts 表使用 ON DELETE CASCADE，自动清理相关记录

## 未来扩展

- [ ] 实现 refinery 迁移逻辑
- [ ] 添加连接池配置参数
- [ ] 支持事务操作
- [ ] 添加批量操作 API
- [ ] 实现软删除机制
- [ ] 支持全文搜索（JSONB GIN 索引）
- [ ] 添加数据归档策略

## 许可证

Apache-2.0
