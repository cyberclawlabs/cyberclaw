---
name: plan
description: Strategic planning with optional interview workflow (CyberClaw-adapted methodology)
source: oh-my-claudecode/skills/plan/SKILL.md
adapted-for: CyberClaw (Sprint 4, 2026-04-18)
argument-hint: "[--direct|--consensus|--review] [--interactive] [--deliberate] <task description>"
pipeline: [deep-interview, plan, autopilot]
next-skill: autopilot
handoff: cyberclaw-store Artifact (canonical) or .omc/plans/ralplan-*.md (OMC-compatible harness)
level: 4
---

<!--
CyberClaw adaptation notes:
- This file is a methodology document. CyberClaw Skills never execute code or
  spawn processes (CLAUDE.md §3.4 and §9). Execution must flow through
  Connector → Capability in the control plane.
- Sub-agent invocations that used Claude Code's `Task(subagent_type="...")`
  primitive have been rewritten to CyberClaw's native
  `SubAgentOrchestrator::spawn_child(AgentId::new("<agent-name>"))`, defined in
  `crates/cyberclaw-agent-runtime/src/sub_agent.rs`. Depth limit 3, max 5
  children, budget fraction 0.5 (see project memory).
- Execution handoffs that used OMC-specific skills have been remapped:
    * `Skill("oh-my-claudecode:ralph")`   → CyberClaw `PersistentLoop` in
       `crates/cyberclaw-control-plane/src/persistent_execution.rs`
    * `Skill("oh-my-claudecode:autopilot")` → CyberClaw `AutopilotRuntime` in
       `crates/cyberclaw-control-plane/src/autopilot_runtime.rs`
    * `Skill("oh-my-claudecode:team")`    → fan-out through
       `SubAgentOrchestrator::spawn_child(...)` within depth/budget limits
- Persistent state (the original `state_read/state_write/state_clear` tool
  family) has no CyberClaw equivalent as an LLM-facing tool. Long-lived plan
  and ralplan state lives in `cyberclaw-store` (Artifact + Semantic Memory
  scopes). Under OMC-compatible harnesses, the original `.omc/state/*.json`
  paths remain valid.
- TaskCreate/TaskList/TaskUpdate/SendMessage tools referenced in OMC are
  replaced by CyberClaw equivalents: `cyberclaw-store` (for plan/task
  artifacts) + `SubAgentOrchestrator` (for sibling dispatch) + the future
  TaskManager Capability (not yet implemented — see CLAUDE.md §11).
-->

<Purpose>
Plan creates comprehensive, actionable work plans through intelligent interaction. It auto-detects whether to interview the user (broad requests) or plan directly (detailed requests), and supports consensus mode (iterative Planner/Architect/Critic loop with RALPLAN-DR structured deliberation) and review mode (Critic evaluation of existing plans).
</Purpose>

<Use_When>
- User wants to plan before implementing -- "plan this", "plan the", "let's plan"
- User wants structured requirements gathering for a vague idea
- User wants an existing plan reviewed -- "review this plan", `--review`
- User wants multi-perspective consensus on a plan -- `--consensus`, "ralplan"
- Task is broad or vague and needs scoping before any code is written
</Use_When>

<Do_Not_Use_When>
- User wants autonomous end-to-end execution -- target `AutopilotRuntime` instead (`crates/cyberclaw-control-plane/src/autopilot_runtime.rs`)
- User wants to start coding immediately with a clear task -- submit an `Execution` directly; for story-driven loops use `PersistentLoop` (`crates/cyberclaw-control-plane/src/persistent_execution.rs`)
- User asks a simple question that can be answered directly -- just answer it
- Task is a single focused fix with obvious scope -- skip planning, just do it
</Do_Not_Use_When>

<Why_This_Exists>
Jumping into code without understanding requirements leads to rework, scope creep, and missed edge cases. Plan provides structured requirements gathering, expert analysis, and quality-gated plans so that execution starts from a solid foundation. The consensus mode adds multi-perspective validation for high-stakes projects.
</Why_This_Exists>

