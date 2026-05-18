#![allow(clippy::duplicate_mod)]
//! E2E 测试：审计追踪（Audit Trail）完整流程
//!
//! 测试场景：
//! - 安全事件记录
//! - 生命周期事件记录
//! - 事件查询和过滤
//! - 跨执行审计聚合
//! - 并发审计写入
//! - 审计事件持久化
//! - 合规报告生成

#[path = "common/mod.rs"]
mod common;

use chrono::Utc;
use cyberclaw_core::execution::ExecutionStatus;
use cyberclaw_core::identity::{ActorRef, ActorType};
use cyberclaw_core::ids::{ActorId, CapabilityId, ExecutionId, ReviewId, SecurityEventId, TraceId};
use cyberclaw_core::security::{SecurityEvent, SecurityEventSource, SecurityEventType, Severity};
use cyberclaw_observability::events::{EventRecorder, InMemoryEventRecorder, ObservabilityEvent};
use cyberclaw_observability::security_event_store::{
    EventFilter, InMemorySecurityEventStore, SecurityEventStore,
};
use std::sync::Arc;

// ─── 辅助函数 ───────────────────────────────────────────────────────────────

fn make_actor(id: &str, name: &str, actor_type: ActorType) -> ActorRef {
    ActorRef {
        id: ActorId::from_string(id.to_string()).unwrap(),
        actor_type,
        tenant_id: None,
        home_node_id: None,
        display_name: name.to_string(),
    }
}

fn make_security_event(
    execution_id: Option<ExecutionId>,
    actor: Option<ActorRef>,
    event_type: SecurityEventType,
    severity: Severity,
) -> SecurityEvent {
    let summary = format!("Test security event: {:?}", event_type);
    SecurityEvent {
        id: SecurityEventId::new(),
        actor,
        timestamp: Utc::now(),
        execution_id,
        case_id: None,
        node_id: None,
        runtime_instance_id: None,
        source: SecurityEventSource::PolicyEngine,
        event_type,
        severity,
        summary,
        details: serde_json::json!({"test": true}),
        trace_id: TraceId::new(),
        credential_evidence: None,
    }
}

// ─── 测试用例 ───────────────────────────────────────────────────────────────

/// E2E-AUDIT-001: 安全事件记录和检索
#[tokio::test]
async fn test_e2e_audit_001_security_event_recording() {
    let recorder = InMemoryEventRecorder::new();
    let exec_id = ExecutionId::from_string("audit-exec-001".to_string()).unwrap();
    let actor = make_actor("user-001", "Test User", ActorType::Human);

    // 记录安全事件
    let event = make_security_event(
        Some(exec_id.clone()),
        Some(actor.clone()),
        SecurityEventType::PolicyDenied,
        Severity::High,
    );

    recorder
        .record_security_event(event)
        .await
        .expect("应成功记录安全事件");

    // 检索事件
    let events = recorder
        .get_security_events()
        .await
        .expect("应成功检索事件");

    assert_eq!(events.len(), 1, "应有 1 个安全事件");
    assert_eq!(events[0].execution_id, Some(exec_id), "执行 ID 应匹配");
    assert!(
        matches!(events[0].event_type, SecurityEventType::PolicyDenied),
        "事件类型应为 PolicyDenied"
    );

    println!("✅ E2E-AUDIT-001: 安全事件记录和检索测试通过");
}

