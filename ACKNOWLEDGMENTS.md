# Acknowledgments

CyberClaw is original code, but it is not original ideas. The
abstractions in this codebase — agent / skill / connector / capability
/ platform plugin, the canonical execution chain, the audit triple,
the trust matrix for skill installation — emerged from comparative
reading across a wide body of academic and open-source work. Where
specific patterns are recognizably borrowed, the relevant files
identify them in their headers.

For academic citations and standards references, see
[CITATIONS.md](CITATIONS.md).

## Research influence (academic)

The system's shape was informed by ongoing work in agentic systems,
governable autonomy, persistent execution, and structured agent
memory. Among the academic works we read most closely:

- **HyperAgents** (arXiv:2603.19461) — multi-agent orchestration,
  role specialization, sub-agent budget contracts. Reflected in
  `crates/cyberclaw-agent-runtime/src/sub_agent.rs`.
- **MemOS** — operating-system framing for agent memory, the L0/L1/L2
  tier model, procedural-memory-as-files. Reflected in
  `docs/architecture/memory/`.
- **Reflexion** (arXiv:2303.11366) — verifier-feedback retry pattern.
  Reflected in `crates/cyberclaw-control-plane/src/persistent_loop.rs`.
- **MemGPT** (arXiv:2310.08560) — hierarchical context management.
  Reflected in the compress/recall pipeline.
- **ReAct** (arXiv:2210.03629) — reasoning ↔ tool-use alternation as
  the foundation of the agentic loop.
- **Constitutional AI** (arXiv:2212.08073) — background for the
  iron-law approach to non-rationalizable governance rules in
  `crates/cyberclaw-governance/`.

## Architectural comparison (open-source projects)

Before settling on the five-object model, we did a side-by-side
comparison of how peer projects organize memory, sandboxing, and
governance. The internal write-up (`REFERENCE_PROJECTS_ANALYSIS.md`)
mapped each of these to specific design decisions in CyberClaw:

