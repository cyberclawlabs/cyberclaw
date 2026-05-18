# cyberclaw-workflow

CyberClaw 工作流引擎 — DAG-based 工作流定义、执行和触发管理。

## Overview

Provides a workflow engine supporting DAG execution with 5 step types and a trigger registry with 5 trigger types. Workflows orchestrate multi-step agent tasks with retry policies, conditional branching, and event-driven triggers.

## Modules

### Engine (`engine.rs`)

| Type | Description |
|------|-------------|
| `WorkflowEngine` | Core DAG executor with step scheduling and retry |
| `WorkflowDefinition` | Workflow schema with steps and edges |
| `WorkflowStep` | Individual step with type, inputs, outputs |
| `WorkflowInstance` | Running workflow state |
| `WorkflowStatus` | Pending/Running/Completed/Failed/Cancelled |
| `RetryPolicy` | Retry configuration per step |

**Step Types:**
- `Task` — single capability execution
- `Parallel` — concurrent step execution
- `Condition` — conditional branching
- `Loop` — iterative execution
- `SubWorkflow` — nested workflow invocation

### Trigger (`trigger.rs`)

| Type | Description |
|------|-------------|
| `TriggerRegistry` | Manages trigger registrations and matching |
| `WorkflowTrigger` | Trigger definition with type and filter |

**Trigger Types:**
- `Manual` — explicit invocation
- `OnExecutionComplete` — fires when an execution finishes
- `OnWebhook` — fires on webhook receipt
- `Cron` — scheduled execution
- `OnEvent` — event-driven (filter not yet implemented)

## Known Debt

- `OnEvent` trigger filter field not yet implemented in `TriggerMatcher`
- No persistent workflow state (in-memory execution only)
- Relatively low test count (40 tests) compared to other crates

## Testing

```bash
cargo test -p cyberclaw-workflow
```
