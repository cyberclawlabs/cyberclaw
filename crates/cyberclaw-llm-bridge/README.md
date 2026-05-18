# cyberclaw-llm-bridge

CyberClaw LLM Bridge — LLM Tool Calling 到 Capability 执行的桥接层。

## Overview

Bridges LLM function calls to the CyberClaw execution model. Translates tool call requests from LLM responses into `Capability` execution requests dispatched through `Connector -> Capability`.

## Modules

| Module | Description |
|--------|-------------|
| `mapper` | `ToolCallMapper` — maps LLM function call names/args to Capability references |
| `executor` | `ToolExecutor` — executes mapped capabilities via `CapabilityDispatcher` |
| `standard_mappings` | Pre-defined mappings for common LLM tool patterns |
| `tool_filter` | Filters tool calls based on governance rules |
| `types` | Bridge-specific types |
| `error` | `BridgeError`, `BridgeResult` |

## Architecture

```
LLM Response (function_call)
    |
    v
ToolCallMapper ──> CapabilityRef
    |
    v
ToolExecutor ──> CapabilityDispatcher ──> Connector ──> Capability
    |
    v
Formatted Result ──> LLM (next turn)
```

## Testing

```bash
cargo test -p cyberclaw-llm-bridge
```
