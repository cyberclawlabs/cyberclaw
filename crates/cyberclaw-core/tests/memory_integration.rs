use cyberclaw_core::execution::{AgentRef, Execution, ExecutionBudget, ExecutionStatus};
use cyberclaw_core::ids::{AgentId, ExecutionId, TraceId};
use cyberclaw_core::memory::{MemoryConfig, MemoryContextProvider};
use cyberclaw_core::memory_context::{
    MemoryContextRequest, ProceduralDocument, WorkingEntryKind, WorkingMemoryEntry,
};

fn make_execution_id(s: &str) -> ExecutionId {
    ExecutionId::from_string(s.to_string()).unwrap()
}

fn make_agent_id(s: &str) -> AgentId {
    AgentId::from_string(s.to_string()).unwrap()
}

fn make_trace_id(s: &str) -> TraceId {
    TraceId::from_string(s.to_string()).unwrap()
}

fn make_test_execution(id: &str) -> Execution {
    let exec_id = make_execution_id(id);
    let agent_id = make_agent_id("test-agent");

    Execution {
        id: exec_id.clone(),
        root_execution_id: exec_id.clone(),
        parent_execution_id: None,
        owner_node_id: None,
        scheduled_node_id: None,
        placement_group: None,
        lease_id: None,
        handoff_count: 0,
        case_id: None,
        task_id: None,
        agent: AgentRef {
            id: agent_id,
            role: "test".to_string(),
        },
        status: ExecutionStatus::Pending,
        join_strategy: None,
        budget: ExecutionBudget::default(),
        workspace: None,
        trace_id: make_trace_id("trace-001"),
        risk_level: cyberclaw_core::capability::RiskLevel::Low,
        execution_mode: cyberclaw_core::execution::ExecutionMode::Normal,
        started_at: None,
        finished_at: None,
    }
}

#[test]
fn test_memory_integration_full_workflow() {
    // 创建记忆提供者
    let config = MemoryConfig::default();
    let provider = MemoryContextProvider::new(config);

    // 1. 添加工作记忆
    let exec_id = make_execution_id("exec-001");
    provider.add_working_entry(WorkingMemoryEntry {
        execution_id: Some(exec_id.clone()),
        kind: WorkingEntryKind::PhaseStart,
        summary: "Starting data processing phase".to_string(),
        artifact_refs: vec![],
        trace_id: Some(make_trace_id("trace-001")),
        encrypted: false,
    });

    provider.add_working_entry(WorkingMemoryEntry {
        execution_id: Some(exec_id.clone()),
        kind: WorkingEntryKind::ToolResult,
        summary: "Scanned 100 files successfully".to_string(),
        artifact_refs: vec![],
        trace_id: Some(make_trace_id("trace-001")),
        encrypted: false,
    });

    provider.add_working_entry(WorkingMemoryEntry {
        execution_id: Some(exec_id.clone()),
        kind: WorkingEntryKind::PhaseEnd,
        summary: "Data processing phase completed".to_string(),
        artifact_refs: vec![],
        trace_id: Some(make_trace_id("trace-001")),
        encrypted: false,
    });

    // 2. 添加情景记忆
    let execution = make_test_execution("exec-001");
    provider.add_execution(execution);
    provider.add_summary("Processing phase completed successfully with no errors".to_string());

    // 3. 添加程序性文档
    let doc = ProceduralDocument {
        source: "security-manual".to_string(),
        title: "Data Processing Guidelines".to_string(),
        content: "Always validate input data before processing".to_string(),
    };
    provider
        .add_procedural_document(doc)
        .expect("Should add document");

    // 4. 获取上下文
    let request = MemoryContextRequest {
        case_id: None,
        execution_id: Some(exec_id.clone()),
        max_items: 10,
    };

    let context = provider.get_context(request);

    // 验证工作记忆（按添加顺序）
    assert_eq!(context.working_items.len(), 3);
    assert_eq!(
        context.working_items[0].summary,
        "Starting data processing phase"
    );
    assert_eq!(
        context.working_items[1].summary,
        "Scanned 100 files successfully"
    );
    assert_eq!(
        context.working_items[2].summary,
        "Data processing phase completed"
    );

    // 验证情景记忆
    assert_eq!(context.episodic.executions.len(), 1);
    assert_eq!(context.episodic.executions[0].id, exec_id);
    assert_eq!(context.episodic.summary_notes.len(), 1);
    assert_eq!(
        context.episodic.summary_notes[0],
        "Processing phase completed successfully with no errors"
    );

    // 验证程序性文档
    assert_eq!(context.procedural_docs.len(), 1);
    assert_eq!(
        context.procedural_docs[0].title,
        "Data Processing Guidelines"
    );

    // 5. 测试上下文大小限制
    assert!(provider.get_context_size() > 0);
    assert!(!provider.is_context_too_large());

    // 6. 测试压缩检查点（为 Agent 8 预留）
    let checkpoint = provider.create_compaction_checkpoint();
    assert_eq!(checkpoint.working_entries.len(), 3);
    assert_eq!(checkpoint.procedural_docs.len(), 1);
    assert!(!checkpoint.timestamp.is_empty());

    // 7. 测试清空功能
    provider.clear_all();
    let empty_request = MemoryContextRequest {
        case_id: None,
        execution_id: None,
        max_items: 10,
    };
    let empty_context = provider.get_context(empty_request);
    assert!(empty_context.working_items.is_empty());
    assert!(empty_context.episodic.executions.is_empty());
    assert!(empty_context.procedural_docs.is_empty());
}

