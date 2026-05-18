# CyberClaw Ecosystem Agents

This directory holds **methodology documents** for named Agent roles used across
CyberClaw workflows. These are portable prompt scaffolds, not executors. In
CyberClaw, actual execution flows through `Connector → Capability` in the
control plane (see root `CLAUDE.md §3`), and sibling agent dispatch is handled
by `SubAgentOrchestrator` in
`crates/cyberclaw-agent-runtime/src/sub_agent.rs`.

All 8 agents below were ported from `oh-my-claudecode` as part of
Sprint 4 (2026-04-18) and adapted to CyberClaw's native runtime.

## Ported Agents

| Agent           | One-line purpose                                                                 | Source origin (oh-my-claudecode)          |
|-----------------|-----------------------------------------------------------------------------------|-------------------------------------------|
| `analyst`       | Turn decided scope into testable acceptance criteria before planning              | `agents/analyst.md`                       |
| `planner`       | Produce 3-6 step actionable work plans; never implement                           | `agents/planner.md`                       |
| `architect`     | Read-only codebase analysis with file:line evidence for every claim               | `agents/architect.md`                     |
| `executor`      | Smallest-viable-diff implementer with explore/architect consultation limits       | `agents/executor.md`                      |
| `critic`        | Read-only final quality gate with gap analysis, multi-perspective, Realist Check  | `agents/critic.md`                        |
| `verifier`      | Fresh-evidence completion checks; rejects "should work" claims                    | `agents/verifier.md`                      |
| `code-reviewer` | Severity-rated review (CRITICAL/HIGH/MEDIUM/LOW) with spec-compliance-first       | `agents/code-reviewer.md`                 |
| `git-master`    | Atomic-commit strategy with project-style detection                               | `agents/git-master.md`                    |

## Agents NOT Ported (Sprint 4 scope limit)

These agents exist in the oh-my-claudecode source but were intentionally
not ported in this sprint: `explore`, `debugger`, `tracer`, `security-reviewer`,
`test-engineer`, `designer`, `writer`, `qa-tester`, `scientist`,
`document-specialist`, `code-simplifier`. They remain candidates for a
future sprint. Until they are ported, referring to them by name in a
`SubAgentOrchestrator::spawn_child(AgentId::new("<name>"))` call will
require that a corresponding Agent record exists in `cyberclaw-store`.

## CyberClaw-Specific Adaptations

Every agent file has been adapted for CyberClaw. The key substitutions are:

1. `Task(subagent_type="oh-my-claudecode:X", ...)`
   → `SubAgentOrchestrator::spawn_child(AgentId::new("X"))`
   (`crates/cyberclaw-agent-runtime/src/sub_agent.rs`)

2. `state_read` / `state_write` / `state_clear` (OMC MCP tools)
   → `cyberclaw-store` crate (Artifact + Semantic Memory scopes)

3. `TaskCreate` / `TaskList` / `SendMessage` (OMC MCP tools)
   → CyberClaw equivalents: `cyberclaw-store` + `SubAgentOrchestrator`
     (+ future TaskManager Capability — see `CLAUDE.md §11`)

4. Paths like `.omc/plans/*.md` remain valid under OMC-compatible harnesses;
   under CyberClaw runtime, plan artifacts live in `cyberclaw-store`.

The full translation map with before/after snippets is in
`docs/implementation/sprint4-omc-adaptation-map.md`.

## Architectural Constraints

- No agent in this directory executes code or spawns processes directly.
  They are prompt methodology documents.
- All code execution in CyberClaw must flow through `Connector → Capability`
  under governance review (see `crates/cyberclaw-governance`).
- Skills never execute — this is enforced by project policy
  (root `CLAUDE.md §3.4`).

## Sibling Artifact

- `ecosystem/skills/README.md` — 8 ported skill methodology documents.
