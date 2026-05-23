# LLM Provider Configuration

Notes on the LLM provider integrations CyberClaw supports — what works, what's tricky, what to avoid.

## Supported provider modes

CyberClaw wires LLM providers via `LLM_PROVIDER` in `~/.cyberclaw/llm.env` (sourced by `~/.cyberclaw/bin/start-cyberclaw.sh`). Two modes are exercised in production:

### 1. `generic` — OpenAI-compatible upstream

Works for any upstream that speaks the OpenAI chat-completions schema. Examples verified end-to-end in v1.2.4:

```bash
# DeepSeek (api.deepseek.com)
LLM_PROVIDER=generic
LLM_API_KEY=sk-<your-deepseek-key>
LLM_BASE_URL=https://api.deepseek.com/v1
LLM_DEFAULT_MODEL=deepseek-chat

# MiniMax via OpenAI-compat
LLM_PROVIDER=generic
LLM_API_KEY=sk-cp-<your-minimax-key>
LLM_BASE_URL=https://api.minimax.io/v1
LLM_DEFAULT_MODEL=MiniMax-M2.7-HighSpeed
```

### 2. `anthropic` — Anthropic shim (MiniMax Extended Thinking compat)

For upstreams that speak the Anthropic `/v1/messages` schema. MiniMax also exposes one:

```bash
# MiniMax via Anthropic shim (returns Extended Thinking blocks)
LLM_PROVIDER=anthropic
LLM_API_KEY=sk-cp-<your-minimax-key>
ANTHROPIC_API_KEY=sk-cp-<same-key>
LLM_BASE_URL=https://api.minimaxi.com/anthropic
ANTHROPIC_BASE_URL=https://api.minimaxi.com/anthropic
LLM_DEFAULT_MODEL=MiniMax-M2.7-HighSpeed
```

> **v1.2.2 compat fix**: MiniMax's Anthropic shim returns `{"type":"thinking","thinking":"...","signature":"..."}` content blocks that have no `text` field. Pre-v1.2.2 cyberclaw's `AnthropicContent` deserializer required `text: String` and rejected the whole response with `error decoding response body`. Fixed by making `text: Option<String>` and `filter_map`ing thinking blocks out of user-visible content in `crates/cyberclaw-llm/src/providers/anthropic.rs`. Regression test: `test_minimax_anthropic_shim_thinking_block_does_not_break_deserialization`.

## ⚠ Known network gotchas

### MiniMax `cn.minimax.io` may be DNS-intercepted by local proxies

On developer machines running Surge / Clash / ShadowRocket / similar with the MiniMax rule enabled, the DNS for `cn.minimax.io` (and sometimes `api.minimax.io`) resolves to the TestNet range `198.18.0.x`. The proxy then forwards the connection on the developer's behalf. Server processes started by the dev (e.g. `cyberclaw-server` launched via the start script) inherit the same DNS resolver and **also hit the TestNet IP**, but they do NOT participate in the proxy's HTTP-CONNECT tunnelling. The TLS handshake fails with `SSL_connect: SSL_ERROR_SYSCALL`.

Symptoms:

- `dig +short cn.minimax.io` returns `198.18.0.117` (or similar in `198.18.0.0/15`)
- `curl https://cn.minimax.io/v1/messages` fails with `SSL_ERROR_SYSCALL in connection`
- cyberclaw-server reports `HTTP request failed: error decoding response body` or `connection refused`

Workarounds:

1. **Use `api.minimaxi.com/anthropic` instead** — global endpoint that is not DNS-rerouted by the typical proxy rule.
2. **Disable the MiniMax proxy rule** while running cyberclaw-server, so the upstream DNS returns the real IP.
3. **Run cyberclaw-server in a separate network namespace** that bypasses the proxy.

### `api.minimax.io` quota exhaustion (HTTP 429)

After heavy chat-driven testing the MiniMax account quota may exhaust. Symptoms: `LLM chat completion failed: API error: 429 - {"type":"error","error":{"type":"rate_limit_error","message":"usage limit exceeded (2056)"}}`. Resolution: top up the account, or switch to a different provider (DeepSeek / Anthropic / OpenAI) via the model picker in `/admin/v2/models` — the v1.1.0 `~/.cyberclaw/models.json` CRUD makes this a UI change with no server restart needed.

## Provider routing through cyberclaw

After v1.2.4, all three user-facing chat paths get the same **silent-abandon enforcement** + **41-tool palette** + **StuckDetector** protections:

| Path | Handler | Notes |
|---|---|---|
| `POST /v1/chat/completions` | `chat.rs::chat_completions` | OpenAI-compat. v1.2.4 added inline enforcement loop. |
| `POST /v1/agent/chat/completions` | `chat_handler.rs::agent_chat_completions` | Native cyberclaw — full `DefaultAgenticLoop` with all protections. |
| `POST /api/v1/chat/message` | `chat_conversations.rs::send_chat_message` | WebUI path — delegates to `agent_chat_completions`. |
| `cyberclaw-cli chat` (CLI REPL) | hits `/v1/agent/chat/completions` since v1.2.4 | Was `/v1/chat/completions` pre-v1.2.4. |

## Configuration files (production)

- `~/.cyberclaw/llm.env` — provider env vars, sourced by start script
- `~/.cyberclaw/models.json` — WebUI model picker source-of-truth (CRUD via `/admin/v2/models`)
- `~/.cyberclaw/jwt-secret` — JWT signing secret (chmod 600)
- `~/.cyberclaw/cli-token` — CLI JWT (regenerate via `python -c "..."` if expired)
- `~/.cyberclaw/profiles.toml` — SOUL preset profiles per user
- `~/.cyberclaw/config.toml` — server bind / CORS / logging
- `~/.cyberclaw/audit.db` — SQLite audit log (chmod 600)

See also:
- [docs/architecture/runtime/RUNTIME_BLUEPRINT_V2.0.md](../architecture/runtime/RUNTIME_BLUEPRINT_V2.0.md) — agentic_loop + dispatch architecture
- `apps/cyberclaw-server/config.toml` — production config template
- [CHANGELOG.md](../../CHANGELOG.md) §v1.2.2/v1.2.4 — full vendor-compat + enforcement history
