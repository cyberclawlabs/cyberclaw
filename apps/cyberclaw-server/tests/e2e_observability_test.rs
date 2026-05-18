#![allow(clippy::duplicate_mod)]
//! E2E 测试：可观测性系统完整流程
//!
//! 测试场景：
//! - Metrics（指标采集和查询）
//! - Tracing（分布式追踪）
//! - 性能监控和统计
//! - 并发场景下的可观测性
//! - 指标导出和格式验证

#[path = "common/mod.rs"]
mod common;

use chrono::Utc;
use cyberclaw_core::execution::ExecutionStatus;
use cyberclaw_core::ids::{ExecutionId, ReviewId, TraceId};
use cyberclaw_observability::metrics::{export_metrics, recorders};
use cyberclaw_observability::tracing as observability_tracing;

/// E2E-OBS-001: Metrics 基础采集和记录
#[tokio::test]
async fn test_e2e_obs_001_metrics_basic_recording() {
    // 记录执行完成指标
    recorders::record_execution_complete(&ExecutionStatus::Completed, 5.5);
    recorders::record_execution_complete(&ExecutionStatus::Failed, 2.3);
    recorders::record_execution_complete(&ExecutionStatus::Completed, 8.1);

    // 导出指标
    let metrics = export_metrics().expect("should export metrics");

    // 验证指标包含预期内容
    assert!(
        metrics.contains("cyberclaw_execution_total"),
        "应包含执行计数指标"
    );
    assert!(
        metrics.contains("cyberclaw_execution_duration_seconds"),
        "应包含执行时长指标"
    );

    // 验证指标格式正确（Prometheus 格式）
    assert!(metrics.contains("# HELP"), "应包含 HELP 注释");
    assert!(metrics.contains("# TYPE"), "应包含 TYPE 注释");

    println!("✅ E2E-OBS-001: Metrics 基础采集和记录测试通过");
}

/// E2E-OBS-002: Capability 指标采集
#[tokio::test]
async fn test_e2e_obs_002_capability_metrics() {
    // 记录多个 capability 调用
    recorders::record_capability_invocation("test.read_file", true, 0.15);
    recorders::record_capability_invocation("test.write_file", true, 0.23);
    recorders::record_capability_invocation("test.execute_command", false, 1.5);
    recorders::record_capability_invocation("test.query_database", true, 0.8);

    // 导出并验证
    let metrics = export_metrics().expect("should export metrics");

    assert!(
        metrics.contains("cyberclaw_capability_invocation_total"),
        "应包含 capability 调用计数"
    );
    assert!(
        metrics.contains("cyberclaw_capability_invocation_duration_seconds"),
        "应包含 capability 调用时长"
    );

    // 验证包含 success 和 failure 标签
    assert!(metrics.contains("status=\"success\""), "应包含成功标签");
    assert!(metrics.contains("status=\"failure\""), "应包含失败标签");

    println!("✅ E2E-OBS-002: Capability 指标采集测试通过");
}

/// E2E-OBS-003: Review 队列指标监控
#[tokio::test]
async fn test_e2e_obs_003_review_queue_metrics() {
    // 模拟 review 队列变化
    recorders::update_review_queue_size(0);
    recorders::update_review_queue_size(3);
    recorders::update_review_queue_size(7);
    recorders::update_review_queue_size(5);

    // 记录 review 等待时间
    recorders::record_review_wait_time("Low", 5.0);
    recorders::record_review_wait_time("Medium", 30.0);
    recorders::record_review_wait_time("High", 120.0);
    recorders::record_review_wait_time("Critical", 600.0);

    // 导出并验证
    let metrics = export_metrics().expect("should export metrics");

    assert!(
        metrics.contains("cyberclaw_review_queue_size"),
        "应包含队列大小指标"
    );
    assert!(
        metrics.contains("cyberclaw_review_wait_seconds"),
        "应包含等待时间指标"
    );

    // 验证风险级别标签
    assert!(
        metrics.contains("risk_level=\"Low\""),
        "应包含 Low 风险级别"
    );
    assert!(
        metrics.contains("risk_level=\"Critical\""),
        "应包含 Critical 风险级别"
    );

    println!("✅ E2E-OBS-003: Review 队列指标监控测试通过");
}

