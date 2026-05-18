# CyberClaw

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-Apache%202.0-blue?style=for-the-badge" alt="License"></a>
  <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/Rust-1.75%2B-orange?style=for-the-badge&logo=rust&logoColor=white" alt="Rust"></a>
  <a href="docs/README.md"><img src="https://img.shields.io/badge/Docs-portal-blueviolet?style=for-the-badge" alt="Docs"></a>
  <a href="README.zh-CN.md"><img src="https://img.shields.io/badge/Lang-中文-red?style=for-the-badge" alt="中文"></a>
</p>

<p align="center"><strong>Safe, controlled agent platform for high-stakes business environments.</strong></p>

> ⚠ **Status: Beta — research and development. Do not connect real funds, production databases, or live business systems to CyberClaw at this stage.**

CyberClaw is an agent runtime that makes governance, audit, identity, and external integration the agent's only path to real systems. Every action passes rule evaluation, escalates to human approval when required, and is fully recorded in a verifiable audit chain. Built for security operations, DevOps, Web3, and other high-stakes business contexts.

[Docs](docs/README.md) · [Quick start](docs/GUIDE.md) · [简体中文](README.zh-CN.md)

---

## Security architecture

Security is not a layered add-on in CyberClaw. It is native to the design, from the programming language up through the interface boundary — every action an agent takes toward the outside world must traverse all of these:

