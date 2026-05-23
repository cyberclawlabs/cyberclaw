# Release Gating — Templates

## Gate as code (GitHub Actions example)

```yaml
name: release-gate
on:
  push:
    tags: ['v*.*.*']
jobs:
  gate:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Build
        run: make build

      - name: Unit tests
        run: make test-unit

      - name: Integration tests
        run: make test-integ
        timeout-minutes: 30

      - name: Lint
        run: make lint
        # exit 1 on warnings — set in tooling config

      - name: SBOM
        run: syft packages dir:. -o cyclonedx-json > sbom.json

      - name: Dependency scan
        run: trivy fs --severity HIGH,CRITICAL --exit-code 1 .

      - name: Container scan
        run: trivy image --severity CRITICAL --exit-code 1 ghcr.io/org/svc:${{ github.ref_name }}

      - name: Smoke staging
        run: ./scripts/smoke.sh staging

      - name: Check SLO budget
        run: ./scripts/slo-budget-gate.sh svc-name 25

      - name: Approval check
        if: github.ref == 'refs/heads/main'
        run: ./scripts/require-approval.sh
```

## SLO budget gate (pseudocode)

```bash
#!/usr/bin/env bash
# Args: $1 service name, $2 min remaining %
service=$1
min_remaining=${2:-25}

remaining=$(curl -sf "${SLO_API}/svc/${service}/budget/remaining-pct")
if (( $(echo "$remaining < $min_remaining" | bc -l) )); then
  echo "SLO budget for $service is $remaining%; gate requires >= ${min_remaining}%"
  exit 1
fi
echo "SLO budget OK ($remaining%)"
```

## Friday-block gate

```bash
#!/usr/bin/env bash
# Block prod deploys Friday afternoon and weekends, US/Pacific time
tz="America/Los_Angeles"
dow=$(TZ=$tz date +%u)   # 1=Mon ... 7=Sun
hour=$(TZ=$tz date +%H)

if [[ "$dow" -eq 6 || "$dow" -eq 7 ]]; then
  echo "Weekend deploy blocked. Need leadership override (set OVERRIDE_FRIDAY=1)."
  [[ -z "$OVERRIDE_FRIDAY" ]] && exit 1
elif [[ "$dow" -eq 5 && "$hour" -ge 14 ]]; then
  echo "Friday afternoon deploy blocked. Need leadership override."
  [[ -z "$OVERRIDE_FRIDAY" ]] && exit 1
fi
echo "Deploy window OK."
```

## Required PR fields

A PR title must follow `<scope>: <verb> <object>` (conventional-commits).

A PR description must contain (template-checked):

```
## Summary
<one-paragraph what + why>

## Risk
- [ ] No DB schema change
- [ ] DB schema change is expand-only
- [ ] DB schema change is contract — see expand-contract sequence link
- [ ] Stateful migration (data backfill)
- [ ] Behavior change behind feature flag X
- [ ] Breaking API change (versioned)

## Rollback
<concrete steps to undo>

## Verification
<how reviewer / on-call confirms it's working in prod>
```

## Canary metric checks (Argo Rollouts example)

```yaml
apiVersion: argoproj.io/v1alpha1
kind: AnalysisTemplate
metadata:
  name: success-rate
spec:
  args:
  - name: service-name
  metrics:
  - name: success-rate
    interval: 30s
    successCondition: result[0] >= 0.99
    failureLimit: 3
    provider:
      prometheus:
        address: http://prometheus.monitoring:9090
        query: |
          sum(rate(http_requests_total{
            service="{{args.service-name}}",
            code!~"5.."
          }[2m])) / sum(rate(http_requests_total{
            service="{{args.service-name}}"
          }[2m]))
```

Canary auto-aborts if success rate drops below 99 % across three windows.

## Decision matrix — gate severity

| Failure type | Block ship? | Page on-call? |
|---|---|---|
| Unit test fail | Yes | No |
| Integration test fail | Yes | No |
| Lint warning | No (PR comment) | No |
| New high-severity CVE in dep | Yes | No (file ticket) |
| Container image fails CIS scan | Yes | No |
| Staging smoke fail | Yes | Yes if recurring |
| SLO budget < threshold | Yes (except Sev-1) | No (it's a planning signal) |
| Reviewer not assigned | Yes | No |
| Friday afternoon | Yes (override possible) | No |

## Backout-plan completeness checklist

Reviewer checks the PR's "Rollback" section answers:

1. **What command/action reverts this?** (Exact CLI or PR revert link.)
2. **What is the time-to-revert estimate?**
3. **What data loss (if any) accompanies revert?**
4. **What is the impact on in-flight requests during revert?**
5. **What signal tells us we need to revert?**
6. **Who is authorised to call it?** (Service owner + on-call.)

If any answer is "n/a" without justification, the PR is not ready.
