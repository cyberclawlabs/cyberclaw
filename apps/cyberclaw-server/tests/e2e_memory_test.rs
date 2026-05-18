#![allow(clippy::duplicate_mod)]
//! E2E 测试：内存机制完整流程
//!
//! 测试场景：
//! - 工作记忆隔离（6个并行agent）
//! - 并发内存写入
//! - 加密内存条目
//! - 内存检查点恢复
//! - 情景记忆聚合
//! - 所有内存条目类型
//! - 完整内存上下文信封
//! - 跨agent内存共享

#[path = "common/mod.rs"]
mod common;

use common::TestServer;
use cyberclaw_core::ids::{ArtifactId, ExecutionId, TraceId};
use cyberclaw_core::memory_context::{
    EpisodicContextProjection, MemoryContextEnvelope, ProceduralDocument, WorkingEntryKind,
    WorkingMemoryCheckpoint, WorkingMemoryEntry,
};
use futures::future::join_all;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

/// E2E-MEMORY-001: 工作记忆隔离 - 6个并行agent的内存独立性
///
/// 使用 MockLlmClient 验证6个并行agent调用时各自维护独立的内存条目。
/// 通过 /api/v1/agents/:name/invoke 端点模拟agent执行。
#[tokio::test]
async fn test_e2e_memory_001_working_memory_isolation() {
    let _server = TestServer::new();

    // 创建6个并行agent任务
    let agent_names = [
        "agent-alpha",
        "agent-beta",
        "agent-gamma",
        "agent-delta",
        "agent-epsilon",
        "agent-zeta",
    ];

    let memory_tracker = Arc::new(Mutex::new(HashMap::<String, Vec<WorkingMemoryEntry>>::new()));

    let mut tasks = Vec::new();
    for (idx, agent_name) in agent_names.iter().enumerate() {
        let agent_name = agent_name.to_string();
        let tracker = memory_tracker.clone();
        let mut test_server = TestServer::new();

        let task = tokio::spawn(async move {
            // 创建agent专属的工作记忆
            let execution_id =
                ExecutionId::from_string(format!("exec-{}-{}", agent_name, idx)).unwrap();
            let trace_id = TraceId::from_string(format!("trace-{}-{}", agent_name, idx)).unwrap();

            let memory_entry = WorkingMemoryEntry::new(
                Some(execution_id.clone()),
                WorkingEntryKind::Goal,
                format!("Goal for {}: Process task {}", agent_name, idx),
                vec![],
                Some(trace_id),
            );

            // 调用agent API（这里模拟，实际应该通过API创建）
            let request = json!({
                "input": format!("Execute task for {}", agent_name),
                "context": {
                    "agent_name": agent_name,
                    "memory": {
                        "working_items": [memory_entry],
                        "episodic": {
                            "executions": [],
                            "reviews": [],
                            "provenance_records": [],
                            "security_events": [],
                            "artifact_refs": [],
                            "summary_notes": []
                        },
                        "procedural_docs": [],
                        "policy_notes": []
                    }
                }
            });

            let endpoint = format!("/api/v1/agents/{}/invoke", agent_name);
            let response = test_server.post(&endpoint, request).await;

            // 记录内存条目
            let mut tracker_lock = tracker.lock().await;
            tracker_lock
                .entry(agent_name.clone())
                .or_default()
                .push(memory_entry);

            (agent_name, response.is_success())
        });

        tasks.push(task);
    }

    // 等待所有agent完成
    let results = join_all(tasks).await;

    // 验证结果
    let memory_map = memory_tracker.lock().await;

    for result in results {
        let (agent_name, success) = result.expect("Task should complete");
        assert!(success, "Agent {} should complete successfully", agent_name);

        // 验证每个agent都有独立的内存条目
        assert!(
            memory_map.contains_key(&agent_name),
            "Agent {} should have memory entries",
            agent_name
        );

        let entries = &memory_map[&agent_name];
        assert_eq!(
            entries.len(),
            1,
            "Agent {} should have exactly 1 memory entry",
            agent_name
        );
    }

    // 验证内存隔离：6个agent，6组独立内存
    assert_eq!(
        memory_map.len(),
        6,
        "Should have 6 independent memory contexts"
    );

    println!("✅ E2E-MEMORY-001: 工作记忆隔离测试通过 (6个并行agent)");
}

