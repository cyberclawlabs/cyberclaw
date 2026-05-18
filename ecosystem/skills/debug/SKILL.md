---
name: debug
description: Diagnose the current CyberClaw session or repo state using logs, traces, memory, and focused reproduction
source: oh-my-claudecode/skills/debug/SKILL.md
adapted-for: CyberClaw (Sprint 4, 2026-04-18)
---

<!--
CyberClaw adaptation notes:
- Skills never execute code in CyberClaw (CLAUDE.md §3.4 and §9). This
  methodology describes what to inspect; actual inspection is carried out
  by the agent via observability surfaces.
- "trace tools" and "state tools" in the original doc refer to OMC's
  `trace_*` / `state_*` MCP helpers. The CyberClaw equivalents are:
    * Traces and events     → `cyberclaw-observability` crate (Event bus,
                              Trace timeline, Metrics)
    * Memory / persistence  → `cyberclaw-store` crate (Semantic /
                              Procedural / Artifact scopes)
  When the harness also exposes OMC MCP tools, those still work in parallel.
-->

# Debug

Use this skill when the user wants help diagnosing a current CyberClaw (or OMC-compatible) session problem, workflow breakage, or confusing runtime behavior.

## Goal
Find the real failure signal quickly and explain the next corrective step.

## Workflow
1. Read the user's issue description carefully.
2. Inspect the most relevant local evidence first:
   - **Traces / events**: via `cyberclaw-observability` (Event bus, Trace timeline). Under OMC harnesses, `trace_*` MCP helpers also apply.
   - **Memory / state**: via `cyberclaw-store` (Semantic / Procedural / Artifact scopes). Under OMC harnesses, `state_*` MCP helpers also apply.
   - **Governance / review ledger** when the issue involves auth, permissions, or policy decisions (`crates/cyberclaw-governance`).
   - Failing tests or commands.
3. Reproduce the issue narrowly if possible (smallest failing `cargo test` target, or a minimal Execution replay).
4. Distinguish symptoms from root cause.
5. Recommend the smallest next fix or verification step.

## Rules
- Prefer real evidence over guesses.
- Use the observability / store surfaces when the issue involves orchestration, hooks, or agent flow.
- If the issue is actually a product/runtime bug rather than app code, say so plainly.
- Do not prescribe broad rewrites before isolating the failure.

## Output
- Observed failure
- Root-cause hypothesis
- Evidence for that hypothesis
- Smallest next action