/// E2E-OBS-004: Agent 和 Skill 执行指标
#[tokio::test]
async fn test_e2e_obs_004_agent_skill_metrics() {
    // 记录 agent 执行
    recorders::record_agent_execution("agent-001", &ExecutionStatus::Completed);
    recorders::record_agent_execution("agent-002", &ExecutionStatus::Running);
    recorders::record_agent_execution("agent-003", &ExecutionStatus::Failed);

    // 记录 skill 调用
    recorders::record_skill_invocation("skill-code-review", true);
    recorders::record_skill_invocation("skill-test-generation", true);
    recorders::record_skill_invocation("skill-documentation", false);

    // 导出并验证
    let metrics = export_metrics().expect("should export metrics");

    assert!(
        metrics.contains("cyberclaw_agent_execution_total"),
        "应包含 agent 执行计数"
    );
    assert!(
        metrics.contains("cyberclaw_skill_invocation_total"),
        "应包含 skill 调用计数"
    );

    println!("✅ E2E-OBS-004: Agent 和 Skill 执行指标测试通过");
}

/// E2E-OBS-005: 重试机制指标
#[tokio::test]
async fn test_e2e_obs_005_retry_metrics() {
    // 模拟重试场景
    let operation = "database_connection";

    // 第一次失败，触发重试
    recorders::record_retry_attempt(operation);
    // 第二次失败，再次重试
    recorders::record_retry_attempt(operation);
    // 第三次失败，重试次数耗尽
    recorders::record_retry_exhausted(operation);

    // 导出并验证
    let metrics = export_metrics().expect("should export metrics");

    assert!(
        metrics.contains("cyberclaw_retry_attempts_total"),
        "应包含重试尝试计数"
    );
    assert!(
        metrics.contains("cyberclaw_retry_exhausted_total"),
        "应包含重试耗尽计数"
    );
    assert!(
        metrics.contains("operation=\"database_connection\""),
        "应包含操作名称标签"
    );

    println!("✅ E2E-OBS-005: 重试机制指标测试通过");
}

/// E2E-OBS-006: Tracing spans 创建和使用
#[tokio::test]
async fn test_e2e_obs_006_tracing_spans() {
    let execution_id = ExecutionId::from_string("exec-trace-001".to_string()).unwrap();
    let trace_id = TraceId::from_string("trace-001".to_string()).unwrap();
    let review_id = ReviewId::from_string("review-001".to_string()).unwrap();

    // 创建 execution span
    let exec_span = observability_tracing::execution_span(&execution_id, &trace_id);
    let _exec_guard = exec_span.enter();

    // 创建嵌套的 status transition span
    {
        let status_span = observability_tracing::status_transition_span(
            &execution_id,
            &ExecutionStatus::Pending,
            &ExecutionStatus::Running,
        );
        let _status_guard = status_span.enter();

        // 在 span 中执行操作
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    }

    // 创建 capability span
    {
        let cap_span = observability_tracing::capability_span(&execution_id, "test.capability");
        let _cap_guard = cap_span.enter();

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    }

    // 创建 review span
    {
        let review_span = observability_tracing::review_span(&execution_id, &review_id);
        let _review_guard = review_span.enter();

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    }

    // 创建 agent span
    {
        let agent_span = observability_tracing::agent_span(&execution_id, "test-agent");
        let _agent_guard = agent_span.enter();

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    }

    // 创建 skill span
    {
        let skill_span = observability_tracing::skill_span(&execution_id, "test-skill");
        let _skill_guard = skill_span.enter();

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    }

    // 验证所有 span 都正确创建和清理（通过 guard 自动管理）
    println!("✅ E2E-OBS-006: Tracing spans 创建和使用测试通过");
}

/// E2E-OBS-007: 嵌套 span 层次结构
#[tokio::test]
async fn test_e2e_obs_007_nested_span_hierarchy() {
    let execution_id = ExecutionId::from_string("exec-nested-001".to_string()).unwrap();
    let trace_id = TraceId::from_string("trace-nested-001".to_string()).unwrap();

    // Level 1: Execution span
    let exec_span = observability_tracing::execution_span(&execution_id, &trace_id);
    let _exec_guard = exec_span.enter();

    // Level 2: Agent span (嵌套在 execution 中)
    {
        let agent_span = observability_tracing::agent_span(&execution_id, "parent-agent");
        let _agent_guard = agent_span.enter();

        // Level 3: Skill span (嵌套在 agent 中)
        {
            let skill_span = observability_tracing::skill_span(&execution_id, "child-skill");
            let _skill_guard = skill_span.enter();

            // Level 4: Capability span (嵌套在 skill 中)
            {
                let cap_span =
                    observability_tracing::capability_span(&execution_id, "deep.capability");
                let _cap_guard = cap_span.enter();

                // 在最深层执行操作
                tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;
            }
        }
    }

    println!("✅ E2E-OBS-007: 嵌套 span 层次结构测试通过");
}

