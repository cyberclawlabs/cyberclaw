---
name: skill
description: Skill management methodology — list, add, remove, search, edit, sync (CyberClaw-adapted)
source: oh-my-claudecode/skills/skill/SKILL.md
adapted-for: CyberClaw (Sprint 4, 2026-04-18)
argument-hint: "<command> [args]"
level: 2
---

<!--
CyberClaw adaptation notes:
- Skills never execute in CyberClaw (CLAUDE.md §3.4 and §9). This file
  describes a **methodology** for the agent to follow when the user asks
  to inspect / add / remove / edit skills. The actual filesystem work is
  done by the agent via Read/Write/Edit/Glob, bound by workspace policy.
- Canonical skill scopes under CyberClaw:
    * Ecosystem bundled:  `ecosystem/skills/<name>/SKILL.md`
    * Project-level:      `ecosystem/skills/<name>/SKILL.md` (version-controlled)
    * Harness-personal:   under OMC-compatible harnesses only — the
                          original `${CLAUDE_CONFIG_DIR:-~/.claude}/skills/omc-learned/`
                          remains a valid personal scope.
  The original `.omc/skills/` project path is still honored under OMC
  harnesses; under CyberClaw runtime, skills live in `ecosystem/skills/`.
- Subprocess-shelling example blocks in the OMC original (`find ... | grep`)
  have been retained as shell instructions for the agent to execute under
  its normal Bash tool — they are NOT invoked from inside this skill file.
  Scripts that would shell out to `claude -p` or other LLM subprocesses
  are removed per architectural constraint (CLAUDE.md §3.4).
-->

# Skill Management CLI (methodology)

Meta-skill describing how the agent should respond when the user issues
CLI-style commands for managing local skills (list, add, remove, edit,
search, info, sync, setup, scan).

## Subcommands

### /skill list

Show all available skills organized by scope.

**Behavior:**
1. Scan bundled ecosystem skills in `ecosystem/skills/` (read-only — these
   are shipped with the repo).
2. Under OMC-compatible harnesses, also scan
   `${CLAUDE_CONFIG_DIR:-~/.claude}/skills/omc-learned/` and `.omc/skills/`.
3. Parse YAML frontmatter for metadata.
4. Display in organized table format:

```
BUILT-IN / ECOSYSTEM SKILLS (ecosystem/skills/):
| Name          | Description                    | Scope     |
|---------------|--------------------------------|-----------|
| plan          | Strategic planning             | ecosystem |
| verify        | Completion evidence checks     | ecosystem |

USER SKILLS (OMC harness only, ~/.claude/skills/omc-learned/):
| Name          | Triggers       | Quality | Usage | Scope |
|---------------|----------------|---------|-------|-------|
| error-handler | fix, error     | 95%     | 42    | user  |

PROJECT SKILLS (OMC harness only, .omc/skills/):
| Name          | Triggers       | Quality | Usage | Scope   |
|---------------|----------------|---------|-------|---------|
| test-runner   | test, run      | 92%     | 15    | project |
```

**Fallback:** If quality/usage stats are not available, show "N/A".

**Ecosystem skill note:** Ecosystem skills are bundled with CyberClaw and
are discoverable/readable, but not removed or edited through `/skill
remove` or `/skill edit`.

---

### /skill add [name]

Interactive wizard for creating a new skill.

**Behavior:**
1. **Ask for skill name** (if not provided). Validate: lowercase,
   hyphens only, no spaces.
2. **Ask for description** — clear one-liner.
3. **Ask for triggers** — comma-separated keywords.
4. **Ask for argument hint** (optional).
5. **Ask for scope**:
   - `project` → `.omc/skills/<name>/SKILL.md` (OMC harness) OR
     `ecosystem/skills/<name>/SKILL.md` (if the user explicitly wants to
     add to the CyberClaw ecosystem and has the appropriate permission).
   - `user` → `${CLAUDE_CONFIG_DIR:-~/.claude}/skills/omc-learned/<name>/SKILL.md`
     (OMC harness only — CyberClaw itself does not have a user-home skill
     scope).
6. **Create skill file** with template (see templates below).
7. **Report success** with file path.
8. **Suggest** editing with `/skill edit <name>`.

---

### /skill remove \<name\>

Remove a skill by name.

**Behavior:**
1. Search for skill in both scopes (project / user under OMC).
2. If found, display skill info and ask for confirmation:
   "Delete '<name>' skill from <scope>? (yes/no)".
3. If confirmed, delete the skill directory and report success.
4. If not found, report the miss.

**Safety:** Never delete without explicit user confirmation. Ecosystem
skills bundled with the CyberClaw repo cannot be removed through this
command — that requires a normal code change.

---

### /skill edit \<name\>

Edit an existing skill interactively.

**Behavior:**
1. Find the skill by name.
2. Read current content.
3. Display current frontmatter values.
4. Ask what to change (description / triggers / argument-hint / content / rename / cancel).
5. For the selected field, show current value and ask for the new one.
6. Update YAML frontmatter or content and write back.
7. Report success.

---

### /skill search \<query\>

Search skills by content, triggers, name, or description.

**Behavior:**
1. Scan all skill scopes.
2. Match query (case-insensitive) against name, description, triggers, and
   full markdown content.
3. Display matches with context. Rank name/trigger matches above content
   matches.

---

### /skill info \<name\>

Show detailed information about a skill: name, description, triggers,
argument-hint, scope, file path, and full content.

---

### /skill sync

Sync skills between scopes (OMC harness only).

**Behavior:**
1. Scan user and project scopes.
2. Compare: user-only, project-only, common.
3. Offer: copy user→project, copy project→user, view differences, cancel.
4. Never overwrite without confirmation.