/// E2E-AUDIT-002: 生命周期事件记录
#[tokio::test]
async fn test_e2e_audit_002_lifecycle_event_recording() {
    let recorder = InMemoryEventRecorder::new();
    let exec_id = ExecutionId::from_string("audit-exec-002".to_string()).unwrap();
    let actor = make_actor("agent-001", "Test Agent", ActorType::Agent);

    // 记录执行创建事件
    recorder
        .record_event(ObservabilityEvent::ExecutionCreated {
            execution_id: exec_id.clone(),
            requested_by: actor.clone(),
            timestamp: Utc::now(),
        })
        .await
        .expect("应成功记录执行创建事件");

    // 记录状态变更事件
    recorder
        .record_event(ObservabilityEvent::ExecutionStatusChanged {
            execution_id: exec_id.clone(),
            from_status: ExecutionStatus::Pending,
            to_status: ExecutionStatus::Running,
            timestamp: Utc::now(),
        })
        .await
        .expect("应成功记录状态变更事件");

    // 记录能力调用事件
    let cap_id = CapabilityId::from_string("cap-001".to_string()).unwrap();
    recorder
        .record_event(ObservabilityEvent::CapabilityInvoked {
            execution_id: exec_id.clone(),
            capability_id: cap_id.clone(),
            timestamp: Utc::now(),
        })
        .await
        .expect("应成功记录能力调用事件");

    // 检索执行相关的所有事件
    let events = recorder.get_events_for_execution(&exec_id).await;

    assert_eq!(events.len(), 3, "应有 3 个生命周期事件");

    // 验证事件类型
    let created_count = recorder.count_events_by_type("ExecutionCreated").await;
    let status_count = recorder
        .count_events_by_type("ExecutionStatusChanged")
        .await;
    let capability_count = recorder.count_events_by_type("CapabilityInvoked").await;

    assert_eq!(created_count, 1);
    assert_eq!(status_count, 1);
    assert_eq!(capability_count, 1);

    println!("✅ E2E-AUDIT-002: 生命周期事件记录测试通过");
}

/// E2E-AUDIT-003: 安全事件查询和过滤
#[tokio::test]
async fn test_e2e_audit_003_security_event_filtering() {
    let store = InMemorySecurityEventStore::new();

    let exec1 = ExecutionId::from_string("filter-exec-001".to_string()).unwrap();
    let exec2 = ExecutionId::from_string("filter-exec-002".to_string()).unwrap();

    let actor1 = make_actor("user-001", "User One", ActorType::Human);
    let actor2 = make_actor("user-002", "User Two", ActorType::Human);

    // 记录不同的安全事件
    store
        .store(make_security_event(
            Some(exec1.clone()),
            Some(actor1.clone()),
            SecurityEventType::PolicyDenied,
            Severity::High,
        ))
        .await
        .unwrap();

    store
        .store(make_security_event(
            Some(exec2.clone()),
            Some(actor2.clone()),
            SecurityEventType::PermissionViolation,
            Severity::Critical,
        ))
        .await
        .unwrap();

    store
        .store(make_security_event(
            Some(exec1.clone()),
            Some(actor1.clone()),
            SecurityEventType::PermissionViolation,
            Severity::Medium,
        ))
        .await
        .unwrap();

    // 测试按执行 ID 过滤
    let filter = EventFilter {
        execution_id: Some(exec1.clone()),
        ..Default::default()
    };
    let results = store.query(filter).await.unwrap();
    assert_eq!(results.len(), 2, "exec1 应有 2 个事件");

    // 测试按事件类型过滤
    let filter = EventFilter {
        event_type: Some(SecurityEventType::PermissionViolation),
        ..Default::default()
    };
    let results = store.query(filter).await.unwrap();
    assert_eq!(results.len(), 2, "应有 2 个 PermissionViolation 事件");

    // 测试组合过滤
    let filter = EventFilter {
        execution_id: Some(exec1.clone()),
        event_type: Some(SecurityEventType::PolicyDenied),
        ..Default::default()
    };
    let results = store.query(filter).await.unwrap();
    assert_eq!(results.len(), 1, "应有 1 个符合条件的事件");

    println!("✅ E2E-AUDIT-003: 安全事件查询和过滤测试通过");
}

