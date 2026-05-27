# Changelog

All notable changes to CyberClaw will be documented here.
This project follows [Keep a Changelog](https://keepachangelog.com/en/1.0.0/) format and [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [v1.7.1] — 2026-05-27

Reliability + observability hardening on top of v1.3.0. Sustained-load tested, release-profile tuned, additional per-turn safeguards against the most common chat-time failure modes.

### What's new for users

- **Per-turn response check.** The chat handler now runs a fast heuristic check on every assistant reply and emits a structured `tracing` event when one of these failure modes is detected: empty reply after a model refusal, fabrication of "tool result"-shaped content while a tool intent is still pending, first-turn A/B choice prompts for requests that already have a concrete deliverable anchor, and "saved to /tmp/X" claims with no corresponding file-write tool call. Observability only — the response is not modified — so existing integrations are unaffected.
- **Cross-turn recall reflex.** A new rule in the agent constitution directs the model to look up prior conversation context via `memory_search` before asking the user to repeat themselves or guessing what they meant by "earlier" / "前面". Pairs with an optional `SessionSearchInjector` for operators who want passive top-k injection (off by default).
- **Smaller release binary.** Release profile now enables fat LTO + `codegen-units=1` + `strip=true`. Binary size reduced by ~30%. Runtime performance is within noise of the previous build on macOS arm64 single-process benchmarks.
- **Sustained-load baseline published.** New baseline numbers under continuous load on a single Apple Silicon process: 12,000,000 requests over one hour, zero failures, ~3,333 RPS average, audit chain remained verifiable end-to-end.

### Quality gates

- `cargo test --workspace --lib`: 4,073 pass / 0 fail / 4 ignored / 17 suites.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- WebUI Playwright suite: 95 / 95 pass.

### Security

- `git-secrets` pre-commit hook installed; `.gitallowed` policy file registers OpenAI / Anthropic / GitHub PAT / JWT patterns and explicit placeholders so test fixtures and example values don't trigger false positives.
- Internal test scripts no longer carry hard-coded API keys; they now require `MINIMAX_API_KEY` (or equivalent) to be exported before running.

### Migration

- No API changes. v1.3.0 → v1.7.1 is a drop-in upgrade.
- The new per-turn check is observability-only — no behavior change for existing clients.
- The cross-session recall behavior surfaces only when (a) the user references prior context and (b) `memory_search` is available in the bound tool palette. Operators don't have to enable anything; it's an LLM-driven reflex from the constitution rule.

### Notes

- The optional `SessionSearchInjector` ships with a provider trait but no default backend — operators wire it explicitly to their `SkillHub` / FTS5 store if they want passive top-k context injection.
- On macOS arm64, the `mimalloc` allocator was evaluated and reverted (the system allocator outperformed it in single-process benchmarks). The Linux story may differ; the relevant Cargo dependency lines remain commented in source for future evaluation.

---

## [v1.3.0] — 2026-05-26

Major architectural rework. v1.3.0 ships 4 work packages plus a new LLM hallucination defense layer, addressing the root causes of issues that v1.2.x patches could only suppress.

### What's new for users

- **Conversation sessions are now server-side.** Submit a message, get back an `X-Conversation-Id` header, send the same ID on the next turn — the server holds the full conversation including tool call structure. No more lost context across turns. New endpoint `POST /v2/agent/chat/completions`; legacy `/v1/agent/chat/completions` retained for backward compatibility.
- **Single-tier budget**. Removed the L1/L2/L3 profile tiers. Every request gets a generous 128k token / 300s wall-clock ceiling. The agentic loop self-stops when done. No more `[budget exhausted]` errors on simple prompts.
- **Typed SSE protocol** (new `cyberclaw-wire` crate). All server→client frames now carry a `{"v":1, "type":"...", "data":{...}}` envelope. New frame types: `Heartbeat` (every 15s — no more stuck spinners), `ToolStart`/`ToolComplete` (TUI now shows `[tool: X ✓ Nms]` inline), typed `Error` with `ErrorKind::{Billing, RateLimit, AuthInvalid, ContextOverflow, ...}` for colored TUI rendering.
- **Hallucination defense (`ToolFactVerifier`)**. New verifier in the agentic loop's verifier chain detects when the LLM claims success for tool operations that actually failed or were never called. Default-on; rollback via `CYBERCLAW_TOOL_FACT_VERIFICATION=off`. Specifically catches: "Done. Written to /tmp/X" when /tmp write was denied, "File saved to PATH" when no tool ever wrote to PATH. Forces a one-shot retry with corrective feedback.
- **Stronger credential governance.** `fs.write` / `fs.edit` / `cmd.run` (and bash redirect via `cmd.run`) now block writes containing credential patterns (AWS access keys, API keys, `password=`, `secret=`, GitHub tokens, etc.) with a clear `[GOVERNANCE DENY — D010]` user-facing message.
- **Workspace boundary errors are now anti-hallucination**. When a write is blocked by workspace boundary, the error message contains `[BOUNDARY DENY] ... The file was NOT created. Do not claim it was written.` — helps the LLM tell the user honestly.
- **TUI improvements** — Esc no longer accidentally exits the TUI (now clears input; exit via Ctrl-C); banner shows app version + model + conversation ID; PgUp/PgDn scroll conversation history; ratatui status bar shows heartbeat elapsed seconds; assistant timestamps reflect reply-complete time in local timezone.

### Migration

- **CLI users**: no action — `cyberclaw chat` auto-uses v2 endpoint and handles new conversation IDs.
- **External v1 API consumers**: continue to work unchanged. v1 endpoint preserved; deprecate in v1.4.
- **Self-hosted deployments**: set `TZ` env var to your timezone if you want server-side timestamps in local time (currently UTC by default — known limitation, fixed properly in v1.3.1).
- **Credential workflow**: if your workflow legitimately needs to write credentials to disk (e.g., automated key rotation), use the existing approval queue (`cyberclaw review approve <id>`) to override the D010 gate per-call.

### Notes

- All 4 architectural debt classes from prior v1.2.x patches now have structural fixes
- 86 commits since v1.2.19, 4056+ workspace tests passing
- Comprehensive QA: 13 rounds of side-by-side comparison vs reference agent platform
- Zero breaking changes for v1 endpoint consumers

---

## [v1.2.19] — 2026-05-23

TUI bug-fix sprint. Patch release closing 16 user-facing bugs surfaced by 9 rounds of real-LLM end-to-end testing. Zero new features; pure stability + UX. Recommended upgrade for all users.

### Fixed

- **Anthropic streaming hang** — outer wall-clock hard cap (180-240s depending on profile) prevents indefinite spinner on stalled agentic loops.
- **Governance silent denial** — when a request is denied by policy (e.g., `Read /etc/shadow`), the TUI now shows a diagnostic message instead of an empty response.
- **Tool approval visibility** — when a capability dispatch enters the approval queue, the TUI displays `⏳ Awaiting approval for <tool> — check /approvals` instead of a silent spinner. Approval is required for high-risk capabilities under Auto Mode.
- **MiniMax provider pricing** — `/usage` slash command now correctly displays USD cost for `MiniMax-M2.x` and `MiniMax-M2.x-HighSpeed` model variants.
- **Web search provider override** — `WEB_SEARCH_PROVIDER=exa` environment variable now correctly routes to Exa instead of falling through to DuckDuckGo. Endpoint override only forces DDG when the endpoint URL itself is DDG-shaped.
- **CJK text streaming** — SSE response parser now correctly handles multi-byte UTF-8 characters split across HTTP chunk boundaries (common with non-English LLM output). Prior behavior crashed the stream.
- **JWT expiry guidance** — when the CLI token expires, the TUI now shows `提示：JWT 已过期，请运行 rm ~/.cyberclaw/cli-token 后重新执行 cyberclaw onboard 获取新令牌` instead of a raw 401 error.
- **Queued message handling on auth failure** — when a 401/auth error fires, any queued user messages are explicitly cleared with a `[queue] N 条排队消息已丢弃（认证失败）` notice rather than being silently sent again on the next stream.
- **Scroll back through conversation history** — `PgUp/PgDn` scroll the conversation viewport; `Home/End` jump to top/live tail. Auto-pin to bottom unless user is reading history. The input box footer now shows `PgUp/PgDn 滚屏`.
- **TUI banner** — top of TUI displays `⚡ CYBERCLAW v1.2.19 │ <model> │ <conversation-id>` for brand visibility and quick session identification.
- **`--new` conversation flag** — `cyberclaw chat --new` now correctly creates a fresh conversation even when `--conversation` or `--resume` is also present (previously `--new` was silently ignored).
- **Multi-line shell scripts** — `cmd.run` capability now accepts arguments containing `\n` / `\r` (e.g., heredoc-piped Python scripts via `bash -c "python3 - << 'EOF' ... EOF"`). Previously rejected as control characters.
- **Assistant timestamp accuracy** — `assistant` message timestamps in the conversation pane now reflect actual reply-complete time, not user-submit time (previously identical).
- **Local timezone display** — TUI timestamps now display in the user's local timezone instead of UTC. Storage/audit timestamps remain UTC.
- **Multi-turn + tool call reliability** — Resolves MiniMax API error 2013 ("tool result's tool id not found") that previously broke any session after the first tool call. The provider adapter now normalizes system-role message ordering to satisfy MiniMax's strict requirement that system messages appear only once, at position 0.
- **Workspace boundary guidance** — agents now receive their workspace root path in the system prompt and know to write files inside the workspace rather than `/tmp/`. Configurable via `CYBERCLAW_AGENT_WORKSPACE_ROOT` env var.

### Changed

- **Loop profile selection** — multi-turn conversations and turn-1 prompts that mention file paths, tools, or agentic keywords now automatically use the high-budget L3 profile (128k tokens, 240s wall-clock). Previously these often hit L1's smaller 32k budget and reported "budget exhausted".
- **Dynamic budget upgrade** — the agentic loop governor now auto-promotes the budget tier (L1 → L2 → L3) when consumption reaches 75% of the current ceiling and the loop has executed more than one iteration. Up to two promotions per session. A new `BudgetUpgraded` SSE event surfaces this transition.
- **L1 token budget** raised from 8,000 to 32,000 (matches L2). L1 wall-clock raised from 60s to 180s. These minimum allocations accommodate the actual size of the system prompt + tools schema.
- **`/usage` slash command output** — now displays token totals + rate-limit headroom (RPM/TPM) + cost USD breakdown + credential pool status (when configured). Existing token-count display preserved.

### Notes

- Zero BREAKING changes — all fixes are additive or behavior-preserving
- All fixes verified across 9 rounds of side-by-side TUI testing vs hermes-agent
- Recommended for any user experiencing budget exhausted errors, silent denial, multi-turn breakage, or TUI rendering issues on v1.2.18

---

## [v1.2.18] — 2026-05-23

Sustaining release focused on LLM cost reduction, reliability, and observability. Zero BREAKING changes — all additive.

### Added

- **Anthropic prompt-cache auto-injection**. System prompt + last 3 messages now automatically tagged with `cache_control: ephemeral`, reducing input token cost on multi-turn Anthropic sessions by approximately 75%. Toggle via `LLM_ANTHROPIC_PROMPT_CACHE_ENABLED` (default on).
- **Rate-limit headroom display**. `/usage` slash command now shows remaining RPM / TPM with reset timestamps for the active provider, parsed from `x-ratelimit-*` response headers. Lets the operator see how close they are to provider limits before getting a 429.
- **Semantic LLM error classification**. The retry/failover layer now distinguishes 16 specific failure reasons (`Billing`, `RateLimit`, `ContextOverflow`, `AuthInvalid`, etc.) instead of binary transient/non-transient. Each carries recovery hints that drive different actions: context overflow auto-triggers compression, billing exhaustion triggers credential rotation, rate-limit triggers backoff.
- **LLM-driven context compression**. Long sessions now produce structured summaries via the configured LLM instead of mechanical truncation. Iterative merge preserves prior summary context across multiple compression cycles, preventing regenerate-from-scratch thrash. Falls back to deterministic truncation if the LLM call fails.
- **Multi-key credential pool**. New `[[llm.credentials]]` config schema lets a single provider be backed by multiple API keys with automatic rotation on billing / rate-limit / auth-fail errors. Four selection strategies: `FillFirst` (default), `RoundRobin`, `Random`, `LeastUsed`. Cooldown duration scales with the failure reason. Single-key deployments unchanged.
- **Token cost estimation (USD)**. `/usage` now displays per-session and per-model cost breakdown across input / output / cache_read / cache_write tokens. Built-in pricing table covers Anthropic, OpenAI, DeepSeek, MiniMax, Volcengine Ark, and Gemini families. Unknown models report token counts without cost.
- **Bilingual UI strings (English + Simplified Chinese)**. Approval prompts, slash command help, error messages, and status lines now switch between English and Chinese based on `CYBERCLAW_LOCALE` env var, `LANG` env var, or `[localization] default_locale` config. Defaults to English; missing Chinese keys fall back to English automatically.

### Changed

- `/usage` command output expanded to include rate-limit headroom, cost USD breakdown, and credential pool status (when configured). Existing token-count display preserved.

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
