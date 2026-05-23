# CyberClaw

- Status: Active
- Scope: Repository
- Owner: CyberClaw Maintainers
- Last Updated: 2026-04-18

<div align="center">

**Governable agent infrastructure for high-stakes, real-world systems**

[![Rust](https://img.shields.io/badge/rust-1.75%2B-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)
[![Docs](https://img.shields.io/badge/docs-portal-blue.svg)](docs/README.md)

[English](README.md) | [简体中文](README.zh-CN.md)

</div>

### What Is CyberClaw?

CyberClaw is not trying to build a more conversational Agent. It is built to let Agents participate in high-stakes, real-world systems without giving up safety, control, or auditability.

It is for teams that already want AI in analysis, coordination, and execution, but cannot accept a model directly touching production systems. Security teams can use it for code audit, alert triage, incident response, and audit trails. AI teams can use it to move Agents from demos into real workflows. Lean teams can use it to approach a one-person SOC model, covering more analysis, response, and operations with less headcount.

The core move is not “add more tools to the model.” CyberClaw separates reasoning, execution, governance, audit, and external integration into explicit boundaries, so automation is built on controlled execution, policy checks, approval paths, and traceable artifacts.

CyberClaw is a general governable Agent platform, with Web3 as its strongest current deployment surface. In environments involving wallets, signers, treasuries, multisig operations, on-chain workflows, and incident handling, governance and execution boundaries are not optional features. They are the starting condition.

### Core Scenarios

#### Web3-first scenarios

CyberClaw is designed for workflows such as:

- treasury and multisig pipelines that require context gathering, approvals, and traceability before execution
- signer-gated on-chain runbooks where policy, execution, and audit must remain separate
- protocol operations that unify on-chain actions, external systems, and internal approvals
- chain incident handling that needs alerts, escalation, response, and auditable follow-up

#### Other high-stakes scenarios

The same control model also maps well to:

- code audit and PR risk review
- alert triage, escalation, and incident response
- release gates, rollback proposals, and change approval
- governed database queries, writes, transactions, and migrations

#### Current repository support

The current repository already includes adjacent connectors and references for:

- GitHub issue / PR / review flows via the GitHub connector example
- Slack messaging / channel creation / file upload flows via the Slack connector example
- governed database query / execute / transaction / migration patterns via the database connector example
- deployment, health-check, and production setup guidance in the deployment docs
- audit enrichment and security event infrastructure in the platform plugin and observability docs

### Example Governed Flows

These are the kinds of workflows CyberClaw is built to structure:

| Scenario | Example governed flow |
|--------|---------|
| Web3 treasury operation | An `Agent` gathers balances, requests, signer context, and policy inputs; a `Skill` turns them into an execution proposal; governance applies approval and policy checks before a wallet-related `Connector` exposes the allowed `Capability`; approvals, traces, and artifacts are recorded. |
| Code audit / PR risk review | An `Agent` collects pull request context, changed files, and policy inputs; a `Skill` structures the review method; the GitHub `Connector` provides repository collaboration context; MCP capabilities such as `mcp.prompt.code_review` can supply governed review prompts; governance still controls follow-up writes. |
| Alert triage / escalation | An `Agent` receives an alert, pulls traces, logs, and related repository context, and assembles a triage proposal; Slack `Connector` flows can notify operators, while GitHub `Connector` flows can create follow-up issues; governance constrains risky next steps. |
| Security incident response | An `Agent` proposes containment, escalation, patch coordination, or investigation steps after triage; governance gates operational changes and external writes; the platform keeps an auditable chain of alerts, approvals, actions, and artifacts. |
| Release gate / change approval | An `Agent` turns a release task, change request, or production incident into a governed workflow across GitHub and Slack; it can prepare issues, PR context, checklists, and notifications, but only through approved `Connector` and `Capability` boundaries. |
| Database change gate | An `Agent` analyzes SQL, migration plans, and impact scope before execution; the Database `Connector` exposes bounded capabilities such as `db.query`, `db.execute`, `db.transaction`, and `db.migrate`, each with different risk levels, so governance can require stronger approval and isolation for high-risk changes. |

### Start Here

- [Docs Portal](docs/README.md)
- [Getting Started](docs/getting-started/README.md)
- [Builder Guide](docs/builders/README.md)
- [Security & Governance](docs/security/README.md)
- [Web3 Guide](docs/web3/README.md)
- [Skill Hub MVP](docs/business/brand/SKILL_HUB_MVP.md)
- [I18N Content Strategy](docs/business/brand/I18N_CONTENT_STRATEGY.md)

### Languages

Current repository-facing language support:

- `en` - canonical open-source entry
- `zh-CN` - maintained localized entry

Planned public site expansion:

- `ja`
- `ko`
- `es`

### Who CyberClaw Is For

#### Users and Integrators

- Run agents with explicit execution boundaries
- Add skills and connectors without bypassing governance
- Explore Web3 and other high-stakes automation scenarios

#### Ecosystem Builders

- Build and publish Skills, Connectors, and Platform Plugins
- Reuse the platform's controlled execution model
- Extend CyberClaw without breaking the five-object boundary

### Quickstart

```bash
git clone https://github.com/cyberclawlabs/cyberclaw.git
cd cyberclaw
cargo build
cargo run -p cyberclaw-cli -- --help
```

To inspect the local package surface:

```bash
cargo run -p cyberclaw-cli -- status
```

### Why CyberClaw

- Governable execution instead of unconstrained agent behavior
- Connector and Capability boundaries for controlled actions
- Audit, traceability, and observability as first-class concerns
- Extensible Skill / Connector / Platform Plugin surface
- Strong fit for Web3 and other high-stakes environments

### Platform Building Blocks

CyberClaw is organized around five platform objects:

| Object | Purpose |
|--------|---------|
| `Agent` | Role, orchestration, execution budget |
| `Skill` | Knowledge, method, prompt, references |
| `Connector` | Runtime and external system integration |
| `Capability` | Smallest governed action unit |
| `Platform Plugin` | Platform-level enhancement hooks |

For external builders, the easiest entry is still:

- `Skill`: how the agent should work
- `Tool` surface: how governed capabilities are exposed externally

Internally, execution remains bound to the platform chain:

`Task/Case -> Resolver -> Execution -> Governance -> Connector -> Capability -> Artifact/Trace`

### Web3 Today

CyberClaw is a general platform. Web3 is currently the strongest scenario for showing why governance, controlled execution, auditability, and risk-aware automation matter.

See [Web3 Guide](docs/web3/README.md).

### Documentation Map

- [Docs Index](docs/INDEX.md)
- [Architecture](docs/architecture/README.md)
- [Implementation](docs/implementation/README.md)
- [Business](docs/business/README.md)
- [Contributing](CONTRIBUTING.md)
- [Security Policy](SECURITY.md)
- [Changelog](CHANGELOG.md)

### Current Status

#### Implemented

- Core platform crates and runtime layers
- Governance, observability, and isolation foundations
- CLI and server entry points
- Architecture and implementation documentation base

#### In Progress

- Public-facing product docs reshaping
- Open-source launch surface refinement
- Skill Hub discovery surface design

#### Roadmap

- Dedicated GitHub Pages homepage at `cyberclawlabs.ai`
- Independent Skill Hub experience
- Expanded builder-facing ecosystem workflows

### Contact

- Public contact: `info@cyberclawlabs.ai`

Do not use this README as the sole source of implementation truth. For current implementation reality, combine code, tests, implementation reports, and review records.