<Execution_Policy>
- Auto-detect interview vs direct mode based on request specificity.
- Ask one question at a time during interviews — never batch multiple questions.
- Gather codebase facts via an `explore` sub-agent (`SubAgentOrchestrator::spawn_child(AgentId::new("explore"))`) before asking the user about them.
- Plans must meet quality standards: 80%+ claims cite file/line, 90%+ criteria are testable.
- Consensus mode runs fully automated by default; add `--interactive` to enable user prompts at draft review and final approval steps.
- Consensus mode uses RALPLAN-DR short mode by default; switch to deliberate mode with `--deliberate` or when the request explicitly signals high risk (auth/security, data migration, destructive/irreversible changes, production incident, compliance/PII, public API breakage).
</Execution_Policy>

<Steps>

### Mode Selection

| Mode | Trigger | Behavior |
|------|---------|----------|
| Interview | Default for broad requests | Interactive requirements gathering |
| Direct | `--direct`, or detailed request | Skip interview, generate plan directly |
| Consensus | `--consensus`, "ralplan" | Planner -> Architect -> Critic loop until agreement with RALPLAN-DR structured deliberation (short by default, `--deliberate` for high-risk); add `--interactive` for user prompts at draft and approval steps |
| Review | `--review`, "review this plan" | Critic evaluation of existing plan |

### Interview Mode (broad/vague requests)

1. **Classify the request**: Broad (vague verbs, no specific files, touches 3+ areas) triggers interview mode.
2. **Ask one focused question** for preferences, scope, and constraints.
3. **Gather codebase facts first**: Before asking "what patterns does your code use?", spawn an `explore` sub-agent via `SubAgentOrchestrator::spawn_child(AgentId::new("explore"))` to find out, then ask informed follow-up questions.
4. **Build on answers**: Each question builds on the previous answer.
5. **Consult Analyst** (`SubAgentOrchestrator::spawn_child(AgentId::new("analyst"))`) for hidden requirements, edge cases, and risks.
6. **Create plan** when the user signals readiness: "create the plan", "I'm ready", "make it a work plan".

### Direct Mode (detailed requests)

1. **Quick Analysis**: Optional brief analyst consultation via `SubAgentOrchestrator::spawn_child(AgentId::new("analyst"))`.
2. **Create plan**: Generate comprehensive work plan immediately.
3. **Review** (optional): Critic review via `SubAgentOrchestrator::spawn_child(AgentId::new("critic"))` if requested.

### Consensus Mode (`--consensus` / "ralplan")

**RALPLAN-DR modes**: **Short** (default, bounded structure) and **Deliberate** (for `--deliberate` or explicit high-risk requests). Both modes keep the same Planner -> Architect -> Critic sequence and the same user-approval gates when `--interactive`.

**State lifecycle**: CyberClaw persists via the `cyberclaw-store` crate (Artifact + Semantic Memory) rather than `.omc/state/*.json` JSON blobs. Under an OMC-compatible harness, the original `state_read/state_write/state_clear` lifecycle applies; under CyberClaw runtime, plan-lifecycle state is a named Artifact in `cyberclaw-store` scoped by session.

- **On entry**: Mark ralplan as active in `cyberclaw-store` for the current session.
- **On handoff to execution**: Mark ralplan inactive BEFORE invoking the execution surface (`PersistentLoop` or `AutopilotRuntime`). Do not issue a terminal-clear signal here; terminal-clear is only for abort/rejection.
- **On true terminal exit** (rejection, non-interactive plan output, error/abort): Issue the terminal-clear signal. No execution mode follows, so clearing the session ralplan state is safe.
- Do NOT clear during intermediate Critic approval or max-iteration presentation steps, since the user may still select "Request changes".

