use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use cyberclaw_store::{InMemoryLeveledStore, LeveledMemoryRecord, LeveledMemoryStore, MemoryLevel};
use std::sync::Arc;
use tokio::runtime::Runtime;

fn memory_write_bench(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("memory_store");
    for &size in &[100usize, 1000, 10000] {
        group.bench_with_input(
            BenchmarkId::new("store_leveled", size),
            &size,
            |b, &size| {
                let store = Arc::new(InMemoryLeveledStore::new());
                b.to_async(&rt).iter(|| {
                    let store = Arc::clone(&store);
                    async move {
                        for i in 0..size {
                            let now = chrono::Utc::now();
                            let record = LeveledMemoryRecord {
                                id: format!("bench-{}", i),
                                session_id: "bench-session".to_string(),
                                agent_id: "bench-agent".to_string(),
                                level: MemoryLevel::L1Summary,
                                key: format!("key-{}", i),
                                content: serde_json::Value::String(format!("test content {}", i)),
                                created_at: now,
                                updated_at: now,
                                ttl_seconds: MemoryLevel::L1Summary.default_ttl_seconds(),
                                source_execution_id: None,
                                embedding: None,
                                tags: Vec::new(),
                            };
                            let _ = store.store_leveled(record).await;
                        }
                    }
                });
            },
        );
    }
    group.finish();
}

criterion_group!(benches, memory_write_bench);
criterion_main!(benches);