/// E2E-AUDIT-004: 跨执行审计聚合
#[tokio::test]
async fn test_e2e_audit_004_cross_execution_audit_aggregation() {
    let recorder = InMemoryEventRecorder::new();
    let store = InMemorySecurityEventStore::new();

    let actor = make_actor("agent-001", "Audit Agent", ActorType::Agent);

    // 模拟 3 个并行执行
    for i in 1..=3 {
        let exec_id = ExecutionId::from_string(format!("cross-exec-{:03}", i)).unwrap();

        // 生命周期事件
        recorder
            .record_event(ObservabilityEvent::ExecutionCreated {
                execution_id: exec_id.clone(),
                requested_by: actor.clone(),
                timestamp: Utc::now(),
            })
            .await
            .unwrap();

        // 安全事件
        store
            .store(make_security_event(
                Some(exec_id.clone()),
                Some(actor.clone()),
                SecurityEventType::PolicyDenied,
                Severity::High,
            ))
            .await
            .unwrap();
    }

    // 聚合所有执行的事件
    let all_lifecycle = recorder.get_events().await.unwrap();
    let all_security = store.query(EventFilter::default()).await.unwrap();

    assert_eq!(all_lifecycle.len(), 3, "应有 3 个生命周期事件");
    assert_eq!(all_security.len(), 3, "应有 3 个安全事件");

    // 验证每个执行都有对应的事件
    for i in 1..=3 {
        let exec_id = ExecutionId::from_string(format!("cross-exec-{:03}", i)).unwrap();

        let lifecycle_events = recorder.get_events_for_execution(&exec_id).await;
        assert_eq!(
            lifecycle_events.len(),
            1,
            "执行 {} 应有 1 个生命周期事件",
            i
        );

        let filter = EventFilter {
            execution_id: Some(exec_id),
            ..Default::default()
        };
        let security_events = store.query(filter).await.unwrap();
        assert_eq!(security_events.len(), 1, "执行 {} 应有 1 个安全事件", i);
    }

    println!("✅ E2E-AUDIT-004: 跨执行审计聚合测试通过");
}

/// E2E-AUDIT-005: 并发审计写入
#[tokio::test]
async fn test_e2e_audit_005_concurrent_audit_writes() {
    use futures::future::join_all;

    let recorder = Arc::new(InMemoryEventRecorder::new());
    let store = Arc::new(InMemorySecurityEventStore::new());

    let mut tasks = Vec::new();

    // 启动 10 个并发任务写入审计事件
    for i in 0..10 {
        let recorder_clone = Arc::clone(&recorder);
        let store_clone = Arc::clone(&store);

        let task = tokio::spawn(async move {
            let exec_id = ExecutionId::from_string(format!("concurrent-exec-{:03}", i)).unwrap();
            let actor = make_actor(
                &format!("actor-{:03}", i),
                &format!("Actor {}", i),
                ActorType::Agent,
            );

            // 写入生命周期事件
            recorder_clone
                .record_event(ObservabilityEvent::ExecutionCreated {
                    execution_id: exec_id.clone(),
                    requested_by: actor.clone(),
                    timestamp: Utc::now(),
                })
                .await
                .unwrap();

            // 写入安全事件
            store_clone
                .store(make_security_event(
                    Some(exec_id),
                    Some(actor),
                    SecurityEventType::RuntimeAnomalyDetected,
                    Severity::Medium,
                ))
                .await
                .unwrap();
        });

        tasks.push(task);
    }

    // 等待所有任务完成
    let results = join_all(tasks).await;
    assert!(results.iter().all(|r| r.is_ok()), "所有并发写入应成功");

    // 验证事件数量
    let lifecycle_count = recorder.get_events().await.unwrap().len();
    let security_count = store.count(EventFilter::default()).await.unwrap();

    assert_eq!(lifecycle_count, 10, "应有 10 个生命周期事件");
    assert_eq!(security_count, 10, "应有 10 个安全事件");

    println!("✅ E2E-AUDIT-005: 并发审计写入测试通过");
}

