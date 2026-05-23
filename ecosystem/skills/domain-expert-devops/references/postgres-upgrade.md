# Postgres Major Upgrade Checklist

Going across a major boundary (14 → 15, 15 → 16, 16 → 17, etc.) is a planned
event. This document is the canonical checklist; treat divergences as risks
to be specifically justified.

## Phase 0 — Plan (T - 4 weeks)

- [ ] Read every release-notes entry between current and target version.
      Specifically scan: "Backward Incompatibilities", "Migration to ..." sections.
- [ ] Inventory installed extensions: `SELECT * FROM pg_extension;`. For each,
      verify target-version compatibility (vendor docs).
- [ ] Inventory queries via `pg_stat_statements`. Save baseline:
      `SELECT * FROM pg_stat_statements ORDER BY total_exec_time DESC LIMIT 200;`
- [ ] Inventory replication: physical standbys, logical replication slots,
      cdc/Debezium consumers.
- [ ] Inventory clients: list of services that connect; for each, the driver
      version. Ensure driver supports target server version.
- [ ] Identify upgrade method:
  - Single instance, accept downtime: `pg_upgrade`.
  - Zero-downtime: logical replication + cutover.
  - Cloud-managed (RDS / Aurora / Cloud SQL): use provider's blue-green or
    snapshot+restore path.
- [ ] Estimate downtime window for chosen method. Get business sign-off.
- [ ] Identify rollback path. **There is no in-place downgrade.** Rollback =
      restore the pre-upgrade backup.

## Phase 1 — Staging dry-run (T - 2 weeks)

- [ ] Provision staging cluster with prod-size data clone (or production data
      copy on isolated infra).
- [ ] Run full upgrade procedure end-to-end on staging.
- [ ] Time each step. Record actual durations.
- [ ] Re-create extensions per vendor docs. Verify each.
- [ ] Run `ANALYZE` on whole database.
- [ ] Run synthetic workload (production-realistic queries) and capture
      `pg_stat_statements` for the new version.
- [ ] Diff query plans for top 50 expensive queries pre vs post. Investigate
      any with > 20 % cost or latency change.
- [ ] Test rollback path: restore the pre-upgrade backup, verify clients can
      connect, verify data integrity.

## Phase 2 — Pre-flight (T - 1 day)

- [ ] Take a fresh verified backup. Verify by restoring to a scratch cluster
      and running an integrity check (`pg_dumpall --schema-only` matches).
- [ ] Confirm replication lag: `SELECT * FROM pg_stat_replication;` —
      `replay_lag` should be near zero on all standbys.
- [ ] Confirm long-running transactions: `SELECT pid, query_start, query FROM
      pg_stat_activity WHERE state = 'active' AND query_start < now() - interval '5 minutes';`
      — kill or let finish before upgrade.
- [ ] Snapshot `pg_stat_statements`, `pg_stat_all_tables`, `pg_stat_user_indexes`
      as final baseline.
- [ ] Notify all stakeholders of upgrade window. Confirm on-call team is staffed.
- [ ] Pause non-critical writers (batch jobs, ETL) — reduces post-upgrade
      replication catch-up.

## Phase 3 — Upgrade (T zero)

### pg_upgrade route

```bash
# 1. Stop old PG
sudo systemctl stop postgresql-14

# 2. Install new PG binaries (parallel install)
sudo apt install postgresql-17 postgresql-contrib-17

# 3. Run pg_upgrade in --check mode first
sudo -u postgres /usr/lib/postgresql/17/bin/pg_upgrade \
    --old-datadir=/var/lib/postgresql/14/main \
    --new-datadir=/var/lib/postgresql/17/main \
    --old-bindir=/usr/lib/postgresql/14/bin \
    --new-bindir=/usr/lib/postgresql/17/bin \
    --check

# 4. If check passes, run for real with --link (fastest; uses hardlinks)
sudo -u postgres /usr/lib/postgresql/17/bin/pg_upgrade \
    --old-datadir=/var/lib/postgresql/14/main \
    --new-datadir=/var/lib/postgresql/17/main \
    --old-bindir=/usr/lib/postgresql/14/bin \
    --new-bindir=/usr/lib/postgresql/17/bin \
    --link

# 5. Start new PG
sudo systemctl start postgresql-17

# 6. Run the analyze-in-stages script produced by pg_upgrade
sudo -u postgres /usr/lib/postgresql/17/bin/vacuumdb \
    --all --analyze-in-stages

# 7. Once happy, run cleanup script
sudo -u postgres ./delete_old_cluster.sh
```

Notes:
- `--link` makes the upgrade nearly instant (no data copy) but the old datadir
  is destroyed in the process — rollback requires restoring from backup.
- For databases > 100 GB and downtime ≤ 5 min: `--link` is the only realistic
  option.

