# Development Guide

- Status: Active
- Scope: Repository
- Owner: CyberClaw Maintainers
- Last Updated: 2026-03-20

This document covers local development for the current CyberClaw workspace.

## Read This First

Before making structural or architectural changes, read:

1. [DOCUMENTATION_SYSTEM.md](DOCUMENTATION_SYSTEM.md)
2. [DOCUMENT_METADATA_TEMPLATE.md](DOCUMENT_METADATA_TEMPLATE.md)
3. [docs/INDEX.md](docs/INDEX.md)
4. [docs/architecture/overview/ARCHITECTURE_V2.0.md](docs/architecture/overview/ARCHITECTURE_V2.0.md)
5. [CLAUDE.md](CLAUDE.md)
6. [AGENTS.md](AGENTS.md)

## Prerequisites

Required:

- Rust 1.75+
- Cargo
- Git

Recommended:

- rust-analyzer
- cargo-nextest
- cargo-watch
- lldb or gdb

## Workspace Layout

Current workspace members are defined in `Cargo.toml`.

### Applications

- `apps/cyberclaw-cli`
- `apps/cyberclaw-server`

### Crates

- `crates/cyberclaw-core`
- `crates/cyberclaw-control-plane`
- `crates/cyberclaw-agent-runtime`
- `crates/cyberclaw-workflow`
- `crates/cyberclaw-governance`
- `crates/cyberclaw-observability`
- `crates/cyberclaw-connectors`
- `crates/cyberclaw-skill-runtime`
- `crates/cyberclaw-store`

## Build

```bash
cargo build
cargo build --release
cargo check --workspace
```

## Run

### CLI

```bash
cargo run -p cyberclaw-cli -- status
```

### Server

```bash
cargo run -p cyberclaw-server
```

Current application entrypoints are scaffold-level; check each app `src/main.rs` before assuming runtime behavior.

## Test

```bash
cargo test --workspace
cargo test -p cyberclaw-control-plane
cargo test -p cyberclaw-connectors
```

## Quality Gates

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Debugging

```bash
cargo build
lldb target/debug/cyberclaw-server
lldb target/debug/cyberclaw-cli
```

or:

```bash
gdb target/debug/cyberclaw-server
gdb target/debug/cyberclaw-cli
```

## Documentation Sync Rules

If you change object model, workspace structure, execution chain, governance flow, memory boundaries, retrieval boundaries, or roadmap interpretation, update documentation in the same change.

Minimum sync targets:

1. affected document under `docs/`
2. local `README.md` for that directory when applicable
3. [docs/INDEX.md](docs/INDEX.md) if navigation changed
4. root entry files when the change affects project-wide understanding
5. crate-local README or CHANGELOG when a single crate boundary changes
