---
name: explore
description: Scoped read-only codebase mapping and fact-finding (CyberClaw-adapted)
source: derived from oh-my-claudecode/agents/explore (no source skill existed; built from the explore agent's methodology)
adapted-for: CyberClaw (Sprint 4, 2026-04-18)
level: 1
---

<!--
CyberClaw adaptation notes:
- NOTE ON SOURCE: oh-my-claudecode ships `explore` as an **agent**, not as a
  skill (`tmp/claw-research/oh-my-claudecode/skills/` has no `explore/`
  directory). This SKILL.md is therefore a derivation that captures the
  read-only exploration methodology the explore agent follows, so that it
  can be referenced as a skill from other CyberClaw methodology documents
  (notably `ecosystem/skills/plan/SKILL.md`).
- Skills never execute in CyberClaw (CLAUDE.md §3.4 and §9). The actual
  codebase scans are performed by the agent that invokes this skill via
  `SubAgentOrchestrator::spawn_child(AgentId::new("explore"))` — not by the
  skill itself.
- Sibling dispatch uses CyberClaw's native `SubAgentOrchestrator` in
  `crates/cyberclaw-agent-runtime/src/sub_agent.rs`, not Claude Code's
  `Task()` primitive.
-->

# Explore

Use this skill methodology when you need fast, scoped, read-only answers
about a codebase: "where is X implemented?", "what patterns does this repo
use for Y?", "what tests exist for Z?".

## Goal

Replace user questions-about-the-codebase with evidence. When a planner,
architect, or executor is about to ask the user something the code can
answer, they should instead dispatch an explore sub-agent via
`SubAgentOrchestrator::spawn_child(AgentId::new("explore"))` and keep the
user out of the loop.

## When to Use

- Before asking the user a codebase fact question ("where is auth?", "what
  error style does the repo use?")
- Before producing a plan that references files or patterns
- Before code review, when you need to find all callers of a symbol
- Before debugging, when you need to find the most recent similar pattern

## When NOT to Use

- When the question is a **preference** (timeline, priority, risk tolerance) —
  ask the user instead.
- When you already have the answer in context — do not re-dispatch.
- When the task requires writing code — use `executor` instead.

## Workflow

1. **Define the question** narrowly: one topic, one scope.
2. **Prefer parallel tools**: Glob for file layout, Grep for symbol/pattern
   hits, Read for confirmation. Run these in parallel where independent.
3. **Budget**: explore should be cheap and bounded. Budget a handful of
   searches and a few file reads, not a full codebase walk.
4. **Report**: return file:line references, not prose summaries. The
   caller will interpret.
5. **Do not modify**: explore is read-only. No Edit, Write, ast_grep_replace.

## Output

- Summary (one paragraph, high-level answer)
- Evidence (list of `path/to/file.rs:line` references with one-line notes)
- Open questions (what you could not find, if any)

## Tool Usage

- Glob, Grep, Read, ast_grep_search (read-only).
- Optionally `lsp_*` hover/definition/references helpers when the harness
  provides them.
- **Not** Edit / Write / ast_grep_replace.
- **Not** Bash commands that mutate state.

## Failure Modes To Avoid

- Dumping entire files when a line range would do.
- Exploring outside the requested scope ("while I'm here, let me also look
  at...").
- Answering with prose when file:line evidence is available.
- Running more searches than needed; budget first, then search.

## Examples

**Good**: Planner asks explore: "find authentication implementation."
Explore returns: "Auth flows through `crates/cyberclaw-control-plane/src/auth.rs:42-120` (JWT validation with constant-time comparison). Related middleware at `crates/cyberclaw-control-plane/src/middleware/auth.rs:15-60`. Tests in `crates/cyberclaw-control-plane/tests/auth_test.rs`." One paragraph summary, concrete references.

**Bad**: Explore returns a 2000-line codebase tour. Wrong tool, wrong
budget.
