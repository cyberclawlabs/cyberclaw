# Configuration reference

CyberClaw configuration is layered. The runtime resolves values in this
priority order, highest first:

1. CLI flags (per-command, e.g. `--server`, `--format`)
2. Environment variables (12-factor convention)
3. `~/.cyberclaw/config.toml` (operator-scoped)
4. Manifest fields (per-object, in `agent.toml` / `SKILL.md` frontmatter / `connector.toml`)
5. Compiled-in defaults

A higher layer always wins. The runtime never silently merges; if two
layers declare the same key, the higher layer's value is used and the
lower one is ignored.

This document covers the env-var and TOML layers. Manifest fields are
documented in [`docs/reference/manifests.md`](../reference/manifests.md).
The CLI flag surface is in [`docs/reference/cli.md`](../reference/cli.md).

## Production-required environment variables

These five must be set before the server will start outside development
mode. The server fails fast — it does not start with placeholder values.

| Variable | Purpose |
|---|---|
| `JWT_SECRET` | HMAC secret for operator JWTs. Generate with `openssl rand -base64 48`. Minimum 32 bytes. |
| `LLM_API_KEY` | Provider key for the configured `LLM_PROVIDER`. Provider-specific aliases (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `ARK_API_KEY`) are also accepted. |
| `CYBERCLAW_APPROVAL_SECRET` | HMAC secret for approval-row signatures. Distinct from `JWT_SECRET`. Minimum 32 chars. |
| `CYBERCLAW_CLUSTER_SHARED_TOKEN` | Bearer token shared between replicas for `/internal/cluster/*` calls. Required when `CYBERCLAW_CLUSTER_MODE=multi`. |
| `CYBERCLAW_WEBHOOK_SECRET_<PLATFORM>` | Per-platform HMAC secret for webhook verification. The server rejects webhooks for any platform whose secret is unset. |

## Server runtime

| Variable | Default | Notes |
|---|---|---|
| `CYBERCLAW_ADDR` | `127.0.0.1:38090` | Bind address. Use `0.0.0.0:port` to listen externally. |
| `SERVER_PORT` | `38090` | Convenience alias for the port; `CYBERCLAW_ADDR` overrides. |
| `SERVER_HOST` | `127.0.0.1` | Convenience alias for the host. |
| `ENVIRONMENT` | `production` | One of `development` / `staging` / `production`. Toggles HSTS, error verbosity, and demo seeds. |
| `CYBERCLAW_DEV_MODE` | `0` | `1` lowers some safety checks for local dev. **Never enable in production.** |
| `USE_TLS` | `0` | `1` enables Rustls. Requires `TLS_CERT_PATH` + `TLS_KEY_PATH`. |
| `TLS_CERT_PATH` | — | PEM cert chain. |
| `TLS_KEY_PATH` | — | PEM private key. |
| `ALLOWED_ORIGINS` | `http://127.0.0.1` | CORS allowlist. Comma-separated. |
| `CYBERCLAW_CORS_ORIGINS` | — | Alternative spelling honored for backward compat. |
| `CYBERCLAW_CONFIG_PATH` | `~/.cyberclaw/config.toml` | Override the operator config location. |

## LLM providers

The bridge speaks Anthropic, OpenAI, and OpenAI-compatible providers
(Volcengine Ark, DeepSeek, Together AI, Groq, MiniMax, etc.). Provider
selection is via `LLM_PROVIDER`; each provider reads its own key
environment.

