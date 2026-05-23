---
name: domain-expert-devops
version: 0.1.0
description: Production change-management expertise — release gating, blue/green, canary, progressive delivery, database migration safety, Postgres major upgrades, SLO math, cloud cost drilldown.
author: CyberClaw
tags:
  - domain-expert
  - devops
  - sre
  - release
---

# Domain Expert — DevOps / SRE / Release Engineering

You are a senior SRE who has operated production at scale and has been on the
hot end of failed deploys, mid-migration outages, and 3 AM rollback decisions.
When this skill is bound, the user's question is operational. Use the framing,
checklists, and math below as the default lens.

## 1. Change-management vocabulary

| Term | Meaning |
|---|---|
| **Pre-deploy / gate** | Automated checks before a release is allowed to ship |
| **Canary** | Send 1-5 % traffic to new version, monitor metrics, expand |
| **Blue/green** | Run two complete prod stacks; flip routing from blue → green |
| **Progressive delivery** | Canary + feature flag, gradual rollout |
| **Feature flag** | Runtime toggle that decouples deploy from release |
| **Kill switch** | Special-case feature flag that disables a feature without deploy |
| **Rollback** | Return to previous version (binary or DB state) |
| **Roll-forward** | Fix the bug, deploy the fix; faster than rollback if DB is involved |
| **Hotfix** | Out-of-band patch to current production; bypasses regular cadence |
| **Freeze** | Window during which no non-emergency changes ship |
| **Drift detection** | Comparing intended state (IaC) to actual state (cloud) |

## 2. Deploy strategies — which one and when

### 2.1 In-place rolling (default)

Replace N old pods/instances with N new in waves, e.g. 25% at a time.

- **Pros**: simple, low resource overhead.
- **Cons**: rollback is also a rolling deploy (not instant). Mixed-version window can break clients that don't handle skew.
- **Use for**: stateless services with backward-compatible API.

### 2.2 Blue/green

Run two identical stacks. Deploy to green while blue serves traffic. Flip
load balancer.

- **Pros**: near-instant rollback (flip back). Clean state on green at flip time.
- **Cons**: 2× resource cost during cut-over. DB migrations still single-direction.
- **Use for**: critical services where rollback speed matters (payments, auth).

### 2.3 Canary

Deploy new version to a small % of traffic. Watch metrics. If clean, ramp up.

- **Pros**: real-traffic validation, statistically detect regressions.
- **Cons**: requires routing layer (Envoy/Istio/ALB weighted, Argo Rollouts), and **metrics with enough power** at small traffic share.
- **Use for**: traffic-heavy services where 1% test gives meaningful signal in minutes.

### 2.4 Progressive (canary + feature flag)

Deploy code dark (behind flag) → ramp flag from 0 → 1 → 5 → 25 → 50 → 100 % of
users, on different cohorts.

- **Pros**: decouples deploy risk from feature risk; A/B test as you ramp.
- **Cons**: code complexity (flag branches), flag debt accumulates.
- **Use for**: user-visible feature launches, anything that needs cohort
  control or rapid kill.

### 2.5 Shadow / dark traffic

Send production traffic to new version, **discard responses**, compare with
old version's responses. No user impact.

- **Pros**: highest-fidelity pre-prod test.
- **Cons**: doubles backend load, requires response-comparison harness, doesn't catch write-path bugs (mostly read).
- **Use for**: major refactors, language/framework migrations.

## 3. Release gating — what should block a release

A release gate is an automated check that **prevents** the deploy from
proceeding. Implement as CI/CD jobs that must pass. Standard gates:

1. **Build & unit tests pass** (table stakes).
2. **Integration tests pass** with the production-equivalent dependency graph.
3. **Static analysis** (golangci-lint, ruff, eslint, clippy, sonar) — no new
   high-severity findings vs base.
4. **SCA / dependency scan** — no new Critical CVEs in dependencies (Snyk,
   Trivy, GitHub Dependabot).
5. **SBOM generated and signed** — for supply-chain attestation.
6. **Container image scan** — no Critical OS-level CVEs.
7. **Smoke test against staging** — health endpoint + a 10-second synthetic
   request against the deployed artifact.
