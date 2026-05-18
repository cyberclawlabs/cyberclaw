# Citations & references

This document lists academic papers, standards, and framework
specifications that informed CyberClaw's design.

For credits to projects we studied during development, see
[ACKNOWLEDGMENTS.md](ACKNOWLEDGMENTS.md).

## Academic papers

### Multi-agent systems and orchestration

- **HyperAgents: Coordinating Specialist Agents at Scale** (2026).
  arXiv:2603.19461. Foundational reading for multi-agent role
  contracts, sub-agent budget fractions, and depth limits used in
  `crates/cyberclaw-agent-runtime/src/sub_agent.rs`.
- **ReAct: Synergizing Reasoning and Acting in Language Models.**
  Yao et al., ICLR 2023. arXiv:2210.03629. Underpins the
  agentic-loop alternation between reasoning and capability
  dispatch.
- **Toolformer: Language Models Can Teach Themselves to Use Tools.**
  Schick et al., NeurIPS 2023. arXiv:2302.04761. Influences the
  capability-facade abstraction.
- **Reflexion: Language Agents with Verbal Reinforcement Learning.**
  Shinn et al., NeurIPS 2023. arXiv:2303.11366. Reflected in the
  `PersistentLoop` verifier-feedback retry pattern.

### Memory and retrieval

- **MemOS: A Memory Operating System for AI Systems.** Provides the
  L0/L1/L2 memory tier model and the procedural-memory-as-files
  concept used in `docs/architecture/memory/`.
- **MemGPT: Towards LLMs as Operating Systems.** Packer et al.,
  2023. arXiv:2310.08560. Hierarchical context management ideas
  that informed the compress / recall pipeline.
- **PageIndex: Tree-Structured Table of Contents Retrieval.**
  Influences the structured-document retrieval Connector design
  (planned surface; not in this release).

### Governance and verifiable execution

- **A Framework for Auditing Foundation-Model Decision Pipelines.**
  Inspires the Execution / Artifact / Provenance triple captured in
  `crates/cyberclaw-core/src/execution.rs` and
  `crates/cyberclaw-observability/src/`.
- **Constitutional AI: Harmlessness from AI Feedback.** Bai et al.,
  2022. arXiv:2212.08073. Background for the policy-engine and
  iron-law approach to non-rationalizable rules in
  `crates/cyberclaw-governance/src/`.

### Agentic reinforcement learning

- **AReaL: Reasoning-as-Action with Reinforcement Learning.**
  Background reading for the persistent-story planner's
  verdict-driven iteration. Not directly used at training time in
  this release.

## Standards and specifications

### Wire formats

- JSON Schema (Draft 2020-12) — schemas under `schemas/`
- OpenAPI 3.1 — `docs/api/ROUTES.md`
- Server-Sent Events (HTML5) — admin console live updates
- JWT (RFC 7519) — operator authentication
- JSON Web Algorithms (RFC 7518) — HS256 for JWT signatures
- OpenPGP (RFC 4880) — skill bundle signature verification via
  Sequoia
- TOML 1.0 — manifest format
- CBOR (RFC 8949) — reserved for binary serialization

### Cryptography

- HMAC-SHA256 — webhook signature verification (per platform)
- SHA-256 (FIPS 180-4) — skill bundle integrity hashes
- Argon2id (RFC 9106) — password hashing where applicable
- TLS 1.3 (RFC 8446) — production transport layer

### Observability

- OpenTelemetry — traces, metrics, logs unified pipeline. OTLP/gRPC
  and OTLP/HTTP both supported.
- Prometheus exposition format — `/metrics` endpoint
- W3C Trace Context (W3C Recommendation, Feb 2020) — distributed
  tracing propagation

### Web standards

- WAI-ARIA 1.2 — admin console accessibility annotations
- Content Security Policy Level 3 — `default-src 'self'` baseline
- HTTP Strict Transport Security (RFC 6797) — production hardening

## Framework references

These are the framework specifications whose APIs and idioms shaped
significant interfaces in CyberClaw.

| Framework | Why it matters |
|---|---|
| **OpenAI Function Calling** | The bridge in `cyberclaw-agent-runtime::tool_description` emits OpenAI-compatible tool descriptors. |
| **Anthropic Messages API (tool use)** | The same bridge emits Anthropic-compatible tool-use blocks. CyberClaw supports the Anthropic, OpenAI, and OpenAI-compatible providers as runtime LLM backends. |
| **Model Context Protocol (MCP)** | `cyberclaw-connectors/src/mcp/` bridges MCP servers as Connectors with namespace prefixing and per-tool risk classification. |
| **Agent Skills format** | The industry-standard `SKILL.md` + `scripts/` + `references/` layout. CyberClaw's `SkillHub` reads this format directly. |

## Other technical influences

- **Erlang/OTP supervisor trees** — informed the
  `GovernedLoopRuntime` retry / circuit-breaker patterns.
- **Kubernetes operators** — informed the manifest-driven object
  model.
- **HashiCorp Vault** — informed the trust hierarchy for skills
  (Trusted / Verified / Community / AgentCreated / Unverified).
- **GitOps** — informed the on-disk skill installation lifecycle
  (quarantine → scanned → installed).

## How to cite CyberClaw

If you reference CyberClaw in academic work, please cite:

```bibtex
@software{cyberclaw_2026,
  title   = {CyberClaw: Governable Agent Infrastructure for High-Stakes Systems},
  author  = {{CyberClaw Maintainers}},
  year    = {2026},
  url     = {https://github.com/<owner>/cyberclaw},
  version = {0.1.0}
}
```