| Variable | Default | Notes |
|---|---|---|
| `LLM_PROVIDER` | `anthropic` | `anthropic` / `openai` / `volcengine` / `deepseek` / `together` / `generic` |
| `LLM_API_KEY` | — | Generic key for the active provider. Provider-specific aliases override. |
| `LLM_BASE_URL` | provider default | Overridden when targeting a proxy or compatible endpoint. |
| `LLM_DEFAULT_MODEL` | provider default | The model used by the planner and the chat handler if no model is specified. |
| `CYBERCLAW_DEFAULT_MODEL` | — | Alias for `LLM_DEFAULT_MODEL`. |
| `ANTHROPIC_API_KEY` | — | Anthropic-specific key. Wins over `LLM_API_KEY` when `LLM_PROVIDER=anthropic`. |
| `ANTHROPIC_BASE_URL` | `https://api.anthropic.com` | Override for proxies. |
| `OPENAI_API_KEY` | — | OpenAI-specific key. |
| `OPENAI_BASE_URL` | `https://api.openai.com/v1` | OpenAI-compatible providers should set this here. |
| `ARK_API_KEY` | — | Volcengine Ark key. |
| `ARK_BASE_URL` | `https://ark.cn-beijing.volces.com/api/v3` | Volcengine Ark endpoint. |
| `CYBERCLAW_EMBED_ENABLED` | `0` | `1` enables embedding-backed retrieval. |
| `CYBERCLAW_EMBED_API_KEY` | — | Key for the embedding provider. |
| `CYBERCLAW_EMBED_BASE_URL` | — | Embedding endpoint. |
| `CYBERCLAW_EMBED_MODEL` | provider default | Embedding model name. |
| `CYBERCLAW_EMBED_DIMENSION` | `1536` | Vector dimension for the configured model. |

## Audit + persistence

| Variable | Default | Notes |
|---|---|---|
| `CYBERCLAW_AUDIT_DB` | `~/.cyberclaw/audit.db` | SQLite path for the hash-chained audit log. |
| `CYBERCLAW_AUDIT_ARCHIVE_DIR` | `~/.cyberclaw/archive` | Where snapshots are written. |
| `CYBERCLAW_AUDIT_ARCHIVE_INTERVAL_SECS` | `3600` | Snapshot frequency. |
| `CYBERCLAW_AUDIT_ARCHIVE_RETAIN_DAYS` | `30` | Snapshots older than this are pruned. |
| `CYBERCLAW_AUDIT_ARCHIVE_GPG_KEY` | — | If set, snapshots are encrypted with this OpenPGP recipient. |
| `CYBERCLAW_KANBAN_DB` | `~/.cyberclaw/kanban.db` | SQLite path for the task board. |
| `CYBERCLAW_CONVERSATIONS_PATH` | `~/.cyberclaw/conversations.json` | Where chat conversations are persisted. |
| `CYBERCLAW_AGENT_STATE_FILE` | `~/.cyberclaw/agent-instances` | Per-CLI agent runtime state. |
| `CYBERCLAW_ECOSYSTEM_DIR` | `<repo>/ecosystem` | Where to scan for installable agents/skills/connectors/plugins. |

## Multi-replica

| Variable | Default | Notes |
|---|---|---|
| `CYBERCLAW_CLUSTER_MODE` | `single` | `single` for one-node, `multi` to enable Raft + cross-replica routes. |
| `CYBERCLAW_CLUSTER_SHARED_TOKEN` | — | Required when mode is `multi`. Bearer for `/internal/cluster/*`. |
| `CYBERCLAW_RAFT_BIND_ADDR` | `127.0.0.1:7700` | Raft listener address. |
| `CYBERCLAW_RAFT_PEERS` | — | Comma-separated `node-id@host:port` list. |
| `CYBERCLAW_ASSIGNMENT_PULL_URL` | — | Where worker brains fetch assigned sessions. |
| `CYBERCLAW_ASSIGNMENT_POLL_INTERVAL_MS` | `2000` | How often workers poll. |
| `CYBERCLAW_ASSIGNMENT_REQUEST_TIMEOUT_MS` | `5000` | Per-poll timeout. |

## Memory and context

| Variable | Default | Notes |
|---|---|---|
| `CYBERCLAW_AUTO_COMPRESS_THRESHOLD` | `0.85` | Fraction of context budget that triggers compression. |
| `CYBERCLAW_AUTO_COMPRESS_COOLDOWN_SECS` | `30` | Minimum interval between compressions. |
| `CYBERCLAW_GRAPH_BACKEND` | `inmemory` | `inmemory` / `sled` / `sqlite` for the memory graph. |

