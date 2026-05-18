#![allow(clippy::duplicate_mod)]
//! E2E 测试：Chat Completions API 完整流程
//!
//! 测试场景：
//! - 非流式请求完整流程
//! - 流式请求完整流程
//! - 多轮对话上下文保持
//! - 参数调整和验证
//! - 错误处理
//!
//! 注意：这些测试使用 MockLlmClient，不需要真实 LLM 服务。
//! 如需使用真实 LLM，设置环境变量 LLM_PROVIDER=openai|ark|anthropic|generic。

#[path = "common/mod.rs"]
mod common;
use serde_json::json;

use common::assertions::*;
use common::{TestDataGenerator, TestServer};

/// E2E-CHAT-001: 非流式 Chat Completion 完整流程
///
/// 使用 MockLlmClient 验证完整 API 结构：
/// - 响应包含必要字段 (id, object, model, choices, usage)
/// - choices 数组非空且包含 assistant 消息
/// - usage 包含 token 计数字段
#[tokio::test]
async fn test_e2e_chat_001_non_streaming_completion() {
    let mut server = TestServer::new();

    // 发送非流式请求
    let request = TestDataGenerator::chat_completion_request("Hello, how are you?", false);
    let response = server.post("/v1/chat/completions", request).await;

    // 验证响应
    assert_success(&response);
    assert_json_field(&response, "id");
    assert_json_field(&response, "object");
    assert_json_field(&response, "model");
    assert_json_field(&response, "choices");
    assert_json_field(&response, "usage");

    let json = response.json();
    assert_eq!(json["object"], "chat.completion");
    // 验证返回的 model 字段存在且非空
    assert!(json["model"].is_string(), "model field should be a string");
    assert!(
        !json["model"].as_str().unwrap_or("").is_empty(),
        "model field should not be empty"
    );

    // 验证 choices 结构
    let choices = json["choices"]
        .as_array()
        .expect("choices should be an array");
    assert!(!choices.is_empty(), "choices should not be empty");

    let first_choice = &choices[0];
    assert_json_field(&response, "choices");
    assert!(first_choice["message"]["role"] == "assistant");
    assert!(first_choice["message"]["content"].is_string());
    assert!(
        !first_choice["message"]["content"]
            .as_str()
            .unwrap_or("")
            .is_empty(),
        "response content should not be empty"
    );

    // 验证 usage 结构
    assert!(json["usage"].is_object());
    assert!(json["usage"]["prompt_tokens"].is_number());
    assert!(json["usage"]["completion_tokens"].is_number());
    assert!(json["usage"]["total_tokens"].is_number());

    println!("E2E-CHAT-001: 非流式请求完整流程测试通过");
}

/// E2E-CHAT-002: 流式 Chat Completion 完整流程
///
/// 使用 MockLlmClient 验证流式响应：
/// - 响应不为空
/// - 包含多行 SSE 数据
#[tokio::test]
async fn test_e2e_chat_002_streaming_completion() {
    let mut server = TestServer::new();

    // 发送流式请求
    let request = TestDataGenerator::chat_completion_request("Tell me a story", true);
    let response = server.post("/v1/chat/completions", request).await;

    // 验证响应
    assert_success(&response);

    // 验证流式响应的文本内容
    let text = response.text();
    assert!(!text.is_empty(), "Streaming response should not be empty");

    // 流式响应应该包含多个数据块
    let lines: Vec<&str> = text.lines().collect();
    assert!(
        lines.len() > 1,
        "Streaming response should contain multiple lines, got: {}",
        lines.len()
    );

    println!(
        "E2E-CHAT-002: 流式请求完整流程测试通过 (接收到 {} 行数据)",
        lines.len()
    );
}

/// E2E-CHAT-003: 多轮对话上下文保持
///
/// 验证多轮对话场景：每轮请求都能成功返回响应，
/// choices 数组非空（具体内容取决于 LLM 实现）。
#[tokio::test]
async fn test_e2e_chat_003_multi_turn_conversation() {
    let mut server = TestServer::new();

    // 从环境变量读取 model 名称
    let model = std::env::var("LLM_DEFAULT_MODEL").unwrap_or_else(|_| "gpt-4".to_string());

    // 第一轮对话
    let request1 = json!({
        "model": model,
        "messages": [
            {"role": "user", "content": "My name is Alice"}
        ],
        "stream": false
    });

    let response1 = server.post("/v1/chat/completions", request1).await;
    assert_success(&response1);

    let assistant_reply = response1.json()["choices"][0]["message"]["content"]
        .as_str()
        .expect("should have content");
    assert!(
        !assistant_reply.is_empty(),
        "first round assistant reply should not be empty"
    );

    // 第二轮对话（包含上下文）
    let request2 = json!({
        "model": model,
        "messages": [
            {"role": "user", "content": "My name is Alice"},
            {"role": "assistant", "content": assistant_reply},
            {"role": "user", "content": "What is my name?"}
        ],
        "stream": false
    });

    let response2 = server.post("/v1/chat/completions", request2).await;
    assert_success(&response2);

    // 验证第二轮对话成功
    assert_json_field(&response2, "choices");
    let _json = response2.json();
    let choices = _json["choices"].as_array().unwrap();
    assert!(!choices.is_empty());

    println!("E2E-CHAT-003: 多轮对话上下文保持测试通过");
}

