# CyberClaw CHAOS Runbook

Operator guide for controlled failure injection and system recovery validation.

## Purpose

Verify the system degrades gracefully under realistic failure conditions and
that operators can detect, contain, and recover within the stated SLOs.

Run this runbook in staging before every major release and after significant
infrastructure changes.

---

## Preconditions

1. Staging environment running with `CYBERCLAW_ENV=staging`
2. Admin JWT in `~/.cyberclaw/admin.jwt`
3. At least one skill under `~/.cyberclaw/ecosystem/skills/`
4. Audit log present at `~/.cyberclaw/audit.db`

```bash
export JWT=$(cat ~/.cyberclaw/admin.jwt)
export BASE=https://staging.cyberclaw.local
```

---

## Scenario 1 — Server restart under active chat

**Goal:** Confirm in-memory org-memory store survives a restart (or degrades
cleanly when it doesn't — in-memory store is intentionally ephemeral).

```bash
# 1. Seed an org-memory entry
curl -s -X POST $BASE/api/v1/learning/org-memory \
  -H "Authorization: Bearer $JWT" \
  -H "Content-Type: application/json" \
  -d '{"kind":"rule","content":"test rule for chaos"}'

# 2. Kill server
kill $(pgrep cyberclaw-server)

# 3. Restart server
cyberclaw-server &

# 4. Verify: empty (expected — in-memory store) or populated if a persistent
#    store has been wired.
curl -s $BASE/api/v1/learning/org-memory | jq '.entries | length'
```

**Expected:** Server restarts in < 5s. Health endpoint returns 200 within 10s.
In-memory store loss is documented behaviour, not a bug.

---

## Scenario 2 — Audit DB corruption detection

**Goal:** Confirm `verify-chain` detects and reports corruption.

```bash
# 1. Archive current audit
cyberclaw audit archive

# 2. Corrupt one byte in the DB (staging only — never production)
python3 -c "
import sys, struct
with open('$HOME/.cyberclaw/audit.db', 'r+b') as f:
    f.seek(1024)
    f.write(b'\\xff')
"

# 3. Run verify-chain — must report corruption
cyberclaw audit verify-chain $HOME/.cyberclaw/audit.db

# 4. Restore from archive
cyberclaw audit restore --yes $(cyberclaw audit list | head -1 | awk '{print $1}')

# 5. Re-verify — must be clean
cyberclaw audit verify-chain $HOME/.cyberclaw/audit.db
```

**Expected:** Step 3 exits non-zero and prints `corrupted_at: <row>`.
Step 5 prints `corrupted_at: null`.

---

## Scenario 3 — LLM client unavailable (evolution run)

**Goal:** Confirm evolution cycles fail cleanly and log a terminal
`failed` outcome rather than hanging.

```bash
# 1. Point to a non-existent LLM endpoint
export CYBERCLAW_LLM_ENDPOINT=http://127.0.0.1:19999

# 2. Trigger an evolution run
CYCLE=$(curl -s -X POST $BASE/api/v1/learning/evolution/run \
  -H "Authorization: Bearer $JWT" \
  -H "Content-Type: application/json" \
  -d '{
    "skill_id": "sk_chaos_test",
    "target_skill_path": "'"$HOME"'/.cyberclaw/ecosystem/skills/first-available",
    "trigger_dataset": [{"query":"test","should_trigger":true}],
    "max_iterations": 1
  }' | jq -r .cycle_id)

echo "Cycle: $CYCLE"

# 3. Wait for terminal outcome (up to 30s)
for i in $(seq 1 30); do
  sleep 1
  if grep -q "$CYCLE" ~/.cyberclaw/evolution.log 2>/dev/null; then
    tail -n1 ~/.cyberclaw/evolution.log | jq '{outcome, meta}'
    break
  fi
done
```

**Expected:** Outcome is `failed`. `meta.reason` contains a network or
connection-refused error. No goroutine / thread hangs. Cycle completes within
`max_iterations × 10s` wall time.

---

## Scenario 4 — PolicyEngine blocks high-risk capability

**Goal:** Confirm governance blocks a high-risk call when threshold is set to
`Critical` (auto-approves everything below that).

```bash
# 1. Set threshold to Critical (auto-approves Low/Medium/High; blocks Critical)
export CYBERCLAW_POLICY_REVIEW_THRESHOLD=Critical

# 2. Trigger a capability call that maps to Critical severity
# (Use a test agent with mkfs.* in its requested capabilities)
# Check the governance log for a "denied" decision:
curl -s "$BASE/api/v1/security/permission/rules" \
  -H "Authorization: Bearer $JWT" | jq '.rules | map(select(.action=="DENY"))'

# 3. Attempt to invoke the blocked capability via a task submission
# Expected: 403 or queued-for-review response
```

**Expected:** Capability call is blocked or queued. Audit log records a
`security` entry with `result: Failure`. No bypass observed.

---

## Scenario 5 — Memory pressure (large org-memory store)

**Goal:** Confirm list endpoint degrades gracefully with a large store.

```bash
# 1. Bulk-seed 500 entries
for i in $(seq 1 500); do
  curl -s -X POST $BASE/api/v1/learning/org-memory \
    -H "Authorization: Bearer $JWT" \
    -H "Content-Type: application/json" \
    -d "{\"kind\":\"free_text\",\"content\":\"chaos entry $i\"}" > /dev/null
done

# 2. Time the list call
time curl -s "$BASE/api/v1/learning/org-memory?limit=500" \
  -H "Authorization: Bearer $JWT" | jq '.entries | length'
```

**Expected:** Response time < 500ms. Server does not OOM. `limit` cap (500)
is respected.

---

## Scenario 6 — Concurrent evolution runs (same skill)

**Goal:** Confirm two simultaneous evolution runs on the same skill do not
corrupt each other's log output.

```bash
SKILL_DIR="$HOME/.cyberclaw/ecosystem/skills/test-skill"
mkdir -p "$SKILL_DIR"
echo -e "---\nname: chaos-test\ndescription: chaos\n---\nbody" > "$SKILL_DIR/SKILL.md"

for i in 1 2; do
  curl -s -X POST $BASE/api/v1/learning/evolution/run \
    -H "Authorization: Bearer $JWT" \
    -H "Content-Type: application/json" \
    -d "{\"skill_id\":\"sk_chaos_$i\",\"target_skill_path\":\"$SKILL_DIR\",\"trigger_dataset\":[{\"query\":\"test\",\"should_trigger\":true}],\"max_iterations\":1}" &
done
wait

sleep 10
echo "Log entries written:"
wc -l ~/.cyberclaw/evolution.log
```

**Expected:** Two separate cycle_ids appear in the log. Each has a valid JSON
line. No JSON parse errors in the log.

**Known caveat:** No per-skill concurrency limit is enforced (documented in
`EVOLUTION_RUN_RUNBOOK.md §Known caveats`). Two parallel optimizations on the
same skill is accepted behaviour for now.

---

## Recovery decision tree

| Observation | Action |
|---|---|
| Server does not restart within 10s | Check `journalctl -u cyberclaw-server` for panic backtrace |
| Audit verify-chain fails post-restore | Run `cyberclaw audit list` and restore from the next-oldest snapshot |
| Evolution cycle never writes log | Check `CYBERCLAW_EVOLUTION_LOG` env; verify disk space |
| PolicyEngine not blocking | Verify `CYBERCLAW_POLICY_REVIEW_THRESHOLD` is set at process start |
| Memory pressure OOM | Reduce `?limit=` cap; add OS-level cgroup memory limit |

---

## Verification gate

```bash
cargo test -p cyberclaw-server --test e2e_evolution_test
cargo test -p cyberclaw-server -- audit --nocapture
```

Both must pass before signing off on a CHAOS runbook run.

## See also

- [`docs/guides/EVOLUTION_RUN_RUNBOOK.md`](EVOLUTION_RUN_RUNBOOK.md) — evolution endpoint operator guide
- [`apps/cyberclaw-server/src/audit.rs`](../../apps/cyberclaw-server/src/audit.rs) — audit hash-chain implementation
- [`apps/cyberclaw-cli/src/commands/audit.rs`](../../apps/cyberclaw-cli/src/commands/audit.rs) — CLI subcommands
