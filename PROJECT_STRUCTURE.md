# Project structure

A tour of every directory and file that ships in the public release.
Excluded paths are documented in
[`RELEASE_MANIFEST.md`](RELEASE_MANIFEST.md).

## Top-level layout

```
cyberclaw/
├── README.md                  ← project overview, quickstart, scenarios
├── README.zh-CN.md            ← Simplified Chinese mirror
├── LICENSE                    ← Apache-2.0 full text
├── CHANGELOG.md               ← versioned change log (Keep a Changelog)
├── CONTRIBUTING.md            ← contribution guide
├── DEVELOPMENT.md             ← dev environment setup
├── SECURITY.md                ← vulnerability reporting and posture
├── ACKNOWLEDGMENTS.md         ← credits to research and dependencies
├── CITATIONS.md               ← academic, standards, framework refs
├── PROJECT_STRUCTURE.md       ← this file
├── RELEASE_MANIFEST.md        ← what's in / out, binary hashes
├── DOCUMENTATION_SYSTEM.md    ← how the docs/ tree is organized
│
├── Cargo.toml                 ← Rust workspace manifest
├── Cargo.lock                 ← exact dependency graph (reproducible)
├── deny.toml                  ← cargo-deny: license + advisory policy
├── package.json               ← npm: WebUI build (Babel) + e2e
├── package-lock.json          ← exact npm dependency graph
├── babel.config.js            ← JSX → JS preset
├── playwright.config.ts       ← E2E test configuration
├── Dockerfile                 ← multi-stage container build
├── docker-compose.yml         ← local dev stack
├── docker-compose.prod.yml    ← production-shaped stack
├── .dockerignore              ← keeps build context lean
├── .env.example               ← environment-variable template (no secrets)
├── .gitignore                 ← VCS exclusions
│
├── apps/                      ← runnable binaries (server + cli)
├── crates/                    ← 15 Rust workspace crates
├── ecosystem/                 ← shipped Agents/Skills/Connectors/Plugins
├── web/                       ← Admin Console SPA
├── bin/                       ← pre-compiled release binaries
├── docs/                      ← documentation portal
├── examples/                  ← runnable code samples
├── schemas/                   ← JSON Schemas for manifests
├── scripts/                   ← maintenance scripts
├── deploy/                    ← deployment recipes
└── .github/workflows/         ← CI definitions
```

## `apps/` — runnable binaries

```
apps/
├── cyberclaw-server/          ← HTTP server (axum) + admin console host
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs            ← startup, signal handling, env wiring
│       ├── lib.rs             ← router composition
│       ├── state.rs           ← AppState: every shared service
│       ├── api/               ← REST handlers (per-resource modules)
│       ├── middleware/        ← auth, rate limit, body limit, security headers
│       ├── audit.rs           ← append-only audit DB (sqlite + hash chain)
│       └── ...
│
└── cyberclaw-cli/             ← TUI: chat, doctor, skill, cluster, …
    ├── Cargo.toml
    └── src/
        ├── main.rs            ← clap subcommand dispatch
        ├── commands/          ← one file per top-level command
        │   ├── chat.rs        ← interactive REPL with an agent
        │   ├── onboard.rs     ← guided setup wizard
        │   ├── doctor.rs      ← 7-check health diagnostic
        │   ├── skill.rs       ← `skill list` (HTTP)
        │   ├── connector.rs   ← `connector list / register`
        │   ├── tools.rs       ← `tools state / promote / demote`
        │   ├── cluster.rs     ← register / heartbeat / state / assign
        │   ├── memory.rs      ← memory CRUD
        │   ├── audit.rs       ← snapshot / verify-chain / restore
        │   ├── mcp.rs         ← MCP server hot-reload
        │   ├── workflow.rs    ← workflow chains
        │   ├── review.rs      ← review queue
        │   └── ...
        └── http_client.rs     ← shared `get_json` / `post_json`
```

## `crates/` — workspace crates

15 crates with strict dependency direction (core ← runtime ← server).
Each has its own `README.md` with module-level details.

```
crates/
├── cyberclaw-core/            ← types, traits, manifests, ID newtypes
├── cyberclaw-control-plane/   ← orchestration: registry, resolver, executor, planners
├── cyberclaw-connectors/      ← Connector registry + dispatcher + native connectors
├── cyberclaw-governance/      ← policy engine, scanner, sanitizer, approval
├── cyberclaw-observability/   ← events, traces, metrics, audit projections
├── cyberclaw-agent-runtime/   ← agentic loop, sub-agent, prompt assembler, tool description
├── cyberclaw-skill-runtime/   ← skill hub, scanner, sandboxing, signature verify
├── cyberclaw-plugin-runtime/  ← platform plugin runtime
├── cyberclaw-store/           ← persistence: sqlite stores, audit DB, KV
├── cyberclaw-workflow/        ← workflow engine + state store
├── cyberclaw-llm/             ← LLM client abstraction (multi-provider)
├── cyberclaw-llm-bridge/      ← bridges LLM tool-use ↔ capability dispatch
├── cyberclaw-consensus/       ← Raft node + state machine for multi-replica
├── cyberclaw-scheduler/       ← cron + interval triggers, lease-based ownership
└── cyberclaw-memory-extraction/ ← cold-path extraction: MemCell / Episode / AtomicFact / Foresight
```

### Dependency direction

```
                       ┌──────────────────┐
                       │  apps/server     │
                       │  apps/cli        │
                       └────────┬─────────┘
                                │
        ┌──────────┬────────────┼─────────────┬───────────────┐
        ▼          ▼            ▼             ▼               ▼
  observability governance  control-plane  agent-runtime   skill-runtime
        │          │            │             │               │
        └──────────┴────────────┼─────────────┴───────────────┘
                                ▼
                          ┌──────────┐
                          │   core   │
                          └──────────┘
```