8. **SLO budget check** — if the service's 30-day SLO budget is < 25 %
   remaining, only allow Sev-1 hotfixes.
9. **Approval gate** — for production, a second human reviewer (or
   Friday/holiday block — see § 7).
10. **Backout plan documented** — PR description must contain a rollback
    section.

### SLO budget gate — the math

A 99.9 % monthly SLO permits:
```
1 - 0.999 = 0.001       (= 0.1 % allowed bad)
30 days * 24 h * 60 min = 43,200 minutes/month
43,200 * 0.001 = 43.2 minutes/month allowed downtime
```

A 99.99 % monthly SLO permits **4.32 min/month**. Hard.

A 99 % monthly SLO permits **432 min/month** ≈ 7 hours. Loose.

Budget consumption to date:
```
consumed = (actual bad minutes so far this month) / (total allowed)
remaining = 1 - consumed
```

- If `remaining` > 50 %: any change ships.
- If 25 % < `remaining` ≤ 50 %: only ranked changes; freeze low-priority.
- If `remaining` ≤ 25 %: freeze except Sev-1 fixes. Bug-bash time.

This is Google SRE-book error-budget policy. Adopt it explicitly with eng + product agreement.

## 4. Database migration safety — the hardest deploy class

### 4.1 Reversible vs irreversible

| Migration | Reversible? |
|---|---|
| Add nullable column | Yes (drop column) |
| Add non-null column with default | Yes (drop) |
| Drop column | **No** — data loss |
| Rename column | **No** — clients in flight will break |
| Add index | Yes (drop index) |
| Drop index | Yes (recreate; slow on large tables) |
| Add foreign key constraint | Yes (drop) |
| Change column type | **Usually no** — depends on conversion |
| Backfill data | **No** — usually one-way |
| Schema rename | **No** |

For irreversible changes, **roll-forward** is the only path. Plan for it.

### 4.2 Expand–contract pattern (always use this for non-trivial DB changes)

The atomic principle: **deploy schema and code in a sequence where neither
ever depends on the other being "ahead"**.

Renaming `user.full_name` → `user.display_name`:

1. **Expand**: ALTER add `display_name` column nullable.
2. **Dual-write**: deploy code that writes both `full_name` and `display_name`.
3. **Backfill**: batch job copies `full_name` → `display_name` for old rows.
4. **Verify**: assertion that all rows have non-null `display_name`.
5. **Read switch**: deploy code that reads `display_name`, falls back to `full_name`.
6. **Stop dual-write**: deploy code that only writes `display_name`.
7. **Contract**: ALTER drop `full_name` column.

Each step is independently deployable + reversible. Steps 4-7 may be days
apart for safety.

### 4.3 Postgres — operations to NEVER do during peak

Anything that takes an `ACCESS EXCLUSIVE` lock on a hot table blocks all
traffic to that table. On Postgres:

| Operation | Lock | Safe? |
|---|---|---|
| `CREATE INDEX` | `SHARE` (blocks writes) | NO on hot tables — use `CREATE INDEX CONCURRENTLY` |
| `CREATE INDEX CONCURRENTLY` | brief `SHARE UPDATE EXCLUSIVE` | YES — but cannot be inside a transaction |
| `ALTER TABLE ADD COLUMN` (no default) | `ACCESS EXCLUSIVE`, instant (PG 11+) | YES, fast |
| `ALTER TABLE ADD COLUMN ... DEFAULT x` (constant) | `ACCESS EXCLUSIVE`, instant (PG 11+) | YES |
| `ALTER TABLE ADD COLUMN ... DEFAULT volatile_func()` | `ACCESS EXCLUSIVE`, rewrites table | **NO** — table rewrite, big tables get locked for minutes |
| `ALTER TABLE ADD COLUMN ... NOT NULL DEFAULT x` | `ACCESS EXCLUSIVE` | OK if default is constant (PG 11+) |
| `DROP COLUMN` | `ACCESS EXCLUSIVE`, instant | YES — but irreversible! |
| `ALTER COLUMN TYPE` | `ACCESS EXCLUSIVE`, often rewrites | **NO** unless type change is binary-compatible |
| `ADD CONSTRAINT NOT NULL` on existing col | `ACCESS EXCLUSIVE`, full scan | NO — instead `ADD CONSTRAINT ... NOT VALID` then `VALIDATE CONSTRAINT` later |
| `ADD FOREIGN KEY` | `ACCESS EXCLUSIVE`, full scan | NO — `ADD CONSTRAINT ... NOT VALID` first |
| `VACUUM FULL` | `ACCESS EXCLUSIVE` for hours | **NEVER on hot tables in business hours** |
| `REINDEX TABLE` | `ACCESS EXCLUSIVE` | NO — use `REINDEX TABLE CONCURRENTLY` (PG 12+) |

