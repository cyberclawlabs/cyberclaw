# Architecture

This document describes how CyberClaw is structured. It assumes you have read the [project README](../README.md) and understand the project's positioning.

The bet is straightforward: when every AI agent action passes through identity verification, policy evaluation, optional human approval, and a verifiable audit chain — and there is no other path to the outside world — agents can be trusted to do more. This document explains how that path is constructed.

---

## The five-object model

The platform is organized around five first-class objects. They are not naming conventions; they are runtime-enforced boundaries.

### Agent

The actor. An Agent carries an identity (`agent_id`), a trust level (`Standard` / `Trusted` / `Restricted`), and a budget envelope (iterations, tokens, wall-clock time).

An Agent never accesses external systems directly. It can only emit `CapabilityRequest` messages.

### Skill

The method an Agent uses to do its work — a prompt template, a procedure description, a knowledge pack. Skills are loaded into the Agent's context but cannot themselves invoke external systems. A Skill that says "now write the file" doesn't write a file; it makes the Agent issue an `fs.write` request.

### Connector

The bridge to one external system. Each Connector implements a specific protocol (filesystem, HTTP, browser via CDP, MCP tool bridge, a messaging platform, etc.) and exposes one or more Capabilities.

Connectors are the only outbound interface. No code path in the platform reaches external systems except through a Connector.

### Capability

One authorized operation, identified by a canonical name like `fs.write`, `eth.sign`, `browser.navigate`, or `slack.send_message`. Capabilities are the unit of governance: rules grant or deny per `(agent_id × capability_id)`.

### Platform Plugin

A platform-level extension that hooks into the runtime — a custom audit sink, an additional policy enforcement layer, a new connector category, custom telemetry. Platform Plugins cannot bypass governance; they participate in the same dispatch path as everything else.

---

## Request dispatch — end to end

When an Agent invokes a tool, the platform performs the following steps in order. There is no fast path that skips any step.

1. **Request construction.** The Agent emits a `CapabilityRequest` carrying its identity, the target `capability_id`, the arguments (typed via JSON Schema), and a human-readable reason.

2. **Policy evaluation.** The governance engine reads the active YAML rule set, matches on `agent_id × capability_id` (with priority and tie-breakers), and returns one of `allow`, `deny`, or `review`.

3. **Review queue.** On `review`, the request is enqueued in the approval store and the Agent's loop blocks until a human operator decides. The decision is itself a signed action carrying the reviewer's identity.

4. **Deny path.** On `deny`, an error response is returned to the Agent. The denial is recorded in the audit chain with the matching rule's `reason` so the operator can later see exactly why.

5. **Dispatch.** On `allow`, the request is routed to the Connector registered for the Capability. The Connector executes against the external system.

6. **Output scanning.** The Connector's return value passes through two scanners — a prompt-injection detector and a credential-pattern detector — before the value re-enters the Agent's context.

7. **Audit write.** A single row is written to the append-only audit table containing: requester identity, full request, policy decision, connector return value, and a trace ID linking the steps.

8. **Loop continuation.** The Agent receives the (possibly sanitized) result and continues its iteration loop.

---

## Governance

Policy is described declaratively in a YAML file. The file is read at startup and can be hot-reloaded without a server restart.

```yaml
- kind: deny
  capability_id: cmd.run.rm_rf
  priority: 100
  reason: "irreversible"

- kind: review
  capability_id: eth.sign
  reason: "wallet operations require human approval"

- kind: allow
  agent_id: data-scientist
  capability_id: browser.navigate
  priority: 100
  reason: "data-scientist agent trusted for browsing"
```

**Evaluation order** within a single dispatch:

1. Deny rules pass first — highest priority wins.
2. Allow rules pass next — highest priority wins.
3. Tie-break is file order (earlier rule wins).
4. Unmatched dispatches fall through to the default `PolicyEngine`, which evaluates by capability risk level against the configured `CYBERCLAW_POLICY_REVIEW_THRESHOLD` (default: `low` — strictest).

Lifting governance out of `if`-statements scattered across application code and into one reviewable file means security, compliance, and platform teams can co-own policy without touching application source.

---

## Security architecture

Eight structurally enforced layers, ordered from low-level to high-level. Failure in one layer does not bypass the others.

