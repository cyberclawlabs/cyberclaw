---
name: verify
description: Verify that a change really works before you claim completion (CyberClaw-adapted)
source: oh-my-claudecode/skills/verify/SKILL.md
adapted-for: CyberClaw (Sprint 4, 2026-04-18)
---

<!--
CyberClaw adaptation notes:
- Skills never execute code in CyberClaw (CLAUDE.md §3.4 and §9). This
  methodology describes what to run; actual execution is the agent's job
  via Connector → Capability.
- Default verification commands for the CyberClaw Rust workspace:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace
- Verification evidence lives in the Execution / Artifact / Provenance
  skeleton (see `docs/architecture/runtime/RUNTIME_BLUEPRINT_V2.0.md`),
  not in `.omc/state` JSON blobs.
-->

# Verify

Use this skill when the user wants confidence that a feature, fix, or refactor actually works.

## Goal
Turn vague "it should work" claims into concrete evidence.

## Workflow
1. Identify the exact behavior that must be proven.
2. Prefer existing tests first.
3. If coverage is missing, run the narrowest direct verification commands available.
4. If direct automation is not enough, describe the manual validation steps and gather concrete observable evidence.
5. Report only what was actually verified.

## Verification order
1. Existing tests (`cargo test --workspace` for Rust, package-specific commands for ecosystem/ packages)
2. Typecheck / build (`cargo clippy --workspace --all-targets -- -D warnings`)
3. Narrow direct command checks
4. Manual or interactive validation

## CyberClaw-specific defaults
- For Rust workspace changes, always show output from:
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace`
- For single-crate changes, you may scope tests to that crate but still run workspace clippy.
- Record verification evidence as an Artifact via `cyberclaw-store` when the surrounding Execution demands persistence.

## Rules
- Do not say a change is complete without evidence.
- If a check fails, include the failure clearly.
- If no realistic verification path exists, say that explicitly instead of bluffing.
- Prefer concise evidence summaries over noisy logs.

## Output
- What was verified
- Which commands/tests were run
- What passed
- What failed or remains unverified
