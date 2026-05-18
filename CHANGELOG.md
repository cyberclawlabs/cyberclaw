# Changelog

All notable changes to CyberClaw will be documented here.
This project follows [Keep a Changelog](https://keepachangelog.com/en/1.0.0/) format and [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [v1.2.16] — 2026-05-18

**Initial public release.**

This is the first open-source release of CyberClaw. Prior internal development history is not included in this public changelog; future releases will follow standard Keep-a-Changelog format.

### Core architecture

- Five-object model: `Agent` / `Skill` / `Connector` / `Capability` / `Platform Plugin` — runtime-enforced boundaries with no bypass path.
- Hash-chained audit log: append-only SQLite, verifiable via `cyberclaw audit verify`, exportable to OTLP-HTTP for Jaeger / Tempo / Grafana Cloud.
- Declarative governance: YAML rules (`allow` / `deny` / `review`) evaluated before every Capability dispatch, with hot-reload.

### Security primitives

- Iron Law prompt invariants: server-controlled system-prompt section the model cannot rewrite.
- Tool output sanitization: prompt-injection scan + credential pattern detection on every connector return.
- Auto Mode capability scoping: high-risk capabilities temporarily revoked under autopilot; circuit breaker on consecutive failures.
- Connector-level boundaries: `fs.write` workspace root enforcement, RFC1918 egress rejection.
- Constant-time auth: `subtle::ConstantTimeEq` for JWT and cluster tokens; HMAC-SHA256 webhook signature validation.

### Integrations

- **LLM providers**: Anthropic, OpenAI, DeepSeek, MiniMax, Volcengine Ark, plus any OpenAI-compatible endpoint.
- **Connectors**: filesystem, HTTP, browser via CDP, MCP tool bridge.
- **Messaging**: Slack, Telegram, Discord, Lark, WeChat, LINE, and a generic webhook adapter.
- **Multi-agent orchestration**: sub-agent reduce (`Concat` / `MajorityVote` / `LlmSummary`) and Mixture-of-Agents aggregation.
- **Operator console**: React admin WebUI with JWT auth, approval queue, audit viewer, and live chat against any agent; companion CLI.
- **Deployment**: single-node or Raft cluster mode (multi-replica consistency, task assignment).
- **Observability**: OTLP-HTTP trace export, Prometheus metrics.

### Known limitations (Beta status)

- Distributed approvals across replicas are not yet wired (planned for v2.x).
- HTTP API and CLI surface may shift before GA.
- Production deployments should validate in read-only / staging / internal-tool form before connecting real funds, production databases, or live business systems.

See [README.md](README.md) for project overview, [docs/](docs/README.md) for full documentation, and [ACKNOWLEDGMENTS.md](ACKNOWLEDGMENTS.md) for credits.