/// E2E-AUDIT-006: 安全事件严重性统计
#[tokio::test]
async fn test_e2e_audit_006_security_event_severity_stats() {
    let recorder = InMemoryEventRecorder::new();

    let actor = make_actor("system", "System", ActorType::System);

    // 记录不同严重性级别的安全事件
    for _ in 0..3 {
        recorder
            .record_security_event(make_security_event(
                None,
                Some(actor.clone()),
                SecurityEventType::PolicyDenied,
                Severity::High,
            ))
            .await
            .unwrap();
    }

    for _ in 0..2 {
        recorder
            .record_security_event(make_security_event(
                None,
                Some(actor.clone()),
                SecurityEventType::PermissionViolation,
                Severity::Critical,
            ))
            .await
            .unwrap();
    }

    for _ in 0..5 {
        recorder
            .record_security_event(make_security_event(
                None,
                Some(actor.clone()),
                SecurityEventType::RuntimeAnomalyDetected,
                Severity::Medium,
            ))
            .await
            .unwrap();
    }

    // 统计不同严重性级别的事件数量
    let high_count = recorder.count_security_events_by_severity("High").await;
    let critical_count = recorder.count_security_events_by_severity("Critical").await;
    let medium_count = recorder.count_security_events_by_severity("Medium").await;

    assert_eq!(high_count, 3, "应有 3 个 High 级别事件");
    assert_eq!(critical_count, 2, "应有 2 个 Critical 级别事件");
    assert_eq!(medium_count, 5, "应有 5 个 Medium 级别事件");

    println!("✅ E2E-AUDIT-006: 安全事件严重性统计测试通过");
}

/// E2E-AUDIT-007: 审计事件持久化（FIFO 驱逐）
#[tokio::test]
async fn test_e2e_audit_007_audit_event_persistence_fifo() {
    // 创建容量为 5 的存储
    let store = InMemorySecurityEventStore::with_capacity(5);
    let actor = make_actor("user", "User", ActorType::Human);

    // 写入 10 个事件
    for i in 0..10 {
        let exec_id = ExecutionId::from_string(format!("fifo-exec-{:03}", i)).unwrap();
        store
            .store(make_security_event(
                Some(exec_id),
                Some(actor.clone()),
                SecurityEventType::PolicyDenied,
                Severity::High,
            ))
            .await
            .unwrap();
    }

    // 验证只保留最新的 5 个事件
    let all_events = store.query(EventFilter::default()).await.unwrap();
    assert_eq!(all_events.len(), 5, "应只保留 5 个最新事件");

    // 验证保留的是最后 5 个事件（5-9）
    for i in 5..10 {
        let exec_id = ExecutionId::from_string(format!("fifo-exec-{:03}", i)).unwrap();
        let filter = EventFilter {
            execution_id: Some(exec_id.clone()),
            ..Default::default()
        };
        let found = store.query(filter).await.unwrap();
        assert_eq!(found.len(), 1, "执行 fifo-exec-{:03} 应被保留", i);
    }

    println!("✅ E2E-AUDIT-007: 审计事件持久化（FIFO 驱逐）测试通过");
}

