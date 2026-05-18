use criterion::{black_box, criterion_group, criterion_main, Criterion};
use cyberclaw_consensus::raft::{LogEntry, RaftLog, RaftState};

fn bench_log_append(c: &mut Criterion) {
    c.bench_function("log_append", |b| {
        let mut log = RaftLog::new();
        let mut index = 0u64;

        b.iter(|| {
            index += 1;
            let entry = LogEntry::new(index, 1, vec![1, 2, 3, 4]);
            log.append(black_box(entry));
        });
    });
}

fn bench_log_get(c: &mut Criterion) {
    c.bench_function("log_get", |b| {
        let mut log = RaftLog::new();

        // Populate log
        for i in 1..=1000 {
            log.append(LogEntry::new(i, 1, vec![1, 2, 3, 4]));
        }

        b.iter(|| {
            log.get(black_box(500));
        });
    });
}

fn bench_state_transitions(c: &mut Criterion) {
    c.bench_function("state_transitions", |b| {
        let mut state = RaftState::new();
        let mut term = 0u64;

        b.iter(|| {
            term += 1;
            state.become_candidate();
            state.become_leader();
            state.become_follower(black_box(term));
        });
    });
}

fn bench_log_truncate(c: &mut Criterion) {
    c.bench_function("log_truncate", |b| {
        b.iter_batched(
            || {
                let mut log = RaftLog::new();
                for i in 1..=100 {
                    log.append(LogEntry::new(i, 1, vec![1, 2, 3, 4]));
                }
                log
            },
            |mut log| {
                log.truncate(black_box(50));
            },
            criterion::BatchSize::SmallInput,
        );
    });
}

criterion_group!(
    benches,
    bench_log_append,
    bench_log_get,
    bench_state_transitions,
    bench_log_truncate
);
criterion_main!(benches);
