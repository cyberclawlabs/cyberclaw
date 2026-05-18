#![allow(clippy::duplicate_mod)]
//! E2E 测试：Agent 执行完整流程
//!
//! 测试场景：
//! - Agent 列表查询
//! - Agent 调用和执行
//! - Agent 状态跟踪
//! - 并发 Agent 执行
//! - Agent 错误处理

#[path = "common/mod.rs"]
mod common;
use serde_json::json;

use common::assertions::*;
use common::{TestDataGenerator, TestServer};

/// E2E-AGENT-001: Agent 列表查询
#[tokio::test]
async fn test_e2e_agent_001_list_agents() {
    let mut server = TestServer::new();

    // 查询 agents 列表
    let response = server.get("/api/v1/agents").await;

    // 验证响应
    assert_success(&response);

    let json = response.json();
    assert!(json.is_array(), "Response should be an array");

    println!("✅ E2E-AGENT-001: Agent 列表查询测试通过");
}

/// E2E-AGENT-002: Agent 状态查询
#[tokio::test]
async fn test_e2e_agent_002_get_agent_status() {
    let mut server = TestServer::new();

    // Contract (apps/cyberclaw-server/src/api/agents.rs:711-715): GET status
    // returns 404 for unregistered agents. Functional happy-path is covered by
    // scripts/testing/smoke-business.sh which goes through agent creation.
    let response = server.get("/api/v1/agents/test-agent/status").await;
    assert_eq!(
        response.status.as_u16(),
        404,
        "unknown agent must return 404 per current contract"
    );

    println!("✅ E2E-AGENT-002: Agent 状态查询 404 路径测试通过");
}

/// E2E-AGENT-003: Agent 调用和执行
#[tokio::test]
async fn test_e2e_agent_003_invoke_agent() {
    let mut server = TestServer::new();

    // 调用 agent
    let request = TestDataGenerator::invoke_agent_request("Analyze this data");
    let response = server
        .post("/api/v1/agents/test-agent/invoke", request)
        .await;

    // 验证响应
    assert_success(&response);
    assert_json_field(&response, "agent_id");
    assert_json_field(&response, "success");
    assert_json_field(&response, "output");

    let json = response.json();
    assert_eq!(json["agent_id"], "test-agent");

    println!("✅ E2E-AGENT-003: Agent 调用和执行测试通过");
}

/// E2E-AGENT-004: Agent 完整执行流程（状态跟踪）
#[tokio::test]
async fn test_e2e_agent_004_full_execution_flow() {
    let mut server = TestServer::new();

    let agent_name = "flow-test-agent";

    // 1. 初始状态：agent 尚未注册 → 404（contract: agents.rs:711-715）
    let pre_status = server
        .get(&format!("/api/v1/agents/{}/status", agent_name))
        .await;
    assert_eq!(
        pre_status.status.as_u16(),
        404,
        "agent unknown before invoke must return 404"
    );

    // 2. 调用 agent — invoke 路径会动态创建状态
    let invoke_request = TestDataGenerator::invoke_agent_request("Execute task");
    let invoke_response = server
        .post(
            &format!("/api/v1/agents/{}/invoke", agent_name),
            invoke_request,
        )
        .await;
    assert_success(&invoke_response);
    assert_json_field(&invoke_response, "success");

    // 3. 调用后再查 status — 现在 agent_status_store 应该有记录
    let final_status_response = server
        .get(&format!("/api/v1/agents/{}/status", agent_name))
        .await;
    assert_success(&final_status_response);

    let final_status = final_status_response.json();
    let status_str = final_status["status"].as_str().unwrap();

    // 状态应该是 completed 或 failed（JSON 序列化为小写）
    assert!(
        status_str == "completed"
            || status_str == "failed"
            || status_str == "Completed"
            || status_str == "Failed",
        "Agent status should be completed/Completed or failed/Failed, got: {}",
        status_str
    );

    println!(
        "✅ E2E-AGENT-004: Agent 完整执行流程测试通过 (最终状态: {})",
        status_str
    );
}

