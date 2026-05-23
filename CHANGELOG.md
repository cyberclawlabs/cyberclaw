# Changelog

All notable changes to CyberClaw will be documented here.
This project follows [Keep a Changelog](https://keepachangelog.com/en/1.0.0/) format and [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [v1.2.17] — 2026-05-23

Sustaining release on top of `v1.2.16` initial public release. Closes 4 architecture gaps and ships 5 sustaining fixes. Internal business-matrix evaluation against a comparable agent platform: `+25.5pp` accuracy gap closure with `30%` faster median latency.

### Added

- **`DispatchInterceptor` architecture** (`crates/cyberclaw-connectors/src/dispatch_interceptor/`). Trait + 3 default interceptors run for every Capability dispatch:
  - `WallClockInterceptor` — records dispatch duration; surfaces budget overruns.
  - `SandboxInjectionInterceptor` — attaches the resolved sandbox profile to the execution context.
  - `TruncationMetadataInterceptor` — surfaces a structured `_meta.truncated` flag when a connector output is clipped, letting the model retry with a wider window instead of acting on partial data.
  Custom interceptors implement the trait and register on `CapabilityDispatcher`.
- **`SandboxProfile`** (`crates/cyberclaw-connectors/src/sandbox/profile.rs`). Three named container-isolation profiles for `cmd.run`: `minimal` (no network, no fs writes), `dev` (workspace writes ok, no network), `isolated` (full lockdown). Replaces ad-hoc mount logic that previously caused duplicate-mount failures.
- **`AgenticLoopGovernor`** (`crates/cyberclaw-agent-runtime/src/loop_governor.rs`). Wall-clock + token + repetition gates with L1 / L2 / L3 budget profiles. Stops runaway loops before they hit model limits or accumulate identical-call cycles.
- **`ScopedMemory`** (`crates/cyberclaw-store/src/scoped_memory.rs`). K-turn full-retention window with automatic eviction of older turns, used by the governor for repetition detection.
- **`OutputVerifier` + `VerifierChain`** (`crates/cyberclaw-agent-runtime/src/verify.rs`). `OutputVerifier` trait + combinator + 3 built-in verifiers: `CodeBlockVerifier`, `JsonStructureVerifier`, `RegexAssertVerifier`. Structured post-execution checks.
- **3 domain-expert skills** (`ecosystem/skills/domain-expert-{web3,soc,devops}/`). Packaged skill bundles + AND-of-OR keyword auto-binding via `SkillBinder` (`crates/cyberclaw-skill-runtime/src/skill_binder.rs`).
- **3 web3 connector examples** (`ecosystem/connectors/{safe-multisig,signer-vault,wallet-eth}-example/`). Reference implementations for Safe multisig, signer vault, and Ethereum wallet.
- **Empty-response diagnostic fallback** (`apps/cyberclaw-server/src/api/chat_handler.rs`). When the model returns finish_reason=stop with empty content, the response now includes a structured diagnostic (finish_reason / iterations / tokens) instead of a bare empty body.
- **Master-agent `_meta.truncated` awareness** (`ecosystem/agents/master-agent/SYSTEM_PROMPT.md`). Agent now recognizes the truncation flag emitted by the new interceptor and adjusts follow-up strategy.

### Changed

- **BREAKING — `search.grep` `case_insensitive` default flipped to `true`** (`crates/cyberclaw-connectors/src/local/search.rs`). Most agent queries want case-insensitive matching (`grep -i` convention); the previous case-sensitive default produced confusing under-counts. Callers that rely on case-sensitive matching must pass `case_insensitive: false` explicitly.
- **BREAKING — `search.grep` Count mode now returns per-line total** (`crates/cyberclaw-connectors/src/local/search.rs`). The fallback path previously returned file count (a bug: 4 files with 7 total import lines reported as `4`). It now sums matching lines across all files. Callers consuming the old broken count as a file total must switch to `output_mode: "files_with_matches"`.
- **BREAKING — Container runtime mount path changed from `/workspace` to host_cwd 1:1** (`crates/cyberclaw-connectors/src/runtime/container.rs`). Workspaces now mount at their real host path inside the container — `/path/to/workspace` resolves the same inside and outside. Tooling that hardcoded `/workspace` as workdir must update to the real host path.

### Fixed

- Agentic loop no longer silently terminates on whitespace-only content with `stop` finish reason. The loop now injects a system nudge and continues the turn (`crates/cyberclaw-agent-runtime/src/agentic_loop.rs`).

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
