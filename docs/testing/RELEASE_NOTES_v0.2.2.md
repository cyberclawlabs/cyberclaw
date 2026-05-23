# CyberClaw v0.2.2 — micro-sprint release notes

**Date:** 2026-05-13
**Scope:** depth-of-validation pass after v0.2.1 GA
**Tag candidate:** `v0.2.2-rc1`

---

## Why this release exists

v0.2.1 closed with 3847 verifications green, but the smoke depth was shallow in
places: we counted facades instead of validating their schemas, we asserted
trace events instead of trace shape, and we treated 422 as failure when it
really means "route reachable but schema not satisfied."

This micro-sprint deepens the validation. **One real bug was caught.**

---

## Real bug fixed (deep validation worked)

**`local_task` connector facades had empty `input_schema = {}`** (6 facades: task_create / task_get / task_list / task_update / task_stop / task_output).

| Before | After |
|---|---|
| LLM sees the tool name but no params. Schema is `{}`. Agent attempts call with random keys → handler rejects → "looks like the tool is broken" | All 6 facades now have full JSON Schema: required + optional fields per handler signature |

Root cause: `crates/cyberclaw-connectors/src/local/task.rs:434-532` had `input_schema: None` on every entry. The deep validator (`smoke-facade-schema.sh`) caught this on first run.

**Diff size:** 1 file, ~80 lines (schema JSON for 6 facades).

---

## New validation surfaces

### `smoke-facade-schema.sh` — deep facade validator
Replaces "count 41 facades" with per-facade schema validation:

- F1 total ≥ 30
- F2 no empty `name`
- F3 `description` ≥ 10 chars (catches placeholder descriptions)
- F4 `risk_level` in `{info, low, medium, high, critical}`
- F5 `input_schema` is an object with `properties` key
- F6 `effects` non-empty

Current result: **41/41 facades pass all six gates.**

### `smoke-memory.sh` M4.2 — trace shape (honest fix)
Old assertion `events=N` was wrong: trace records *execution provenance* (writes by Capability runs), not human admin edits.

New assertion validates the response has both `written_by` and `read_by` keys.
This is the actual contract of the endpoint.

---

## Things that **looked** like gaps but weren't

These were investigated during the micro-sprint and found to be already correct:

| Suspected gap | Reality |
|---|---|
| `learning.evolution.run` doesn't emit audit | It does (`learning.rs:584-600`). Smoke sent `{}` which fails schema → 422 → never reaches audit. **Audit code is correct.** |
| `memory.edit` doesn't emit audit | It does (`memory.rs:930-948`). Smoke M7 confirmed audit `memory.*` events present. |
| `memory.edit` doesn't write trace | By design. Trace tracks execution provenance, not human edits. M4.2 honest-fixed. |
| 41 facades all valid | 35 were. 6 were broken — fixed in this release. |

The micro-sprint was useful precisely because the "obvious gaps" turned out to be wrong assumptions and a real bug hid behind the easier story.

---

## Verification

```
cargo clippy --workspace --release                  0 warning
cargo test  -p cyberclaw-connectors --lib           654 pass / 0 fail
cargo test  --test governance_red_team              5 pass / 0 fail
smoke-p6-endpoints.sh                               50/50
smoke-tui.sh                                        14/14
smoke-memory.sh                                     12/0
smoke-learning.sh                                   8/0
smoke-audit-chain.sh                                6/0
smoke-tool-bridge.sh                                7/0
smoke-facade-schema.sh                              6/0  ← NEW
smoke-governance.sh                                 6/0
smoke-business.sh                                   8/0
smoke-persistent.sh                                 4/0
```

All 10 smoke scripts exit 0.

---

## What's still deferred (unchanged from AGI plan §6)

- MoA / Profile / Memory edit e2e via UI driver
- message-gateway webhook real platform
- Visual regression (screenshot diff)
- Performance / load baseline
- Cross-browser matrix
- Plugin hot-reload regression

These are productisation gaps, not AGI gaps. They remain v0.3.0 backlog.

---

## Commit

`b<sha>` — `test(v0.2.2): deep facade validator + 6 local_task input_schema fix`

★ Closure principle: depth ≠ breadth. We didn't add 10 more smoke scripts —
we made 1 smoke script (`smoke-facade-schema.sh`) substantially deeper
and that single addition caught 6 real bugs in production code.