/// E2E-CHAT-004: 参数调整和验证
///
/// 验证带额外参数的请求（temperature, max_tokens, top_p）能成功处理。
#[tokio::test]
async fn test_e2e_chat_004_parameter_adjustment() {
    let mut server = TestServer::new();

    // 从环境变量读取 model 名称
    let model = std::env::var("LLM_DEFAULT_MODEL").unwrap_or_else(|_| "gpt-4".to_string());

    // 测试不同的 temperature 值
    let request = json!({
        "model": model,
        "messages": [
            {"role": "user", "content": "Be creative!"}
        ],
        "temperature": 1.5,
        "max_tokens": 50,
        "top_p": 0.9,
        "stream": false
    });

    let response = server.post("/v1/chat/completions", request).await;
    assert_success(&response);

    // 验证响应包含 usage 字段
    let _json = response.json();
    assert_json_field(&response, "usage");
    assert_json_field(&response, "choices");

    println!("E2E-CHAT-004: 参数调整和验证测试通过");
}

/// E2E-CHAT-005: 错误处理 - 无效请求
#[tokio::test]
async fn test_e2e_chat_005_error_handling_invalid_request() {
    let mut server = TestServer::new();

    // 测试缺少 messages 字段
    let invalid_request = json!({
        "model": "gpt-4"
        // 缺少 messages
    });

    let response = server.post("/v1/chat/completions", invalid_request).await;

    // 应该返回 4xx 错误
    assert!(
        response.status.is_client_error(),
        "Should return client error for invalid request"
    );

    println!("E2E-CHAT-005: 错误处理（无效请求）测试通过");
}

/// E2E-CHAT-006: 错误处理 - 空消息
#[tokio::test]
async fn test_e2e_chat_006_error_handling_empty_messages() {
    let mut server = TestServer::new();

    // 测试空消息数组
    let invalid_request = json!({
        "model": "gpt-4",
        "messages": [],
        "stream": false
    });

    let response = server.post("/v1/chat/completions", invalid_request).await;

    // 应该返回 4xx 或 5xx 错误
    assert!(
        response.status.is_client_error() || response.status.is_server_error(),
        "Should return error for empty messages"
    );

    println!("E2E-CHAT-006: 错误处理（空消息）测试通过");
}

/// E2E-CHAT-007: 并发请求测试
///
/// 使用 MockLlmClient 测试 10 个并发请求全部成功。
/// 修复：使用高速率限制配置避免默认配置（1 req/s）导致的限流失败。
#[tokio::test]
async fn test_e2e_chat_007_concurrent_requests() {
    use futures::future::join_all;

    // 从环境变量读取 model 名称
    let model = std::env::var("LLM_DEFAULT_MODEL").unwrap_or_else(|_| "gpt-4".to_string());

    // 使用高速率限制配置，避免默认配置（rate_limit_per_second=1）导致并发测试失败
    let config = cyberclaw_server::ServerConfig {
        rate_limit_per_second: 1000,
        rate_limit_burst_size: 200,
        ..cyberclaw_server::ServerConfig::default()
    };

    // 创建 10 个并发请求
    let mut tasks = Vec::new();

    for i in 0..10 {
        let model = model.clone();
        let config = config.clone();
        let request = json!({
            "model": model,
            "messages": [
                {"role": "user", "content": format!("Concurrent request {}", i)}
            ],
            "stream": false
        });

        // 每个请求使用独立的服务器实例，配置高速率限制
        let task = tokio::spawn(async move {
            // 固定使用 Mock LLM，避免真实供应商波动影响默认门禁稳定性。
            let mut test_server = TestServer::new_with_mock_llm_config(config);
            let response = test_server.post("/v1/chat/completions", request).await;
            response.is_success()
        });

        tasks.push(task);
    }

    // 等待所有请求完成
    let results = join_all(tasks).await;

    // 验证所有请求都成功
    let success_count = results.iter().filter(|r| *r.as_ref().unwrap()).count();

    assert_eq!(
        success_count, 10,
        "All 10 concurrent requests should succeed"
    );

    println!("E2E-CHAT-007: 并发请求测试通过 ({}/10 成功)", success_count);
}