/// E2E-OBS-008: 并发场景下的 metrics 采集
#[tokio::test]
async fn test_e2e_obs_008_concurrent_metrics() {
    use futures::future::join_all;

    // 创建 20 个并发任务记录指标
    let mut tasks = Vec::new();

    for i in 0..20 {
        let task = tokio::spawn(async move {
            for j in 0..10 {
                // 记录各种指标
                let duration = (i * 10 + j) as f64 * 0.1;
                recorders::record_execution_complete(&ExecutionStatus::Completed, duration);
                recorders::record_capability_invocation("concurrent.test", true, duration * 0.5);
                recorders::record_agent_execution("concurrent-agent", &ExecutionStatus::Running);

                tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;
            }
        });

        tasks.push(task);
    }

    // 等待所有任务完成
    let results = join_all(tasks).await;

    // 验证所有任务都成功
    assert_eq!(
        results.iter().filter(|r| r.is_ok()).count(),
        20,
        "所有并发任务应该成功"
    );

    // 导出并验证指标
    let metrics = export_metrics().expect("should export metrics");
    assert!(
        metrics.contains("cyberclaw_execution_total"),
        "并发场景应正确记录指标"
    );

    println!("✅ E2E-OBS-008: 并发场景下的 metrics 采集测试通过");
}

/// E2E-OBS-009: 并发场景下的 tracing spans
#[tokio::test]
async fn test_e2e_obs_009_concurrent_tracing() {
    use futures::future::join_all;

    // 创建 10 个并发任务，每个创建独立的 trace
    let mut tasks = Vec::new();

    for i in 0..10 {
        let task = tokio::spawn(async move {
            let execution_id = ExecutionId::from_string(format!("exec-concurrent-{}", i)).unwrap();
            let trace_id = TraceId::from_string(format!("trace-concurrent-{}", i)).unwrap();

            // 创建独立的 span 树
            let exec_span = observability_tracing::execution_span(&execution_id, &trace_id);
            let _exec_guard = exec_span.enter();

            {
                let agent_span =
                    observability_tracing::agent_span(&execution_id, &format!("agent-{}", i));
                let _agent_guard = agent_span.enter();

                tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;
            }
        });

        tasks.push(task);
    }

    // 等待所有任务完成
    let results = join_all(tasks).await;

    // 验证所有任务都成功
    assert_eq!(
        results.iter().filter(|r| r.is_ok()).count(),
        10,
        "所有并发 tracing 任务应该成功"
    );

    println!("✅ E2E-OBS-009: 并发场景下的 tracing spans 测试通过");
}

/// E2E-OBS-010: 完整执行生命周期的可观测性
#[tokio::test]
async fn test_e2e_obs_010_full_execution_observability() {
    let execution_id = ExecutionId::from_string("exec-full-obs-001".to_string()).unwrap();
    let trace_id = TraceId::from_string("trace-full-obs-001".to_string()).unwrap();
    let start_time = Utc::now();

    // 1. 创建 execution span
    let exec_span = observability_tracing::execution_span(&execution_id, &trace_id);
    let _exec_guard = exec_span.enter();

    // 2. 记录执行开始
    recorders::record_execution_state_change("None", "Pending");

    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

    // 3. 状态转换: Pending -> Running
    {
        let status_span = observability_tracing::status_transition_span(
            &execution_id,
            &ExecutionStatus::Pending,
            &ExecutionStatus::Running,
        );
        let _status_guard = status_span.enter();

        recorders::record_execution_state_change("Pending", "Running");
    }

    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

    // 4. Agent 执行
    {
        let agent_span = observability_tracing::agent_span(&execution_id, "main-agent");
        let _agent_guard = agent_span.enter();

        recorders::record_agent_execution("main-agent", &ExecutionStatus::Running);

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // 5. Skill 调用
        {
            let skill_span = observability_tracing::skill_span(&execution_id, "analysis-skill");
            let _skill_guard = skill_span.enter();

            recorders::record_skill_invocation("analysis-skill", true);

            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

            // 6. Capability 调用
            {
                let cap_span = observability_tracing::capability_span(&execution_id, "file.read");
                let _cap_guard = cap_span.enter();

                let cap_start = Utc::now();
                tokio::time::sleep(tokio::time::Duration::from_millis(15)).await;
                let cap_duration = (Utc::now() - cap_start).num_milliseconds() as f64 / 1000.0;

                recorders::record_capability_invocation("file.read", true, cap_duration);
            }
        }

        recorders::record_agent_execution("main-agent", &ExecutionStatus::Completed);
    }

    // 7. 状态转换: Running -> Completed
    {
        let status_span = observability_tracing::status_transition_span(
            &execution_id,
            &ExecutionStatus::Running,
            &ExecutionStatus::Completed,
        );
        let _status_guard = status_span.enter();

        recorders::record_execution_state_change("Running", "Completed");
    }

    // 8. 记录执行完成
    let total_duration = (Utc::now() - start_time).num_milliseconds() as f64 / 1000.0;
    recorders::record_execution_complete(&ExecutionStatus::Completed, total_duration);

    // 9. 导出并验证指标
    let metrics = export_metrics().expect("should export metrics");

    // 验证各个阶段的指标都被记录
    assert!(
        metrics.contains("cyberclaw_execution_total"),
        "应记录执行总数"
    );
    assert!(
        metrics.contains("cyberclaw_execution_state"),
        "应记录状态变化"
    );
    assert!(
        metrics.contains("cyberclaw_agent_execution_total"),
        "应记录 agent 执行"
    );
    assert!(
        metrics.contains("cyberclaw_skill_invocation_total"),
        "应记录 skill 调用"
    );
    assert!(
        metrics.contains("cyberclaw_capability_invocation_total"),
        "应记录 capability 调用"
    );

    println!("✅ E2E-OBS-010: 完整执行生命周期的可观测性测试通过");
}

