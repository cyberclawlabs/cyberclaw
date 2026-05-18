# cyberclaw-skill-runtime

Skill 运行时核心 crate，提供 CyberClaw 平台的 Skill 注册、加载、扫描和执行上下文管理。

## Overview

Skills in CyberClaw provide methods, knowledge, and templates — but do **not** directly execute capabilities. All execution flows through `Connector -> Capability`. This crate manages the Skill lifecycle: discovery, loading, security scanning, and context binding into the agentic loop.

## Key Components

### Core Runtime

| Module | Description |
|--------|-------------|
| `runtime` | `MinimalSkillRuntime` — core `SkillRuntime` trait implementation with in-memory registry |
| `context` | `SkillContext`, `SkillReference`, `ToolDeclaration` — context a skill provides to the agentic loop |
| `config` | `SkillConfig` — skill configuration |
| `handler` | `SkillHandler` — skill request processing |
| `error` | `SkillRuntimeError` |

### Skill Loading

| Module | Description |
|--------|-------------|
| `loaders/claude_code` | `ClaudeCodeSkillLoader` — loads Claude Code skill format (SKILL.md + scripts/) |
| `loaders/codex` | `CodexSkillLoader` — loads Codex skill format |
| `loaders/openclaw` | `OpenClawSkillLoader` — loads OpenClaw skill format (skill.toml) |
| `loaders/hot_reload` | `HotReloadWatcher` — filesystem-based hot reload support |
| `loaders/unified` | `UnifiedSkillLoader` — dispatches to the correct format loader |

### Security

| Module | Description |
|--------|-------------|
| `skill_scanner` | `SkillScanner` — 40 threat patterns across 10 categories with trust-aware verdict matrix |

### Ecosystem

| Module | Description |
|--------|-------------|
| `plugin_registry` | `PluginRegistry` — declarative plugin registration (replaces dynamic loading) |
| `skills/calculator` | `CalculatorSkill` — built-in calculator skill |
| `skills/echo` | `EchoSkill` — built-in echo skill |

## Skill Format Compatibility

CyberClaw prioritizes compatibility with mainstream skill ecosystems:

| Format | Loader | Manifest |
|--------|--------|----------|
| Claude Code Skill | `ClaudeCodeSkillLoader` | `SKILL.md` + `scripts/` + `references/` + `assets/` |
| Codex Skill | `CodexSkillLoader` | `manifest.yaml` |
| OpenClaw Skill | `OpenClawSkillLoader` | `skill.toml` |

## Security Scanning

`SkillScanner` evaluates skills before installation:

- **40 threat patterns** across 10 categories (code injection, data exfiltration, privilege escalation, etc.)
- **Trust-aware verdicts**: scanning thresholds vary by `SkillTrustLevel`
- **Verdicts**: `Allow`, `Warn`, `Quarantine`, `Deny`

## Architecture

```
Skill Package (SKILL.md / manifest.yaml / skill.toml)
    |
    v
UnifiedSkillLoader ──> Format Detection ──> ClaudeCode / Codex / OpenClaw Loader
    |
    v
SkillScanner ──> Threat Analysis ──> ScanVerdict
    |
    v (if Allow/Warn)
MinimalSkillRuntime ──> SkillContext ──> SkillBinder (in agent-runtime)
                                              |
                                              v
                                        PromptAssembler ──> Agentic Loop
```

## Important Design Notes

- **Skills do NOT execute capabilities** — they provide context, templates, and method knowledge to the agentic loop. Actual execution is always `Connector -> Capability`.
- **SkillHub** (`skill_hub.rs`) provides the full lifecycle: remote discovery -> quarantine -> scan -> install, with audit JSONL logging and lock file support.
- **Hot reload** watches the filesystem for skill changes and reloads without restart.
- **Known limitation**: Remote registry integration is filesystem-only (no network discovery yet).

## Testing

All 184 tests are inline (`#[cfg(test)]` modules). No separate `tests/` integration tests yet.

```bash
cargo test -p cyberclaw-skill-runtime
```