`connectors`, `store`, `workflow`, `consensus`, `scheduler`, `llm`,
`llm-bridge`, `plugin-runtime`, `memory-extraction` are leaf crates
that depend only on `core` (and tokio + serde).

For a one-paragraph explanation of every crate's role, read
[`docs/modules/README.md`](docs/modules/README.md).

## `ecosystem/` — shipped extension packages

Real, runnable extensions that demonstrate every extension point.

```
ecosystem/
├── agents/             (11)   ← role definitions
├── skills/            (113)   ← methodologies: SKILL.md + scripts/ + references/
├── connectors/          (9)   ← capability backends (incl. wallet-eth, safe-multisig, signer-vault for Web3)
└── platform-plugins/    (2)   ← server-level enhancements
```

Each entry has its own manifest:

| Object | Manifest filename | Role |
|---|---|---|
| Agent | `agent.toml` | Who is doing it (role + skill set) |
| Skill | `SKILL.md` (frontmatter) or `skill.toml` | How it should be done |
| Connector | `connector.toml` | What backend executes capabilities |
| Capability | declared inside its owning Connector | The minimum action unit |
| Platform Plugin | `plugin.toml` | Cross-cutting platform enhancement |

See [`docs/builders/`](docs/builders/) for building each of these.

## `web/` — Admin Console (React SPA)

A bundlerless React 18 SPA loaded from CDN UMDs. Babel compiles JSX to
`web/dist/*.js`; the server serves `web/cyberclaw.html` as the SPA
shell.

```
web/
├── cyberclaw.html             ← SPA shell HTML (served at /admin)
├── src/                       ← 34 .jsx files (one page = one file)
│   ├── app.jsx                ← root component, Operate/Govern split
│   ├── shell.jsx              ← navigation rail and chrome
│   ├── api.jsx                ← all REST client calls (window.cc.api)
│   ├── ui.jsx                 ← shared design system primitives
│   ├── icons.jsx              ← SVG icon set
│   ├── i18n.jsx               ← bilingual (en / zh-CN) string table
│   ├── data.jsx               ← static data + helpers
│   ├── onboarding.jsx         ← guided setup overlay
│   ├── pages_a.jsx            ← Status, Agents, Skills, Tasks, …
│   ├── pages_b.jsx            ← Reviews, Clarifications, Handoffs, Audit
│   ├── pages_c.jsx            ← Capabilities, Channels, Nodes, Memory Console
│   ├── pages_kanban.jsx
│   ├── pages_browser_console.jsx
│   ├── pages_multimodal.jsx
│   ├── pages_im_platforms.jsx
│   ├── pages_curator.jsx
│   ├── pages_moa.jsx          ← Mixture-of-Agents
│   ├── pages_capability_monitor.jsx
│   ├── pages_tools.jsx        ← Tools (active vs deferred)
│   ├── pages_cluster.jsx      ← Cluster brains and sessions
│   └── pages_*.jsx
└── dist/                      ← Babel-compiled output (also published)
```

Build: `npm run build:web`.

## `bin/` — pre-compiled binaries

```
bin/
├── README.md
└── darwin-arm64/
    ├── cyberclaw-server       (~38 MB)
    └── cyberclaw-cli          (~13 MB)
```

SHA-256 hashes in [`RELEASE_MANIFEST.md`](RELEASE_MANIFEST.md).

## `docs/` — documentation portal

Layered by audience and lifecycle. The full table of contents is
[`docs/INDEX.md`](docs/INDEX.md).

```
docs/
├── README.md                  ← portal landing
├── INDEX.md                   ← role-based reading paths
├── PRODUCTION_READINESS_CHECKLIST.md
├── ENVIRONMENT_VARIABLES.md
│
├── getting-started/           ← installation, quickstart, deployment
├── architecture/              ← high-level design (overview, runtime, memory, governance, retrieval)
├── api/                       ← HTTP API surface
├── reference/                 ← lookup tables (api / cli / manifests)
├── builders/                  ← build a connector / skill / plugin
├── guides/                    ← topic-focused walkthroughs
├── deployment/                ← production operations
├── security/                  ← governance and audit posture
├── business/                  ← non-engineering audiences
├── user-guide/                ← end-user / operator documentation
├── templates/                 ← starter manifests
├── web3/                      ← Web3 deployment surface
├── modules/                   ← compact one-paragraph guide to every crate
└── configuration/             ← unified env / config / governance reference
```

Conventions and update rules live in
[`DOCUMENTATION_SYSTEM.md`](DOCUMENTATION_SYSTEM.md).

## `examples/`

```
examples/
└── p2_demo.rs                 ← container runtime + cron scheduler + runtime selector
```

Run with `cargo run --example p2_demo`.

## `schemas/`

JSON Schemas for every manifest format. Use these for editor
completion (VS Code, JetBrains) and CI validation.

```
schemas/
├── agent.schema.json
├── skill.schema.json
├── connector.schema.json
└── plugin.schema.json
```

## `scripts/`

```
scripts/
├── deploy/                    ← deployment recipes
├── load-test.py               ← simple load generator for /chat
├── validate-docs.sh           ← internal-link auditor
└── check_markdown_links.py    ← Markdown link auditor
```

## `deploy/`

Production deployment artifacts (k8s manifests, systemd units, container
recipes).

## `.github/workflows/`

```
.github/workflows/
└── ci.yml                     ← cargo build/test/clippy + npm build:web
```
