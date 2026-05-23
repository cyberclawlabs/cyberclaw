# Modules

CyberClaw is a Rust workspace of 15 crates plus two binary apps. The
dependency direction is strict: `cyberclaw-core` is the leaf, the
runtimes and governance build on it, the control plane composes them
together, and `apps/cyberclaw-server` and `apps/cyberclaw-cli` sit at
the top. No crate depends back toward the apps. This guarantees that
swapping a runtime, a connector, or the persistence layer never
ripples upward beyond its own boundary.

This page is a one-stop tour. Every crate has its own
`crates/<name>/README.md` with module-level depth.

## Apps

| Crate | One-line role |
|---|---|
| [`apps/cyberclaw-server`](../../apps/cyberclaw-server/) | HTTP server, admin console host, all REST handlers, middleware lanes, audit DB. |
| [`apps/cyberclaw-cli`](../../apps/cyberclaw-cli/) | Operator-facing TUI: `chat`, `onboard`, `doctor`, plus resource commands that go through the server over HTTP. |

## Foundation

| Crate | Role |
|---|---|
| [`cyberclaw-core`](../../crates/cyberclaw-core/) | The leaf. Every type that crosses a crate boundary is defined here: ID newtypes (`ExecutionId`, `CapabilityId`, …), risk levels, governance decisions, provenance records, security context, the working/episodic/procedural memory traits, the agent trust model. No business logic. |
| [`cyberclaw-memory-extraction`](../../crates/cyberclaw-memory-extraction/) | Cold-path memory extraction. Reads completed traces and produces structured memory units (MemCell boundaries, AtomicFact, Episode, Foresight) with quality scoring. Off-path: never blocks the live agent loop. |

## Runtimes

| Crate | Role |
|---|---|
| [`cyberclaw-agent-runtime`](../../crates/cyberclaw-agent-runtime/) | The agentic loop. `AgenticLoop` + `LoopState`, `SubAgentOrchestrator` (depth-3, max-5 children, budget-fraction inheritance), `PromptAssembler` with `Static`/`PerTurn`/`Volatile` cache policies, `CapabilityFacade` + `ToolDescriptionBridge` (Anthropic / OpenAI / generic-text projections), `DeferredToolRegistry` (active / deferred / hidden states), `ToolResultPipeline` (4 stages: budget → format → scan → preview). |
| [`cyberclaw-skill-runtime`](../../crates/cyberclaw-skill-runtime/) | Skill installation lifecycle: `SkillHub` (quarantine → scan → install), `SkillScanner` (subprocess / pip-install / credential signature detection), trust-matrix-driven verdict, OpenPGP signature verification via Sequoia, on-disk audit log. |
| [`cyberclaw-plugin-runtime`](../../crates/cyberclaw-plugin-runtime/) | Platform plugin runtime: cross-cutting enhancements (audit sinks, gateway adapters, scheduler triggers) that never carry business logic. |

## Connectors

| Crate | Role |
|---|---|
| [`cyberclaw-connectors`](../../crates/cyberclaw-connectors/) | The only execution surface. `Connector` trait, `CapabilityDispatcher`, `ConnectorRegistry`, plus the bundled native connectors: `local::{cmd, fs, lsp, memory, search, slides, task, web, workdir_checkpoint}`, `browser` (Playwright over WebDriver BiDi), `mcp` (MCP server bridge with namespace prefixing and per-tool risk classification), `handoff`, `runtime` (container + selector). |
| [`cyberclaw-llm`](../../crates/cyberclaw-llm/) | LLM client abstraction. `LlmClient` trait + concrete clients for Anthropic, OpenAI, and OpenAI-compatible (`GenericOpenAiClient`). Streaming + non-streaming, tool-use blocks, structured output. |
| [`cyberclaw-llm-bridge`](../../crates/cyberclaw-llm-bridge/) | Glue between LLM tool-use blocks and capability dispatch. `ToolCallMapper`, `ToolExecutor`. The agent runtime never deals with raw LLM tool blocks; this bridge does. |

## Governance

