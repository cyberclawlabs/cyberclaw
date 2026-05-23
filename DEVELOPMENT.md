# Development Guide

Local development for the CyberClaw workspace.

For project overview see [`README.md`](README.md). For architecture and platform usage see [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) and [`docs/GUIDE.md`](docs/GUIDE.md).

## Prerequisites

Required:

- Rust 1.75+ ([rustup](https://rustup.rs))
- Cargo
- Git

Recommended:

- `rust-analyzer` for IDE integration
- `cargo-nextest` for faster test runs
- `cargo-watch` for build-on-change
- `lldb` or `gdb` for native debugging
- Node.js 18+ and `pnpm` for WebUI work

## Workspace layout

Defined in `Cargo.toml` (`[workspace.members]`):

**Applications**

- `apps/cyberclaw-server` — HTTP server + WebUI host
- `apps/cyberclaw-cli` — operator CLI

**Core crates**

- `cyberclaw-core` — types, traits, protocol
- `cyberclaw-control-plane` — registration, resolution, dispatch
- `cyberclaw-agent-runtime` — agentic loop, sub-agents
- `cyberclaw-workflow` — declarative workflows
- `cyberclaw-governance` — policy engine, approval flow
- `cyberclaw-observability` — events, traces, metrics
- `cyberclaw-connectors` — external system bridges
- `cyberclaw-skill-runtime` — Skill loading and execution
- `cyberclaw-store` — persistence
- `cyberclaw-scheduler` — cron and workflow triggers
- `cyberclaw-plugin-runtime` — platform plugins
- `cyberclaw-llm` / `cyberclaw-llm-bridge` — LLM client + protocol bridge
- `cyberclaw-consensus` — Raft for cluster mode
- `cyberclaw-memory-extraction` — semantic memory

## Build

```bash
cargo build                            # debug
cargo build --release                  # release
cargo build --release -p cyberclaw-server   # one app
cargo check --workspace                # type check without codegen
```

## Run

```bash
cargo run -p cyberclaw-cli -- doctor   # CLI health check
cargo run -p cyberclaw-server          # server on 127.0.0.1:38090
```

Required env vars for a server run are listed in `.env.example`.

## Test

```bash
cargo test --workspace                          # everything
cargo test -p cyberclaw-control-plane           # one crate
cargo test --workspace --no-fail-fast 2>&1 | grep "test result:"   # summary line
```

Integration tests live under `tests/` at workspace root; unit tests live inline with each crate.

## Quality gates

Run before sending a PR:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The CI checks the same three steps; failing locally is a faster signal.

## Debugging

```bash
lldb target/debug/cyberclaw-server     # macOS / Linux
gdb  target/debug/cyberclaw-server     # Linux
```

Set `RUST_LOG=debug` (or `RUST_LOG=cyberclaw_server=debug,info` to filter) for tracing-level logs.

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md) for branching, commit, and PR conventions.
