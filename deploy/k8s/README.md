# CyberClaw on Kubernetes

Production deployment manifests for cyberclaw-server. Sprint 20 W1 — first
shipped K8s wiring; previously the only deployment surface was the
single-machine `scripts/deploy/staging-podman.sh`.

## Layout

```
deploy/k8s/
├── README.md           — this file
├── base/               — kustomize base (shared across envs)
│   ├── kustomization.yaml
│   ├── deployment.yaml
│   ├── service.yaml
│   ├── configmap.yaml
│   ├── pvc.yaml
│   └── networkpolicy.yaml
└── overlays/
    ├── staging/        — staging overlay (1 replica, podman:dev image, debug logs)
    └── production/     — production overlay (3 replicas, signed image, RestrictedPSP)
```

## Quick start

### Staging
```bash
kubectl apply -k deploy/k8s/overlays/staging
kubectl -n cyberclaw-staging rollout status deploy/cyberclaw-server
kubectl -n cyberclaw-staging port-forward svc/cyberclaw-server 38090:80
curl http://localhost:38090/health
```

### Production
1. **Pre-flight**: secrets must already exist (see Secrets section).
2. Apply:
   ```bash
   kubectl apply -k deploy/k8s/overlays/production
   kubectl -n cyberclaw rollout status deploy/cyberclaw-server
   ```
3. Watch the rollout. The deployment uses `RollingUpdate` with `maxSurge: 1, maxUnavailable: 0` so the cluster always serves at least the previous replica count.

## Secrets

The base `Deployment` references these secrets — they MUST be created
out-of-band (via Vault ESO, sealed-secrets, or the cloud secret manager
of your choice). The manifests do **NOT** ship credentials.

| Secret | Keys | Purpose |
|---|---|---|
| `cyberclaw-llm` | `LLM_API_KEY`, `LLM_BASE_URL`, `LLM_DEFAULT_MODEL`, `LLM_PROVIDER` | LLM provider credentials |
| `cyberclaw-jwt` | `JWT_SECRET` | 48-byte base64 — rotate every 90 days |
| `cyberclaw-cluster` | `CYBERCLAW_CLUSTER_SHARED_TOKEN` | Inter-node auth (32+ char) |
| `cyberclaw-tls` | `tls.crt`, `tls.key` | Ingress TLS (cert-manager managed) |
| `cyberclaw-mcp` | `CYBERCLAW_MCP_HTTP_URL`, `CYBERCLAW_MCP_HTTP_HEADERS` | Optional, only if enabling MCP servers |
| `cyberclaw-web-search` | `WEB_SEARCH_API_KEY` | Optional, only if `WEB_SEARCH_PROVIDER` set |

Example with External Secrets Operator + Vault:
```yaml
apiVersion: external-secrets.io/v1
kind: ExternalSecret
metadata:
  name: cyberclaw-jwt
  namespace: cyberclaw
spec:
  refreshInterval: 1h
  secretStoreRef: { name: vault-backend, kind: ClusterSecretStore }
  target: { name: cyberclaw-jwt }
  data:
    - secretKey: JWT_SECRET
      remoteRef: { key: secret/cyberclaw/prod, property: jwt_secret }
```

## Storage

Two PersistentVolumeClaims:

- `cyberclaw-data` (10 GiB, ReadWriteOnce) → mounted at `/var/lib/cyberclaw`
  Holds `memory.db` (LeveledMemoryStore) and ecosystem skill quarantine.
- `cyberclaw-audit` (5 GiB, ReadWriteOnce) → mounted at `/home/cyberclaw/.cyberclaw`
  Holds the append-only `audit.db` + WAL.

Both must use a StorageClass that supports `ReadWriteOnce` and survives
pod re-scheduling (e.g. EBS gp3, Azure Disk, GCP PD). Multi-replica
deployments require either:
  (a) ReadWriteMany volumes (NFS / EFS / Azure Files), or
  (b) Sharded deployment with one PVC per replica via StatefulSet.

The shipped base uses (a) — replace the `storageClassName` in
`base/pvc.yaml` to match your cluster.

## Container runtime caveat

`cmd.exec` and other High-risk capabilities use the host's container
runtime (docker/podman) for isolation. Inside K8s this means **the pod
needs access to a container runtime socket**. Three options:

1. **Sidecar pattern** (recommended) — run a docker-in-docker (DinD)
   sidecar in the same pod, mount its socket into the cyberclaw-server
   container. See `overlays/production/dind-sidecar.yaml.example`.
2. **Privileged DaemonSet** — bind-mount `/var/run/docker.sock` from
   the node. Requires `securityContext.privileged: true` on the
   cyberclaw-server container — fails most production PSPs.
3. **Disable container isolation** — set `CYBERCLAW_RUNTIME_STRATEGY=process`
   (without DinD), High-risk capabilities run as bare subprocess inside
   the cyberclaw-server pod. Faster but no isolation; only acceptable
   when capability-handler whitelisting is the sole defense.

The base manifest currently uses option (3) for compatibility.
Sidecar example is provided in `overlays/production/`.

## Health probes

Liveness:  `GET /health` every 10s, fail after 3 retries
Readiness: `GET /api/v1/system/ready` every 5s, fail after 2 retries

Health endpoint returns 200 when the HTTP server is up. Ready endpoint
checks: LeveledMemoryStore reachable, AuditSink writable, at least
one Connector registered. See `apps/cyberclaw-server/src/api/health.rs`.

## Resource limits

| Resource | Request | Limit |
|---|---|---|
| CPU | 500m | 2 |
| Memory | 512Mi | 2Gi |

Adjust per workload. For staging, the limit fits inside a t3.medium
(2 vCPU / 4 GiB).

## Observability

The deployment exposes Prometheus metrics on `:9090/metrics`. The base
includes a `ServiceMonitor` (gated on `prometheus.io/scrape: true`).
Grafana dashboard json lives at `deploy/monitoring/grafana/dashboards/`.

## Network policy

`base/networkpolicy.yaml` denies all egress except:
- DNS to kube-dns
- LLM provider host (`api.minimaxi.chat` :443 in staging)
- Cluster-internal `cyberclaw-server` ↔ peer cyberclaw pods (Raft)

Customise allowed egress in your overlay's `networkpolicy-patch.yaml`.

## Rollback

Standard kubectl rollback:
```bash
kubectl -n cyberclaw rollout undo deploy/cyberclaw-server
```

The audit log is append-only with hash chain — a rollback does NOT
truncate audit. After rollback, run `cyberclaw-cli audit verify-chain`
to confirm the previous version's writes are intact.

## What the base does NOT include (deliberate)

- Ingress controller selection (use whatever your cluster has — nginx,
  traefik, ALB)
- Cert-manager Issuer (cluster-specific)
- Pod identity / IRSA / Workload Identity (cloud-specific)
- Backup CronJob — see RB-11 in `docs/implementation/deploy/RUNBOOKS.md`

These belong in your overlay because they vary across clusters.
