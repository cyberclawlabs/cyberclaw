# Usage Guide

This guide takes you from cloning the repository to running a hardened deployment and extending the platform.

CyberClaw is in **Beta** — do not connect real funds, production databases, or live business systems at this stage. Validate first in read-only, staging, or internal-tool form.

---

## Prerequisites

- **Rust toolchain** — 1.75 or later. Install via [rustup](https://rustup.rs).
- **Node.js** — 18+ for building the WebUI.
- **OpenSSL** — for TLS certificates if you enable HTTPS.
- **Operating system** — Linux or macOS. Windows works under WSL2.

## 1. Install

Clone and build:

```bash
git clone https://github.com/cyberclawlabs/cyberclaw.git
cd cyberclaw
cargo build --release -p cyberclaw-server
cargo build --release -p cyberclaw-cli
```

The compiled binaries land in `target/release/cyberclaw-server` and `target/release/cyberclaw-cli`.

The WebUI builds separately when you have a frontend toolchain installed:

```bash
cd web
pnpm install
pnpm build
```

## 2. Configure

CyberClaw reads its configuration from environment variables and an optional YAML rules file. For a first run, the only required variables are an LLM provider key and an approval secret:

```bash
cp .env.example .env
# Edit .env and set at least:
#   LLM_API_KEY=...                        # provider key (any OpenAI-compatible)
#   CYBERCLAW_APPROVAL_SECRET=...          # 32-char random string for approval HMAC
```

Common environment variables, ordered by typical configuration sequence:

| Variable | Purpose |
|---|---|
| `LLM_API_KEY` | LLM provider key (provider chosen by `LLM_PROVIDER`). |
| `LLM_PROVIDER` | `anthropic` / `openai` / `deepseek` / `volcengine` / `minimax` / etc. |
| `LLM_BASE_URL` | Optional — override for OpenAI-compatible endpoints. |
| `CYBERCLAW_APPROVAL_SECRET` | HMAC secret for approval signatures (≥32 chars). |
| `JWT_SECRET` | HMAC secret for operator JWTs. Required outside development. |
| `CYBERCLAW_ADDR` | Bind address (default `127.0.0.1:38090`). |
| `CYBERCLAW_POLICY_RULES_PATH` | Path to the governance YAML file. Auto-detected at `~/.cyberclaw/governance.yaml` if present. |
| `CYBERCLAW_AUDIT_DB` | SQLite path for the audit chain (default `~/.cyberclaw/audit.db`). |
| `USE_TLS` / `TLS_CERT_PATH` / `TLS_KEY_PATH` | Enable HTTPS. |
| `ENVIRONMENT` | `development` / `staging` / `production`. Affects HSTS and error verbosity. |

The full annotated list is in `.env.example` at the repo root, and a complete reference is in [ENVIRONMENT_VARIABLES.md](ENVIRONMENT_VARIABLES.md).

### Governance rules

Drop a `governance.yaml` next to the audit database:

```bash
mkdir -p ~/.cyberclaw
cat > ~/.cyberclaw/governance.yaml <<'YAML'
- kind: deny
  capability_id: cmd.run.rm_rf
  reason: "irreversible"

- kind: review
  capability_id: eth.sign
  reason: "wallet operations require human approval"
YAML
```

The server auto-detects this file at startup (`CYBERCLAW_POLICY_RULES_PATH` overrides). Rule changes take effect on the next dispatch — no restart required.

### Sandbox profiles

Container-isolated capabilities accept a `sandbox` field with one of three profiles:

- `minimal` — no network, read-only filesystem. For deterministic compute.
- `dev` — workspace writes allowed, no outbound network. Default for development environments.
- `isolated` — full lockdown: no network, no filesystem writes outside an explicit allowlist.

Set the profile per-capability in the registry, or rely on the environment default (`dev` for development, `isolated` for production).

### Multi-key credential pool (v1.2.18+)

A single LLM provider can be backed by multiple API keys with automatic rotation on billing exhaustion, rate-limit, or auth failures. Add `[[llm.credentials]]` entries to your config:

```toml
[[llm.credentials]]
provider = "anthropic"
api_key = "sk-ant-key-1"
max_concurrent = 4

[[llm.credentials]]
provider = "anthropic"
api_key = "sk-ant-key-2"
max_concurrent = 4
```

Selection strategy is `fill_first` by default; alternatives `round_robin`, `random`, `least_used` are supported. Cooldown duration scales with the failure reason — billing exhaustion locks the key for 24h, rate-limit for 60s, auth-invalid permanently (manual re-enable required).

Single-key deployments using the existing `api_key` field continue to work unchanged.

### Locale (v1.2.18+)

CLI/TUI approval prompts, slash command help, and status messages switch between English and Simplified Chinese based on:

1. `CYBERCLAW_LOCALE` env var (`en` or `zh`)
2. `LANG` env var (e.g. `zh_CN.UTF-8` resolves to `zh`)
3. `[localization] default_locale` config setting

Defaults to English. Missing Chinese keys fall back to English automatically.

## 3. Run locally

```bash
target/release/cyberclaw-server
# Default bind: http://127.0.0.1:38090
```

Verify the server is healthy:

```bash
curl http://127.0.0.1:38090/health    # → "OK"
```

Open the operator console:

```
http://127.0.0.1:38090/admin/v2/
```

You will see a login screen. The first time, you can authenticate either by:

- Running `cyberclaw onboard` — creates an operator account interactively.
- Or hand-editing `~/.cyberclaw/users.toml` to add an operator entry.

## 4. Daily operations

### CLI commands

```bash
cyberclaw doctor              # diagnose configuration health
cyberclaw chat                # start an interactive chat session with an agent
cyberclaw sessions ls         # list active sessions
cyberclaw sessions show <id>  # inspect a session
cyberclaw memory search "..." # search the memory store
cyberclaw audit tail          # follow the audit chain in real time
cyberclaw audit verify        # verify the hash chain end to end
```

### WebUI tabs

- **Chat** — live conversation against any configured agent.
- **Agents** — create / edit / delete agent definitions.
- **Sessions** — historical and active conversation list.
- **Skills** — install, configure, or quarantine Skills.
- **Connectors** — see registered Connectors and their Capabilities.
- **Audit** — browse the chain with filters by agent / capability / decision.
- **Approvals** — pending review queue. Approve or reject with a reason.
- **Profiles** — SOUL preset library (system-prompt personalities).
- **Models** — configure LLM providers and model routing.
- **MoA** — configure Mixture-of-Agents and run test prompts.

## 5. Production deployment

Beyond a local run, production deployment requires four additional steps.

### Generate strong secrets

```bash
JWT_SECRET=$(openssl rand -base64 48)
APPROVAL=$(openssl rand -base64 48)
echo "JWT_SECRET=$JWT_SECRET" >> .env
echo "CYBERCLAW_APPROVAL_SECRET=$APPROVAL" >> .env
```

Set `ENVIRONMENT=production` and the server will fail-fast if any required secret is missing or shorter than 32 chars.

### Enable TLS

```bash
echo "USE_TLS=1" >> .env
echo "TLS_CERT_PATH=/etc/letsencrypt/live/example.com/fullchain.pem" >> .env
echo "TLS_KEY_PATH=/etc/letsencrypt/live/example.com/privkey.pem" >> .env
```

`USE_TLS=1` automatically enables HSTS in production mode.

### Configure the audit archive

```bash
echo "CYBERCLAW_AUDIT_ARCHIVE_DIR=/var/lib/cyberclaw/archive" >> .env
echo "CYBERCLAW_AUDIT_ARCHIVE_INTERVAL_SECS=3600" >> .env
echo "CYBERCLAW_AUDIT_ARCHIVE_GPG_KEY=ABC123..."  >> .env     # optional, encrypts snapshots
```

### Run multi-replica (optional)

```bash
echo "CYBERCLAW_CLUSTER_MODE=multi" >> .env
echo "CYBERCLAW_CLUSTER_SHARED_TOKEN=$(openssl rand -hex 32)" >> .env
echo "CYBERCLAW_RAFT_BIND_ADDR=10.0.0.1:7700" >> .env
echo "CYBERCLAW_RAFT_PEERS=node-2@10.0.0.2:7700,node-3@10.0.0.3:7700" >> .env
```

Before exposing the server to real traffic, verify TLS certificates, audit-archive disk space, and that all required secrets (JWT, approval HMAC, webhook secrets per platform) are set from the environment rather than defaults.

## 6. Extending the platform

Three extension surfaces. Each preserves the runtime's safety guarantees by passing through the same dispatch path.

### Add a Skill

A Skill is a method bundle: prompt template + procedural knowledge + reference assets. Skills cannot reach external systems — they describe how an Agent should reason and what Connectors to invoke.

Create `ecosystem/skills/my-skill/SKILL.md`:

```markdown
---
name: my-skill
description: short description
---

# My Skill

Detailed method, prompt template, examples...
```

Register and verify:

```bash
cyberclaw skills install my-skill
cyberclaw skills list
```

### Add a Connector

A Connector bridges CyberClaw to one external system. Implement the `Connector` trait in Rust, declare the Capabilities you expose, and the dispatcher routes matching requests to you.

Outline:

1. Create a new crate or add to `crates/cyberclaw-connectors/src/your_connector.rs`.
2. Implement `impl Connector for YourConnector` and the dispatch entry.
3. Register in `crates/cyberclaw-connectors/src/lib.rs`.
4. Add governance rules to enable the new Capability for specific Agents.

Existing connectors in the same crate are the easiest reference.

### Add a Platform Plugin

A Platform Plugin extends the runtime itself — a custom audit sink, a new policy enforcer, additional observability. Plugins implement hooks the dispatcher calls in well-defined positions; they cannot bypass governance.

The plugin runtime crate is at `crates/cyberclaw-plugin-runtime`. Reference implementations are provided as examples.

## 7. Troubleshooting

| Symptom | First check |
|---|---|
| `cyberclaw doctor` fails | run with `RUST_LOG=debug` to see which preflight check trips. |
| 401 from WebUI | JWT secret mismatch — your operator token was signed with a different secret than the server. Re-run `cyberclaw onboard`. |
| Tool call returns 500 in WebUI | look at server logs. Most common cause: an unconfigured connector for that Capability. |
| Audit chain verify fails | someone tampered with the audit table, OR an older row used a different hashing version. Inspect the chain at the divergent row. |
| Agent stuck in approval queue | open WebUI → Approvals tab, locate the request, approve or reject. |
| Webhook ingress refused | check that the matching `CYBERCLAW_WEBHOOK_SECRET_<PLATFORM>` is set. The server refuses webhooks for any platform without a configured secret. |
| Container task fails with "no such path" | v1.2.17 changed the container mount layout from `/workspace` to host-cwd 1:1. Tooling that hard-coded `/workspace` as workdir must be updated to use the real host path. |
| `search.grep` returns unexpectedly many matches | v1.2.17 flipped the `case_insensitive` default to `true`. Pass `case_insensitive: false` to restore case-sensitive matching. |

For deeper issues, open a GitHub issue at [cyberclawlabs/cyberclaw](https://github.com/cyberclawlabs/cyberclaw/issues) with the relevant `RUST_LOG=debug` output (with secrets redacted).

---

For internal architecture details, see [ARCHITECTURE.md](ARCHITECTURE.md).