Ecosystem skills are out of scope for sync; they are managed via code
review + PR on the CyberClaw repo.

---

### /skill setup

Interactive wizard that:
1. Ensures skill directories exist (creating them if not).
2. Scans and reports inventory.
3. Offers quick actions: add new skill, list with details, scan
   conversation for patterns (→ invoke `learner`), import skill, done.

The agent runs these steps via its normal tool surface (Bash for `mkdir`,
Glob/Grep/Read for scanning, Read/Write for file operations). No subprocess
invocation of `claude -p` or any other LLM CLI is performed from within
this skill.

---

### /skill scan

Quick command to scan both skill directories (subset of `/skill setup`).

---

## Skill Templates

### Error Solution Template

```markdown
---
name: [error-name]
description: Solution for [specific error in specific context]
triggers:
  - [error message fragment]
  - [file path]
  - [symptom]
---

# [Error Name]

## The Insight
What is the underlying cause of this error? What principle did you discover?

## Why This Matters
What goes wrong if you don't know this? What symptom led here?

## Recognition Pattern
- Error message: "[exact error]"
- File: [specific file path]
- Context: [when does this occur]

## The Approach
1. [Specific action with file/line reference]
2. [Specific action with file/line reference]
3. [Verification step]
```

### Workflow Skill Template

```markdown
---
name: [workflow-name]
description: Process for [specific task in this codebase]
triggers:
  - [task description]
  - [file pattern]
  - [goal keyword]
---

# [Workflow Name]

## The Insight
What makes this workflow different from the obvious approach?

## Why This Matters
What fails if you don't follow this process?

## Recognition Pattern
- Task type: [specific task]
- Files involved: [specific patterns]
- Indicators: [how to recognize]

## The Approach
1. [Step with specific commands/files]
2. [Step with specific commands/files]
3. [Verification]

## Gotchas
- [Common mistake and how to avoid it]
- [Edge case and how to handle it]
```

### Code Pattern Template

```markdown
---
name: [pattern-name]
description: Pattern for [specific use case in this codebase]
triggers:
  - [code pattern]
  - [file type]
  - [problem domain]
---

# [Pattern Name]

## The Insight
What's the key principle behind this pattern?

## Why This Matters
What problems does this pattern solve in THIS codebase?

## Recognition Pattern
- File types: [specific files]
- Problem: [specific problem]
- Context: [codebase-specific context]

## The Approach
1. [Principle-based step]
2. [Principle-based step]

## Anti-Pattern
What NOT to do and why:
[Illustrative snippet of the anti-pattern]
```

### Integration Skill Template

```markdown
---
name: [integration-name]
description: How [system A] integrates with [system B] in this codebase
triggers:
  - [system name]
  - [integration point]
  - [config file]
---

# [Integration Name]

## The Insight
What's non-obvious about how these systems connect?

## Why This Matters
What breaks if you don't understand this integration?

## Recognition Pattern
- Files: [specific integration files]
- Config: [specific config locations]
- Symptoms: [what indicates integration issues]

## The Approach
1. [Configuration step with file paths]
2. [Setup step with specific details]
3. [Verification step]

## Gotchas
- [Integration-specific pitfall #1]
- [Integration-specific pitfall #2]
```

---

## Error Handling

**All commands must handle:**
- File/directory doesn't exist
- Permission errors
- Invalid YAML frontmatter
- Duplicate skill names
- Invalid skill names (spaces, special chars)

**Error format:**
```
Error: <clear message>
Suggestion: <helpful next step>
```

---

## Usage Examples

```
/skill list
/skill add my-custom-skill
/skill remove old-skill
/skill edit error-handler
/skill search typescript error
/skill info my-custom-skill
/skill sync
/skill setup
/skill scan
```

## Usage Modes

### Direct Command Mode

When invoked with an argument, skip the interactive wizard.

### Interactive Mode

When invoked without arguments, run the full guided wizard.

---

## Benefits of Local Skills

- **Automatic Application**: the agent detects triggers and applies skills
  automatically — no need to remember or search.
- **Version Control**: project-level skills are committed with the code so
  the whole team benefits.
- **Evolving Knowledge**: skills improve over time as better approaches and
  refined triggers emerge.
- **Reduced Token Usage**: instead of re-solving the same problems, apply
  known patterns efficiently.
- **Codebase Memory**: preserves institutional knowledge that would
  otherwise be lost in conversation history.

---

## Skill Quality Guidelines

Good skills are:

1. **Non-Googleable** — can't easily be found via search.
2. **Context-Specific** — reference actual files/errors from THIS codebase.
3. **Actionable with Precision** — tell exactly WHAT to do and WHERE.
4. **Hard-Won** — required significant debugging effort.

---

## Related Skills

- `learner` — extract a skill from the current conversation.
- `plan` — strategic planning (invokes explore/analyst/architect/critic).
- `omc-reference` — CyberClaw agent catalog and runtime map.

---

## Implementation Notes

1. **YAML Parsing**: use frontmatter extraction for metadata.
2. **File Operations**: use Read/Write/Edit — never shell out to an LLM CLI.
3. **User Confirmation**: always confirm destructive operations.
4. **Clear Feedback**: use concise prefixes for clarity.
5. **Scope Resolution**: always check all applicable scopes (ecosystem /
   project / user).
6. **Validation**: enforce naming conventions (lowercase, hyphens only).

---

## Future Enhancements

- `/skill export <name>` — export skill as shareable file.
- `/skill import <file>` — import skill from file.
- `/skill stats` — show usage statistics across all skills.
- `/skill validate` — check all skills for format errors.
- `/skill template <type>` — create from predefined templates.