/// E2E-AUDIT-008: 完整审计追踪（执行全生命周期）
#[tokio::test]
async fn test_e2e_audit_008_complete_audit_trail() {
    let recorder = InMemoryEventRecorder::new();
    let exec_id = ExecutionId::from_string("complete-exec-001".to_string()).unwrap();
    let user = make_actor("user-001", "Test User", ActorType::Human);
    let _agent = make_actor("agent-001", "Test Agent", ActorType::Agent);

    // 1. 执行创建
    recorder
        .record_event(ObservabilityEvent::ExecutionCreated {
            execution_id: exec_id.clone(),
            requested_by: user.clone(),
            timestamp: Utc::now(),
        })
        .await
        .unwrap();

    // 2. 状态变更：Pending -> Running
    recorder
        .record_event(ObservabilityEvent::ExecutionStatusChanged {
            execution_id: exec_id.clone(),
            from_status: ExecutionStatus::Pending,
            to_status: ExecutionStatus::Running,
            timestamp: Utc::now(),
        })
        .await
        .unwrap();

    // 3. Agent 执行开始
    recorder
        .record_event(ObservabilityEvent::AgentExecutionStarted {
            execution_id: exec_id.clone(),
            agent_id: "agent-001".to_string(),
            timestamp: Utc::now(),
        })
        .await
        .unwrap();

    // 4. Skill 调用
    recorder
        .record_event(ObservabilityEvent::SkillInvoked {
            execution_id: exec_id.clone(),
            skill_id: "skill-001".to_string(),
            timestamp: Utc::now(),
        })
        .await
        .unwrap();

    // 5. Capability 调用
    let cap_id = CapabilityId::from_string("cap-001".to_string()).unwrap();
    recorder
        .record_event(ObservabilityEvent::CapabilityInvoked {
            execution_id: exec_id.clone(),
            capability_id: cap_id.clone(),
            timestamp: Utc::now(),
        })
        .await
        .unwrap();

    // 6. Capability 完成
    recorder
        .record_event(ObservabilityEvent::CapabilityCompleted {
            execution_id: exec_id.clone(),
            capability_id: cap_id,
            success: true,
            duration_ms: 150,
            timestamp: Utc::now(),
        })
        .await
        .unwrap();

    // 7. Skill 完成
    recorder
        .record_event(ObservabilityEvent::SkillCompleted {
            execution_id: exec_id.clone(),
            skill_id: "skill-001".to_string(),
            success: true,
            duration_ms: 200,
            timestamp: Utc::now(),
        })
        .await
        .unwrap();

    // 8. Review 入队
    let review_id = ReviewId::from_string("review-001".to_string()).unwrap();
    recorder
        .record_event(ObservabilityEvent::ReviewEnqueued {
            review_id: review_id.clone(),
            execution_id: Some(exec_id.clone()),
            risk_level: "High".to_string(),
            target: cyberclaw_core::review::ReviewTarget::Execution {
                execution_id: exec_id.clone(),
            },
            timestamp: Utc::now(),
        })
        .await
        .unwrap();

    // 9. Review 批准
    recorder
        .record_event(ObservabilityEvent::ReviewApproved {
            review_id,
            execution_id: Some(exec_id.clone()),
            approved_by: user.clone(),
            wait_time_ms: 5000,
            target: cyberclaw_core::review::ReviewTarget::Execution {
                execution_id: exec_id.clone(),
            },
            timestamp: Utc::now(),
        })
        .await
        .unwrap();

    // 10. Agent 执行完成
    recorder
        .record_event(ObservabilityEvent::AgentExecutionCompleted {
            execution_id: exec_id.clone(),
            agent_id: "agent-001".to_string(),
            status: ExecutionStatus::Completed,
            duration_ms: 10000,
            timestamp: Utc::now(),
        })
        .await
        .unwrap();

    // 11. 状态变更：Running -> Completed
    recorder
        .record_event(ObservabilityEvent::ExecutionStatusChanged {
            execution_id: exec_id.clone(),
            from_status: ExecutionStatus::Running,
            to_status: ExecutionStatus::Completed,
            timestamp: Utc::now(),
        })
        .await
        .unwrap();

    // 验证完整审计追踪
    let events = recorder.get_events_for_execution(&exec_id).await;
    assert_eq!(events.len(), 11, "应有 11 个生命周期事件");

    // 验证事件类型分布
    assert_eq!(recorder.count_events_by_type("ExecutionCreated").await, 1);
    assert_eq!(
        recorder
            .count_events_by_type("ExecutionStatusChanged")
            .await,
        2
    );
    assert_eq!(
        recorder.count_events_by_type("AgentExecutionStarted").await,
        1
    );
    assert_eq!(recorder.count_events_by_type("SkillInvoked").await, 1);
    assert_eq!(recorder.count_events_by_type("CapabilityInvoked").await, 1);
    assert_eq!(
        recorder.count_events_by_type("CapabilityCompleted").await,
        1
    );
    assert_eq!(recorder.count_events_by_type("SkillCompleted").await, 1);
    assert_eq!(recorder.count_events_by_type("ReviewEnqueued").await, 1);
    assert_eq!(recorder.count_events_by_type("ReviewApproved").await, 1);
    assert_eq!(
        recorder
            .count_events_by_type("AgentExecutionCompleted")
            .await,
        1
    );

    println!("✅ E2E-AUDIT-008: 完整审计追踪测试通过");
}

