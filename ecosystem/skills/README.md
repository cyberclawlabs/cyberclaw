# CyberClaw Ecosystem Skills

This directory holds **skill methodology documents** — read-only SKILL.md
files plus optional sidecar assets. Skills in CyberClaw describe HOW to do
something, not code that runs. Actual execution must flow through
`Connector → Capability` in the control plane (see root `CLAUDE.md §3`
and `§9`).

All 8 skills below were ported from `oh-my-claudecode` as part of
Sprint 4 (2026-04-18) and adapted to CyberClaw's native runtime.

## Ported Skills

| Skill           | One-line purpose                                                              | Source origin (oh-my-claudecode)       |
|-----------------|--------------------------------------------------------------------------------|-----------------------------------------|
| `plan`          | Strategic planning with interview/direct/consensus/review modes                | `skills/plan/SKILL.md`                 |
| `explore`       | Scoped read-only codebase mapping and fact-finding                             | Derived from `agents/explore` methodology |
| `verify`        | Turn "should work" into concrete, fresh evidence                               | `skills/verify/SKILL.md`                |
| `code-reviewer` | Severity-rated review methodology (CRITICAL/HIGH/MEDIUM/LOW)                   | Derived from `agents/code-reviewer` methodology |
| `debug`         | Session/runtime diagnosis via traces, events, memory                           | `skills/debug/SKILL.md`                 |
| `learner`       | Extract reusable skills from the current conversation                          | `skills/learner/SKILL.md`               |
| `omc-reference` | CyberClaw agent catalog, native runtime map, commit protocol                   | `skills/omc-reference/SKILL.md`         |
| `skill`         | Skill management methodology (list/add/remove/edit/search)                     | `skills/skill/SKILL.md`                 |

## New Workflow Skills (Sprint 11/12 Wave)

| Skill                           | One-line purpose                                                              | Source origin (superpowers)            |
|---------------------------------|--------------------------------------------------------------------------------|-----------------------------------------|
| `brainstorming`                 | Structured ideation: clarify goal → diverge options → converge evaluation → decide | `skills/brainstorming/SKILL.md`         |
| `test-driven-development`       | Red-green-refactor cycle: write failing test → implement → refactor             | `skills/test-driven-development/SKILL.md` |
| `subagent-driven-development`   | Parallel lane decomposition: task split → territory allocation → criteria → execute | `skills/subagent-driven-development/SKILL.md` |

## Note on Derived Skills

The oh-my-claudecode source ships `explore` and `code-reviewer` as
**agents**, not as skills. To satisfy the Sprint 4 deliverable count and
to allow other methodology documents (notably `plan`) to reference them by
skill-name, we derived `ecosystem/skills/explore/SKILL.md` and
`ecosystem/skills/code-reviewer/SKILL.md` from the corresponding agent
methodology. The source origin column makes this explicit.

## OMC Skills NOT Ported (already replaced by native runtimes)

The following OMC workflow skills are explicitly **not** ported because
CyberClaw already provides native runtime equivalents delivered in
Sprint 1:

| OMC skill     | CyberClaw native equivalent                                                  |
|---------------|-------------------------------------------------------------------------------|
| `ralph`       | `PersistentLoop` in `crates/cyberclaw-control-plane/src/persistent_execution.rs` |
| `autopilot`   | `AutopilotRuntime` in `crates/cyberclaw-control-plane/src/autopilot_runtime.rs` |
| `team`        | `SubAgentOrchestrator` in `crates/cyberclaw-agent-runtime/src/sub_agent.rs` |
| `ultrawork`   | Parallel `SubAgentOrchestrator::spawn_child` invocations within depth/budget caps |
| `ralplan`     | Consensus mode inside `plan` (handled in this directory)                     |
| `sciomc`      | Not ported in this sprint                                                    |
| `ultraqa`     | Not ported in this sprint                                                    |

## CyberClaw-Specific Adaptations

Every skill file has been adapted for CyberClaw. The key substitutions are:

1. `Task(subagent_type="oh-my-claudecode:X", ...)`
   → `SubAgentOrchestrator::spawn_child(AgentId::new("X"))`
2. `state_read` / `state_write` / `state_clear` → `cyberclaw-store` crate
3. `TaskCreate` / `TaskList` / `SendMessage` → CyberClaw equivalents
   (`cyberclaw-store` + `SubAgentOrchestrator` + future TaskManager Capability)
4. `Skill("oh-my-claudecode:ralph")` → `PersistentLoop`
5. `Skill("oh-my-claudecode:autopilot")` → `AutopilotRuntime`
6. `Skill("oh-my-claudecode:team")` → `SubAgentOrchestrator`

The full translation map with before/after snippets is in
`docs/implementation/sprint4-omc-adaptation-map.md`.

## Architectural Constraints

- No skill in this directory executes code or spawns processes.
- No skill shells out to `claude -p` or any other LLM CLI.
- All runtime references point at CyberClaw crate/file paths, not at
  Claude Code SDK primitives.

## Sibling Artifact

- `ecosystem/agents/README.md` — 8 ported agent methodology documents.
