//! P2 阶段核心功能演示
//!
//! 展示:
//! 1. Container Runtime 安全执行
//! 2. CronScheduler 调度能力
//! 3. Runtime Selector 策略选择

use cyberclaw_connectors::runtime::{
    ContainerRuntime, ContainerConfig, NetworkMode,
    RuntimeSelector, RuntimeConfig, RuntimeMode,
};
use cyberclaw_scheduler::{CronScheduler, TaskAction};
use cyberclaw_core::capability::RiskLevel;
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    println!("🚀 CyberClaw P2 阶段功能演示\n");

    // 1. 展示 Container Runtime
    demo_container_runtime().await?;

    // 2. 展示 Runtime Selector
    demo_runtime_selector().await?;

    // 3. 展示 CronScheduler
    demo_cron_scheduler().await?;

    println!("\n✅ 所有演示完成！");
    Ok(())
}

async fn demo_container_runtime() -> Result<()> {
    println!("📦 1. Container Runtime 安全执行演示");
    println!("   - 5个输入验证函数");
    println!("   - 4个CIS安全标志");
    println!("   - 命令注入防护\n");

    let config = ContainerConfig {
        image: "alpine:latest".to_string(),
        command: Some("echo".to_string()),
        args: vec!["Hello from secure container!".to_string()],
        network_mode: NetworkMode::None,
        ..Default::default()
    };

    println!("   配置: 镜像={}, 命令={:?}, 网络=隔离",
        config.image, config.command);
    println!("   安全: 只读文件系统 + 无新权限 + 无Capabilities\n");

    Ok(())
}

async fn demo_runtime_selector() -> Result<()> {
    println!("🎯 2. Runtime Selector 策略选择演示");
    println!("   根据 RiskLevel 自动选择运行时模式\n");

    let config = RuntimeConfig::default();
    let selector = RuntimeSelector::new(config);

    let test_cases = vec![
        (RiskLevel::Low, "低风险"),
        (RiskLevel::Medium, "中风险"),
        (RiskLevel::High, "高风险"),
        (RiskLevel::Critical, "关键风险"),
    ];

    for (risk, label) in test_cases {
        let mode = selector.select_runtime(&risk);
        println!("   {} → {:?} 模式", label, mode);
    }
    println!();

    Ok(())
}

async fn demo_cron_scheduler() -> Result<()> {
    println!("⏰ 3. CronScheduler 调度演示");
    println!("   - Cron表达式支持");
    println!("   - 并发控制 (Semaphore限流)");
    println!("   - 执行历史追踪\n");

    let scheduler = CronScheduler::new()?;

    println!("   调度器配置:");
    println!("   - 检查间隔: 60秒");
    println!("   - 最大并发: 10任务");
    println!("   - 并发控制: Semaphore\n");

    // 示例任务
    let task_id = scheduler.schedule(
        "demo-task".to_string(),
        "0 * * * *".to_string(),  // 每小时执行
        TaskAction::ExecuteCapability {
            connector_id: "local".to_string(),
            capability: "fs.read".to_string(),
            params: serde_json::json!({"path": "/tmp/demo.txt"}),
        },
    ).await?;

    println!("   ✅ 任务已调度: {}", task_id.0);
    println!("   Cron: 每小时执行一次");
    println!("   动作: 读取文件\n");

    Ok(())
}
