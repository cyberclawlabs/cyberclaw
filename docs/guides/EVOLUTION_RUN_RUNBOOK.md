# Evolution `/run` Runbook

Operator-facing guide for triggering and observing skill description
optimization cycles through the `LlmEvolutionDispatcher`.

## What this is

`POST /api/v1/learning/evolution/run` accepts a target Skill plus a
trigger-query dataset and runs one optimization cycle through the
`EvolutionOrchestrator` + `LlmEvolutionDispatcher` (Sprint 21 path #2).
The endpoint returns `202 Accepted` immediately and writes the
terminal `EvolutionCycle` record into `~/.cyberclaw/evolution.log` when
the spawned task finishes. The existing
`GET /api/v1/learning/evolution/timeline` surfaces those rows in the
admin UI.

This is a **plumbing-complete starter**. Production cycles need:

- A curated trigger-query dataset for the target Skill
- Real LLM credentials wired into `AppState.llm_client`
- Per-skill concurrency control (today, two POSTs against the same
  skill spawn two parallel optimizations)

## Request

| Field | Type | Required | Notes |
|---|---|---|---|
| `skill_id` | string | yes | Identifier of the Skill being optimized. Used in audit + log meta. |
| `target_skill_path` | string | yes | Filesystem path to the Skill directory. Must contain `SKILL.md`. |
| `trigger_dataset` | array<{query, should_trigger}> | yes | At least one element. Split 60/40 train/holdout. |
| `max_iterations` | u32 | no | Override; default `5`. |
| `min_pass_rate` | f32 | no | Override; default `0.9`. |
| `model` | string | no | LLM model id. Falls back to `CYBERCLAW_EVOLUTION_MODEL` env, then `gpt-4o-mini`. |

### Example

```bash
JWT=$(cat ~/.cyberclaw/admin.jwt)
curl -s -X POST https://staging.cyberclaw.local/api/v1/learning/evolution/run \
  -H "Authorization: Bearer $JWT" \
  -H "Content-Type: application/json" \
  -d '{
    "skill_id": "sk_resilient-research",
    "target_skill_path": "/srv/cyberclaw/ecosystem/skills/resilient-research",
    "trigger_dataset": [
      { "query": "research the impact of AI on accounting", "should_trigger": true },
      { "query": "what time is it in Tokyo", "should_trigger": false },
      { "query": "summarize the latest LLaMA paper", "should_trigger": true }
    ],
    "max_iterations": 3,
    "model": "gpt-4o-mini"
  }'
```

Response:

```json
{ "cycle_id": "evo_a1b2c3d4...", "status": "running" }
```

## Auth

Admin role required (`require_admin` checks `~/.cyberclaw/users.toml`).
Non-admins receive `403 Forbidden`. The audit row written before
spawning includes `cycle_id`, `model`, `trigger_count`,
`max_iterations`, and `min_pass_rate` for forensic replay.

## Reading results

### Admin UI

Open the **Learning → Evolution Timeline** tab. Each cycle becomes a
row with:

- `principal` — `<skill_id> (skill)`
- `mutation` — human summary derived from outcome + skill_id
- `fitness_delta` — formatted from `meta.pass_rate`
- `status` — `accepted` (converged) / `rolled-back` (failed) /
  `pending-observe` (max iterations)
- `evaluator` — `model + iterations + pass_rate`
- `diff` — winning variant id or failure reason

### Direct log

```bash
tail -n 1 ~/.cyberclaw/evolution.log | jq .
```

Each line is one `EvolutionCycle` JSON record:

```json
{
  "id": "evo_a1b2c3d4...",
  "started_at": "2026-05-03T10:15:00Z",
  "ended_at": "2026-05-03T10:15:42Z",
  "outcome": "max_iterations",
  "gene_changed": "var_xyz...",
  "meta": {
    "pass_rate": 0.42,
    "skill_id": "sk_resilient-research",
    "model": "gpt-4o-mini"
  }
}
```

## Outcome → status decision tree

| Outcome | When | Operator action |
|---|---|---|
| `converged` | Holdout pass rate ≥ `min_pass_rate` | Promote `gene_changed` variant — adopt the rewritten SKILL.md |
| `max_iterations` | Hit iteration cap without converging | Inspect `meta.pass_rate`. Below 0.5 → re-evaluate dataset; 0.5–0.85 → bump `max_iterations` and re-run; ≥ 0.85 → operator judgement call |
| `failed` | Hard error (missing SKILL.md, LLM unavailable, dispatcher exception) | Read `meta.reason`. Common causes: wrong `target_skill_path`, expired LLM credentials, network drop |

## Verification gate

```bash
cargo test -p cyberclaw-server --test e2e_evolution_test
```

Runs the mock-LLM contract test (5 assertions; ~1s).

For real-LLM availability:

```bash
set -a && source apps/cyberclaw-server/.env && set +a
cargo test -p cyberclaw-server --test e2e_evolution_test \
  evolution_run_real_llm_availability_gate -- --ignored --nocapture
```

The ignored test only runs when `LLM_PROVIDER` is set to a non-mock
provider (`openai` / `ark` / `anthropic` / `generic`). It verifies the
endpoint contract still holds against production-shaped LLM I/O. Use
this as a pre-deploy gate after rotating LLM keys or changing the
default model.

## Known caveats (path #2 honest scope)

These are documented inline in the code as `// PRODUCTION:` comments
and reproduced here so operators don't re-discover them:

1. **Mutation prompt is generic.** `LlmEvolutionDispatcher::execute_mutation`
   does not yet include few-shot examples calibrated for any specific
   workflow. Quality of rewrites is bounded by the base model's
   instruction-following, not by domain priors.
2. **Case scoring is binary.** `execute_case` parses `{hit:bool, why:string}`
   from the LLM. Partial credit (e.g. matched-but-wrong-tool) is not
   represented. This biases convergence toward over-confident cases.
3. **No multi-model voting.** The same model rewrites and scores. For
   production-grade evaluation, split the two: cheap model rewrites,
   stronger/different model scores.
4. **Smoke regression is a stub.** `run_smoke_regression` always
   passes. Replace with a curated baseline trigger set for production.
5. **No per-skill concurrency limit.** Two simultaneous POSTs against
   the same `skill_id` spawn two independent optimizations. If this
   becomes an operational issue, add a `Mutex<HashSet<String>>` keyed
   by `skill_id` in `AppState` and gate the spawn on it.

## See also

- [`apps/cyberclaw-server/src/api/learning.rs`](../../apps/cyberclaw-server/src/api/learning.rs) — endpoint impl
- [`crates/cyberclaw-control-plane/src/llm_evolution_dispatcher.rs`](../../crates/cyberclaw-control-plane/src/llm_evolution_dispatcher.rs) — LLM-backed `EvolutionDispatcher` impl
- [`crates/cyberclaw-control-plane/src/skill_creator.rs`](../../crates/cyberclaw-control-plane/src/skill_creator.rs) — façade façade
- [`docs/architecture/idioms/EVOLUTION_IDIOMS.md`](../architecture/idioms/EVOLUTION_IDIOMS.md) — design rules
