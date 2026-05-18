---
name: code-reviewer
description: Severity-rated code review methodology — spec compliance, security, logic, SOLID (CyberClaw-adapted)
source: derived from oh-my-claudecode/agents/code-reviewer.md (no source skill existed; built from the code-reviewer agent's methodology)
adapted-for: CyberClaw (Sprint 4, 2026-04-18)
level: 3
---

<!--
CyberClaw adaptation notes:
- NOTE ON SOURCE: oh-my-claudecode ships `code-reviewer` as an **agent**, not
  as a skill (`tmp/claw-research/oh-my-claudecode/skills/` has no
  `code-reviewer/` directory). This SKILL.md is a derivation that captures
  the review methodology so other skills/workflows (e.g. `plan`,
  `omc-reference`) can link to it as a skill.
- Skills never execute in CyberClaw (CLAUDE.md §3.4 and §9). The actual
  review is performed by the agent invoked via
  `SubAgentOrchestrator::spawn_child(AgentId::new("code-reviewer"))`.
- Cross-validation (spawning another reviewer) goes through CyberClaw's
  native `SubAgentOrchestrator`, not Claude Code's `Task()` primitive.
-->

# Code Reviewer

Use this skill methodology when code changes need quality and security
review before merging or releasing.

## Goal

Ensure code quality and security through systematic, severity-rated review
that catches spec compliance gaps, security issues, logic defects, and
SOLID violations — before they reach production.

## When to Use

- PRs or patches that touch 2+ files
- Any change on a security-sensitive boundary (auth, governance, crypto)
- Before release, for the changes in the release window
- When an executor reports completion and wants sign-off

## When NOT to Use

- For style-only changes (use linter/formatter output instead)
- To approve your own authoring output (review is a separate pass; use a
  different lane to sign off)
- For planning quality checks — use `critic` instead

## Two-Stage Review

1. **Stage 1 — Spec Compliance** (MUST PASS FIRST)
   - Does the implementation cover ALL requirements?
   - Does it solve the RIGHT problem?
   - Anything missing? Anything extra?
   - Would the requester recognize this as their request?

2. **Stage 2 — Code Quality** (only after Stage 1 passes)
   - `lsp_diagnostics` on each modified file (no type errors approved)
   - `ast_grep_search` for dangerous patterns (stray debug prints, empty
     error arms, hardcoded secrets)
   - Apply the review checklist: security, quality, performance, best
     practices, SOLID

For trivial changes (single-line, typo, no behavior change) skip Stage 1
and do a brief Stage 2 only.

## Severity Rating

| Severity | Definition | Examples |
|----------|------------|----------|
| CRITICAL | Blocks execution, data loss / breach / financial risk | SQL injection, hardcoded credentials, off-by-one that panics on well-formed input |
| HIGH | Should fix before merge | Missing error handling on a user-facing path, unchecked `unwrap()` on untrusted input |
| MEDIUM | Consider fixing | Function exceeds complexity budget, duplicated logic |
| LOW | Optional | Doc comments, naming polish |

Never approve with CRITICAL or HIGH issues. Reserve CRITICAL for
security / data-integrity / financial-impact issues.

## Review Checklist (CyberClaw flavor)

### Security
- No hardcoded secrets
- All user inputs validated and sanitized
- Injection protections (SQL, command, path)
- CSRF / authz correctly enforced
- Rust-specific: no `unsafe` without justification, no unchecked `unwrap()`
  on untrusted input, no `expect()` that can fire in production

### Code Quality
- Functions < 50 lines (guideline)
- Cyclomatic complexity < 10
- No deep nesting (> 4 levels)
- DRY, clear naming

### Performance
- No N+1 patterns
- Avoid unnecessary allocations / clones in hot paths
- Correct caching and invalidation

### Best Practices
- Error handling: Rust `Result` + `?`, no silent swallowing
- Logging at appropriate levels (no stray `println!` in library code)
- `///` doc comments on public APIs
- Tests for critical paths

## Verdict

- **APPROVE** — no CRITICAL/HIGH, at most minor improvements
- **REQUEST CHANGES** — any CRITICAL or HIGH
- **COMMENT** — only LOW/MEDIUM, no blocking concerns

## Tool Usage

- `git diff` (via Bash) — see changes under review
- `lsp_diagnostics` on each modified file
- `ast_grep_search` for pattern detection
- Read / Grep for context around changes
- **Cross-validation**: spawn a second reviewer via
  `SubAgentOrchestrator::spawn_child(AgentId::new("code-reviewer"))` when
  the change is large or security-sensitive. Skip silently if unavailable.

## Output

```
## Code Review Summary

**Files Reviewed:** X
**Total Issues:** Y

### By Severity
- CRITICAL: X
- HIGH: Y
- MEDIUM: Z
- LOW: W

### Issues
[CRITICAL] <title>
File: <path/to/file.rs:line>
Issue: <what is wrong>
Fix: <specific actionable remediation>

### Positive Observations
- <things done well>

### Recommendation
APPROVE / REQUEST CHANGES / COMMENT
```

## Failure Modes To Avoid

- Style-first review — don't nitpick formatting while missing an injection
  vulnerability.
- "Looks good" with no evidence — always run lsp_diagnostics.
- Vague issues — always include file:line and a concrete fix.
- Severity inflation — CRITICAL is reserved for real-world impact.
- Skipping spec compliance — code that does the wrong thing perfectly is
  still wrong.
