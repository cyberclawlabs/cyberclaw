# CyberClaw WebUI ↔ TUI Parity Matrix

**Purpose:** prevent UI入口漂移 regression. Every operation listed below must work identically from both webui (React) and TUI (ratatui) entry points, hitting the same store/audit chain underneath.

Last verified: 2026-05-13 (post v0.2.1 self-evolution)

---

## Parity table

| # | Capability | WebUI path | TUI command | Underlying store / audit | Parity |
|---|---|---|---|---|---|
| 1 | Login | sidebar avatar → `/login` | startup challenge → JWT in memory | `users.toml` + `admin_login` audit Auth | ✅ |
| 2 | Skill list | `SkillsPage` (`/skills`) | `/skills` slash-command | `skills_store` (filesystem + admin overlay) | ✅ |
| 3 | Skill toggle | `SkillsPage` toggle switch | `/skill toggle <id>` | `admin_store.set_skill_enabled` + audit `skill.toggle` | ✅ |
| 4 | Skill create | `SkillsPage` "Create" button | `/skill create <name>` | `skills_store.create` + audit `skill.create` | ✅ |
| 5 | Skill content | `SkillsPage` detail drawer | `/skill content <id>` | `skills_store.get_content` (read-only) | ✅ |
| 6 | Memory list | `MemoryPage` Tab1 | `/orgmem list` | `LeveledMemoryStore` + audit `memory.list` | ✅ |
| 7 | Memory edit | `MemoryPage` edit modal | `/orgmem edit <id>` | `LeveledMemoryStore.edit` + audit `memory.edit` | ✅ |
| 8 | Memory search | `MemoryPage` search box | `/orgmem search <q>` | `LeveledMemoryStore.search` | ✅ |
| 9 | Profile list | `ProfilesPage` | `/profile list` | `profile_store` (profiles.toml) + audit `profile.*` | ✅ |
| 10 | Profile inject | chat `profile_id` dropdown | `/profile use <id>` | `chat_handler.resolve_system_prompt` | ✅ |
| 11 | MoA config | `MoaPage` | `/moa show` (read-only in TUI) | `moa.toml` + audit `moa.config.update` | ✅ |
| 12 | Daily digest | `LearningPage` Tab1 | `/digest` | `learning.daily_digest` (in-memory aggregator) | ✅ |
| 13 | Curator | `LearningPage` Tab3 | `/curator` | `learning.curator` (read-only KPI) | ✅ |
| 14 | Security policies | `SecurityPage` | `/security` | `governance.policy_rules` (read-only) | ✅ |
| 15 | Audit logs | `LogsPage` | `/audit list` | `audit.logs` + hash chain | ✅ |
| 16 | Audit verify | `LogsPage` "Verify chain" | `/audit verify` | `audit.verify_chain` | ✅ |
| 17 | Executions | `TasksPage` Tab1 | `/exec list` | `executions_store` + persistence | ✅ |
| 18 | Execution trace | `TasksPage` exec row → trace | `/exec trace <id>` | `trace_store` + provenance | ✅ |
| 19 | Reviews | `ReviewsPage` | `/reviews list` | `reviews_store` + approval policy | ✅ |
| 20 | Tools state | `AgentsPage` Tab2 (`Tool Inspector`) | `/tools state` | `deferred_tool_registry` | ✅ |

---

## Parity rules

1. **Single source of truth:** webui and TUI both call the same `/api/v1/*` REST surface — no parallel data paths.
2. **No webui-only audit events:** every mutation logged from webui must also be loggable from TUI (and vice versa).
3. **Schema-locked responses:** the JSON shape returned to webui is the same one TUI consumes.
4. **i18n consistency:** TUI labels follow the same Chinese-simplified terminology as `web/src/i18n.jsx`.

## Out-of-parity (documented):

- **MoA edit in TUI** — TUI shows config read-only. Edits go through webui. *Reason:* MoA JSON nesting is too deep for ergonomic TUI form input. *Mitigation:* `/moa show` in TUI surfaces full config so operator can grep + edit via webui.
- **Plugin install in TUI** — TUI only lists installed plugins. Install/uninstall goes through webui or CLI. *Reason:* file upload UX. *Mitigation:* `cyberclaw plugin install <path>` covers headless installs.

These two are intentional ergonomic asymmetries, not architectural drift.

---

## Smoke coverage map

| Parity row | Validated by smoke script |
|---|---|
| 1 (Auth) | smoke-p6-endpoints.sh preflight |
| 2-5 (Skills) | smoke-p6-endpoints.sh A1-A6 |
| 6-8 (Memory) | smoke-memory.sh M1-M7 |
| 9-10 (Profiles) | smoke-p6-endpoints.sh B1-B7 |
| 11 (MoA) | smoke-p6-endpoints.sh D1-D3 |
| 12-13 (Learning) | smoke-learning.sh L1-L4 |
| 14 (Security) | smoke-governance.sh G1-G6 |
| 15-16 (Audit) | smoke-audit-chain.sh A1-A6 |
| 17-18 (Executions) | smoke-persistent.sh P1-P3 |
| 19 (Reviews) | smoke-persistent.sh P5 / smoke-governance.sh G2-G3 |
| 20 (Tools) | smoke-tool-bridge.sh T1-T5 |

If a row's smoke script ever fails, the parity claim for that row is invalidated.

---

## Regression policy

When a new page or TUI command is added, this file MUST be updated in the same PR:

- Add a row to the parity table.
- Either mark it parity-tested (and add to the smoke coverage map) or document why it's out-of-parity.
- The parity is **not** measured by code coverage but by user-visible operation match.