## Browser & external integrations

| Variable | Default | Notes |
|---|---|---|
| `CYBERCLAW_BROWSER_ENABLED` | `0` | `1` enables Playwright-backed browser capabilities. |
| `CYBERCLAW_BROWSER_DEBUG_URL` | `http://127.0.0.1:9222` | Chromium DevTools endpoint. |
| `CYBERCLAW_BROWSER_WS_URL` | — | WebSocket override. |
| `CYBERCLAW_BROWSER_TIMEOUT_MS` | `30000` | Per-action timeout. |
| `CYBERCLAW_CONTAINER_IMAGE` | `python:3.11-slim` | Default image for container-runtime capabilities. |
| `CYBERCLAW_LARK_APP_ID` | — | Lark / Feishu app id (for IM connector). |
| `CYBERCLAW_LARK_APP_SECRET` | — | Paired secret. |

## Evolution & feedback loop

| Variable | Default | Notes |
|---|---|---|
| `CYBERCLAW_EVOLUTION_LOG` | `~/.cyberclaw/evolution.log` | Where the evolution agent writes per-iteration verdicts. |
| `CYBERCLAW_EVOLUTION_MODEL` | `LLM_DEFAULT_MODEL` | Model used by the evolution loop. |
| `CYBERCLAW_FEEDBACK_LOOP_ENABLED` | `0` | `1` runs the feedback loop in the background. |
| `CYBERCLAW_FEEDBACK_LOOP_INTERVAL_SECS` | `300` | Loop cadence. |
| `CYBERCLAW_FEEDBACK_LOOP_MIN_COUNT` | `5` | Minimum samples before the loop fires. |
| `CYBERCLAW_FEEDBACK_LOOP_WINDOW_SECS` | `3600` | Sample window. |
| `CYBERCLAW_HANDOFF_ENABLED` | `1` | `0` disables multi-agent handoff. |
| `CYBERCLAW_CURATOR_ENABLED` | `1` | `0` disables the skill-curation loop. |

## Demo & development

These should not be enabled in production. They exist to let local
developers and demo environments bootstrap without going through the
full onboarding wizard.

| Variable | Default | Notes |
|---|---|---|
| `CYBERCLAW_ADMIN_SEED_DEMO` | `0` | `1` seeds demo agents/skills at startup. |
| `CYBERCLAW_CHAT_AUTO_ROUTE` | `0` | `1` lets the chat handler infer agent from prompt without an explicit selection. |
| `CYBERCLAW_CHAT_INTENT_HINT` | — | Hard-codes an intent classifier hint for the demo flow. |

## Operator config file (`~/.cyberclaw/config.toml`)

Generated by `cyberclaw onboard`. Schema:

```toml
# Identity
operator_id      = "qa-admin"
display_name     = "QA Admin"
connection_mode  = "local"            # local | remote | embedded

# LLM
llm_provider     = "anthropic"
llm_api_key      = "sk-..."           # never commit this file
llm_default_model = "claude-sonnet-4-6"
workspace_root   = "/Users/me/work"

# Skills
enabled_skills   = ["code-reviewer", "test-driven-development"]

# Governance (optional inline overrides; full rules go in governance.toml)
[governance]
trust_default = "Standard"
review_threshold = 0.7
[[governance.rules]]
pattern = "fs.write:**/.env*"
verdict = "Deny"

# Cluster (optional)
[cluster]
mode = "single"
brain_id = "node-local"
```

## Declarative policy rules file (`~/.cyberclaw/governance.yaml`)

The `RuleBasedPolicyEngine` (S27) reads a flat YAML rule list that gates
every `Connector → Capability` dispatch. **v1.2.15+** auto-wires this file
when `~/.cyberclaw/bin/start-cyberclaw.sh` runs and the file exists; the
boot log confirms with `S27: policy rules loaded path=... rule_count=N`.