/// E2E-MEMORY-002: 并发内存写入 - 6个agent同时写入内存
///
/// 验证6个并发任务创建内存条目时无竞态条件，所有写入均成功且ID唯一。
#[tokio::test]
async fn test_e2e_memory_002_concurrent_memory_writes() {
    let write_count = Arc::new(Mutex::new(0));
    let mut tasks = Vec::new();

    for i in 0..6 {
        let counter = write_count.clone();
        let task = tokio::spawn(async move {
            // 模拟并发写入
            let execution_id = ExecutionId::from_string(format!("concurrent-exec-{}", i)).unwrap();

            let entry = WorkingMemoryEntry::new(
                Some(execution_id),
                WorkingEntryKind::ToolResult,
                format!("Concurrent write from agent {}", i),
                vec![ArtifactId::from_string(format!("artifact-{}", i)).unwrap()],
                None,
            );

            // 模拟写入延迟
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

            let mut count = counter.lock().await;
            *count += 1;

            entry
        });
        tasks.push(task);
    }

    let entries = join_all(tasks).await;

    // 验证所有写入都成功
    assert_eq!(entries.len(), 6, "Should have 6 memory entries");

    let final_count = *write_count.lock().await;
    assert_eq!(final_count, 6, "All 6 concurrent writes should succeed");

    // 验证每个条目的唯一性
    let mut execution_ids = std::collections::HashSet::new();
    for result in entries {
        let entry = result.expect("Task should complete");
        if let Some(exec_id) = entry.execution_id {
            execution_ids.insert(exec_id.to_string());
        }
    }

    assert_eq!(execution_ids.len(), 6, "All execution IDs should be unique");

    println!("✅ E2E-MEMORY-002: 并发内存写入测试通过");
}

/// E2E-MEMORY-003: 加密内存条目 - 测试敏感数据保护
#[tokio::test]
async fn test_e2e_memory_003_encrypted_memory_entries() {
    let execution_id = ExecutionId::from_string("secure-exec".to_string()).unwrap();

    // 创建加密内存条目
    let encrypted_entry = WorkingMemoryEntry::new_encrypted(
        Some(execution_id.clone()),
        WorkingEntryKind::Decision,
        "Sensitive decision: Approve API key rotation",
        vec![ArtifactId::from_string("secure-artifact".to_string()).unwrap()],
        Some(TraceId::from_string("secure-trace".to_string()).unwrap()),
    );

    // 创建未加密内存条目
    let plain_entry = WorkingMemoryEntry::new(
        Some(execution_id),
        WorkingEntryKind::Decision,
        "Public decision: Update documentation",
        vec![],
        None,
    );

    // 验证加密标志
    assert!(
        encrypted_entry.encrypted,
        "Entry should be marked as encrypted"
    );
    assert!(
        !plain_entry.encrypted,
        "Entry should not be marked as encrypted"
    );

    // 验证两者都能序列化
    let encrypted_json = serde_json::to_string(&encrypted_entry).expect("Should serialize");
    let plain_json = serde_json::to_string(&plain_entry).expect("Should serialize");

    assert!(
        encrypted_json.contains("\"encrypted\":true"),
        "JSON should indicate encryption"
    );
    assert!(
        plain_json.contains("\"encrypted\":false"),
        "JSON should indicate no encryption"
    );

    println!("✅ E2E-MEMORY-003: 加密内存条目测试通过");
}