### Logical replication route (zero-downtime)

1. Provision new PG17 cluster.
2. On PG14 primary: `CREATE PUBLICATION upgrade_pub FOR ALL TABLES;`
3. On PG17: schema-only dump from PG14, apply.
4. On PG17: `CREATE SUBSCRIPTION upgrade_sub CONNECTION '...' PUBLICATION upgrade_pub;`
5. Wait for initial sync. Monitor with `pg_stat_subscription`.
6. Once `lag = 0`, cut over:
   - Stop writes to PG14 (app-level freeze).
   - Wait for PG17 to catch up final batch.
   - Update connection strings to point at PG17.
   - Re-enable writes.
7. After cut-over, set up logical replication PG17 → PG14 in case of urgent
   rollback (optional but advised for the first 24 h).
8. Drop subscription/publication after confidence period.

Caveats:
- Logical replication does not replicate sequences. After cut-over, you must
  bump every sequence on PG17 to current value: `SELECT setval('schema.seq',
  (SELECT MAX(id) FROM schema.tbl));`
- Large tables can take days for initial copy. Test in staging.
- `pg_largeobject`, DDL, certain types not replicated — confirm none of these
  are critical to your schema.

## Phase 4 — Post-upgrade (T + 1 h)

- [ ] Re-install extensions:
  - `pg_stat_statements` — `ALTER EXTENSION pg_stat_statements UPDATE;`
  - `pgvector`, `postgis`, `timescaledb` — per vendor `ALTER EXTENSION ...
    UPDATE TO 'X.Y';`
  - Some extensions require `DROP` + `CREATE` rather than `UPDATE` —
    check vendor docs.
- [ ] Run `ANALYZE` (already done if you used `vacuumdb --analyze-in-stages`).
- [ ] Verify clients reconnect cleanly. Check error logs.
- [ ] Compare `pg_stat_statements` top queries vs pre-upgrade snapshot.
      Specifically watch: queries whose plan changed (look at `query_id`
      stability). Investigate any with > 20 % latency regression.
- [ ] Re-establish replication to standbys (physical replication standbys must
      be re-baselined from new primary).
- [ ] Re-establish logical replication consumers (Debezium, CDC).
- [ ] Take a fresh backup.

## Phase 5 — Watch (T + 24 h to T + 7 d)

- Monitor p99 latency on the top 20 queries.
- Monitor connection pool: PG17 default `max_connections` may differ; verify
  your pool sizing is appropriate.
- Monitor autovacuum activity. Major version may change autovacuum thresholds
  / cost limits.
- Monitor disk usage. `pg_upgrade --link` doesn't immediately free old datadir;
  ensure cleanup ran.
- Document any anomalies for the next upgrade.

## Common gotchas (version-specific)

### PG 14 → 15
- Hash partitioning improvements; verify your partitioned tables.
- `extra_float_digits` default change from 1 to 0 — verify any clients relying
  on textual float precision.
- New SQL features (MERGE) — no migration risk, but available.

### PG 15 → 16
- `pg_stats_ext` improvements (extended statistics); planner may pick different
  plans for queries that use multi-column dependencies.
- Logical replication of partitioned tables now supported (was previously
  per-partition).
- `pg_stat_io` new view — re-baseline IO monitoring dashboards.

### PG 16 → 17
- `pg_combinebackup` for incremental backup workflows; revise backup strategy.
- VACUUM speed improvements (~50 % faster on large tables); autovacuum cost
  defaults may be re-tunable.
- Logical replication failover support — review your slot positions.
- COPY ... ON_ERROR — useful for batch ingestion; no migration risk.

### Cross-version (all upgrades)
- **Collation changes**: glibc / ICU collation library updates between OS
  versions can change sort order. Re-build indexes on string columns:
  `REINDEX TABLE CONCURRENTLY ...`. This is the #1 silent data-integrity hazard
  on upgrades that also bump OS.
- **Time zone data**: confirm `pg_timezone_names` is current.

## Rollback plan

Already executed pg_upgrade `--link`? Cluster files are mixed; rollback = restore from backup.

1. Stop new PG (PG17).
2. `mv /var/lib/postgresql/17/main /var/lib/postgresql/17/main.failed`.
3. Restore PG14 datadir from backup.
4. Start PG14 (still installed in parallel).
5. Update connection strings if you changed them.
6. Communicate restored state to stakeholders.

Time-to-restore depends on backup format:
- pg_basebackup full + WAL: ~10-30 min for typical DB.
- Cloud snapshot restore: 30 min to several hours.
- Logical dump restore (pg_dumpall): hours for large DBs.

Plan accordingly. If 60 min rollback is unacceptable, use logical-replication
upgrade route (which keeps PG14 hot during the upgrade window).
