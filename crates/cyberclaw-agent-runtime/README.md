# cyberclaw-agent-runtime

Agent 运行时核心 crate，提供 CyberClaw 平台的 Agent 执行基础设施。

## Overview

This crate implements the runtime layer that powers Agent execution in CyberClaw. It provides the agentic loop, context management, tool orchestration, and sub-agent coordination — all within the platform's governed execution model where actual execution flows through `Connector -> Capability`.

## Key Components

### Core Runtime

| Module | Description |
|--------|-------------|
| `runtime` | `MinimalAgentRuntime` — core `AgentRuntime` trait implementation |
| `agentic_loop` | `DefaultAgenticLoop` — LLM-call/parse/execute cycle with budget control, stuck detection, and parallel planning |
| `config` | `AgentConfig`, `RuntimeConfig`, `ServiceConfig` |
| `types` | `AgentRequest`, `AgentResponse` |
| `error` | `AgentRuntimeError` |

### Tool Management

| Module | Description |
|--------|-------------|
| `tool_description` | `CapabilityFacade` — read-only LLM projection of Capability metadata (Anthropic/OpenAI/Text formats) |
| `builtin_tools` | `BuiltinToolRegistry` — 10 default tools mapped to Connector+Capability pairs |
| `deferred_registry` | `DeferredToolRegistry` — active/deferred states with risk-gated promote/demote |
| `tool_result_pipeline` | 4-stage pipeline: budget -> format -> scan -> preview |
| `tool_result_budget` | `ToolResultBudget` — token budget enforcement for tool outputs |

### Context & Memory

| Module | Description |
|--------|-------------|
| `prompt_assembler` | `PromptAssembler` — multi-priority section assembly with `CachePolicy` (Static/PerTurn/Volatile) |
| `context_compressor` | `ContextCompressor` — 4-stage compression with circuit breaker and `MemoryLevel` (L0/L1/L2) |
| `memory_integration` | `MemoryIntegration` — frozen snapshots, debounced writes, scan-on-write |

### Multi-Agent

| Module | Description |
|--------|-------------|
| `sub_agent` | `SubAgentOrchestrator` — spawn/run/cancel/collect with depth limit (3), max children (5), budget fraction (0.5) |
| `loop_delegate` | `LoopDelegate` trait — `AutopilotDelegate`, `InteractiveDelegate`, `NoOpDelegate` |
| `streaming` | `StreamSink` trait, `ChannelStreamSink` — streaming output support |

### Loop Governance & Verification

| Module | Description |
|--------|-------------|
| `loop_governor` | `AgenticLoopGovernor` — wall-clock / token / repetition gates with L1/L2/L3 enforcement profiles |
| `verify` | `OutputVerifier` trait + `VerifierChain` + 3 built-in verifiers: `CodeBlockVerifier`, `JsonStructureVerifier`, `RegexAssertVerifier` |

### Skill Integration

| Module | Description |
|--------|-------------|
| `skill_binder` | `SkillBinder` — auto-bind skills by AND-of-OR keyword group matching; a skill is bound when all keyword groups each have at least one match in the agent context |

## Architecture

```
AgentRequest
    |
    v
MinimalAgentRuntime
    |
    v
DefaultAgenticLoop ──> PromptAssembler ──> LLM
    |                                        |
    |  <── parse response ──────────────────-┘
    |
    v
ToolResultPipeline ──> BuiltinToolRegistry / DeferredToolRegistry
    |                         |
    v                         v
ContextCompressor        Connector -> Capability (via OrchestratorGateway)
    |
    v
MemoryIntegration
```

## Important Design Notes

- **CapabilityFacade is NOT a first-class object** — it is a read-only projection for LLM consumption. The canonical execution path is always `Connector -> Capability`.
- **SubAgentOrchestrator** enforces depth limits and budget fractions to prevent unbounded recursion.
- **ContextCompressor** uses a circuit breaker that trips after repeated compression failures.
- **GAP-4 fix (2026-05-23)**: `agentic_loop` now detects whitespace-only assistant content paired with a `stop` finish reason. Previously this was silently treated as `Done`, causing the loop to terminate without a real response. It now injects a system nudge and continues the loop (`commit 69c1226`).
- **Known debt**: `ChannelStreamSink::close()` is a no-op (relies on drop); `AgenticLoopPool::checkout` uses 10ms sleep busy-wait (should use Semaphore).

## Testing

All 432 tests are inline (`#[cfg(test)]` modules). No separate `tests/` integration tests yet.

```bash
cargo test -p cyberclaw-agent-runtime
```