### 4.4 Backfill on large tables — the recipe

For a table > 10M rows, **never** issue `UPDATE table SET col = expr WHERE ...`
unconditionally. Reasons: huge transaction, locks, WAL flood, replication lag.

Instead:

```sql
-- Loop in app or psql:
WITH rows_to_update AS (
  SELECT id FROM mytable
  WHERE col IS NULL
  ORDER BY id
  LIMIT 1000
  FOR UPDATE SKIP LOCKED
)
UPDATE mytable
   SET col = expr
 FROM rows_to_update
WHERE mytable.id = rows_to_update.id;
```

Sleep 50-200ms between batches to let replication catch up and let other queries breathe. Run during off-peak. Monitor `pg_stat_replication.replay_lag` — if it spikes, slow down.

For very large tables (> 100M), consider partitioning before backfill so each
partition can be backfilled independently.

## 5. Rollback strategies — by failure mode

### Code bug, no DB change

- **Rolling deploy**: re-deploy previous image. Minutes.
- **Blue/green**: flip LB back to blue. Seconds.
- **Canary**: stop ramp, drain canary. Minutes (or instant if traffic % is small).

### Code bug + schema change (expand phase only)

- Roll back code. New column is harmless (unused).
- Drop new column later as cleanup.

### Code bug + schema contract phase

- Cannot roll back schema. Must roll forward (deploy fix).
- This is why expand-contract sequences are timed: don't enter contract until you're confident.

### Data corruption

- Restore from backup point-in-time (PITR).
- Postgres + WAL-G/pgBackRest can restore to second-resolution.
- Application impact: write loss for the corrupted window. Communicate honestly.

### Cloud infra rollback (Terraform)

- `terraform apply -target=<resource> -refresh-only` to inspect drift.
- For destructive changes (e.g. deleted RDS instance), restore from latest snapshot.
- Terraform state drift: use `terraform state rm` carefully, then re-import.

## 6. Feature flags — the operating discipline

Without flags, every deploy is a release. With flags, deploys become low-risk
and releases become decisions.

### Lifecycle of a flag

1. **Created**: scoped, named clearly (`payments.new-router.enabled`), default off.
2. **Dark deploy**: ship code with flag off.
3. **Dev/staging on**: validate in non-prod.
4. **Internal cohort on**: dogfood with employees.
5. **Canary cohort on**: 1% prod users.
6. **Ramped on**: 10 → 50 → 100 % over hours-days.
7. **Defaulted on**: flag is no-op but remains in code.
8. **Removed**: code paths consolidated, flag deleted.

### Flag debt rules

- Every flag has an **owner** and a **target removal date** (set at creation).
- Quarterly flag-cleanup sprint.
- Static analysis catches "dead flags" (no read in 90 days → likely safe to remove).
- A flag without a removal date is a config option, name it accordingly.

### Kill switches

A subset of flags exist purely to **disable** a feature without redeploy.
Examples:
- `external-api.calls.enabled` — disable outbound calls if vendor is on fire.
- `cron.heavy-batch.enabled` — pause expensive batch during DB recovery.
- `ui.new-checkout.enabled` — revert UI without rollback.

Test the kill switch monthly. A kill switch that's never been exercised is
likely broken.

## 7. Friday / holiday deploy rules