1. **Planner** creates initial plan and a compact **RALPLAN-DR summary** before any Architect review. The summary **MUST** include:
   - **Principles** (3-5)
   - **Decision Drivers** (top 3)
   - **Viable Options** (>=2) with bounded pros/cons
   - If only one viable option remains, an explicit **invalidation rationale** for the alternatives that were rejected
   - In **deliberate mode**: a **pre-mortem** (3 failure scenarios) and an **expanded test plan** covering **unit / integration / e2e / observability**
2. **User feedback** *(--interactive only)*: Present the draft plan plus the RALPLAN-DR summary and ask for direction:
   - **Proceed to review** — send to Architect and Critic for evaluation
   - **Request changes** — return to step 1 with user feedback incorporated
   - **Skip review** — go directly to final approval (step 7)
   If NOT `--interactive`, automatically proceed to review (step 3).
3. **Architect** reviews for architectural soundness using `SubAgentOrchestrator::spawn_child(AgentId::new("architect"))`. Architect review **MUST** include: strongest steelman counterargument (antithesis) against the favored option, at least one meaningful tradeoff tension, and (when possible) a synthesis path. In deliberate mode, Architect should explicitly flag principle violations. **Wait for this step to complete before proceeding to step 4.** Do NOT run steps 3 and 4 in parallel.
4. **Critic** evaluates against quality criteria using `SubAgentOrchestrator::spawn_child(AgentId::new("critic"))`. Critic **MUST** verify principle-option consistency, fair alternative exploration, risk mitigation clarity, testable acceptance criteria, and concrete verification steps. Critic **MUST** explicitly reject shallow alternatives, driver contradictions, vague risks, or weak verification. In deliberate mode, Critic **MUST** reject missing/weak pre-mortem or missing/weak expanded test plan. Run only after step 3 is complete.
5. **Re-review loop** (max 5 iterations): If Critic rejects:
   a. Collect all rejection feedback from Architect + Critic
   b. Pass feedback to Planner to produce a revised plan
   c. Return to Step 3 — Architect reviews the revised plan
   d. Return to Step 4 — Critic evaluates the revised plan
   e. Repeat until Critic approves OR max 5 iterations reached
   f. If max iterations reached without approval, present the best version to user with a note that expert consensus was not reached
6. **Apply improvements**: When reviewers approve with improvement suggestions, merge all accepted improvements into the plan artifact before proceeding. Final consensus output **MUST** include an **ADR** section with: **Decision**, **Drivers**, **Alternatives considered**, **Why chosen**, **Consequences**, **Follow-ups**.
7. On Critic approval (with improvements applied): *(--interactive only)* present the plan with these options:
   - **Approve and implement via native fan-out** (Recommended for parallelizable work) — proceed to implementation by spawning executor sub-agents through `SubAgentOrchestrator::spawn_child(AgentId::new("executor"))`. CyberClaw's `SubAgentOrchestrator` is the canonical fan-out surface (replaces OMC's `/team`).
   - **Approve and execute via PersistentLoop** — proceed to implementation via CyberClaw's `PersistentLoop` in `crates/cyberclaw-control-plane/src/persistent_execution.rs` (story-driven sequential execution with verification, ralph-equivalent).
   - **Approve and execute via AutopilotRuntime** — proceed via CyberClaw's `AutopilotRuntime` in `crates/cyberclaw-control-plane/src/autopilot_runtime.rs` (autonomous end-to-end execution with Auto Mode Gate).
   - **Request changes** — return to step 1 with user feedback
   - **Reject** — discard the plan entirely
   If NOT `--interactive`, output the final approved plan, mark ralplan state inactive, and stop. Do NOT auto-execute.
8. *(--interactive only)* User chooses. If user selects **Reject**, issue terminal-clear for ralplan state and stop.
9. On user approval (--interactive only): Mark ralplan state inactive **before** invoking the execution surface.
   - **Approve via native fan-out**: spawn executor sub-agents through `SubAgentOrchestrator::spawn_child` with the approved plan artifact as context. Do NOT implement directly in the planning agent.
   - **Approve via PersistentLoop**: submit the approved plan artifact to `PersistentLoop` for story-driven execution.
   - **Approve via AutopilotRuntime**: submit the approved plan artifact to `AutopilotRuntime`.