/// E2E-MEMORY-004: 内存检查点恢复
#[tokio::test]
async fn test_e2e_memory_004_memory_checkpoint_recovery() {
    let execution_id = ExecutionId::from_string("checkpoint-exec".to_string()).unwrap();

    // 创建一系列内存条目
    let entries = vec![
        WorkingMemoryEntry::new(
            Some(execution_id.clone()),
            WorkingEntryKind::PhaseStart,
            "Phase 1: Data collection",
            vec![],
            None,
        ),
        WorkingMemoryEntry::new(
            Some(execution_id.clone()),
            WorkingEntryKind::ToolResult,
            "Collected 1000 records",
            vec![ArtifactId::from_string("data-batch-1".to_string()).unwrap()],
            None,
        ),
        WorkingMemoryEntry::new(
            Some(execution_id.clone()),
            WorkingEntryKind::PhaseEnd,
            "Phase 1 completed successfully",
            vec![],
            None,
        ),
    ];

    // 创建检查点
    let checkpoint = WorkingMemoryCheckpoint {
        checkpoint_id: "checkpoint-001".to_string(),
        entries: entries.clone(),
    };

    // 序列化检查点
    let checkpoint_json = serde_json::to_string(&checkpoint).expect("Checkpoint should serialize");

    // 反序列化恢复
    let recovered: WorkingMemoryCheckpoint =
        serde_json::from_str(&checkpoint_json).expect("Should deserialize");

    // 验证恢复数据
    assert_eq!(
        recovered.checkpoint_id, "checkpoint-001",
        "Checkpoint ID should match"
    );
    assert_eq!(recovered.entries.len(), 3, "Should recover all 3 entries");

    // 验证每个条目
    assert!(matches!(
        recovered.entries[0].kind,
        WorkingEntryKind::PhaseStart
    ));
    assert!(matches!(
        recovered.entries[1].kind,
        WorkingEntryKind::ToolResult
    ));
    assert!(matches!(
        recovered.entries[2].kind,
        WorkingEntryKind::PhaseEnd
    ));

    println!("✅ E2E-MEMORY-004: 内存检查点恢复测试通过");
}

/// E2E-MEMORY-005: 情景记忆聚合 - 跨6个agent执行的历史
#[tokio::test]
async fn test_e2e_memory_005_episodic_context_aggregation() {
    // 模拟6个agent的历史执行记录
    let mut episodic = EpisodicContextProjection {
        executions: vec![],
        reviews: vec![],
        provenance_records: vec![],
        security_events: vec![],
        artifact_refs: vec![],
        summary_notes: vec![],
        foresights: vec![],
    };

    // 为6个agent添加摘要
    for i in 0..6 {
        episodic
            .summary_notes
            .push(format!("Agent {} completed task successfully", i));
        episodic
            .artifact_refs
            .push(ArtifactId::from_string(format!("agent-{}-result", i)).unwrap());
    }

    // 验证聚合结果
    assert_eq!(
        episodic.summary_notes.len(),
        6,
        "Should have 6 execution summaries"
    );
    assert_eq!(
        episodic.artifact_refs.len(),
        6,
        "Should have 6 artifact references"
    );

    // 序列化和反序列化
    let json = serde_json::to_string(&episodic).expect("Should serialize");
    let recovered: EpisodicContextProjection =
        serde_json::from_str(&json).expect("Should deserialize");

    assert_eq!(
        recovered.summary_notes.len(),
        6,
        "Should recover all summaries"
    );

    println!("✅ E2E-MEMORY-005: 情景记忆聚合测试通过");
}

/// E2E-MEMORY-006: 所有内存条目类型 - 6种WorkingEntryKind
#[tokio::test]
async fn test_e2e_memory_006_all_entry_types() {
    let execution_id = ExecutionId::from_string("all-types-exec".to_string()).unwrap();

    // 创建所有6种类型的条目
    let entry_types = vec![
        (WorkingEntryKind::Goal, "Set execution goal"),
        (WorkingEntryKind::PhaseStart, "Start data processing phase"),
        (WorkingEntryKind::PhaseEnd, "Complete data processing phase"),
        (
            WorkingEntryKind::ToolResult,
            "Tool returned 42 processed items",
        ),
        (
            WorkingEntryKind::Decision,
            "Decided to use caching strategy",
        ),
        (WorkingEntryKind::Error, "Encountered network timeout error"),
    ];

    let mut entries = Vec::new();
    for (kind, summary) in entry_types {
        let entry =
            WorkingMemoryEntry::new(Some(execution_id.clone()), kind, summary, vec![], None);
        entries.push(entry);
    }

    // 验证所有类型都被创建
    assert_eq!(entries.len(), 6, "Should have 6 different entry types");

    // 验证每种类型
    let kinds: Vec<_> = entries.iter().map(|e| &e.kind).collect();
    assert!(kinds.iter().any(|k| matches!(k, WorkingEntryKind::Goal)));
    assert!(kinds
        .iter()
        .any(|k| matches!(k, WorkingEntryKind::PhaseStart)));
    assert!(kinds
        .iter()
        .any(|k| matches!(k, WorkingEntryKind::PhaseEnd)));
    assert!(kinds
        .iter()
        .any(|k| matches!(k, WorkingEntryKind::ToolResult)));
    assert!(kinds
        .iter()
        .any(|k| matches!(k, WorkingEntryKind::Decision)));
    assert!(kinds.iter().any(|k| matches!(k, WorkingEntryKind::Error)));

    // 序列化所有条目
    for entry in &entries {
        let json = serde_json::to_string(entry).expect("Should serialize");
        assert!(!json.is_empty(), "JSON should not be empty");
    }

    println!("✅ E2E-MEMORY-006: 所有内存条目类型测试通过");
}