#[test]
fn test_memory_context_truncation() {
    // 创建小容量配置
    let config = MemoryConfig {
        working_max_entries: 2,
        working_max_chars: 100,
        episodic_max_executions: 2,
        max_context_chars: 50,
        ..Default::default()
    };
    let provider = MemoryContextProvider::new(config);

    // 添加超出限制的内容
    for i in 0..5 {
        provider.add_working_entry(WorkingMemoryEntry {
            execution_id: None,
            kind: WorkingEntryKind::ToolResult,
            summary: format!("Entry number {} with some content", i),
            artifact_refs: vec![],
            trace_id: None,
            encrypted: false,
        });

        provider.add_execution(make_test_execution(&format!("exec-{:03}", i)));
    }

    // 获取上下文
    let request = MemoryContextRequest {
        case_id: None,
        execution_id: None,
        max_items: 10,
    };

    let context = provider.get_context(request);

    // 验证截断
    assert!(context.working_items.len() <= 5); // max_items / 2
    assert!(context.episodic.executions.len() <= 2); // 配置的最大值
}

#[test]
fn test_memory_execution_filtering() {
    let config = MemoryConfig::default();
    let provider = MemoryContextProvider::new(config);

    let exec_id_1 = make_execution_id("exec-001");
    let exec_id_2 = make_execution_id("exec-002");

    // 添加不同执行的条目
    provider.add_working_entry(WorkingMemoryEntry {
        execution_id: Some(exec_id_1.clone()),
        kind: WorkingEntryKind::Goal,
        summary: "Goal for exec 1".to_string(),
        artifact_refs: vec![],
        trace_id: None,
        encrypted: false,
    });

    provider.add_working_entry(WorkingMemoryEntry {
        execution_id: Some(exec_id_2.clone()),
        kind: WorkingEntryKind::Goal,
        summary: "Goal for exec 2".to_string(),
        artifact_refs: vec![],
        trace_id: None,
        encrypted: false,
    });

    provider.add_working_entry(WorkingMemoryEntry {
        execution_id: Some(exec_id_1.clone()),
        kind: WorkingEntryKind::Decision,
        summary: "Decision for exec 1".to_string(),
        artifact_refs: vec![],
        trace_id: None,
        encrypted: false,
    });

    // 获取特定执行的上下文
    let request = MemoryContextRequest {
        case_id: None,
        execution_id: Some(exec_id_1.clone()),
        max_items: 10,
    };

    let context = provider.get_context(request);

    // 验证过滤
    assert_eq!(context.working_items.len(), 2);
    for item in &context.working_items {
        assert_eq!(item.execution_id, Some(exec_id_1.clone()));
    }
}