### Review Mode (`--review`)

1. Read plan artifact from `cyberclaw-store` (or `.omc/plans/` under OMC-compatible harness).
2. Evaluate via Critic using `SubAgentOrchestrator::spawn_child(AgentId::new("critic"))`.
3. Return verdict: APPROVED, REVISE (with specific feedback), or REJECT (replanning required).

### Plan Output Format

Every plan includes:
- Requirements Summary
- Acceptance Criteria (testable)
- Implementation Steps (with file references)
- Risks and Mitigations
- Verification Steps
- For consensus/ralplan: **RALPLAN-DR summary** (Principles, Decision Drivers, Options)
- For consensus/ralplan final output: **ADR** (Decision, Drivers, Alternatives considered, Why chosen, Consequences, Follow-ups)
- For deliberate consensus mode: **Pre-mortem (3 scenarios)** and **Expanded Test Plan** (unit/integration/e2e/observability)

Plans are persisted to `cyberclaw-store` (canonical) or `.omc/plans/` (OMC-compatible). Drafts go to the corresponding drafts scope.
</Steps>

<Tool_Usage>
- Use plain prompts for preference questions (CyberClaw does not currently ship a clickable-options tool). One question at a time.
- Use `SubAgentOrchestrator::spawn_child(AgentId::new("explore"))` to gather codebase facts before asking the user.
- Use `SubAgentOrchestrator::spawn_child(AgentId::new("planner"))` for planning validation on large-scope plans.
- Use `SubAgentOrchestrator::spawn_child(AgentId::new("analyst"))` for requirements analysis.
- Use `SubAgentOrchestrator::spawn_child(AgentId::new("critic"))` for plan review in consensus and review modes.
- **CRITICAL — Consensus mode sibling agent calls MUST be sequential, never parallel.** Always await the Architect result before issuing the Critic call.
- In consensus mode, default to RALPLAN-DR short mode; enable deliberate mode on `--deliberate` or explicit high-risk signals.
- In consensus mode with `--interactive`: prompt the user for feedback (step 2) and final approval (step 7). Without `--interactive`, skip both prompts and output the final plan.
- In consensus mode with `--interactive`, on user approval **MUST** route execution to one of: CyberClaw native fan-out (`SubAgentOrchestrator::spawn_child`), `PersistentLoop`, or `AutopilotRuntime`. Never implement directly in the planning agent.
- **CRITICAL — Consensus mode state lifecycle**: Always deactivate ralplan state before stopping or handing off to execution. Use "mark inactive" for handoff paths (approval → execution) and "terminal-clear" for true terminal exits (rejection, error). Persist via `cyberclaw-store` under CyberClaw, or the original `.omc/state` lifecycle under OMC-compatible harnesses.
</Tool_Usage>

<Examples>
<Good>
Adaptive interview (gathering facts before asking):
```
Planner: [spawns explore sub-agent: "find authentication implementation"]
Planner: [receives: "Auth is in crates/cyberclaw-control-plane/src/auth.rs using JWT + constant-time token comparison"]
Planner: "I see you're using JWT authentication with constant-time token comparison in crates/cyberclaw-control-plane/src/auth.rs.
         For this new feature, should we extend the existing auth or add a separate auth flow?"
```
Why good: Answers its own codebase question first, then asks an informed preference question.
</Good>

<Good>
Single question at a time:
```
Q1: "What's the main goal?"
A1: "Improve performance"
Q2: "For performance, what matters more -- latency or throughput?"
A2: "Latency"
Q3: "For latency, are we optimizing for p50 or p99?"
```
Why good: Each question builds on the previous answer. Focused and progressive.
</Good>

<Bad>
Asking about things you could look up:
```
Planner: "Where is authentication implemented in your codebase?"
User: "Uh, somewhere in crates/ I think?"
```
Why bad: The planner should spawn an explore sub-agent to find this, not ask the user.
</Bad>