Starter template lives at
[`docs/configuration/governance.yaml.example`](./governance.yaml.example) —
7 conservative defaults (block `rm -rf` / `mkfs` / private-IP egress,
allow `audit.read` / `memory.write` / `browser.navigate` / `fs.write`).

Optional hot-reload: set `CYBERCLAW_POLICY_RULES_RELOAD_SECS=10` and the
server re-reads on mtime change without restart.

This is **separate from** the inline `[governance]` table in
`config.toml` (trust matrix / iron-law / allowlist below) — the YAML
RuleSet wins for any rule it matches; unmatched dispatches fall through
to the inline trust matrix.

## Trust matrix file (`~/.cyberclaw/governance.toml`)

Loaded at startup if present; merged with `config.toml`'s
`[governance]` table. Rules-array semantics:

```toml
# Trust matrix: (agent_trust, capability_risk) → verdict
[trust_matrix]
"Trusted.Critical"     = "Allow"
"Trusted.High"         = "Allow"
"Standard.Critical"    = "Ask"
"Standard.High"        = "Ask"
"Standard.Medium"      = "Allow"
"Restricted.Medium"    = "Ask"
"Restricted.Low"       = "Allow"

# Iron-law rules — non-rationalizable, evaluated first.
[[iron_law]]
description = "no fs.write under /etc"
match       = "fs.write"
arg_pattern = { path = "/etc/**" }
verdict     = "Deny"

# Pattern matchers — produce a risk level, not a verdict.
[[matcher]]
match     = "cmd.run"
arg_pattern = { command = "rm -rf *" }
risk      = "Critical"

[[matcher]]
match     = "fs.write"
arg_pattern = { path = "**/.env*" }
risk      = "Critical"

# Capability allowlist (per agent)
[[allowlist]]
agent      = "agent-coder"
capabilities = [
  "fs.read", "fs.write", "fs.list",
  "cmd.run", "cmd.run_streaming",
  "lsp.diagnostics", "lsp.find_references",
]
```

The matcher and iron-law sections both take `arg_pattern` as a
shallow JSON-like map of glob expressions over the capability's
input arguments. See
[`docs/security/governance-model.md`](../security/governance-model.md)
for the full evaluation algorithm.

## Object manifests

Each first-class object has its own TOML manifest. Detailed schemas
are in [`docs/reference/manifests.md`](../reference/manifests.md);
machine-readable JSON Schemas are in [`schemas/`](../../schemas/).

| Object | Manifest | Schema |
|---|---|---|
| Agent | `ecosystem/agents/<name>/agent.toml` | `schemas/agent.schema.json` |
| Skill | `ecosystem/skills/<name>/SKILL.md` (frontmatter) | `schemas/skill.schema.json` |
| Connector | `ecosystem/connectors/<name>/connector.toml` | `schemas/connector.schema.json` |
| Platform Plugin | `ecosystem/platform-plugins/<name>/plugin.toml` | `schemas/plugin.schema.json` |

## Migration & legacy

Variables marked legacy are still read but trigger a warning. They
will be removed in 0.2.0.

| Legacy | Replacement |
|---|---|
| `CYBERCLAW_SERVER_URL` (CLI) | `CYBERCLAW_SERVER` |
| `CYBERCLAW_TOKEN_FILE` | `CYBERCLAW_TOKEN` (env) or `~/.cyberclaw/cli-token` |

## See also

- Quick environment recipes: [`docs/getting-started/installation.md`](../getting-started/installation.md)
- Production hardening: [`docs/deployment/security-checklist.md`](../deployment/security-checklist.md)
- Per-platform deployment: [`docs/deployment/`](../deployment/)
- Original detailed env doc: [`docs/ENVIRONMENT_VARIABLES.md`](../ENVIRONMENT_VARIABLES.md)
