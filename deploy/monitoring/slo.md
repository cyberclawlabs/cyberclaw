# CyberClaw SLO + Error Budget

Sprint 20 W1 — first formal SLO statement. This document is the contract
between platform engineering and product/customer teams: it states
**what reliability commitment we make**, **how we measure it**, and
**what action we take when we burn the error budget**.

> If you change an SLO target or measurement window, this is a customer-
> facing change. Run it through product + finance + on-call ops before
> merging.

## Service Level Objectives

### SLO 1 — Availability

| Property | Value |
|---|---|
| Target | **99.9%** of capability dispatches succeed |
| Measurement window | 30-day rolling |
| Indicator | `1 - (rate(cyberclaw_execution_total{outcome="failed"}) / rate(cyberclaw_execution_total))` |
| Error budget | ~43 minutes of downtime / 0.1% of dispatches per 30d |
| Excludes | Operator-cancelled executions (`outcome="cancelled"`); reviews waiting on humans |

Why 99.9% and not 99.99%: we are an LLM-driven control plane with
external provider dependencies. The LLM provider's own SLA is typically
99.9%. Setting our SLO higher than our largest dependency is dishonest.

### SLO 2 — Capability latency p99

| Property | Value |
|---|---|
| Target | **p99 ≤ 5 seconds** for capability dispatch |
| Measurement window | 30-day rolling |
| Indicator | `histogram_quantile(0.99, ...cyberclaw_capability_invocation_duration_seconds_bucket)` |
| Error budget | 1% of dispatches may exceed 5s |
| Excludes | `bash` (cmd.exec) and other High-risk capabilities — they have their own 30s default timeout |

Rationale: a capability above 5s p99 means the agent loop is starved.
Most capabilities (file ops, memory, todo) are sub-100ms; the 5s bound
is set so an LLM provider hiccup or container cold-start doesn't blow
the SLO immediately.

### SLO 3 — Review wait time p95

| Property | Value |
|---|---|
| Target | **p95 ≤ 30 minutes** for human review queue |
| Measurement window | 7-day rolling |
| Indicator | `histogram_quantile(0.95, ...cyberclaw_review_wait_seconds_bucket)` |
| Error budget | 5% of reviews may wait longer than 30min |

This SLO is **operations-staffing-bounded**, not infrastructure-bounded.
Burning this budget means we need more reviewers, not more replicas.

## Burn-rate alerts

Two-tier burn alerts (matches `deploy/monitoring/alerts.yml`):

  - **Fast burn**: consuming 2% of error budget in 1 hour → page within 5min
  - **Slow burn**: consuming 5% of error budget in 6 hours → ticket within 30min

The fast/slow split prevents flapping (many short outages shouldn't
each generate a page) while still catching the 1-hour catastrophe.

## What we do when we burn budget

### < 25% of budget consumed
No action. Budget exists to be spent on user-facing improvements.

### 25-50% of budget consumed
Engineering raises a "stop accepting risk" flag:
  - No new feature flags flipped without owner approval
  - No new dependency rolled out without a load test
  - Ongoing migrations pause if they're contributing to the burn
The team continues but prioritises SLO recovery over feature velocity.

### 50-100% of budget consumed
Code freeze on the affected component until burn rate drops below
1% per day. Owner writes a postmortem identifying:
  - Root cause of the spike
  - Why automation didn't auto-recover (CircuitBreaker, retry budget)
  - One concrete monitoring or process improvement that lands within
    1 sprint

### >100% (SLO breached)
Customer-facing acknowledgement within 24h. Public postmortem within
1 week. The next 30d window starts with reduced error budget until
the underlying bug is patched + verified for 30 days.

## SLO non-coverage (deliberate)

These are NOT covered by an SLO this quarter:

  - **LLM response quality** — model occasionally hallucinates; that's
    a model problem, not a platform problem. Out of scope until we own
    the inference layer.
  - **Audit log durability** — audit.db is append-only with hash chain
    + nightly backups (RB-11). Loss is treated as a P0 legal incident,
    not a budgeted-failure mode.
  - **Skill marketplace freshness** — skills are operator-curated, not
    a platform service.
  - **Per-tenant SLO** — multi-tenant migration is in Phase 1
    (ADR-0001). Per-tenant SLO will land when Phase 3 enforces tenant
    isolation in the dispatch path.

## Updating an SLO target

Process:
1. Open a PR modifying `deploy/monitoring/slo.md` + `alerts.yml`.
2. Owner from platform-eng + product reviews the customer impact.
3. Finance reviews the cost impact (tighter SLO = more headroom = more replicas).
4. Merge requires approval from both reviewers.

Don't tighten an SLO without first proving the system has hit the new
target for at least 4 weeks under realistic load.