/// E2E-AGENT-005: 并发 Agent 执行
#[tokio::test]
async fn test_e2e_agent_005_concurrent_agent_execution() {
    use futures::future::join_all;

    // 创建 5 个并发 agent 调用
    let mut tasks = Vec::new();

    for i in 0..5 {
        let agent_name = format!("concurrent-agent-{}", i);
        let mut test_server = TestServer::new();

        let task = tokio::spawn(async move {
            let request = TestDataGenerator::invoke_agent_request(&format!("Task {}", i));
            let response = test_server
                .post(&format!("/api/v1/agents/{}/invoke", agent_name), request)
                .await;

            response.is_success()
        });

        tasks.push(task);
    }

    // 等待所有任务完成
    let results = join_all(tasks).await;

    // 验证所有调用都成功
    let success_count = results.iter().filter(|r| *r.as_ref().unwrap()).count();

    assert_eq!(
        success_count, 5,
        "All 5 concurrent agent invocations should succeed"
    );

    println!(
        "✅ E2E-AGENT-005: 并发 Agent 执行测试通过 ({}/5 成功)",
        success_count
    );
}

/// E2E-AGENT-006: Agent 错误处理 - 不存在的 Agent
#[tokio::test]
async fn test_e2e_agent_006_error_nonexistent_agent() {
    let mut server = TestServer::new();

    // Updated contract (agents.rs:711-715): GET status for unknown agent
    // returns 404 so callers can distinguish "idle" from "not registered".
    // Previously this returned a synthetic Idle status — semantic was unclear.
    let response = server.get("/api/v1/agents/nonexistent-agent/status").await;

    assert_eq!(
        response.status.as_u16(),
        404,
        "unknown agent must return 404 (was synthetic Idle pre-v1.0)"
    );

    println!("✅ E2E-AGENT-006: Agent 错误处理（不存在的 Agent）测试通过");
}

/// E2E-AGENT-007: Agent 错误处理 - 无效输入
#[tokio::test]
async fn test_e2e_agent_007_error_invalid_input() {
    let mut server = TestServer::new();

    // 发送无效的调用请求（缺少 input 字段）
    let invalid_request = json!({
        "context": {}
        // 缺少 input 字段
    });

    let response = server
        .post("/api/v1/agents/test-agent/invoke", invalid_request)
        .await;

    // 应该返回 4xx 错误
    assert!(
        response.status.is_client_error() || response.status.is_server_error(),
        "Should return error for invalid input"
    );

    println!("✅ E2E-AGENT-007: Agent 错误处理（无效输入）测试通过");
}

/// E2E-AGENT-008: Agent 执行性能测试
#[tokio::test]
async fn test_e2e_agent_008_execution_performance() {
    use std::time::Instant;

    let mut server = TestServer::new();

    // 执行 20 次 agent 调用，测量性能
    let mut latencies = Vec::new();

    for i in 0..20 {
        let start = Instant::now();
        let request = TestDataGenerator::invoke_agent_request(&format!("Performance test {}", i));
        let response = server
            .post("/api/v1/agents/perf-agent/invoke", request)
            .await;

        latencies.push(start.elapsed());

        // 不强制要求成功，因为可能有些调用会失败（这是正常的）
        if !response.is_success() {
            println!("  警告: 第 {} 次调用失败", i);
        }
    }

    // 计算统计数据
    latencies.sort();
    let avg = latencies.iter().sum::<std::time::Duration>() / latencies.len() as u32;
    let p50 = latencies[latencies.len() / 2];
    let p95 = latencies[latencies.len() * 95 / 100];

    println!("\n📊 Agent 执行性能:");
    println!("├─ 调用次数: {}", latencies.len());
    println!("├─ 平均延迟: {:?}", avg);
    println!("├─ P50 延迟: {:?}", p50);
    println!("└─ P95 延迟: {:?}", p95);

    // 性能断言（根据实际情况调整）
    use std::time::Duration;
    assert!(avg < Duration::from_secs(2), "平均延迟应小于 2秒");

    println!("✅ E2E-AGENT-008: Agent 执行性能测试通过");
}

/// E2E-AGENT-009: Agent 调用带上下文
#[tokio::test]
async fn test_e2e_agent_009_invoke_with_context() {
    let mut server = TestServer::new();

    // 带上下文调用 agent
    let request = json!({
        "input": "Process this with context",
        "context": {
            "user_id": "user-123",
            "session_id": "session-456",
            "metadata": {
                "priority": "high"
            }
        }
    });

    let response = server
        .post("/api/v1/agents/context-agent/invoke", request)
        .await;

    // 验证响应
    assert_success(&response);
    assert_json_field(&response, "success");

    println!("✅ E2E-AGENT-009: Agent 调用带上下文测试通过");
}
