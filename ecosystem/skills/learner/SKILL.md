---
name: learner
description: Extract a learned skill from the current conversation (CyberClaw-adapted)
source: oh-my-claudecode/skills/learner/SKILL.md
adapted-for: CyberClaw (Sprint 4, 2026-04-18)
level: 7
---

<!--
CyberClaw adaptation notes:
- Skills never execute code in CyberClaw (CLAUDE.md §3.4 and §9).
- Persistence of learned skills is routed through `cyberclaw-store` crate
  (Procedural Memory scope), not through `.omc/skills/` flat files. Under an
  OMC-compatible harness, the original `.omc/skills/` paths are still valid.
- File-format frontmatter is preserved so that harnesses that still load
  flat-file skills can discover them unchanged.
-->

# Learner Skill

This is a Level 7 (self-improving) skill. It has two distinct sections:
- **Expertise**: Domain knowledge about what makes a good skill. Updated automatically as patterns are discovered.
- **Workflow**: Stable extraction procedure. Rarely changes.

Only the Expertise section should be updated during improvement cycles.

---

## Expertise

> This section contains domain knowledge that improves over time.
> It can be updated by the learner itself when new patterns are discovered.

### Core Principle

Reusable skills are not code snippets to copy-paste, but **principles and decision-making heuristics** that teach the agent HOW TO THINK about a class of problems.

**The difference:**
- BAD (mimicking): "When you see ConnectionResetError, add this try/except block"
- GOOD (reusable skill): "In async network code, any I/O operation can fail independently due to client/server lifecycle mismatches. The principle: wrap each I/O operation separately, because failure between operations is the common case, not the exception."

### Quality Gate

Before extracting a skill, ALL three must be true:
- "Could someone Google this in 5 minutes?" → NO
- "Is this specific to THIS codebase?" → YES
- "Did this take real debugging effort to discover?" → YES

### Recognition Signals

Extract ONLY after:
- Solving a tricky bug that required deep investigation
- Discovering a non-obvious workaround specific to this codebase
- Finding a hidden gotcha that wastes time when forgotten
- Uncovering undocumented behavior that affects this project

### What Makes a USEFUL Skill

1. **Non-Googleable**: Something you couldn't easily find via search
   - BAD: "How to read files in Rust"
   - GOOD: "CyberClaw Autopilot placeholder fallback requires `#[cfg(not(test))]` to return an error — tests short-circuit to success"

2. **Context-Specific**: References actual files, error messages, or patterns from THIS codebase
   - BAD: "Use `?` for error propagation"
   - GOOD: "`RiskLevel` has `Copy` derive in `crates/cyberclaw-core/src/capability.rs` — do NOT call `.clone()` on it; Clippy will not always catch redundant clones on `Copy` types in generic contexts"

3. **Actionable with Precision**: Tells you exactly WHAT to do and WHERE
   - BAD: "Handle edge cases"
   - GOOD: "When submit() receives an execution_mode = None, the fallback uses `unwrap_or_default()` which defaults to `Normal` — check `crates/cyberclaw-core/src/execution.rs`"

4. **Hard-Won**: Took significant debugging effort to discover
   - BAD: Generic programming patterns
   - GOOD: "Mutex poison recovery in autopilot_runtime + execution_service uses `.lock().unwrap_or_else(|e| e.into_inner())` — straight `.unwrap()` will panic the supervisor if any worker thread poisoned the mutex"

### Anti-Patterns (DO NOT EXTRACT)

- Generic programming patterns (use documentation instead)
- Refactoring techniques (these are universal)
- Library usage examples (use library docs)
- Type definitions or boilerplate
- Anything a junior dev could Google in 5 minutes

---

## Workflow

> This section contains the stable extraction procedure.
> It should NOT be updated during improvement cycles.

### Step 1: Gather Required Information

- **Problem Statement**: The SPECIFIC error, symptom, or confusion that occurred
  - Include actual error messages, file paths, line numbers
  - Example: "Panic in `crates/cyberclaw-agent-runtime/src/sub_agent.rs:245` when depth counter overflowed after 3rd nested spawn"

- **Solution**: The EXACT fix, not general advice
  - Include code snippets, file paths, configuration changes

- **Triggers**: Keywords that would appear when hitting this problem again
  - Use error message fragments, file names, symptom descriptions
  - Example: `["sub_agent depth", "spawn_child panic", "depth limit exceeded"]`

- **Scope**: Almost always Project-level unless it's a truly universal insight

### Step 2: Quality Validation

The system REJECTS skills that are:
- Too generic (no file paths, line numbers, or specific error messages)
- Easily Googleable (standard patterns, library usage)
- Vague solutions (no code snippets or precise instructions)
- Poor triggers (generic words that match everything)

### Step 3: Classify as Expertise or Workflow

Before saving, determine if the learning is:
- **Expertise** (domain knowledge, pattern, gotcha) → Save as `{topic}-expertise.md`
- **Workflow** (operational procedure, step sequence) → Save as `{topic}-workflow.md`

This classification ensures expertise can be updated independently without destabilizing workflows.

### Step 4: Save Location

- **CyberClaw canonical**: through `cyberclaw-store` (Procedural Memory scope), keyed by `skill/<skill-name>`.
- **OMC-compatible harness fallback**:
  - User-level: `${CLAUDE_CONFIG_DIR:-~/.claude}/skills/omc-learned/<skill-name>.md` — Rare. Only for truly portable insights.
  - Project-level: `.omc/skills/<skill-name>.md` — Default. Version-controlled with repo.

### Required File Format

Every learned skill file MUST start with YAML frontmatter so learned-skill flat-file discovery can load it.
Do **not** write plain markdown without frontmatter.

Minimum required frontmatter:

```yaml
---
name: <skill-name>
description: <one-line description>
triggers:
  - <trigger-1>
  - <trigger-2>
---
```

### Skill Body Template

```markdown
---
name: <skill-name>
description: <one-line description>
triggers:
  - <trigger-1>
  - <trigger-2>
---

# [Skill Name]

## The Insight
What is the underlying PRINCIPLE you discovered? Not the code, but the mental model.

## Why This Matters
What goes wrong if you don't know this? What symptom led you here?

## Recognition Pattern
How do you know when this skill applies? What are the signs?

## The Approach
The decision-making heuristic, not just code. How should the agent THINK about this?

## Example (Optional)
If code helps, show it - but as illustration of the principle, not copy-paste material.
```

**Key**: A skill is REUSABLE if the agent can apply it to NEW situations, not just identical ones.

## Related Tools

- `cyberclaw-store` (Procedural Memory scope) — canonical persistence for learned skills in CyberClaw.
- `cyberclaw-observability` — traces and events that often seed good learnings.