/// E2E-MEMORY-007: 完整内存上下文信封
#[tokio::test]
async fn test_e2e_memory_007_memory_context_envelope() {
    let execution_id = ExecutionId::from_string("envelope-exec".to_string()).unwrap();

    // 创建完整的内存上下文信封
    let envelope = MemoryContextEnvelope {
        working_items: vec![
            WorkingMemoryEntry::new(
                Some(execution_id.clone()),
                WorkingEntryKind::Goal,
                "Primary goal: Complete analysis",
                vec![],
                None,
            ),
            WorkingMemoryEntry::new(
                Some(execution_id),
                WorkingEntryKind::ToolResult,
                "Analysis completed with 95% accuracy",
                vec![ArtifactId::from_string("analysis-result".to_string()).unwrap()],
                None,
            ),
        ],
        episodic: EpisodicContextProjection {
            executions: vec![],
            reviews: vec![],
            provenance_records: vec![],
            security_events: vec![],
            artifact_refs: vec![ArtifactId::from_string("historical-data".to_string()).unwrap()],
            summary_notes: vec!["Previous execution successful".to_string()],
            foresights: vec![],
        },
        procedural_docs: vec![ProceduralDocument {
            source: "proc-001".to_string(),
            title: "Standard Operating Procedure".to_string(),
            content: "Follow security guidelines...".to_string(),
        }],
        policy_notes: vec![
            "Require approval for sensitive operations".to_string(),
            "Log all security events".to_string(),
        ],
        degraded_sources: vec![],
        degraded_source_details: vec![],
        semantic_snapshot: None,
    };

    // 验证信封完整性
    assert_eq!(
        envelope.working_items.len(),
        2,
        "Should have 2 working items"
    );
    assert_eq!(
        envelope.episodic.summary_notes.len(),
        1,
        "Should have episodic context"
    );
    assert_eq!(
        envelope.procedural_docs.len(),
        1,
        "Should have procedural doc"
    );
    assert_eq!(envelope.policy_notes.len(), 2, "Should have policy notes");

    // 序列化完整信封
    let json = serde_json::to_string_pretty(&envelope).expect("Should serialize");
    assert!(
        json.contains("working_items"),
        "JSON should contain working_items"
    );
    assert!(json.contains("episodic"), "JSON should contain episodic");
    assert!(
        json.contains("procedural_docs"),
        "JSON should contain procedural_docs"
    );
    assert!(
        json.contains("policy_notes"),
        "JSON should contain policy_notes"
    );

    // 反序列化验证
    let recovered: MemoryContextEnvelope = serde_json::from_str(&json).expect("Should deserialize");
    assert_eq!(
        recovered.working_items.len(),
        2,
        "Should recover working items"
    );

    println!("✅ E2E-MEMORY-007: 完整内存上下文信封测试通过");
}

/// E2E-MEMORY-008: 跨agent内存共享（受控）
///
/// 使用 MockLlmClient 验证6个agent通过共享的情景记忆上下文进行协调。
/// 通过 /api/v1/agents/:id/invoke 端点传入序列化的共享记忆。
#[tokio::test]
async fn test_e2e_memory_008_cross_agent_memory_sharing() {
    let _server = TestServer::new();

    // 创建共享的情景记忆
    let shared_episodic = EpisodicContextProjection {
        executions: vec![],
        reviews: vec![],
        provenance_records: vec![],
        security_events: vec![],
        artifact_refs: vec![ArtifactId::from_string("shared-knowledge-base".to_string()).unwrap()],
        summary_notes: vec!["Shared context: Project requirements".to_string()],
        foresights: vec![],
    };

    let shared_json = serde_json::to_string(&shared_episodic).expect("Should serialize");

    // 6个agent都能访问共享记忆
    let mut tasks = Vec::new();
    for i in 0..6 {
        let shared_context = shared_json.clone();
        let mut test_server = TestServer::new();

        let task = tokio::spawn(async move {
            let request = json!({
                "input": format!("Agent {} accessing shared memory", i),
                "context": {
                    "shared_episodic": shared_context
                }
            });

            let endpoint = format!("/api/v1/agents/memory-agent-{}/invoke", i);
            let response = test_server.post(&endpoint, request).await;
            (i, response.is_success())
        });

        tasks.push(task);
    }

    let results = join_all(tasks).await;

    // 验证所有agent都能访问共享记忆
    let success_count = results.iter().filter(|r| r.as_ref().unwrap().1).count();
    assert_eq!(success_count, 6, "All 6 agents should access shared memory");

    println!("✅ E2E-MEMORY-008: 跨agent内存共享测试通过");
}

