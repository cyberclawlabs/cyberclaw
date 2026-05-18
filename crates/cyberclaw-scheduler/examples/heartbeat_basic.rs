//! Heartbeat 监控基础示例
//!
//! 演示如何使用 HeartbeatMonitor 监控节点健康状态。
//!
//! 运行方式:
//! ```bash
//! cargo run --example heartbeat_basic
//! ```

use cyberclaw_scheduler::prelude::*;
use std::sync::Arc;
use tokio::time::{sleep, Duration};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 初始化日志
    tracing_subscriber::fmt::init();

    // 创建自定义配置
    let config = HeartbeatConfig {
        interval_secs: 5,       // 每 5 秒检查一次
        timeout_multiplier: 3,  // 15 秒无心跳则标记为离线
        cpu_threshold: 80.0,    // CPU 使用率阈值
        memory_threshold: 85.0, // 内存使用率阈值
        disk_threshold: 90.0,   // 磁盘使用率阈值
    };

    // 创建 HeartbeatMonitor
    let monitor = Arc::new(HeartbeatMonitor::new(config));

    // 注册几个节点
    let node1 = NodeId::from_string("node-1".to_string());
    let node2 = NodeId::from_string("node-2".to_string());
    let node3 = NodeId::from_string("node-3".to_string());

    monitor.register_node(node1.clone()).await?;
    monitor.register_node(node2.clone()).await?;
    monitor.register_node(node3.clone()).await?;

    println!("✓ 已注册 3 个节点");

    // 启动监控器（在后台运行）
    let monitor_clone = monitor.clone();
    tokio::spawn(async move {
        if let Err(e) = monitor_clone.start().await {
            eprintln!("监控器错误: {}", e);
        }
    });

    println!("✓ 监控器已启动");

    // 模拟节点上报心跳
    for i in 0..10 {
        sleep(Duration::from_secs(2)).await;

        // Node 1: 正常运行
        monitor.report_heartbeat(&node1, 50.0, 60.0, 70.0).await?;

        // Node 2: CPU 使用率逐渐升高
        let cpu = 50.0 + (i as f64 * 5.0);
        monitor.report_heartbeat(&node2, cpu, 60.0, 70.0).await?;

        // Node 3: 模拟间歇性心跳（每隔一次才上报）
        if i % 2 == 0 {
            monitor.report_heartbeat(&node3, 50.0, 60.0, 70.0).await?;
        }

        // 打印所有节点状态
        let nodes = monitor.list_nodes().await?;
        println!("\n[Round {}] 节点状态:", i + 1);
        for node in nodes {
            println!(
                "  {} - {:?} (CPU: {:.1}%, Memory: {:.1}%, Disk: {:.1}%)",
                node.id, node.status, node.cpu_usage, node.memory_usage, node.disk_usage
            );
        }
    }

    // 停止监控器
    monitor.stop().await?;
    println!("\n✓ 监控器已停止");

    Ok(())
}
