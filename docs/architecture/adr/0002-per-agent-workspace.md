# ADR-0002: Per-Agent Workspace Isolation

**Status**: Proposed (Phase 1 implemented)
**Date**: 2026-04-28
**Deciders**: CyberClaw platform team
**Sprint**: 20 W1

## Context

`LocalConnector` currently has a single `workspace: PathBuf` field set
at construction time from `CYBERCLAW_WORKSPACE` env (default `/tmp` in
staging). Every capability handler — `fs.read`, `fs.write`, `fs.edit`,
`cmd.exec`, `search.grep`, `search.glob`, `host.*` — uses this same
path for:

  - Default working directory when none is supplied in input
  - Path validation root (`validate_path` rejects anything not under it)
  - Search root for grep/glob

This is **single-tenant by construction**. Two agents running in the
same process share `/tmp` — agent A can read agent B's files via
`fs.read(/tmp/file)`. The Container runtime wired in commit `6ad42f0`
isolates `cmd.exec` at the kernel level (alpine container per
dispatch) but NOT `fs.*` — those still hit the host filesystem.

Multi-tenant production (ADR-0001 Phase 3) requires per-tenant data
isolation. Per-agent isolation is a strict subset: even within a
single tenant, agents shouldn't read each other's scratch files.

## Decision

Per-agent workspace migration is a **2-phase** plan:

### Phase 1 — Default workdir derivation (this commit)

**Invariant after Phase 1**: when a capability handler computes its
*default* working directory (no `workdir` field in input), that
default is `<workspace_root>/agents/<actor_id>/` instead of
`<workspace_root>/`. Path validation still uses `workspace_root` —
agents can still escape via an explicit `path: "/tmp/other-agent/..."`.

Concrete deliverables:
  - `LocalConnector::resolve_agent_workspace(actor: &ActorRef) -> PathBuf`
    returns `workspace_root/agents/<actor_id>`. Creates the dir
    on first call. Falls back to `workspace_root` when the actor is
    a system actor (no useful id).
  - `cmd.exec` uses `resolve_agent_workspace(&request.actor)` for
    its default workdir.
  - Future call sites adopt incrementally without breaking changes.

This is intentionally **soft isolation**. It establishes the directory
layout (`workspace/agents/<actor_id>/`) so Phase 2's stricter
validation has somewhere to land.

### Phase 2 — Strict containment (Sprint 20 W2)

**Invariant after Phase 2**: a capability dispatch from agent A
cannot read or write any path outside `<workspace_root>/agents/<A>/`,
regardless of what the input specifies.

Concrete deliverables:
  - `LocalConnector::validate_path` takes an `&ActorRef` parameter and
    validates against the agent's subdir, not the shared root.
  - All ~10 callers of `validate_path` thread the actor through.
  - A new env `CYBERCLAW_WORKSPACE_STRICT_AGENT_ISOLATION=true` enables
    the strict mode. Default `false` until the migration is verified
    in staging — preserves backward-compat for single-tenant deployments
    that rely on the shared `/tmp` semantic.
  - The Container runtime path mounts only the agent's subdir into the
    container (currently mounts the full workspace). Aligns kernel-
    level isolation with filesystem isolation.

## Consequences

**Pro**:
  - Multi-agent demos (which already work) gain real filesystem isolation.
    No more "agent B accidentally finds agent A's debug log".
  - Foundation for ADR-0001 Phase 3 (multi-tenant): combined with
    tenant-prefixed paths, gives `<workspace>/tenants/<T>/agents/<A>/`
    structure.
  - Audit logs become more useful — a `fs.write` row against
    `workspace/agents/agent_X/output.txt` is self-attributing.

**Con**:
  - Breaks any test fixture that wrote to `workspace_root` directly
    expecting another agent to read it. Audit needed in Phase 2.
  - Default workdir change in Phase 1 is silently observable: agents
    that were `cd`-ing into a known shared workspace path will
    notice their new working directory differs. We accept this
    because (a) it's the more secure default, (b) any agent
    relying on a hardcoded shared path is already brittle.
  - Adds a directory creation call per agent on first dispatch.
    Negligible perf impact.

## Acceptance criteria

The migration is "done" when:
1. `cmd.exec` default workdir derives from `actor_id` (Phase 1).
2. Two agents in the same process cannot read each other's files
   via `fs.read` even without specifying paths (Phase 2).
3. The Container runtime mounts only the dispatching agent's
   workspace subdir (Phase 2).
4. Integration test: spawn two agents, have agent A write
   `output.txt` via `fs.write` (default path), have agent B run
   `fs.read` (default path) — must return "file not found", not
   agent A's content.

## Status

This ADR is **Proposed (Phase 1 implemented)**. Phase 1 lands in the
same commit. Phase 2 is gated on the multi-tenant Phase 2 plumbing
work (ADR-0001) since both will need to thread `ActorRef` /
`TenantId` through the same call paths.