/// E2E-CHAT-008A: Mock 性能基准门禁
///
/// 该门禁固定使用 MockLlmClient，不依赖外部网络或供应商服务。
/// 目标是为发布提供稳定、可重复的性能基线，避免真实 LLM 外部波动导致门禁不稳定。
#[tokio::test]
async fn test_e2e_chat_008_performance_benchmark() {
    use std::time::{Duration, Instant};
    let benchmark_requests: usize = 100;
    let avg_latency_threshold_ms: u64 = 2_000;
    let timeout_secs: u64 = 60;

    let test_future = async {
        // 性能基准测试需要发送 3（预热）+ 100（基准）= 103 次请求，
        // 默认 ServerConfig 的 burst_size=60 不足以容纳全部请求。
        // 使用高 burst_size / per_second 配置，确保速率限制不干扰性能测量。
        let config = cyberclaw_server::ServerConfig {
            rate_limit_per_second: 1000,
            rate_limit_burst_size: 200,
            ..cyberclaw_server::ServerConfig::default()
        };
        // 固定使用 Mock 模式，保证门禁稳定性与可复现性。
        let mut server = TestServer::new_with_mock_llm_config(config);

        // 预热
        for _ in 0..3 {
            let request = TestDataGenerator::chat_completion_request("Warmup", false);
            let _ = server.post("/v1/chat/completions", request).await;
        }

        // 基准测试：固定 100 次
        let mut latencies = Vec::new();
        let start = Instant::now();

        for _ in 0..benchmark_requests {
            let req_start = Instant::now();
            let request = TestDataGenerator::chat_completion_request("Benchmark", false);
            let response = server.post("/v1/chat/completions", request).await;
            assert_success(&response);
            latencies.push(req_start.elapsed());
        }

        let total_time = start.elapsed();

        // 计算统计数据
        latencies.sort();
        let min = latencies.first().unwrap();
        let max = latencies.last().unwrap();
        let avg = total_time / benchmark_requests as u32;
        let idx = |percent: f64| -> usize {
            ((benchmark_requests as f64 * percent).ceil() as usize)
                .saturating_sub(1)
                .min(latencies.len() - 1)
        };
        let p50 = latencies[idx(0.50)];
        let p95 = latencies[idx(0.95)];
        let p99 = latencies[idx(0.99)];

        println!("\n性能基准测试结果:");
        println!("  总请求数: {}", benchmark_requests);
        println!("  总耗时: {:?}", total_time);
        println!(
            "  吞吐量: {:.2} req/s",
            benchmark_requests as f64 / total_time.as_secs_f64()
        );
        println!("  延迟统计:");
        println!("    最小: {:?}", min);
        println!("    平均: {:?}", avg);
        println!("    P50:  {:?}", p50);
        println!("    P95:  {:?}", p95);
        println!("    P99:  {:?}", p99);
        println!("    最大: {:?}", max);

        // Mock 性能门禁断言：平均延迟 < 2000ms
        assert!(
            avg < Duration::from_millis(avg_latency_threshold_ms),
            "平均延迟应小于 {}ms（当前: {:?}）",
            avg_latency_threshold_ms,
            avg,
        );

        println!("E2E-CHAT-008A: Mock 性能基准测试通过");
    };

    tokio::time::timeout(Duration::from_secs(timeout_secs), test_future)
        .await
        .unwrap_or_else(|_| panic!("E2E-CHAT-008A: Mock 性能门禁超时（>{}s）", timeout_secs));
}

/// E2E-CHAT-008B: Real LLM 可用性门禁（显式触发）
///
/// 默认标记为 ignored，不参与常规发布门禁，避免外部网络/供应商波动卡死发布。
/// 仅在需要验证真实供应商可用性时显式执行：
/// `set -a && source apps/cyberclaw-server/.env && set +a && cargo test -p cyberclaw-server --test e2e_chat_completion_test test_e2e_chat_008_real_llm_availability_gate -- --ignored --nocapture`
#[tokio::test]
#[ignore = "requires external real llm service; run explicitly as availability gate"]
async fn test_e2e_chat_008_real_llm_availability_gate() {
    use std::time::Duration;

    let provider = std::env::var("LLM_PROVIDER").unwrap_or_else(|_| "mock".to_string());
    if provider == "mock" || provider.is_empty() {
        println!("E2E-CHAT-008B skipped: LLM_PROVIDER is mock/empty");
        return;
    }

    let config = cyberclaw_server::ServerConfig {
        rate_limit_per_second: 1000,
        rate_limit_burst_size: 200,
        ..cyberclaw_server::ServerConfig::default()
    };
    let mut server = TestServer::new_with_env_llm_config(config);

    let request_count = 3usize;
    let max_attempts = 3usize;
    let mut success_count = 0usize;

    for i in 0..request_count {
        let request =
            TestDataGenerator::chat_completion_request(&format!("Availability probe {}", i), false);

        let mut succeeded = false;
        for attempt in 1..=max_attempts {
            let resp = tokio::time::timeout(
                Duration::from_secs(45),
                server.post("/v1/chat/completions", request.clone()),
            )
            .await;

            match resp {
                Ok(response) if response.is_success() => {
                    succeeded = true;
                    break;
                }
                _ => {
                    if attempt < max_attempts {
                        tokio::time::sleep(Duration::from_millis(500 * attempt as u64)).await;
                    }
                }
            }
        }

        if succeeded {
            success_count += 1;
        }
    }

    assert!(
        success_count >= 2,
        "E2E-CHAT-008B Real LLM availability failed: success {}/{} (<2)",
        success_count,
        request_count
    );

    println!(
        "E2E-CHAT-008B: Real LLM availability gate passed ({}/{})",
        success_count, request_count
    );
}
