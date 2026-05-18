# CyberClaw Cron Scheduler Examples

本目录包含 CyberClaw Cron 调度器的示例配置和使用案例。

## 快速开始

```rust
use cyberclaw_scheduler::prelude::*;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 创建调度器
    let scheduler = CronScheduler::new()?;

    // 添加每日备份任务
    scheduler.schedule(
        "daily-backup".to_string(),
        "0 2 * * *".to_string(),  // 每天凌晨 2:00
        TaskAction::ExecuteCapability {
            connector_id: "database".to_string(),
            capability: "db.backup".to_string(),
            params: serde_json::json!({
                "target": "/backup/daily",
                "compress": true
            }),
        },
    ).await?;

    // 启动调度器
    scheduler.start().await?;
    Ok(())
}
```

## 示例配置

### 1. 数据库备份任务

每天凌晨 2 点自动备份数据库。

**Cron 表达式**: `0 2 * * *`

```rust
TaskAction::ExecuteCapability {
    connector_id: "database".to_string(),
    capability: "db.backup".to_string(),
    params: serde_json::json!({
        "target": "/backup/daily",
        "compress": true,
        "retention_days": 30
    }),
}
```

### 2. 健康检查任务

每小时检查一次系统健康状态。

**Cron 表达式**: `0 * * * *`

```rust
TaskAction::ExecuteCapability {
    connector_id: "monitoring".to_string(),
    capability: "health.check".to_string(),
    params: serde_json::json!({
        "endpoints": [
            "http://api.example.com/health",
            "http://db.example.com/health"
        ],
        "timeout_seconds": 30
    }),
}
```

### 3. 状态同步任务

每 5 分钟同步一次状态。

**Cron 表达式**: `*/5 * * * *`

```rust
TaskAction::ExecuteCapability {
    connector_id: "sync".to_string(),
    capability: "state.sync".to_string(),
    params: serde_json::json!({
        "source": "local",
        "target": "remote",
        "incremental": true
    }),
}
```

### 4. 周报任务

每周一上午 9 点发送周报。

**Cron 表达式**: `0 9 * * MON`

```rust
TaskAction::ExecuteAgent {
    agent_id: "reporter".to_string(),
    task: "Generate and send weekly report".to_string(),
}
```

### 5. 月度清理任务

每月 1 号凌晨执行清理。

**Cron 表达式**: `0 0 1 * *`

```rust
TaskAction::ExecuteCapability {
    connector_id: "storage".to_string(),
    capability: "cleanup.old_data".to_string(),
    params: serde_json::json!({
        "retention_months": 6,
        "dry_run": false
    }),
}
```

## Cron 表达式格式

```text
┌───────────── 分钟 (0 - 59)
│ ┌───────────── 小时 (0 - 23)
│ │ ┌───────────── 日 (1 - 31)
│ │ │ ┌───────────── 月 (1 - 12)
│ │ │ │ ┌───────────── 星期 (0 - 6) (0 = 周日)
│ │ │ │ │
* * * * *
```

### 常用表达式示例

| 表达式 | 说明 |
|--------|------|
| `* * * * *` | 每分钟 |
| `*/5 * * * *` | 每 5 分钟 |
| `0 * * * *` | 每小时 |
| `0 0 * * *` | 每天午夜 |
| `0 2 * * *` | 每天凌晨 2:00 |
| `0 9 * * MON` | 每周一上午 9:00 |
| `0 0 1 * *` | 每月 1 号午夜 |
| `0 0 1 1 *` | 每年 1 月 1 号午夜 |
| `30 14 * * FRI` | 每周五下午 2:30 |

## API 使用

### 创建调度器

```rust
// 使用默认配置
let scheduler = CronScheduler::new()?;

// 使用自定义配置
let scheduler = CronScheduler::with_config(
    60,    // 检查间隔 (秒)
    10,    // 最大并发执行数
    1000   // 最大历史记录数
)?;
```

### 添加任务

```rust
let task_id = scheduler.schedule(
    "task-name".to_string(),
    "0 * * * *".to_string(),
    TaskAction::ExecuteCapability {
        connector_id: "test".to_string(),
        capability: "test.action".to_string(),
        params: serde_json::json!({}),
    },
).await?;
```

### 管理任务

```rust
// 禁用任务
scheduler.disable_task(&task_id).await?;

// 启用任务
scheduler.enable_task(&task_id).await?;

// 取消任务
scheduler.unschedule(&task_id).await?;

// 获取任务信息
let task = scheduler.get_task(&task_id).await?;

// 列出所有任务
let tasks = scheduler.list_tasks().await;
```

### 查询执行历史

```rust
// 获取任务的执行历史
let history = scheduler.get_task_history(&task_id).await;

// 获取所有执行历史
let all_history = scheduler.get_all_history().await;
```

### 启动和停止

```rust
// 启动调度器 (阻塞直到停止)
scheduler.start().await?;

// 在后台启动
let scheduler_clone = scheduler.clone();
tokio::spawn(async move {
    scheduler_clone.start().await
});

// 检查运行状态
if scheduler.is_running() {
    println!("Scheduler is running");
}

// 停止调度器
scheduler.stop().await;
```

## 完整示例

参见 `examples/full_example.rs` 获取完整的使用示例。

## 最佳实践

1. **任务幂等性**: 确保任务可以安全地重复执行
2. **超时设置**: 为长时间运行的任务设置合理的超时
3. **错误处理**: 任务失败时有适当的错误处理和告警
4. **资源限制**: 控制并发执行的任务数量
5. **监控日志**: 记录任务执行历史和性能指标
6. **测试验证**: 在生产环境前充分测试 Cron 表达式

## 调度精度

- 最小调度精度: 1 分钟
- 检查间隔: 默认 60 秒 (可配置)
- 调度容差: ±1 分钟

## 注意事项

- Cron 表达式使用 5 字段格式 (不包含秒)
- 时区默认使用 UTC
- 任务执行失败不会自动重试 (需在任务内部实现)
- 历史记录数量有限制，超过后会自动清理最旧的记录