- **Language layer** — implemented in Rust. Memory safety and the absence of data races are statically guaranteed by the compiler; entire bug classes (buffer overflow, use-after-free, dangling pointers) are eliminated.
- **Sandbox and execution isolation** — the same Capability can run under multiple runtimes (local, isolated process, container, remote). High-risk operations default to container isolation; a single agent's failure or compromise does not contaminate others.
- **Model layer** — part of the system prompt is fixed by the server and cannot be rewritten by the model.
- **I/O layer** — tool outputs pass through prompt-injection and credential scanning before reaching the model context.
- **Execution layer** — high-risk capabilities are temporarily revoked under autopilot; consecutive failures force exit.
- **Interface layer** — filesystem and network boundaries are hard-coded; misconfigured rules cannot punch through.
- **Authentication layer** — timing-safe cryptographic comparison; external webhooks require signed verification.
- **Audit layer** — every action produces a cryptographically chained record; tampering is detectable (see [Audit](#audit) below).

## Five roles

| | |
|---|---|
| **Agent** | the actor. Each one has an identity, a trust level, a budget. |
| **Skill** | how this actor does its work. A "code-review method", an "incident-response playbook". |
| **Connector** | the bridge to one external system: a database, a wallet, Slack, and so on. |
| **Capability** | one specific operation: "write a file", "sign a transaction", "send a message". |
| **Platform Plugin** | a platform-level extension, like exporting audits to an enterprise SIEM. |

## How a single agent action plays out

When the agent wants to do something — say, "write this report to disk":

1. **Request**: the agent declares its intent — "I want to call `fs.write`, on file X, with content Y, for reason Z".
2. **Decision**: the governance engine looks up its rules and decides: allow, deny, or send to a human for approval.
3. **Execute and record**: on allow, the appropriate Connector runs the action; its output passes through content screening (to block injection attempts); the full chain — request, decision, result — lands in a tamper-evident audit record.
4. **Audit**: every action produces a row recording who asked for what, what the rule decided, and what the connector returned. Each row is cryptographically linked to the previous so tampering is detectable; the whole chain is verifiable in one command.

## Where it fits

Three concrete examples — illustrations, not limits:

- **Security operations** — alert volume routinely exceeds analyst capacity, but automation has been stalled by an unwillingness to trust an agent with real action. Under CyberClaw, triage, incident response, and PR risk review run inside policy. "Agent drafts, SOC approves" graduates from demo to production with a full audit chain attached.
- **DevOps and change management** — release gates, database migrations, and change approvals have always required human gatekeeping. Agents drafting pull requests is common; agents merging them is not. CyberClaw lets the agent run the full migration, generate the change summary, submit for approval, and stop there — producing the operational record that SOX and SOC2 audits will ask for along the way.
- **Web3** — multisig flows, treasury moves, and on-chain runbooks have lived in operator hands. On CyberClaw, an agent can draft transactions, assemble context, and propose execution; signing authority is gated by governance; every action lands on chain and in the audit ledger simultaneously, unifying on-chain and off-chain evidence.

These three are not the limit. Any setting in which an AI agent interacts with a real system and where mistakes carry real cost is in scope. Architecturally: the agent proposes; the runtime defines boundaries. CyberClaw is the runtime.

## Screenshots

<table>
<tr>
  <td><img src="assets/screenshots/tui-chat-idle.png" alt="TUI chat"></td>
  <td><img src="assets/screenshots/tui-tool-call.png" alt="TUI tool call"></td>
</tr>
<tr>
  <td align="center"><sub>TUI · Chat</sub></td>
  <td align="center"><sub>TUI · Tool call</sub></td>
</tr>
<tr>
  <td><img src="assets/screenshots/webui-agents-list.png" alt="WebUI agents list"></td>
  <td><img src="assets/screenshots/webui-trace-detail.png" alt="WebUI trace detail"></td>
</tr>
<tr>
  <td align="center"><sub>WebUI · Agents</sub></td>
  <td align="center"><sub>WebUI · Trace detail</sub></td>
</tr>
<tr>
  <td><img src="assets/screenshots/webui-memory-browse.png" alt="WebUI memory browser"></td>
  <td><img src="assets/screenshots/webui-skill-marketplace.png" alt="WebUI skill marketplace"></td>
</tr>
<tr>
  <td align="center"><sub>WebUI · Memory</sub></td>
  <td align="center"><sub>WebUI · Skill marketplace</sub></td>
</tr>
</table>

## Quick start

```bash
git clone https://github.com/cyberclawlabs/cyberclaw.git
cd cyberclaw
cp .env.example .env       # set LLM_API_KEY and CYBERCLAW_APPROVAL_SECRET
cargo run -p cyberclaw-server
# open http://127.0.0.1:38090/admin/v2/
```

Production deployment (JWT signing, TLS, multi-replica) is documented in [docs/getting-started](docs/GUIDE.md).

## Supported

- **LLM providers** — Anthropic, OpenAI, DeepSeek, MiniMax, Volcengine Ark, any OpenAI-compatible endpoint.
- **External system bridges** — filesystem, HTTP, browser, MCP tool bridge.
- **Messaging platforms** — Slack, Telegram, Discord, Lark, WeChat, LINE, generic webhook.
- **Multi-agent coordination** — sub-agent delegation, majority-vote aggregation, multi-model synthesis.
- **Observability** — trace export compatible with Jaeger, Datadog, Grafana, and other OpenTelemetry endpoints; Prometheus metrics.
- **Operator console** — React admin UI with login, approval queue, audit viewer, and live chat against any agent; companion CLI.
- **Deployment modes** — single-node or Raft cluster (multi-replica consistency, task assignment). Distributed approvals across replicas are in roadmap Phase 2.

## Roadmap

- **Phase 1 — Usable** (v1.x): the five-role model, declarative governance, audit chain, six messaging platforms, multi-provider LLM, autopilot with circuit breaker. **Shipped.**
- **Phase 2 — Governed** (v2.x planned): distributed approvals across replicas, enterprise IAM integration, finer-grained permission scope, multi-tenant isolation, compliance templates (SOX / SOC2 / HIPAA).
- **Phase 3 — Extensible ecosystem** (v3.x planned): third-party Connector / Skill / Plugin registry, signed Skill distribution, shared governance pattern library.

## Contributing

Contributions welcome in:

- New Connectors (messaging platforms, SaaS APIs, internal system bridges)
- New Skills (vertical-specific methods, prompt templates, knowledge packs)
- Governance rule templates (for specific business contexts and compliance frameworks)
- Documentation, examples, case studies

See [CONTRIBUTING.md](CONTRIBUTING.md).

## Acknowledgments

CyberClaw's core architecture draws on a body of academic work and open-source projects.

**Concepts and projects drawn from:**
- [Anthropic Model Context Protocol (MCP)](https://modelcontextprotocol.io/) — protocol design for tool integration.
- [OpenTelemetry](https://opentelemetry.io/) — trace format and export conventions.
- [Nous Research Hermes Agent](https://github.com/NousResearch/hermes-agent) and [OpenClaw](https://github.com/openclaw/openclaw) — multi-agent architecture and Skill concept references.
- HashiCorp Sentinel / Open Policy Agent — policy-as-code and declarative governance.
- AWS IAM — capability-based authorization semantics.

**Primary dependencies:** Tokio, Axum, Serde, Tracing, Prometheus, subtle, HMAC/SHA-2, Reqwest (Rust side); React, TypeScript, Vite, Tailwind CSS (frontend).

**Migrated Skill sources.** Some Skills under `ecosystem/skills/` were ported from upstream projects under their Apache-2.0 / MIT licenses; each Skill's `SKILL.md` header records the original source link:

- **obra/superpowers** — `brainstorming`, `test-driven-development`, `subagent-driven-development`, and others
- **oh-my-claudecode** — `debug`, `plan`, `verify`, `learner`, `skill`, `omc-reference`
- **NousResearch/hermes-agent** — `daily-digest`, `requesting-code-review`, `spike`, `systematic-debugging`, `writing-plans` (some of which hermes in turn adapted from obra/superpowers and gsd-build/get-shit-done)
- **anthropics/skills** — `skill-creator`

Full research background in [ACKNOWLEDGMENTS.md](ACKNOWLEDGMENTS.md); academic papers and standards in [CITATIONS.md](CITATIONS.md).

## Project

- **Homepage** — [cyberclawlabs.ai](https://cyberclawlabs.ai)
- **GitHub** — [github.com/cyberclawlabs/cyberclaw](https://github.com/cyberclawlabs/cyberclaw)
- **Security and contact** — `info@cyberclawlabs.ai` · see [SECURITY.md](SECURITY.md)

## License

Apache-2.0
