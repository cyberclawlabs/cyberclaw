//! CyberClaw Cron Scheduler 完整示例
//!
//! 本示例演示如何使用 CyberClaw Cron 调度器来创建、管理和执行定时任务。

use cyberclaw_scheduler::prelude::*;
use std::time::Duration;
use tokio::time::sleep;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 初始化日志
    tracing_subscriber::fmt::init();

    println!("=== CyberClaw Cron Scheduler Example ===\n");

    // 创建调度器
    println!("1. 创建调度器...");
    let scheduler = CronScheduler::with_config(
        10,   // 检查间隔: 10 秒 (用于演示，生产环境建议 60 秒)
        5,    // 最大并发执行数: 5
        100   // 最大历史记录数: 100
    )?;
    println!("   ✓ 调度器创建成功\n");

    // 添加各种任务
    println!("2. 添加定时任务...");

    // 任务 1: 每分钟执行的测试任务
    let task1_id = scheduler.schedule(
        "test-every-minute".to_string(),
        "* * * * *".to_string(),  // 每分钟
        TaskAction::ExecuteCapability {
            connector_id: "test".to_string(),
            capability: "test.ping".to_string(),
            params: serde_json::json!({
                "message": "Hello from every minute task"
            }),
        },
    ).await?;
    println!("   ✓ 任务 1 创建: test-every-minute (ID: {})", task1_id);

    // 任务 2: 每 5 分钟执行的任务
    let task2_id = scheduler.schedule(
        "test-every-5-minutes".to_string(),
        "*/5 * * * *".to_string(),  // 每 5 分钟
        TaskAction::ExecuteCapability {
            connector_id: "test".to_string(),
            capability: "test.sync".to_string(),
            params: serde_json::json!({
                "source": "local",
                "target": "remote"
            }),
        },
    ).await?;
    println!("   ✓ 任务 2 创建: test-every-5-minutes (ID: {})", task2_id);

    // 任务 3: 每小时执行的任务
    let task3_id = scheduler.schedule(
        "test-hourly".to_string(),
        "0 * * * *".to_string(),  // 每小时
        TaskAction::ExecuteAgent {
            agent_id: "test-agent".to_string(),
            task: "Perform hourly health check".to_string(),
        },
    ).await?;
    println!("   ✓ 任务 3 创建: test-hourly (ID: {})\n", task3_id);

    // 列出所有任务
    println!("3. 列出所有任务:");
    let tasks = scheduler.list_tasks().await;
    for task in &tasks {
        println!("   - {} ({})", task.name, task.cron_expr);
        println!("     ID: {}", task.id);
        println!("     状态: {}", if task.enabled { "启用" } else { "禁用" });
        println!("     下次执行: {:?}", task.next_execution);
        println!();
    }

    // 演示任务管理
    println!("4. 演示任务管理...");

    // 禁用任务 2
    println!("   禁用任务 2...");
    scheduler.disable_task(&task2_id).await?;
    let task2 = scheduler.get_task(&task2_id).await?;
    println!("   ✓ 任务 2 状态: {}\n", if task2.enabled { "启用" } else { "禁用" });

    // 启动调度器 (在后台运行)
    println!("5. 启动调度器...");
    let scheduler_clone = scheduler.clone();
    let scheduler_handle = tokio::spawn(async move {
        scheduler_clone.start().await
    });
    println!("   ✓ 调度器已在后台启动\n");

    // 等待一段时间让任务执行
    println!("6. 等待任务执行 (30 秒)...");
    for i in 1..=30 {
        print!("\r   等待中... {} 秒", i);
        sleep(Duration::from_secs(1)).await;
    }
    println!("\n");

    // 查询执行历史
    println!("7. 查询执行历史:");
    let all_history = scheduler.get_all_history().await;
    println!("   总执行次数: {}", all_history.len());

    for record in all_history.iter().take(5) {
        println!("   - 任务 ID: {}", record.task_id);
        println!("     执行 ID: {}", record.execution_id);
        println!("     时间: {}", record.timestamp.format("%Y-%m-%d %H:%M:%S"));
        println!("     耗时: {} ms", record.duration_ms);
        match &record.result {
            ExecutionResult::Success { .. } => println!("     结果: ✓ 成功"),
            ExecutionResult::Failure { error } => println!("     结果: ✗ 失败 - {}", error),
        }
        println!();
    }

    // 重新启用任务 2
    println!("8. 重新启用任务 2...");
    scheduler.enable_task(&task2_id).await?;
    println!("   ✓ 任务 2 已重新启用\n");

    // 取消任务 3
    println!("9. 取消任务 3...");
    scheduler.unschedule(&task3_id).await?;
    println!("   ✓ 任务 3 已取消\n");

    // 最终任务列表
    println!("10. 最终任务列表:");
    let final_tasks = scheduler.list_tasks().await;
    println!("    当前任务数: {}", final_tasks.len());
    for task in &final_tasks {
        println!("    - {} ({})", task.name, if task.enabled { "启用" } else { "禁用" });
    }
    println!();

    // 停止调度器
    println!("11. 停止调度器...");
    scheduler.stop().await;

    // 等待调度器完全停止
    let _ = tokio::time::timeout(Duration::from_secs(5), scheduler_handle).await;
    println!("    ✓ 调度器已停止\n");

    println!("=== 示例完成 ===");

    Ok(())
}