/// E2E-AUDIT-009: 合规报告生成（安全事件汇总）
#[tokio::test]
async fn test_e2e_audit_009_compliance_report_generation() {
    let store = InMemorySecurityEventStore::new();

    let user1 = make_actor("user-001", "Alice", ActorType::Human);
    let user2 = make_actor("user-002", "Bob", ActorType::Human);
    let agent = make_actor("agent-001", "AutoAgent", ActorType::Agent);

    // 模拟一天内的安全事件
    for i in 0..50 {
        let actor = match i % 3 {
            0 => user1.clone(),
            1 => user2.clone(),
            _ => agent.clone(),
        };

        let event_type = match i % 4 {
            0 => SecurityEventType::PolicyDenied,
            1 => SecurityEventType::PermissionViolation,
            2 => SecurityEventType::RuntimeAnomalyDetected,
            _ => SecurityEventType::PromptInjectionDetected,
        };

        let severity = match i % 3 {
            0 => Severity::High,
            1 => Severity::Medium,
            _ => Severity::Low,
        };

        store
            .store(make_security_event(None, Some(actor), event_type, severity))
            .await
            .unwrap();
    }

    // 生成合规报告统计
    let total_events = store.count(EventFilter::default()).await.unwrap();
    assert_eq!(total_events, 50, "应有 50 个安全事件");

    // 按事件类型统计
    let policy_denied = store
        .query(EventFilter {
            event_type: Some(SecurityEventType::PolicyDenied),
            ..Default::default()
        })
        .await
        .unwrap()
        .len();

    let permission_violation = store
        .query(EventFilter {
            event_type: Some(SecurityEventType::PermissionViolation),
            ..Default::default()
        })
        .await
        .unwrap()
        .len();

    assert_eq!(policy_denied, 13, "应有 13 个策略拒绝事件");
    assert_eq!(permission_violation, 13, "应有 13 个权限违规事件");

    println!("✅ E2E-AUDIT-009: 合规报告生成测试通过");
    println!("  总事件数: {}", total_events);
    println!("  策略拒绝: {}", policy_denied);
    println!("  权限违规: {}", permission_violation);
}

/// E2E-AUDIT-010: 审计事件清理
#[tokio::test]
async fn test_e2e_audit_010_audit_event_cleanup() {
    let recorder = InMemoryEventRecorder::new();

    let actor = make_actor("user", "User", ActorType::Human);

    // 记录生命周期事件
    for i in 0..10 {
        let exec_id = ExecutionId::from_string(format!("cleanup-exec-{:03}", i)).unwrap();
        recorder
            .record_event(ObservabilityEvent::ExecutionCreated {
                execution_id: exec_id,
                requested_by: actor.clone(),
                timestamp: Utc::now(),
            })
            .await
            .unwrap();
    }

    // 记录安全事件
    for _ in 0..5 {
        recorder
            .record_security_event(make_security_event(
                None,
                Some(actor.clone()),
                SecurityEventType::PolicyDenied,
                Severity::High,
            ))
            .await
            .unwrap();
    }

    // 验证事件已记录
    assert_eq!(recorder.get_events().await.unwrap().len(), 10);
    assert_eq!(recorder.get_security_events().await.unwrap().len(), 5);

    // 清理生命周期事件
    recorder.clear().await.unwrap();
    assert_eq!(
        recorder.get_events().await.unwrap().len(),
        0,
        "生命周期事件应被清空"
    );

    // 验证安全事件未受影响
    assert_eq!(
        recorder.get_security_events().await.unwrap().len(),
        5,
        "安全事件应保持不变"
    );

    // 清理安全事件
    recorder.clear_security_events().await.unwrap();
    assert_eq!(
        recorder.get_security_events().await.unwrap().len(),
        0,
        "安全事件应被清空"
    );

    println!("✅ E2E-AUDIT-010: 审计事件清理测试通过");
}
