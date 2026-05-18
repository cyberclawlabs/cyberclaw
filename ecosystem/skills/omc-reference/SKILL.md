---
name: omc-reference
description: CyberClaw agent catalog, native runtime map, commit protocol, and skills registry. Auto-loads when delegating to agents, orchestrating sub-agents, making commits, or invoking skills.
source: oh-my-claudecode/skills/omc-reference/SKILL.md
adapted-for: CyberClaw (Sprint 4, 2026-04-18)
user-invocable: false
---

<!--
CyberClaw adaptation notes:
- The original doc enumerated OMC agents and OMC-specific MCP tools (`state_*`,
  `TeamCreate`, `SendMessage`, etc.). Those have been rewritten to describe
  CyberClaw's native runtime equivalents:
    * `SubAgentOrchestrator` (`crates/cyberclaw-agent-runtime/src/sub_agent.rs`)
      replaces OMC Task() / TeamCreate / SendMessage for sibling dispatch.
    * `cyberclaw-store` replaces the `state_*` / notepad / project-memory
      MCP family for persistence.
    * `PersistentLoop` and `AutopilotRuntime` replace OMC's `ralph` /
      `autopilot` skills as execution engines.
- Skills never execute in CyberClaw (CLAUDE.md §3.4 and §9). This reference
  is methodology only.
-->

# CyberClaw Reference (ported from OMC)

Use this built-in reference when you need detailed catalog information that does not need to live in every `CLAUDE.md` session.

## Agent Catalog (ported)

The following 8 agent methodology documents have been ported into
`ecosystem/agents/` for CyberClaw (Sprint 4):

- `analyst` — requirements clarity and hidden constraints
- `planner` — sequencing and execution plans
- `architect` — system design, boundaries, and long-horizon tradeoffs
- `executor` — implementation and refactoring
- `critic` — plan/design challenge and review
- `verifier` — completion evidence and validation
- `code-reviewer` — comprehensive code review
- `git-master` — commit strategy and history hygiene

Additional agents that exist in the OMC source but are NOT ported in this
sprint (and therefore are NOT present under `ecosystem/agents/`): `explore`,
`debugger`, `tracer`, `security-reviewer`, `test-engineer`, `designer`,
`writer`, `qa-tester`, `scientist`, `document-specialist`, `code-simplifier`.
They can be invoked in principle via `SubAgentOrchestrator::spawn_child(AgentId::new("<name>"))`
once their methodology docs are ported.

## Model Routing (guidance)

- Lightweight lookups — quick exploration and narrow docs work
- Standard implementation, debugging, and review
- Architecture, deep analysis, consensus planning, and high-risk review

Concrete model selection is a runtime concern driven by the Agent record in
`cyberclaw-store`, not by this methodology document.

## Runtime Map (CyberClaw equivalents for OMC concepts)

| Concept (OMC) | CyberClaw equivalent | Location |
|---------------|-----------------------|----------|
| Task() sibling dispatch | `SubAgentOrchestrator::spawn_child(AgentId::new("<agent>"))` | `crates/cyberclaw-agent-runtime/src/sub_agent.rs` |
| `/team` multi-agent fan-out | Repeated `spawn_child` within depth/children caps | `crates/cyberclaw-agent-runtime/src/sub_agent.rs` |
| `state_read` / `state_write` / `state_clear` | `cyberclaw-store` read/write/delete on session-scoped Artifacts | `crates/cyberclaw-store` |
| Notepad / project-memory | `cyberclaw-store` (Semantic / Procedural Memory scopes) | `crates/cyberclaw-store` |
| `ralph` skill (persistent loop) | `PersistentLoop` | `crates/cyberclaw-control-plane/src/persistent_execution.rs` |
| `autopilot` skill | `AutopilotRuntime` (with Auto Mode Gate + Circuit Breaker) | `crates/cyberclaw-control-plane/src/autopilot_runtime.rs` |
| `/ccg` multi-model synthesis | Not ported (no CyberClaw runtime equivalent yet) | — |
| `ultrawork` parallel engine | Repeated `spawn_child` with shared task artifact | — |
| `TeamCreate` / `TaskCreate` / `SendMessage` MCP family | Native fan-out via `SubAgentOrchestrator` + `cyberclaw-store` Artifacts + future TaskManager Capability | — |

## Code Intelligence (harness-provided)

- LSP: `lsp_hover`, `lsp_goto_definition`, `lsp_find_references`, `lsp_diagnostics`, and related helpers (when harness provides them).
- AST: `ast_grep_search`, `ast_grep_replace`.
- Utility: `python_repl`.

These are harness surfaces, not CyberClaw runtime objects.

## Skills Registry (ported into `ecosystem/skills/`)

The following 8 skill methodology documents are available under
`ecosystem/skills/` as of Sprint 4:

- `plan` — planning workflow (replaces OMC's `/plan`, `/planner`, `/ralplan`, `/review`)
- `explore` — scoped read-only codebase mapping (derived from the `explore` agent)
- `verify` — fresh-evidence completion checks
- `code-reviewer` — severity-rated code review (derived from the `code-reviewer` agent)
- `debug` — session/runtime diagnosis
- `learner` — learned-skill extraction
- `omc-reference` — this document
- `skill` — skill management methodology

OMC workflow skills that are **not ported** because they are already
replaced by CyberClaw native runtimes:

- `ralph` → `PersistentLoop`
- `autopilot` → `AutopilotRuntime`
- `team` → `SubAgentOrchestrator`
- `ultrawork`, `ralplan`, `sciomc`, `ultraqa` → subsumed by the planning /
  fan-out / verification surfaces already documented here

## Commit Protocol (CyberClaw-aligned)

CyberClaw repositories follow Conventional Commits (see root `CLAUDE.md §7.1`
and `CONTRIBUTING.md`). The OMC trailer convention is compatible: use git
trailers to preserve decision context in every commit message.

### Format
- Intent line first: `<type>(<scope>): <subject>`
- Optional body with context and rationale
- Structured trailers when applicable

### Common trailers
- `Constraint:` active constraint shaping the decision
- `Rejected:` alternative considered | reason for rejection
- `Directive:` forward-looking warning or instruction
- `Confidence:` `high` | `medium` | `low`
- `Scope-risk:` `narrow` | `moderate` | `broad`
- `Not-tested:` known verification gap

### Example
```text
feat(control-plane): add persistent execution loop

Introduce story-driven PersistentLoop to replace OMC ralph skill for
long-horizon work. Execution evidence is captured as Artifacts and rolled
forward in the Semantic Memory scope of cyberclaw-store.

Constraint: Skills must not execute (CLAUDE.md §3.4, §9)
Rejected: Directly embed a ralph-shaped loop in skill-runtime | would break the Skill/Connector separation
Confidence: high
Scope-risk: narrow
Not-tested: End-to-end persistence across a node restart
```
