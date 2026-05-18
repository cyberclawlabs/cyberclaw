# CyberClaw Scheduler

CyberClaw 平台的调度器模块，提供节点健康监控和定时任务调度功能。

## 功能特性

### Heartbeat 监控系统

- **节点注册与管理**: 动态注册和注销节点
- **健康检查**: 基于资源使用率的健康状态评估
- **异常检测**: 
  - 高资源使用率告警
  - 资源使用率突增检测
  - 心跳超时检测
- **状态管理**: 自动更新节点状态（Healthy, Degraded, Unhealthy, Offline）

### Cron 调度器

- **标准 Cron 表达式**: 支持 5 字段格式 (分钟级精度)
- **任务队列管理**: 可靠的任务队列和并发控制
- **执行历史**: 详细的执行历史记录和审计
- **动态管理**: 支持运行时添加、删除、启用、禁用任务
- **故障容错**: 自动处理任务执行失败

## 快速开始

### Cron 调度器

```rust
use cyberclaw_scheduler::prelude::*;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 创建调度器
    let scheduler = CronScheduler::new()?;

    // 添加定时任务
    let task_id = scheduler.schedule(
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

### Heartbeat 监控

```rust
use cyberclaw_scheduler::prelude::*;
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 创建配置
    let config = HeartbeatConfig {
        interval_secs: 30,           // 每 30 秒检查一次
        timeout_multiplier: 3,       // 90 秒无心跳则标记为离线
        cpu_threshold: 80.0,         // CPU 使用率阈值
        memory_threshold: 85.0,      // 内存使用率阈值
        disk_threshold: 90.0,        // 磁盘使用率阈值
    };

    // 创建 HeartbeatMonitor
    let monitor = Arc::new(HeartbeatMonitor::new(config));

    // 注册节点
    let node_id = NodeId::from_string("my-node".to_string());
    monitor.register_node(node_id.clone()).await?;

    // 启动监控器（在后台运行）
    let monitor_clone = monitor.clone();
    tokio::spawn(async move {
        monitor_clone.start().await.unwrap();
    });

    // 上报心跳
    monitor
        .report_heartbeat(&node_id, 50.0, 60.0, 70.0)
        .await?;

    // 获取节点状态
    let status = monitor.get_node_status(&node_id).await?;
    println!("节点状态: {:?}", status);

    Ok(())
}
```

## 配置选项

### HeartbeatConfig

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `interval_secs` | `u64` | 30 | 监控检查间隔（秒） |
| `timeout_multiplier` | `u32` | 3 | 心跳超时倍数（基于 interval） |
| `cpu_threshold` | `f64` | 80.0 | CPU 使用率告警阈值（%） |
| `memory_threshold` | `f64` | 85.0 | 内存使用率告警阈值（%） |
| `disk_threshold` | `f64` | 90.0 | 磁盘使用率告警阈值（%） |

### 节点状态

- **Healthy**: 所有资源使用率正常
- **Degraded**: 部分资源使用率接近阈值
- **Unhealthy**: 资源使用率严重超标
- **Offline**: 心跳超时

### 异常类型

- `HighCpuUsage(f64)`: CPU 使用率超过阈值
- `HighMemoryUsage(f64)`: 内存使用率超过阈值
- `HighDiskUsage(f64)`: 磁盘使用率超过阈值
- `ResourceSpike { resource, value }`: 资源使用率突增
- `HeartbeatIrregular`: 心跳不规律

## 示例

查看 [examples/](examples/) 目录获取更多示例：

- `heartbeat_basic.rs`: Heartbeat 监控基础用法

运行示例:

```bash
cargo run --example heartbeat_basic
```

## 测试

运行单元测试:

```bash
cargo test -p cyberclaw-scheduler
```

当前测试覆盖:

- ✅ NodeId 创建和操作
- ✅ NodeInfo 资源更新
- ✅ HealthChecker 健康状态判断
- ✅ AnomalyDetector 异常检测
- ✅ HeartbeatMonitor 节点注册/注销
- ✅ HeartbeatMonitor 心跳上报
- ✅ HeartbeatMonitor 状态查询
- ✅ HeartbeatMonitor 启动/停止

测试统计: **24 个单元测试 (13 Heartbeat + 11 Cron)，全部通过**

## 安全特性 - P2 安全加固 (2026-03-23)

### 并发控制（SEC-008）

**文件**: `src/cron_scheduler.rs:52-54, 387-398`

**问题**: 原实现无并发限制，恶意用户可以通过调度大量任务导致资源耗尽（DoS 攻击）。

**修复**: 实现 Semaphore-based 并发控制机制：

- 默认最大并发数：**10 个任务**
- 使用 `tokio::sync::Semaphore` 控制并发
- 超出限制的任务自动排队等待
- 可通过 `max_concurrent_executions` 参数配置

**实现代码**:
```rust
pub struct CronScheduler {
    // ... 其他字段
    max_concurrent_executions: usize,
    execution_semaphore: Arc<Semaphore>,
}

impl CronScheduler {
    pub fn new() -> Result<Self> {
        let max = 10;  // 默认值
        Ok(Self {
            max_concurrent_executions: max,
            execution_semaphore: Arc::new(Semaphore::new(max)),
            // ...
        })
    }

    async fn execute_task(&self, task: &Task) -> Result<()> {
        // 获取许可（如果达到上限则等待）
        let permit = self.execution_semaphore.acquire().await?;

        // 执行任务
        let result = self.do_execute(task).await;

        // 自动释放许可
        drop(permit);

        result
    }
}
```

**安全效果**:
```rust
// ❌ DoS 攻击场景
// 恶意用户调度 1000 个 CPU 密集型任务
for i in 0..1000 {
    scheduler.schedule(
        format!("attack-{}", i),
        "* * * * *".to_string(),  // 每分钟执行
        TaskAction::ExecuteCapability {
            connector_id: "compute".to_string(),
            capability: "cpu.intensive".to_string(),
            params: json!({"duration": 3600}),  // 1 小时
        },
    ).await?;
}

// ✅ 防护结果：
// - 只有 10 个任务同时执行
// - 其余 990 个任务在队列中等待
// - 系统资源受到保护
// - 不会发生内存/CPU 耗尽
```

### 其他安全特性

- **任务执行历史**: 完整的审计日志，记录所有任务执行
- **任务隔离**: 每个任务在独立上下文中执行
- **故障容错**: 任务失败不影响调度器整体运行
- **资源监控**: 通过 Heartbeat 系统监控节点资源

### 测试验证

- ✅ 并发控制测试：`test_concurrent_execution`
- ✅ 任务执行历史测试：`test_execution_history`
- ✅ 启动/停止测试：`test_scheduler_start_stop`
- ✅ Clippy 严格模式通过（`-D warnings`）

## 架构设计

详见 [P2 架构设计文档](../../docs/architecture/p2/P2_ARCHITECTURE_DESIGN.md) 第 3.4.1 节。

## 开发状态

- ✅ Heartbeat 监控系统 - **完成**
  - HeartbeatMonitor 核心实现
  - HealthChecker 健康检查
  - AnomalyDetector 异常检测
  - 节点状态管理
  - 13 个单元测试覆盖

- ✅ Cron 调度器 - **完成**
  - CronScheduler 核心实现
  - Cron 表达式解析
  - 任务队列管理
  - 执行历史记录
  - 11 个单元测试覆盖
  - 完整示例和文档

## License

Apache-2.0