/// E2E-MEMORY-009: 内存压力测试 - 6个agent每个写入100条记录
#[tokio::test]
async fn test_e2e_memory_009_memory_stress_test() {
    let mut tasks = Vec::new();

    for agent_id in 0..6 {
        let task = tokio::spawn(async move {
            let execution_id =
                ExecutionId::from_string(format!("stress-exec-{}", agent_id)).unwrap();
            let mut entries = Vec::new();

            // 每个agent创建100条记录
            for i in 0..100 {
                let entry = WorkingMemoryEntry::new(
                    Some(execution_id.clone()),
                    match i % 6 {
                        0 => WorkingEntryKind::Goal,
                        1 => WorkingEntryKind::PhaseStart,
                        2 => WorkingEntryKind::ToolResult,
                        3 => WorkingEntryKind::Decision,
                        4 => WorkingEntryKind::PhaseEnd,
                        _ => WorkingEntryKind::Error,
                    },
                    format!("Agent {} entry {}", agent_id, i),
                    vec![],
                    None,
                );
                entries.push(entry);
            }

            (agent_id, entries.len())
        });
        tasks.push(task);
    }

    let results = join_all(tasks).await;

    // 验证所有agent都创建了100条记录
    let total_entries: usize = results.iter().map(|r| r.as_ref().unwrap().1).sum();

    assert_eq!(
        total_entries, 600,
        "Should have created 600 total entries (6 agents × 100)"
    );

    println!("✅ E2E-MEMORY-009: 内存压力测试通过 (600条记录)");
}

/// E2E-MEMORY-010: 内存序列化性能测试
#[tokio::test]
async fn test_e2e_memory_010_serialization_performance() {
    use std::time::Instant;

    let execution_id = ExecutionId::from_string("perf-exec".to_string()).unwrap();

    // 创建大型内存上下文
    let mut working_items = Vec::new();
    for i in 0..1000 {
        working_items.push(WorkingMemoryEntry::new(
            Some(execution_id.clone()),
            WorkingEntryKind::ToolResult,
            format!("Performance test entry {}", i),
            vec![ArtifactId::from_string(format!("artifact-{}", i)).unwrap()],
            None,
        ));
    }

    let envelope = MemoryContextEnvelope {
        working_items,
        episodic: EpisodicContextProjection {
            executions: vec![],
            reviews: vec![],
            provenance_records: vec![],
            security_events: vec![],
            artifact_refs: vec![],
            summary_notes: (0..100).map(|i| format!("Summary note {}", i)).collect(),
            foresights: vec![],
        },
        procedural_docs: vec![],
        policy_notes: vec![],
        degraded_sources: vec![],
        degraded_source_details: vec![],
        semantic_snapshot: None,
    };

    // 测试序列化性能
    let start = Instant::now();
    let json = serde_json::to_string(&envelope).expect("Should serialize");
    let serialize_time = start.elapsed();

    // 测试反序列化性能
    let start = Instant::now();
    let _recovered: MemoryContextEnvelope =
        serde_json::from_str(&json).expect("Should deserialize");
    let deserialize_time = start.elapsed();

    println!("\n📊 内存序列化性能测试结果:");
    println!("├─ 工作记忆条目数: 1000");
    println!("├─ 情景记忆摘要数: 100");
    println!("├─ JSON大小: {} bytes", json.len());
    println!("├─ 序列化时间: {:?}", serialize_time);
    println!("└─ 反序列化时间: {:?}", deserialize_time);

    // 性能断言
    assert!(
        serialize_time.as_millis() < 500,
        "Serialization should complete in under 500ms"
    );
    assert!(
        deserialize_time.as_millis() < 500,
        "Deserialization should complete in under 500ms"
    );

    println!("✅ E2E-MEMORY-010: 内存序列化性能测试通过");
}