- **[NanoClaw](https://github.com/qwibitai/nanoclaw)** — file-based memory
  (`CLAUDE.md` per-group) + SQLite episodic memory + container
  isolation per session. Influenced CyberClaw's procedural-memory-as-
  files pattern and the per-agent isolation boundary.
- **[openclaw](https://github.com/openclaw/openclaw)** — SOC threat-
  hunting agent. The `approval_gate` pattern and chain-log audit
  shape directly inform CyberClaw's review queue + hash-chained audit
  log.
- **[IronClaw-NearAI](https://github.com/nearai/ironclaw)** —
  Rust-implemented enterprise agent. PostgreSQL + pgvector hybrid
  search (RRF), WASM + Docker sandbox layering, and the policy engine
  contract are all reference points for `cyberclaw-governance` and
  the Connector dispatcher.
- **[Cline](https://github.com/cline/cline)** — VS Code agent.
  Workspace snapshots, human-in-the-loop UX, and MCP integration
  patterns reflected in the approval card flow and MCP tool bridge.
- **[OpenCode](https://github.com/anomalyco/opencode)** — open-source coding
  agent. Client/server architecture and LSP/Effect framework boundaries
  informed our split between control-plane and connectors.

## Self-evolution and persistent execution lineage

CyberClaw's autopilot, persistent loop, and skill evolution paths sit
in a specific lineage of "agent that improves while it runs":

- **[ralph](https://github.com/snarktank/ralph)** — the canonical
  "loop until the task is done" pattern. CyberClaw's PersistentExecution
  module (`crates/cyberclaw-control-plane/src/persistent_loop.rs`) is
  a direct conceptual descendant. The name "ralph" survives as a first-
  class OMC skill in our dev harness.
- **[Hermes Agent](https://github.com/NousResearch/hermes-agent)** —
  Nous Research's autonomous self-evolving agent. Spirit and
  capability surface (sub-agent delegation, scheduled triggers → IM
  delivery, cross-session semantic memory) is what we benchmarked
  against. See `apps/cyberclaw-server/tests/e2e_evolution_test.rs` and
  `e2e_daily_digest_test.rs`.
- **[Hermes Agent self-evolution](https://github.com/NousResearch/hermes-agent-self-evolution)**
  — the explicit self-improvement extension that pushed us to ship
  cyberclaw's skill-evolution endpoint at GA.
- **[Hermes WebUI](https://github.com/nesquena/hermes-webui)** —
  a reference for how a long-running autonomous agent surfaces its
  state to a human operator.
- **[Evolver](https://github.com/EvoMap/evolver)** — evolutionary
  optimization framework. Useful comparison for the tournament-style
  skill-evolution path.
- **[OpenClaw-RL](https://github.com/Gen-Verse/OpenClaw-RL)** — RL
  variant of OpenClaw. Reference for treating agent improvement as
  a closed-loop training problem rather than only prompt-engineering.

## Development harness

CyberClaw itself was developed inside an agentic harness:

- **[oh-my-claudecode](https://github.com/Yeachan-Heo/oh-my-claudecode)**
  (OMC) — multi-agent orchestration layer that runs on top of
  Claude Code. The autopilot / ralph / team / ai-slop-cleaner skills
  used during cyberclaw's development came from here, and several of
  them were migrated into `ecosystem/skills/` under their original
  licenses. The release acceptance gates in
  `docs/implementation/release/v1.2.16-acceptance-plan.md` were driven
  by OMC modes.
- **[Superpowers](https://github.com/obra/superpowers)** — composable
  skill library for coding agents. Origin of the `brainstorming`,
  `test-driven-development`, and `subagent-driven-development` skills
  in `ecosystem/skills/`.

## Also read

Projects that informed our taxonomy but did not become a direct
borrow: AReaL, EverOS, gbrain, OpenSpace, OpenViking, paseo,
cc-connect, autoresearch, deer-flow.

## Skill ecosystem provenance

Skills under [`ecosystem/skills/`](ecosystem/skills/) come from a mix
of internal authoring and adaptation from existing open-source skill
libraries. Each skill records its provenance under
`metadata.cyberclaw.source` in its `SKILL.md` frontmatter. Examples:

- `data-analysis` — adapted from the deer-flow public skill set
- `chart-visualization` — adapted from the deer-flow public skill set
- `arxiv-research` — adapted from the autoresearch public skill set
- `nano-pdf` — upstream from nanoclaw

If your upstream work appears here without correct attribution in its
frontmatter, please open an issue and we will fix it in the next
release.

## On-chain & Web3 patterns

CyberClaw's Web3 deployment surface (wallets, signers, treasuries,
multisig flows) was informed by the operational patterns of:

- **Safe** (formerly Gnosis Safe) — multisig governance UX
- **Tenderly** — transaction simulation and dry-run
- **Forta Network** — on-chain monitoring and alerting
- **OpenZeppelin Defender** — automated incident response

These are reference systems, not dependencies.

## Direct dependencies

CyberClaw is a Rust workspace built on a thoughtful set of upstream
crates. Each of these solved a problem we would otherwise have had to
solve ourselves.

### Async runtime and HTTP

[tokio](https://tokio.rs/) · [axum](https://github.com/tokio-rs/axum)
· [tower](https://github.com/tower-rs/tower) +
[tower-http](https://github.com/tower-rs/tower-http) ·
[hyper](https://hyper.rs/) ·
[reqwest](https://github.com/seanmonstar/reqwest) ·
[tower_governor](https://github.com/benwis/tower-governor)

### Authentication, security, governance

[jsonwebtoken](https://github.com/Keats/jsonwebtoken) ·
[subtle](https://github.com/dalek-cryptography/subtle) ·
[aho-corasick](https://github.com/BurntSushi/aho-corasick) ·
[regex](https://github.com/rust-lang/regex) ·
[sequoia-openpgp](https://sequoia-pgp.org/)

### Storage

[sqlx](https://github.com/launchbadge/sqlx) ·
[rusqlite](https://github.com/rusqlite/rusqlite) ·
[sled](https://github.com/spacejam/sled) · SQLite FTS5

### Serialization and schemas

[serde](https://serde.rs/) · serde_json · toml ·
[schemars](https://github.com/GREsau/schemars)

### Observability

[tracing](https://github.com/tokio-rs/tracing) +
tracing-subscriber ·
[opentelemetry](https://github.com/open-telemetry/opentelemetry-rust) ·
[prometheus](https://github.com/prometheus/client_rust)

### Time, IDs, errors, CLI

[chrono](https://github.com/chronotope/chrono) ·
[uuid](https://github.com/uuid-rs/uuid) ·
[anyhow](https://github.com/dtolnay/anyhow) +
[thiserror](https://github.com/dtolnay/thiserror) ·
[clap](https://github.com/clap-rs/clap) ·
[inquire](https://github.com/mikaelmello/inquire)

### Frontend (Admin Console)

[React 18](https://react.dev/) ·
[Tailwind CSS](https://tailwindcss.com/) ·
[Babel](https://babeljs.io/) ·
[Prism.js](https://prismjs.com/) ·
[marked](https://marked.js.org/) ·
[Inter](https://rsms.me/inter/) +
[JetBrains Mono](https://www.jetbrains.com/lp/mono/)

### Testing

[Playwright](https://playwright.dev/) ·
[mockito](https://github.com/lipanski/mockito)

## Thank you

To every author of every paper, library, and project listed above,
and to the broader Rust + agent-systems community: thank you.
CyberClaw is built on what you published in the open.

If you spot a missing acknowledgment, please open a PR or an issue.