| Layer | Mechanism |
|---|---|
| **Language** | Implemented in Rust. Memory safety and the absence of data races are compiler-enforced. Buffer overflow, use-after-free, and dangling-pointer bug classes are statically eliminated. |
| **Sandbox / isolation** | The same Capability can run under multiple runtimes: local, isolated process, container, or remote. High-risk operations default to container isolation. A failure or compromise in one Agent does not contaminate another. |
| **Model** | Part of the system prompt is fixed by the server and outside the model's rewritable scope. Rationalizations like "just this once" or "for testing only" cannot edit it. |
| **Input/Output** | Tool outputs run through prompt-injection scanning and credential pattern detection before re-entering the model context. Scraped content cannot pivot into a prompt-injection vector. |
| **Execution** | Under autopilot, high-risk Capabilities are temporarily revoked from the Agent's allowed set. A circuit breaker forces exit after consecutive failures, preventing runaway loops in elevated mode. |
| **Interface** | Filesystem writes are bounded to a configured workspace root by the connector code, not by policy alone. Browser and HTTP block RFC1918 addresses by default. Misconfigured policy cannot punch through these hard boundaries. |
| **Authentication** | JWT and cluster-token comparisons use timing-safe cryptographic comparison. IM webhooks require HMAC-SHA256 signatures; platforms without a configured secret are refused. |
| **Audit** | Every dispatch produces a hash-chained audit row. Tampering is detectable; the whole chain is verifiable. |

The same design principle runs through all eight: failure modes are converted from "norms the author must follow" into "structures that cannot be bypassed". Norms can be argued past. Structure cannot.

---

## Audit chain

Each dispatch produces one row in an append-only SQLite table. Each row contains:

- `trace_id` — links the steps of one logical request.
- `requester` — Agent identity.
- `capability_id`, `arguments`, `reason`.
- `policy_decision` — `allow` / `deny` / `review`, plus the matching rule's reason.
- `connector_result` — the return value (post-sanitization).
- `prev_hash`, `row_hash` — the chain links.

Tamper-evidence comes from `row_hash = H(prev_hash || row_contents)`. Any modification to a past row invalidates every subsequent row's hash. The full chain is verified with:

```bash
cyberclaw audit verify
```

Audit rows can be exported to OTLP-HTTP, making CyberClaw's trace data ingestible by Jaeger, Tempo, Grafana Cloud, Datadog, and other OpenTelemetry-compatible backends.

---

## Multi-agent orchestration

A single Agent can spawn sub-agents with their own budgets, run them in parallel, and reduce their outputs into one result. The reduction strategy is part of the orchestrator's contract:

- **Concat** — concatenate sub-agent outputs in order.
- **MajorityVote** — pick the result with the most agreement.
- **LlmSummary** — run a final LLM call to synthesize a single answer from the sub-agent outputs.

CyberClaw also implements Mixture-of-Agents (MoA), where the same prompt fans out across multiple LLM providers (Anthropic, OpenAI, DeepSeek, MiniMax, etc.) and an aggregator model synthesizes the final answer.

Sub-agent boundaries are real: each sub-agent has its own identity and budget, so policy can target a specific sub-agent (`agent_id: research-helper`) independently from its parent.

---

## Cluster mode

CyberClaw can run in two modes:

- **Single-node** (default) — one process, local SQLite, no Raft.
- **Cluster** (`CYBERCLAW_CLUSTER_MODE=multi`) — multiple replicas coordinated by Raft consensus. State changes (rule updates, audit writes, session bindings) are replicated across replicas.

In cluster mode, replicas authenticate to each other using a shared bearer token (`CYBERCLAW_CLUSTER_SHARED_TOKEN`) over an isolated set of `/internal/cluster/*` routes with their own rate limiting and timing-safe token comparison.

A `BrainCoordinator` distributes Agent sessions across the cluster's `AgenticLoopPool` workers. Workers poll a central assignment endpoint (`CYBERCLAW_ASSIGNMENT_PULL_URL`) and execute sessions in parallel.

Distributed approvals across replicas (so any operator on any replica can approve a pending review queued on another) are on the v2.x roadmap. Today, approvals are tied to the replica where the request was filed.

---

## What CyberClaw is not

A few boundaries are worth naming explicitly:

- **Not a model.** CyberClaw runs LLMs; it does not train or serve them. Bring your own provider.
- **Not a vector database.** Memory is provided but external embedding stores are integrated via Connector, not built in.
- **Not a chatbot framework.** The runtime cares about the policy and audit path between intent and action. Conversation UI is one client of the platform, not its core.
- **Not Web3-specific.** Web3 (multisig, treasury, on-chain runbooks) is a representative high-stakes use case, not the product scope. Security operations and DevOps change management are equally on-target.

---

For environment variable reference, see [ENVIRONMENT_VARIABLES.md](ENVIRONMENT_VARIABLES.md). For deployment and operational guidance, see [GUIDE.md](GUIDE.md).
