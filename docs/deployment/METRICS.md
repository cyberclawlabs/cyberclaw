# CyberClaw Prometheus Metrics

CyberClaw exposes a Prometheus text format endpoint at `GET /metrics` (version 0.0.4).

## Scrape Configuration

```yaml
# prometheus.yml
scrape_configs:
  - job_name: cyberclaw
    static_configs:
      - targets: ["<host>:38090"]
    scrape_interval: 15s
    metrics_path: /metrics
```

The endpoint is **public** (no JWT required). Restrict access at the reverse-proxy or firewall layer using IP allowlists.

## Metrics Reference

### Counters

| Metric | Labels | Description |
|---|---|---|
| `cyberclaw_execution_total` | `status` | Total executions (status = Completed \| Failed \| Cancelled \| …) |
| `cyberclaw_capability_invocation_total` | `status` | Total capability invocations (status = success \| failure) |
| `cyberclaw_agent_execution_total` | `status` | Total agent executions |
| `cyberclaw_skill_invocation_total` | `status` | Total skill invocations |
| `cyberclaw_retry_attempts_total` | `operation` | Transient failures that triggered a retry |
| `cyberclaw_retry_exhausted_total` | `operation` | Operations where all retries were exhausted |
| `cyberclaw_execution_state` | `state` | Execution state transitions (cumulative) |
| `cyberclaw_tenant_execution_total` | `tenant_id`, `status` | Executions per tenant |
| `cyberclaw_tenant_capability_invocation_total` | `tenant_id`, `status` | Capability invocations per tenant |
| `cyberclaw_tenant_active_executions` | `tenant_id` | Active execution activity per tenant |
| `cyberclaw_tenant_review_total` | `tenant_id`, `review_type` | Review requests per tenant |
| `cyberclaw_tenant_governance_decision_total` | `tenant_id`, `decision_type` | Governance decisions per tenant |

### Histograms

| Metric | Labels | Buckets (seconds) | Description |
|---|---|---|---|
| `cyberclaw_execution_duration_seconds` | `status` | 0.1, 0.5, 1, 5, 10, 30, 60, 120, 300 | End-to-end execution time |
| `cyberclaw_capability_invocation_duration_seconds` | `status` | 0.05, 0.1, 0.5, 1, 5, 10 | Capability call latency |
| `cyberclaw_review_wait_seconds` | `risk_level` | 1, 5, 30, 60, 300, 600, 1800, 3600 | Human review wait time |
| `cyberclaw_tenant_execution_duration_seconds` | `tenant_id`, `status` | 0.1, 0.5, 1, 5, 10, 30, 60, 120, 300 | Execution time per tenant |

### Gauges

| Metric | Labels | Description |
|---|---|---|
| `cyberclaw_review_queue_size` | — | Current pending review count |
| `cyberclaw_execution_success_rate` | — | Rolling success rate (0.0–1.0) |

## Label Cardinality Notes

- `execution_id`, `user_id`, and `capability_id` are **not** used as labels to prevent cardinality explosion.
- `tenant_id` is a bounded dimension — safe as a label in controlled deployments.
- `operation` in retry metrics is bounded by the set of named operation strings in code.

## Sample Output

```
# HELP cyberclaw_execution_total Total number of executions by status
# TYPE cyberclaw_execution_total counter
cyberclaw_execution_total{status="Completed"} 142
cyberclaw_execution_total{status="Failed"} 3

# HELP cyberclaw_execution_duration_seconds Execution duration in seconds
# TYPE cyberclaw_execution_duration_seconds histogram
cyberclaw_execution_duration_seconds_bucket{status="Completed",le="1"} 98
cyberclaw_execution_duration_seconds_bucket{status="Completed",le="5"} 130
cyberclaw_execution_duration_seconds_sum{status="Completed"} 310.4
cyberclaw_execution_duration_seconds_count{status="Completed"} 142

# HELP cyberclaw_review_queue_size Current number of pending reviews
# TYPE cyberclaw_review_queue_size gauge
cyberclaw_review_queue_size 2
```

## Grafana Dashboard Panels

### Panel 1 — Execution Rate

```promql
rate(cyberclaw_execution_total{status="Completed"}[5m])
```

Visualization: Time series. Shows completed executions per second.

### Panel 2 — Execution Error Rate

```promql
rate(cyberclaw_execution_total{status="Failed"}[5m])
/
(rate(cyberclaw_execution_total[5m]) > 0)
```

Visualization: Time series (0–1 range). Alert threshold: > 0.05 (5% error rate).

### Panel 3 — P95 Execution Latency

```promql
histogram_quantile(0.95,
  rate(cyberclaw_execution_duration_seconds_bucket[5m])
)
```

Visualization: Time series. Alert threshold: > 30s.

### Panel 4 — Review Queue Depth

```promql
cyberclaw_review_queue_size
```

Visualization: Stat / gauge. Alert threshold: > 50 pending reviews.

## Alerting Examples

```yaml
# alerting_rules.yml
groups:
  - name: cyberclaw
    rules:
      - alert: HighExecutionErrorRate
        expr: |
          rate(cyberclaw_execution_total{status="Failed"}[5m])
          / rate(cyberclaw_execution_total[5m]) > 0.05
        for: 2m
        labels:
          severity: warning
        annotations:
          summary: "Execution error rate above 5%"

      - alert: ReviewQueueBacklog
        expr: cyberclaw_review_queue_size > 50
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "Review queue has {{ $value }} pending items"

      - alert: SlowExecutions
        expr: |
          histogram_quantile(0.95,
            rate(cyberclaw_execution_duration_seconds_bucket[5m])
          ) > 30
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "P95 execution latency above 30s"

      - alert: RetryExhaustion
        expr: rate(cyberclaw_retry_exhausted_total[5m]) > 0
        for: 1m
        labels:
          severity: critical
        annotations:
          summary: "Retry exhaustion detected for operation {{ $labels.operation }}"
```
