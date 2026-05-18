# cyberclaw-memory-extraction

Cold-path memory extraction pipeline. Reads completed execution traces
and produces structured memory units that the agent runtime can recall
in future sessions.

The extraction is intentionally off-path: it never runs during a live
agent loop. It is invoked by background workers after a session ends,
or manually via admin endpoints, so the latency of the critical chain
is unaffected.

## Modules

| Module | Role |
|---|---|
| `memcell` | Boundary detection — identifies coherent stretches of execution that belong together |
| `atomic_fact` | Atomic-fact extraction — pulls out the smallest factual claims a session produced |
| `episode` | Episode extraction — groups atomic facts into narrative episodes |
| `foresight` | Foresight prediction — extracts what the agent expected vs. what happened |
| `quality` | Quality evaluation — scores extracted units before they are committed to long-term memory |
| `llm_extractors` | LLM-driven extraction primitives shared across the modules above |

## Where this fits

This crate sits next to `cyberclaw-store` (which holds the produced
memory) and `cyberclaw-control-plane` (which schedules extraction
runs). It does not depend on the agent runtime; the agent runtime
queries memory via the trait surfaces in `cyberclaw-core::memory` and
remains agnostic about how that memory was produced.

For the broader memory architecture, read
[`docs/architecture/memory/`](../../docs/architecture/memory/).

## Verifying locally

```
cargo build -p cyberclaw-memory-extraction
cargo test  -p cyberclaw-memory-extraction
cargo clippy -p cyberclaw-memory-extraction --all-targets -- -D warnings
```