/// E2E-OBS-011: Metrics 导出格式验证
#[tokio::test]
async fn test_e2e_obs_011_metrics_export_format() {
    // 记录一些指标
    recorders::record_execution_complete(&ExecutionStatus::Completed, 1.0);
    recorders::record_capability_invocation("test", true, 0.5);

    // 导出指标
    let metrics = export_metrics().expect("should export metrics");

    // 验证 Prometheus 格式
    let lines: Vec<&str> = metrics.lines().collect();

    // 应该包含 HELP 和 TYPE 注释
    let help_lines: Vec<&str> = lines
        .iter()
        .filter(|l| l.starts_with("# HELP"))
        .copied()
        .collect();
    let type_lines: Vec<&str> = lines
        .iter()
        .filter(|l| l.starts_with("# TYPE"))
        .copied()
        .collect();

    assert!(!help_lines.is_empty(), "应包含 HELP 注释");
    assert!(!type_lines.is_empty(), "应包含 TYPE 注释");

    // 验证指标名称格式 (应该是 cyberclaw_ 前缀)
    let metric_lines: Vec<&str> = lines
        .iter()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .copied()
        .collect();

    for line in &metric_lines {
        assert!(
            line.starts_with("cyberclaw_"),
            "指标名称应以 cyberclaw_ 开头: {}",
            line
        );
    }

    // 验证指标值格式 (应该包含标签和数值)
    for line in &metric_lines {
        // 格式应该是: metric_name{labels} value
        if line.contains('{') {
            assert!(line.contains('}'), "标签应该闭合: {}", line);
        }
        // 应该包含数值部分
        let parts: Vec<&str> = line.split_whitespace().collect();
        assert!(parts.len() >= 2, "应该包含指标名和值: {}", line);
    }

    println!("✅ E2E-OBS-011: Metrics 导出格式验证测试通过");
}

/// E2E-OBS-012: 性能统计和聚合
#[tokio::test]
async fn test_e2e_obs_012_performance_statistics() {
    // 记录多个执行的性能数据
    let durations = vec![0.5, 1.0, 1.5, 2.0, 3.0, 5.0, 8.0, 10.0, 15.0, 20.0];

    for duration in &durations {
        recorders::record_execution_complete(&ExecutionStatus::Completed, *duration);
    }

    // 记录不同时长的 capability 调用
    let cap_durations = vec![0.1, 0.2, 0.5, 1.0, 2.0, 5.0];

    for duration in &cap_durations {
        recorders::record_capability_invocation("perf.test", true, *duration);
    }

    // 导出并验证
    let metrics = export_metrics().expect("should export metrics");

    // 验证包含 histogram 指标
    assert!(
        metrics.contains("cyberclaw_execution_duration_seconds_bucket"),
        "应包含执行时长分桶"
    );
    assert!(
        metrics.contains("cyberclaw_execution_duration_seconds_sum"),
        "应包含执行时长总和"
    );
    assert!(
        metrics.contains("cyberclaw_execution_duration_seconds_count"),
        "应包含执行次数"
    );

    assert!(
        metrics.contains("cyberclaw_capability_invocation_duration_seconds_bucket"),
        "应包含 capability 时长分桶"
    );

    println!("✅ E2E-OBS-012: 性能统计和聚合测试通过");
}