<Bad>
Batching multiple questions:
```
"What's the scope? And the timeline? And who's the audience?"
```
Why bad: Three questions at once causes shallow answers. Ask one at a time.
</Bad>

<Bad>
Presenting all design options at once:
```
"Here are 4 approaches: Option A... Option B... Option C... Option D... Which do you prefer?"
```
Why bad: Decision fatigue. Present one option with trade-offs, get reaction, then present the next.
</Bad>
</Examples>

<Escalation_And_Stop_Conditions>
- Stop interviewing when requirements are clear enough to plan — do not over-interview.
- In consensus mode, stop after 5 Planner/Architect/Critic iterations and present the best version. Do NOT clear ralplan state here — the user may still select "Request changes". State is cleared only on the user's final choice (approval/rejection) or when outputting the plan in non-interactive mode.
- Consensus mode without `--interactive` outputs the final plan and stops; with `--interactive`, requires explicit user approval before any implementation begins. **Always** issue terminal-clear for ralplan state before stopping.
- If the user says "just do it" or "skip planning", mark ralplan inactive, then hand off to the appropriate execution surface: `PersistentLoop`, `AutopilotRuntime`, or native fan-out. Do NOT implement directly in the planning agent.
- Escalate to the user when there are irreconcilable trade-offs that require a business decision.
</Escalation_And_Stop_Conditions>

<Final_Checklist>
- [ ] Plan has testable acceptance criteria (90%+ concrete)
- [ ] Plan references specific files/lines where applicable (80%+ claims)
- [ ] All risks have mitigations identified
- [ ] No vague terms without metrics ("fast" -> "p99 < 200ms")
- [ ] Plan persisted to `cyberclaw-store` (canonical) or `.omc/plans/` (OMC-compatible harness)
- [ ] In consensus mode: RALPLAN-DR summary includes 3-5 principles, top 3 drivers, and >=2 viable options (or explicit invalidation rationale)
- [ ] In consensus mode final output: ADR section included (Decision / Drivers / Alternatives considered / Why chosen / Consequences / Follow-ups)
- [ ] In deliberate consensus mode: pre-mortem (3 scenarios) + expanded test plan (unit/integration/e2e/observability) included
- [ ] In consensus mode with `--interactive`: user explicitly approved before any execution; without `--interactive`: plan output only, no auto-execution
- [ ] In consensus mode: ralplan state deactivated on every exit path — "mark inactive" for handoff to execution, "terminal-clear" for terminal exits (rejection, error, non-interactive stop)
</Final_Checklist>

<Advanced>
## Design Option Presentation

When presenting design choices during interviews, chunk them:

1. **Overview** (2-3 sentences)
2. **Option A** with trade-offs
3. [Wait for user reaction]
4. **Option B** with trade-offs
5. [Wait for user reaction]
6. **Recommendation** (only after options discussed)

Format for each option:
```
### Option A: [Name]
**Approach:** [1 sentence]
**Pros:** [bullets]
**Cons:** [bullets]

What's your reaction to this approach?
```

## Question Classification

Before asking any interview question, classify it:

| Type | Examples | Action |
|------|----------|--------|
| Codebase Fact | "What patterns exist?", "Where is X?" | Explore first, do not ask user |
| User Preference | "Priority?", "Timeline?" | Ask user |
| Scope Decision | "Include feature Y?" | Ask user |
| Requirement | "Performance constraints?" | Ask user |

## Review Quality Criteria

| Criterion | Standard |
|-----------|----------|
| Clarity | 80%+ claims cite file/line |
| Testability | 90%+ criteria are concrete |
| Verification | All file refs exist |
| Specificity | No vague terms |

## Deprecation Notice

Under CyberClaw, the separate `/planner`, `/ralplan`, and `/review` skill surfaces are unified in this single methodology document. All workflows (interview, direct, consensus, review) are available through `plan`.
</Advanced>