| Crate | Role |
|---|---|
| [`cyberclaw-governance`](../../crates/cyberclaw-governance/) | Where the platform refuses. `PolicyEngine` (allow / ask / deny verdicts), `TrustMatrix` (`(agent_trust × capability_risk) → verdict`), `DangerousCapabilityFilter` (7 default rules), iron-law layer for non-rationalizable refusals, `ToolOutputSanitizer` (Aho-Corasick injection scanning + regex credential detection), `ToolPermissionMatcher` (24 default glob rules), `AutoModeGate` (dynamic permission scoping for autopilot), `CircuitBreaker`. |

## Control plane

| Crate | Role |
|---|---|
| [`cyberclaw-control-plane`](../../crates/cyberclaw-control-plane/) | The composition layer. `ControlPlaneOrchestrator` ties everything together. `EcosystemScanner` discovers manifests on disk; `Registry` indexes them; `Resolver` produces ExecutionPlans. `PersistentLoop` runs story-driven execution with verifier feedback. `BrainCoordinator`, `HeartbeatMonitor`, `LeastLoadedAssigner` for multi-replica session affinity. `WorkflowTrigger` for cron / webhook / event triggers. `CapabilityDiscovery` for runtime PATH/binary scans. `ContextCompressor` for L0 / L1 / L2 memory compression. |
| [`cyberclaw-consensus`](../../crates/cyberclaw-consensus/) | Raft node + state machine + log persistence. Single-node deployments use the same code with one peer. Watch-channel shutdown signaling. |
| [`cyberclaw-scheduler`](../../crates/cyberclaw-scheduler/) | Cron + interval triggers. Lease-based ownership so multi-replica deployments don't double-fire. |
| [`cyberclaw-workflow`](../../crates/cyberclaw-workflow/) | Workflow engine with state store. Sequential task chains, branching, pause-and-resume. |

## Observability + persistence

| Crate | Role |
|---|---|
| [`cyberclaw-observability`](../../crates/cyberclaw-observability/) | `EventRecorder` (in-memory + sink-backed), trace projections, metric collection, audit projections that derive the `(Artifact, Trace, Provenance)` triple from raw events. |
| [`cyberclaw-store`](../../crates/cyberclaw-store/) | Persistence: SQLite-backed registry, task manager, audit DB with HMAC chaining, FTS5 skill index, sled-backed KV stores. Every store has both an `InMemory*` (for tests + dev) and a disk-backed implementation. |

## How they compose at runtime

Reading a chat request top to bottom:

1. `apps/cyberclaw-server/middleware/` admits the request.
2. `apps/cyberclaw-server/api/chat_handler.rs` resolves the agent
   via `cyberclaw-control-plane::ControlPlaneOrchestrator`.
3. `cyberclaw-agent-runtime::PromptAssembler` builds the prompt
   from the agent's manifest + active skills + tool descriptors
   produced by `ToolDescriptionBridge`.
4. `cyberclaw-llm` calls the configured provider; the response
   may contain tool-use blocks.
5. `cyberclaw-llm-bridge` converts each tool-use block into a
   `CapabilityDispatchRequest`.
6. `cyberclaw-governance::PolicyEngine` returns `Allow / Ask /
   Deny` for each request. `Ask` enqueues a Review and pauses.
7. `cyberclaw-connectors::CapabilityDispatcher` routes admitted
   requests to the owning Connector.
8. The Connector executes; `cyberclaw-store::AuditDb` persists
   the row with HMAC chaining; `cyberclaw-observability::EventRecorder`
   emits a live event.
9. The loop iterates until done or the budget is exhausted.

The dependency graph for that flow is one direction:
`server → control-plane → {agent-runtime, governance, connectors,
llm-bridge, llm} → core`.

## Where to dig deeper

For the why behind a specific design choice:

- Architecture overview: [`docs/architecture/overview/`](../architecture/overview/)
- Runtime blueprint: [`docs/architecture/runtime/`](../architecture/runtime/)
- Memory model: [`docs/architecture/memory/`](../architecture/memory/)
- Governance model: [`docs/architecture/governance/`](../architecture/governance/)
- Security architecture: [`docs/architecture/SECURITY_ARCHITECTURE.md`](../architecture/SECURITY_ARCHITECTURE.md)

For the how:

- Build a connector: [`docs/builders/build-a-connector.md`](../builders/build-a-connector.md)
- Build a skill: [`docs/builders/build-a-skill.md`](../builders/build-a-skill.md)
- Build a plugin: [`docs/builders/build-a-plugin.md`](../builders/build-a-plugin.md)