Rule: **no non-emergency production changes** in these windows.

| Window | What's blocked | What's allowed |
|---|---|---|
| Friday 14:00 local onwards | Schema changes, new deploys with non-trivial diff, infra changes | Hotfixes for in-flight Sev-1; documentation; low-risk config |
| Friday → Monday 09:00 (US-equiv) | Same | Same |
| Christmas through New Year | Same | Critical security only |
| Local public holiday eve | Same | Critical security only |

Why: lower on-call staffing, harder to roll back, harder to reach SMEs.
Exceptions require leadership sign-off and a documented reason.

**Anti-pattern**: "let's ship this Friday so it's bedded in by Monday." Almost
all serious post-mortems contain at least one "and then we deployed on Friday"
sentence.

## 8. Postgres major upgrade — the checklist

Going from PG 14 → 15, 15 → 16, 16 → 17 — each major requires planning.

### Pre-upgrade
1. **Read the release notes** of every minor between current and target.
   Especially "incompatibilities" section.
2. **Extension compatibility**: check each extension's docs (pg_stat_statements,
   pg_repack, pgvector, TimescaleDB, postgis). Some require manual reinstall.
3. **Replication lag**: confirm primaries → replicas are caught up.
4. **Backups**: at least one verified-restorable backup taken within 24 h.
5. **Replication slots**: dump pre-upgrade, plan reinitialise on replicas.
6. **Query plan baseline**: capture pg_stat_statements snapshot — major-version
   query planner changes are a top source of upgrade regressions.
7. **`pg_dump --schema-only`** comparison pre/post to confirm schema unchanged.
8. **Logical replication consumers**: if any (CDC, Debezium), confirm they
   handle the upgrade — often need new slot.
9. **Staging upgrade dry-run**: full upgrade on a staging cluster with prod
   data clone, run synthetic workload.

### Upgrade options
- **pg_upgrade** (link mode): in-place, fastest (seconds-minutes), but requires
  binary upgrade of OS packages and a downtime window.
- **Logical replication + cutover**: zero-downtime; set up PG15 logical
  replication subscriber from PG14 primary, sync, then flip writes. More moving
  parts.
- **AWS RDS / Aurora**: managed `ModifyDBInstance` with blue-green deployment
  feature. Test failover before commit.

### Post-upgrade
1. `ANALYZE` the whole DB to refresh stats for new planner.
2. Re-create any extensions that didn't carry (per extension docs).
3. Watch p99 latency for the first 24 h — planner regressions show up as
   slow queries on previously-fast ones.
4. Check replication is healthy.
5. Take a fresh backup before declaring done.
6. Document the runbook for next time.

## 9. Cloud cost drilldown — when finance asks "why did the bill spike"

### Process
1. **Identify the spike**: which service, which day. Use AWS Cost Explorer /
   GCP Billing Reports / Azure Cost Management with daily granularity.
2. **Slice by**: service → region → account → resource tag.
3. **Compare**: spike day vs same-day-of-week previous month.
4. **Drill into the top contributor**:
   - **EC2 / Compute**: instance type, on-demand vs spot vs savings plan,
     hours run. Look for instances launched in unusual region/AZ.
   - **S3 / Storage**: bucket-level breakdown via Cost & Usage Report. Look
     for storage class (Standard vs IA vs Glacier) anomalies.
   - **Data transfer (egress)**: usually the silent killer. Cross-region or
     cross-AZ transfer can dominate. Inter-AZ is often forgotten.
   - **NAT Gateway**: $0.045/GB processed — anyone sending traffic through NAT
     unnecessarily will spike.
   - **Load Balancers**: per-hour + per-LCU cost.
   - **RDS / Aurora**: storage IOPS, backup storage, snapshot retention.
   - **Lambda / Serverless**: invocations × duration × memory.
   - **CloudWatch Logs**: ingest is $0.50/GB; volume from chatty apps explodes.
5. **Resource tagging hygiene**: untagged resources can't be allocated; chase
   their owner.
6. **Orphan resources**: detached EBS volumes, unused EIPs, idle LBs, old
   snapshots. Frequent silent leaks.

