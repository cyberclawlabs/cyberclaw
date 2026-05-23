# Skill Auto-Bind — Keyword-driven domain expert injection

> Status: implemented in `crates/cyberclaw-skill-runtime/src/skill_binder.rs`.
> Wired into chat dispatch in `apps/cyberclaw-server/src/api/chat_handler.rs`.
> Object-model placement: still a `Skill`. SkillBinder is a *binder*, not a
> new ecosystem object.

## 1. Motivation

`req.skill_ids` lets a caller name skills explicitly. In practice, the caller
(end user, frontend, integration) often doesn't know which skills exist or
which apply. A user asking "design a 5-step USDC→ETH multisig runbook"
benefits enormously from a Web3 domain-expert skill being attached to the
prompt, but typing `--skill domain-expert-web3` is not viable for most users.

Auto-bind solves this by letting a skill **declare a keyword pattern** in its
`manifest.yaml`. When the runtime sees a chat request whose prompt matches the
pattern, the skill is automatically appended to the binding set and its
SKILL.md is injected into the system prompt.

This is **additive**: caller-supplied `skill_ids` always win; auto-bound
skills are appended after them (deduplicated).

## 2. Manifest schema

```yaml
apiVersion: cyberclaw.io/v2
kind: Skill
name: domain-expert-web3
spec:
  format: claude-compatible
  auto_bind:
    keywords:
      - any: [multisig, Safe, Gnosis]
      - any: [USDC, ETH, gas, gwei]
      - any: [reentrancy, ERC-20, smart contract]
    priority: 80
```

Top-level field is `spec.auto_bind`. Two leaves:

- `keywords` — a list of **OR-groups**. Each list element is an object with an
  `any:` key whose value is a list of strings. Within a group, **any** match
  satisfies the group. Across groups, **all** groups must be satisfied
  (AND-of-OR semantics). A rule with zero groups never matches (no
  accidental always-bind).
- `priority` — integer (default 0). When multiple skills match, higher
  priority is preferred in ordering. There is no hard cap on number of
  auto-bound skills; callers typically take the top-K.

Matching is **case-insensitive substring**. There is no tokenization, no
stemming, no regex. Keep keywords short and distinctive — `safe` will match
"safehouse" too, so multi-keyword groups are essential when keywords are
ambiguous in isolation.

## 3. Algorithm

```
1. Lowercase the user prompt (last user message in the request).
2. For each registered AutoBindRule:
   a. For each OR-group, check if any keyword (lowercased) is a substring of
      the lowercased prompt.
   b. The rule matches iff EVERY group has at least one hit.
3. Collect all matched rules.
4. Stable-sort matched rules by priority descending.
5. Return the rule list to the caller.
```

Implementation: `SkillBinder::match_prompt(&self, prompt: &str) -> Vec<&AutoBindRule>`.

## 4. Registry lifecycle

At server start (or skill-hub reload), the runtime walks
`{skill_hub_base}/installed/` and calls
`SkillBinder::load_from_dir(installed_dir)`. Each immediate child directory
that contains a `manifest.yaml` is parsed:

- If the manifest carries a valid `spec.auto_bind`, a rule is registered.
- If absent, the skill is silently skipped (it remains explicitly bindable
  via `skill_ids`).
- If the YAML is broken, the offending skill is logged at `warn` and skipped.
  A single broken skill must not break the registry.

The registry is rebuilt on hot-reload events (mirroring the existing
SkillScanner hot-reload integration).

## 5. Injection path

`chat_handler.rs` performs the following sequence per request:

1. Resolve explicit `req.skill_ids` → `agentic_loop.active_skill_bindings()`.
2. Read each SKILL.md and inject body into `loop_config.system_prompt` under
   `## Skill: <name>` headings (existing behavior).
3. **NEW**: query `SkillBinder::match_prompt(last_user_message)`. For each
   matched rule not already bound:
   - Read `{skill_dir}/SKILL.md`.
   - Append under `## Auto-bound skill: <name>` heading.
   - Log at info with the matched skill name + priority.
4. Proceed with agentic loop run.

The master-agent SYSTEM_PROMPT carries a stub section ("Auto-bound domain
expertise") instructing the LLM to treat auto-bound sections as advisory
peer expertise rather than absolute rule.

## 6. Boundaries and non-goals

- **Not a tool router.** Auto-bind decides which *system prompt content* to
  inject. Tools / capabilities are unaffected — they still flow through
  Connector → Capability with PolicyEngine gating.
- **Not a permission widener.** A skill being auto-bound does not grant new
  capabilities. The Skill object model still treats skills as
  methodology / vocabulary, not as executors.
- **Not LLM-driven.** No semantic embedding, no inference. This is a
  deterministic substring matcher so that operators can audit "this prompt
  matched this rule" without re-running a model.
- **No on-the-fly install.** Only skills already present in `installed/` can
  be auto-bound. Installing a new skill is governed by the SkillHub flow.

## 7. Operator playbook

To add a new domain expert:

1. Create `ecosystem/skills/<name>/manifest.yaml` with a `spec.auto_bind`
   block.
2. Author `SKILL.md` with YAML frontmatter (`name`, `version`,
   `description`).
3. Optional `references/*.md` for the SKILL.md to point at.
4. Install via the normal SkillHub install path (or symlink for local dev).
5. Restart the server (or trigger hot-reload).
6. Verify with `curl /api/v2/status` — skill count should increase by 1.
7. Send a probe chat request whose prompt hits the keyword groups; confirm
   the server log shows `Auto-bound skill: <name>` and the response uses
   the skill's vocabulary.

To tune a rule that's firing too often (false positives):

- Add a second OR-group with more-specific terms.
- Increase keyword specificity (`"Safe multisig"` instead of `"Safe"`).
- Lower priority so it ranks below more-specific rules.

To tune a rule that's firing too rarely (false negatives):

- Expand the OR-groups with synonyms / common typos.
- Verify the prompt actually contains the expected terms (the matcher is
  literal — no spell correction).

## 8. Worked example

Manifest:

```yaml
spec:
  auto_bind:
    keywords:
      - any: [multisig, Safe, Gnosis]
      - any: [USDC, ETH, gas, gwei]
    priority: 80
```

| Prompt | Matches? | Why |
|---|---|---|
| "design a 5-step USDC→ETH multisig runbook" | yes | group1=multisig, group2=USDC + ETH |
| "how do safes work" | no | group2 has no hit |
| "what is reentrancy" | no | group1 has no hit |
| "send 100 USDC to alice" | no | group1 has no hit |
| "set up a Gnosis Safe for ETH treasury" | yes | group1=Gnosis + Safe, group2=ETH |

## 9. Telemetry

Each auto-bind decision emits a log line at `info`:

```
auto_bind {request_id} skill=domain-expert-web3 priority=80 groups_matched=2
```

Aggregated metrics (Prometheus, if enabled):

- `cyberclaw_skill_binder_rules_total` — gauge, number of loaded rules.
- `cyberclaw_skill_binder_matches_total{skill}` — counter, per-skill match count.
- `cyberclaw_skill_binder_lookup_duration_seconds` — histogram, matching latency.

Latency target: < 1 ms even with 100+ rules. Substring matching is O(rules \*
keywords \* prompt_length) which stays well under budget at production sizes.
