# ADR-0001: Multi-Tenant Migration Plan

**Status**: Proposed
**Date**: 2026-04-28
**Deciders**: CyberClaw platform team
**Sprint**: 20

## Context

The CyberClaw codebase has carried a typed `TenantId` identifier in
`crates/cyberclaw-core/src/ids.rs` since the original architecture was
laid down. The intent — surfaced in `ActorRef.tenant_id` and
`ReviewRequest.tenant_id` — was to scope every platform action to a
tenant so a single deployment can serve multiple isolated customers.

The reality: **`TenantId` is plumbed but never populated**. As of
commit `dbc00bd` (2026-04-28), `grep "tenant_id: None"` returns 50
occurrences across 20 source files. Every dispatch, every audit row,
every memory write, every review proposal is constructed with
`tenant_id: None`. The system runs single-tenant by default, with the
type-level vocabulary hinting at an unfinished migration.

The first multi-tenant customer will expose the gap immediately:
  - Tenant A's agent reads tenant B's memory record (memory_store
    query is keyed on `(session_id, key)`, no tenant filter)
  - Tenant A sees tenant B's audit log (single global audit.db, no
    tenant column)
  - Tenant A's review queue contains tenant B's items (review queue
    has a tenant filter helper but nobody calls it)

This ADR scopes the migration so the team doesn't ship "multi-tenant"
as a marketing claim before the load-bearing changes land.

## Decision

Multi-tenant migration is a **3-phase** plan executed across Sprint
20-22. Each phase produces a verifiable invariant; phases cannot be
parallelised because each builds on the previous one's invariant.

### Phase 1 — Identity (Sprint 20 W1, this commit)

**Invariant after Phase 1**: every authenticated request has an
optional `TenantId` available on `Claims`. Existing code paths that
pass `tenant_id: None` are unchanged; the *capability to know* lands
without enforcing.

Concrete deliverables:
  - Add `tenant: Option<TenantId>` to `Claims` (JWT extension).
    Existing tokens issued without a tenant claim still parse via
    `serde(default)`.
  - `generate_jwt` accepts a `tenant: Option<&TenantId>` parameter.
  - `Claims::tenant()` getter for downstream consumers.
  - Unit test proves both code paths: token-with-tenant → claims
    have it; token-without-tenant → `claims.tenant() == None`.

This is intentionally narrow. Phase 1 does not change behaviour.

### Phase 2 — Plumbing (Sprint 20 W2-W3)

**Invariant after Phase 2**: when an authenticated request has a
tenant in its Claims, that `TenantId` flows into every constructed
`ActorRef`, `ReviewRequest`, `LeveledMemoryRecord`, and audit
`AuditEntry.detail`. The 50 `tenant_id: None` sites collapse to a
small, audited set (only background jobs / system actors should
remain `None`).

Concrete deliverables:
  - Replace `tenant_id: None` in 50 sites with
    `tenant_id: claims.tenant().cloned()` (or equivalent for non-HTTP
    paths). Each replacement is a one-line edit, but the audit is
    real: each site needs to confirm whether the tenant context is
    actually available, or whether it's a bona-fide system action.
  - `LeveledMemoryRecord` gains `tenant_id: Option<TenantId>` field.
    `LeveledMemoryStore::query_by_key` adds a tenant predicate; the
    SQLite schema gets a `tenant_id` column with a backfill migration
    that sets every existing row to `NULL` (single-tenant legacy data).
  - Audit `AuditEntry.detail` JSON gets a top-level `tenant_id` key
    when known. The audit hash chain still hashes the entire detail
    blob, so introducing this field is a non-breaking change to the
    chain — only the *new* rows include the field.

Phase 2 does NOT enforce any isolation. It guarantees the data is
*available* for filtering.

### Phase 3 — Enforcement (Sprint 21)

**Invariant after Phase 3**: a tenant cannot read or write another
tenant's data through any HTTP API or LLM tool dispatch.

Concrete deliverables:
  - `LeveledMemoryStore::query_by_key` requires an `Option<&TenantId>`
    parameter. When `Some(tenant)`, results are filtered to that
    tenant. When `None`, only rows with `tenant_id IS NULL` are
    returned (preserves single-tenant legacy access).
  - All `/api/v1/memory/*` endpoints pass `claims.tenant()` to the
    store. Cross-tenant reads return 404 (not 403 — leaking existence
    is a side-channel).
  - `MemoryConnector::do_read/do_write/do_search` receives the tenant
    via the `CapabilityExecutionRequest.actor.tenant_id` field, scopes
    accordingly. A unit test puts data under `tenant_a`, dispatches a
    request from `tenant_b`'s agent, and asserts the read returns
    `value: null`.
  - `TodoConnector` does the same.
  - `AuditSink::tail_rows` accepts an `Option<&TenantId>` filter.
    `/api/v1/audit/*` endpoints scope to caller's tenant. Admin
    operators (no tenant in their claim) see global audit.
  - `ReviewQueue` already has the tenant predicate (`is_visible_to_tenant`
    in `core/review.rs:221`); Phase 3 wires it into every list / accept
    / reject path.

Phase 3 is the largest of the three. It is also the only phase that
can break existing tests — every test fixture currently uses
`tenant_id: None` paths.

### Out of scope

Not in this migration:
  - **Tenant onboarding UX** — operators provision tenants via
    out-of-band tooling (CLI / admin SPA flow). Will land in Sprint 22+.
  - **Per-tenant rate limits, quotas, billing** — uses TenantId as the
    key once Phase 3 is done. Sprint 23+.
  - **Per-tenant policy rules** — `PolicyEngine` already supports
    rule scoping; wiring the tenant context is gated on Phase 3.
  - **Cross-tenant administrative roles** — operators with no tenant
    in their claim see all data. The role-vs-tenant matrix design
    is its own ADR.

## Consequences

**Pro**:
  - The platform can host multiple isolated customers in one
    deployment, the original architectural intent.
  - Phase 1 is shipped cheap (~1 file change) and unblocks the next
    phases without committing to the full migration cost upfront.
  - The 3 phases each produce a verifiable invariant — no "we believe
    it works" handwaving.

**Con**:
  - Phase 3 will break every integration test that relies on
    `tenant_id: None` being interchangeable with a populated tenant.
    Estimated test fix-up: ~3 days.
  - Every existing audit row is `tenant_id: NULL` — historical data
    is permanently un-attributed. We accept this; tenant-scoped
    audit starts from Phase 2 deployment time.
  - SQLite schema migration for `LeveledMemoryStore` (Phase 2) is a
    one-way change. Backups taken before Phase 2 cannot be restored
    onto a Phase-2 server without a data migration script. RB-11
    needs an addendum.

## Acceptance criteria

The migration is "done" when:
1. `grep "tenant_id: None"` returns ≤ 5 results (only legitimate
   system-action paths) across the workspace.
2. An integration test creates two tenants, has each write a memory
   record under the same `(scope, key)`, and proves the cross-tenant
   read returns `value: null`.
3. Audit query `tail_rows` with a tenant filter returns only that
   tenant's rows in a multi-tenant fixture.
4. The same proof for review queue, capability dispatch, and the
   admin SPA's panels.

## Status of this ADR

This ADR is **Proposed**. Phase 1 implementation lands in the same
commit as this document.