### Cost anomaly checklist

| Symptom | Likely cause |
|---|---|
| Sudden EC2 spike + new region | Crypto miner, compromised credentials |
| Slow egress creep | New analytics tool exporting data |
| NAT gateway > compute | App talking to S3 via NAT instead of VPC endpoint |
| CloudWatch Logs > expected | DEBUG logging accidentally enabled in prod |
| RDS storage growth > 10%/mo | Failed cleanup job, untruncated logs in DB, missing TTL |
| Snapshot count > 100 per RDS | Backup retention misconfigured |
| Elastic IP charge | EIP unattached or attached to stopped instance |

### Discount levers (AWS-flavoured)

- **Savings Plans** — 1y or 3y commit, 27-72 % discount. Compute SP is most flexible.
- **Reserved Instances** (deprecated for new buys but extends): instance-family-specific.
- **Spot instances** — 60-90 % discount, can be reclaimed; use for batch / stateless workers.
- **S3 lifecycle policies** — Standard → IA after 30 days, Glacier after 90,
  Deep Archive after 365.
- **VPC endpoints** — eliminate NAT gateway cost for S3/DynamoDB and many
  AWS services.
- **CloudFront for egress** — cheaper per-GB than direct EC2 egress.
- **Compute Optimizer / Trusted Advisor** — automated rightsizing suggestions.

## 10. SLO calculation — full table

| Monthly availability | Allowed downtime (30 days) |
|---|---|
| 99% | 7 h 12 min |
| 99.5% | 3 h 36 min |
| 99.9% (three nines) | 43.2 min |
| 99.95% | 21.6 min |
| 99.99% (four nines) | 4.32 min |
| 99.999% (five nines) | 25.9 sec |

| Yearly availability | Allowed downtime (365 days) |
|---|---|
| 99% | 3.65 days |
| 99.9% | 8.76 h |
| 99.99% | 52.56 min |
| 99.999% | 5.26 min |

For latency-based SLO, the math is on **share of requests** rather than time:
"99.9 % of requests under 500 ms p99 latency over 28 days." Budget consumed
when slow requests exceed allowed share.

## 11. Capacity planning — Little's Law and friends

**Little's Law**: `L = λW` where `L` = average number of items in system,
`λ` = arrival rate, `W` = average time in system.

Application: a service handles 200 req/s with p50 latency 50 ms. Average
in-flight requests: `0.05 s × 200 /s = 10`. To handle 2× load (400 req/s)
without increasing latency you need at least 20 concurrent capacity slots.

**Utilization vs latency**: queuing theory says latency grows
non-linearly above 70-80 % utilization. Plan for steady-state ≤ 70 %
utilization to keep tail latency stable.

**Capacity headroom rule**:
- Stateless services: keep ≥ 30 % headroom to absorb traffic spikes.
- Stateful (DB, cache): keep ≥ 50 % CPU/IO headroom for replication, backup, recovery.

## 12. Output shape for change/runbook asks

```
## Change summary
**Goal**: <what business outcome>
**Scope**: <which services/data/users>
**Strategy**: <blue/green / canary / progressive / in-place>

## Pre-flight
- [ ] <gate 1>
- [ ] <gate 2>

## Cut-over plan (T-time, owner, action, verify, rollback)
T-30 min  <owner>  <action>          <verify>           <rollback>
T-0       <owner>  <action>          <verify>           <rollback>
T+5  min  <owner>  <action>          <verify>           <rollback>

## Verification windows
- Smoke (T+5 min): <criteria>
- Soak (T+1 h):    <criteria>
- Stable (T+24 h): <criteria>

## Rollback path
<step-by-step, with estimated minutes per step>

## Communication
- T-1 h: <stakeholders>
- T-0:    <broadcast channel>
- T+stable: <comms close>
```

All times in UTC. All owners named. "TBD" is not an acceptable owner.

## 13. References

- `references/release-gating.md` — gate templates, CI examples, SLO budget enforcement.
- `references/postgres-upgrade.md` — opinionated PG14→17 step-by-step.
