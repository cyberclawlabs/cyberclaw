//! Memory system performance benchmarks
//!
//! Target metrics:
//! - memory_context_query p95 < 50ms
//! - sync_compaction p95 < 100ms

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use cyberclaw_core::execution::{AgentRef, Execution, ExecutionBudget, ExecutionStatus};
use cyberclaw_core::ids::{AgentId, ExecutionId, TraceId};
use cyberclaw_core::memory::{CompactionStrategy, MemoryConfig, MemoryContextProvider};
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

fn setup_provider_with_data(entry_count: usize) -> MemoryContextProvider {
    let config = MemoryConfig::default();
    let provider = MemoryContextProvider::new(config);

    // 添加工作记忆条目
    for i in 0..entry_count {
        provider.add_working_entry(WorkingMemoryEntry {
            execution_id: Some(make_execution_id(&format!("exec-{}", i))),
            kind: WorkingEntryKind::ToolResult,
            summary: format!(
                "This is a test entry number {} with some content to simulate real data",
                i
            ),
            artifact_refs: vec![],
            trace_id: Some(make_trace_id(&format!("trace-{}", i))),
            encrypted: false,
        });
    }

    // 添加执行记录
    for i in 0..entry_count.min(50) {
        let exec = make_test_execution(&format!("exec-{}", i));
        provider.add_execution(exec);
    }

    // 添加摘要
    for i in 0..entry_count.min(50) {
        provider.add_summary(format!(
            "Execution {} completed successfully with result",
            i
        ));
    }

    // 添加程序性文档
    for i in 0..10 {
        let doc = ProceduralDocument {
            source: "benchmark".to_string(),
            title: format!("Rule {}", i),
            content: format!(
                "This is a procedural rule number {} with detailed content",
                i
            ),
        };
        let _ = provider.add_procedural_document(doc);
    }

    provider
}

fn bench_memory_context_query(c: &mut Criterion) {
    let mut group = c.benchmark_group("memory_context_query");

    for size in [10, 50, 100, 200].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            let provider = setup_provider_with_data(size);
            let request = MemoryContextRequest {
                case_id: None,
                execution_id: None,
                max_items: 50,
            };

            b.iter(|| {
                let context = provider.get_context(black_box(request.clone()));
                black_box(context);
            });
        });
    }

    group.finish();
}

fn bench_sync_compaction(c: &mut Criterion) {
    let mut group = c.benchmark_group("sync_compaction");

    for size in [50, 100, 200, 500].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            let provider = setup_provider_with_data(size);
            let strategy = CompactionStrategy::default();

            b.iter(|| {
                let result = provider.compact(black_box(strategy.clone()));
                black_box(result);
            });
        });
    }

    group.finish();
}

fn bench_add_working_entry(c: &mut Criterion) {
    let mut group = c.benchmark_group("add_working_entry");

    let provider = setup_provider_with_data(0);

    group.bench_function("add_single_entry", |b| {
        b.iter(|| {
            provider.add_working_entry(black_box(WorkingMemoryEntry {
                execution_id: Some(make_execution_id("test-exec")),
                kind: WorkingEntryKind::ToolResult,
                summary: "Test entry".to_string(),
                artifact_refs: vec![],
                trace_id: None,
                encrypted: false,
            }));
        });
    });

    group.finish();
}

fn bench_create_checkpoint(c: &mut Criterion) {
    let mut group = c.benchmark_group("create_checkpoint");

    for size in [50, 100, 200].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            let provider = setup_provider_with_data(size);

            b.iter(|| {
                let checkpoint = provider.create_compaction_checkpoint();
                black_box(checkpoint);
            });
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_memory_context_query,
    bench_sync_compaction,
    bench_add_working_entry,
    bench_create_checkpoint
);
criterion_main!(benches);
