# Contributing to CyberClaw

CyberClaw welcomes contributions from platform developers, ecosystem builders, and documentation contributors.

## Where You Can Contribute

### 1. Platform

Contribute to the runtime, governance, observability, control-plane, or connector layers.

Good fit if you work on:

- Rust platform code
- execution and governance boundaries
- runtime isolation
- tracing, audit, and platform operations

### 2. Ecosystem

Contribute to the extension surface around:

- Skills
- Connectors
- Platform Plugins
- examples and templates

Good fit if you want to help builders adopt CyberClaw without modifying the whole core platform.

### 3. Documentation

Contribute to:

- Getting Started
- User Guide
- Builder Guide
- Web3 Guide
- Security & Governance
- Reference docs

## Before You Start

Read these first:

1. [README.md](README.md)
2. [Docs Portal](docs/README.md)
3. [AGENTS.md](AGENTS.md)
4. [CLAUDE.md](CLAUDE.md)

If your change touches architecture, also read [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Contribution Rules

### Respect Platform Boundaries

CyberClaw keeps five platform objects:

- `Agent`
- `Skill`
- `Connector`
- `Capability`
- `Platform Plugin`

Do not:

- promote `Tool` into a new top-level platform object
- bypass `Connector -> Capability`
- let `Skill` become a hidden execution engine
- add broad abstractions without a concrete need

### Engineering Principles

Follow:

- KISS
- YAGNI
- DRY
- SOLID

Practical expectations:

- keep changes small and scoped
- modify the correct boundary instead of patching around it
- update docs when behavior or boundaries change
- do not present design intent as shipped implementation

## Development Workflow

1. Fork the repository.
2. Create your branch in your fork.
3. Make a focused change.
4. Add or update tests when behavior changes.
5. Update docs when user-facing behavior, architecture boundaries, or extension flows change.
6. Open a pull request with a clear explanation of the scope.

## Development Setup

```bash
git clone https://github.com/cyberclawlabs/cyberclaw.git
cd cyberclaw
cargo build
cargo test --workspace
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
```

If you only changed a specific crate or document set, say so clearly in your PR and describe what you verified.

## Documentation Expectations

When you add or change public-facing guidance:

- keep terminology consistent
- distinguish `Implemented`, `In Progress`, and `Roadmap`
- link to real files
- prefer direct, technical language over hype

When you add or change extension guidance:

- make the boundary of `Skill`, `Connector`, and `Platform Plugin` explicit
- document required prerequisites and runtime assumptions
- avoid implying privileged execution where none exists

## Security Reporting

Do not open a public issue for security vulnerabilities.

Use [SECURITY.md](SECURITY.md) and contact `info@cyberclawlabs.ai`.

## Pull Request Guidance

Your PR should explain:

1. what changed
2. why it changed
3. what you verified
4. what docs were updated
5. whether the change affects users, builders, or both

## Good First Contributions

Examples of useful contribution scopes:

- improve a builder guide
- add a clearer quickstart
- tighten a security explanation
- add extension examples
- fix mismatches between docs and implementation

## Communication

Keep discussions technical, respectful, and evidence-based.

CyberClaw values architectural clarity, governance discipline, and truthful public documentation over speed-only changes.
