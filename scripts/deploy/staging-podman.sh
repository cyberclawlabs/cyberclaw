#!/usr/bin/env bash
# CyberClaw — staging deployment via podman.
#
# Boots cyberclaw-server:dev with sane staging defaults: in-memory state,
# placeholder LLM creds (server requires LLM_API_KEY at startup but staging
# smoke tests don't exercise LLM paths), strict CSP, rate-limit lifted high
# enough that exploratory clicking doesn't trip 429.
#
# Usage:
#   ./scripts/deploy/staging-podman.sh build           # podman build
#   ./scripts/deploy/staging-podman.sh up              # run container detached
#   ./scripts/deploy/staging-podman.sh logs            # follow logs
#   ./scripts/deploy/staging-podman.sh down            # stop + rm container
#   ./scripts/deploy/staging-podman.sh smoke           # /health + /admin probe
#   ./scripts/deploy/staging-podman.sh restart         # down + up
#   ./scripts/deploy/staging-podman.sh status          # podman ps + healthcheck
#   ./scripts/deploy/staging-podman.sh monitoring-up   # prometheus + grafana sidecars
#   ./scripts/deploy/staging-podman.sh monitoring-down # stop sidecars
#
# Env overrides:
#   IMAGE_NAME            (default: cyberclaw-server:dev)
#   CONTAINER_NAME        (default: cyberclaw-staging)
#   HOST_PORT             (default: 38090)
#   DATA_VOLUME           (default: cyberclaw-staging-data)
#   NETWORK_NAME          (default: cyberclaw-staging-net)
#   PROM_HOST_PORT        (default: 39090)
#   GRAFANA_HOST_PORT     (default: 33000)
#   GRAFANA_ADMIN_PASS    (default: admin — change before sharing!)
#   JWT_SECRET            (auto-generated if unset; persisted to .staging/jwt.secret)
set -euo pipefail

IMAGE_NAME="${IMAGE_NAME:-cyberclaw-server:dev}"
CONTAINER_NAME="${CONTAINER_NAME:-cyberclaw-staging}"
HOST_PORT="${HOST_PORT:-38090}"
DATA_VOLUME="${DATA_VOLUME:-cyberclaw-staging-data}"
NETWORK_NAME="${NETWORK_NAME:-cyberclaw-staging-net}"
PROM_HOST_PORT="${PROM_HOST_PORT:-39090}"
GRAFANA_HOST_PORT="${GRAFANA_HOST_PORT:-33000}"
GRAFANA_ADMIN_PASS="${GRAFANA_ADMIN_PASS:-admin}"

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SECRETS_DIR="${REPO_ROOT}/.staging"

ensure_network() {
  if ! podman network exists "${NETWORK_NAME}" 2>/dev/null; then
    podman network create "${NETWORK_NAME}" >/dev/null
  fi
}

generate_jwt_secret() {
  mkdir -p "${SECRETS_DIR}"
  if [[ ! -f "${SECRETS_DIR}/jwt.secret" ]]; then
    # Default to the QA helper's hardcoded secret so Playwright-issued
    # tokens (`tests/e2e/helpers/auth.ts::QA_JWT_SECRET`) verify against
    # the staging server. The file is gitignored. Override by setting
    # `JWT_SECRET=...` in the environment before invoking this script
    # (production deployments MUST set their own secret).
    printf 'change-this-to-a-real-secret-at-least-32-chars' > "${SECRETS_DIR}/jwt.secret"
  fi
  cat "${SECRETS_DIR}/jwt.secret"
}

cmd_build() {
  echo "==> podman build ${IMAGE_NAME}"
  cd "${REPO_ROOT}"
  podman build --tag "${IMAGE_NAME}" --file Dockerfile .
}

cmd_up() {
  local jwt_secret
  jwt_secret="${JWT_SECRET:-$(generate_jwt_secret)}"

  if podman container exists "${CONTAINER_NAME}"; then
    echo "==> Container ${CONTAINER_NAME} already exists; removing."
    podman rm -f "${CONTAINER_NAME}" >/dev/null
  fi

  if ! podman volume exists "${DATA_VOLUME}" 2>/dev/null; then
    podman volume create "${DATA_VOLUME}" >/dev/null
  fi

  ensure_network

  echo "==> podman run ${CONTAINER_NAME} (image=${IMAGE_NAME}, port=${HOST_PORT})"
  podman run --detach \
    --name "${CONTAINER_NAME}" \
    --network "${NETWORK_NAME}" \
    --publish "127.0.0.1:${HOST_PORT}:3000" \
    --volume "${DATA_VOLUME}:/var/lib/cyberclaw" \
    --volume "${REPO_ROOT}/web:/app/web:ro,Z" \
    --env "ENVIRONMENT=staging" \
    --env "USE_TLS=false" \
    --env "JWT_SECRET=${jwt_secret}" \
    $(if [[ -f "${REPO_ROOT}/apps/cyberclaw-server/.env" ]]; then echo "--env-file=${REPO_ROOT}/apps/cyberclaw-server/.env"; else echo "--env LLM_PROVIDER=openai --env LLM_API_KEY=sk-staging-placeholder-not-used --env LLM_BASE_URL=http://127.0.0.1:1"; fi) \
    --env "CYBERCLAW_CLUSTER_SHARED_TOKEN=staging_cluster_token_32chars_min_placeholder" \
    --env "RATE_LIMIT_PER_SECOND=1000" \
    --env "RATE_LIMIT_BURST_SIZE=5000" \
    --env "CYBERCLAW_WEB_ROOT=/app/web" \
    --env "SEED_DEMO_USERS=1" \
    --restart unless-stopped \
    "${IMAGE_NAME}"

  echo "==> Waiting for /health …"
  for i in {1..30}; do
    if curl -sf "http://127.0.0.1:${HOST_PORT}/health" >/dev/null 2>&1; then
      echo "==> Healthy after ${i}s"
      cmd_smoke
      return 0
    fi
    sleep 1
  done
  echo "!! Service did not become healthy in 30s; tail logs:"
  podman logs --tail 40 "${CONTAINER_NAME}" || true
  return 1
}

cmd_logs() {
  podman logs --follow "${CONTAINER_NAME}"
}

cmd_down() {
  if podman container exists "${CONTAINER_NAME}"; then
    podman rm -f "${CONTAINER_NAME}"
  fi
}

cmd_status() {
  podman ps --filter "name=${CONTAINER_NAME}" \
    --format 'table {{.Names}}\t{{.Status}}\t{{.Ports}}'
  if podman container exists "${CONTAINER_NAME}"; then
    podman healthcheck run "${CONTAINER_NAME}" 2>&1 || true
  fi
}

cmd_smoke() {
  local base="http://127.0.0.1:${HOST_PORT}"
  echo "--- /health"
  curl -sf "${base}/health" -w '\nHTTP %{http_code}\n'
  echo "--- /admin (HEAD)"
  curl -sIf "${base}/admin" | head -3 || true
  echo "--- /admin/dist/i18n.js (first 80 bytes)"
  curl -sf "${base}/admin/dist/i18n.js" | head -c 80
  echo
}

cmd_restart() {
  cmd_down
  cmd_up
}

cmd_monitoring_up() {
  ensure_network

  if podman container exists cyberclaw-prometheus; then
    podman rm -f cyberclaw-prometheus >/dev/null
  fi
  if podman container exists cyberclaw-grafana; then
    podman rm -f cyberclaw-grafana >/dev/null
  fi

  echo "==> podman run cyberclaw-prometheus (port=${PROM_HOST_PORT})"
  podman run --detach \
    --name cyberclaw-prometheus \
    --network "${NETWORK_NAME}" \
    --publish "127.0.0.1:${PROM_HOST_PORT}:9090" \
    --volume "${REPO_ROOT}/deploy/monitoring/prometheus.yml:/etc/prometheus/prometheus.yml:ro,Z" \
    --volume "cyberclaw-prometheus-data:/prometheus" \
    docker.io/prom/prometheus:v2.55.0 \
    --config.file=/etc/prometheus/prometheus.yml \
    --storage.tsdb.path=/prometheus \
    --web.enable-lifecycle

  echo "==> podman run cyberclaw-grafana (port=${GRAFANA_HOST_PORT})"
  podman run --detach \
    --name cyberclaw-grafana \
    --network "${NETWORK_NAME}" \
    --publish "127.0.0.1:${GRAFANA_HOST_PORT}:3000" \
    --volume "${REPO_ROOT}/deploy/monitoring/grafana/provisioning:/etc/grafana/provisioning:ro,Z" \
    --volume "${REPO_ROOT}/deploy/monitoring/grafana/dashboards:/var/lib/grafana/dashboards:ro,Z" \
    --volume "cyberclaw-grafana-data:/var/lib/grafana" \
    --env "GF_SECURITY_ADMIN_USER=admin" \
    --env "GF_SECURITY_ADMIN_PASSWORD=${GRAFANA_ADMIN_PASS}" \
    --env "GF_USERS_ALLOW_SIGN_UP=false" \
    --env "GF_AUTH_ANONYMOUS_ENABLED=false" \
    docker.io/grafana/grafana:11.2.2

  echo "==> Waiting for Prometheus /-/healthy …"
  for i in {1..30}; do
    if curl -sf "http://127.0.0.1:${PROM_HOST_PORT}/-/healthy" >/dev/null 2>&1; then
      break
    fi
    sleep 1
  done

  echo "==> Waiting for Grafana /api/health …"
  for i in {1..40}; do
    if curl -sf "http://127.0.0.1:${GRAFANA_HOST_PORT}/api/health" >/dev/null 2>&1; then
      break
    fi
    sleep 1
  done

  echo "--- Prometheus targets"
  curl -sf "http://127.0.0.1:${PROM_HOST_PORT}/api/v1/targets" \
    | python3 -c 'import sys, json; d=json.load(sys.stdin); [print(t["labels"]["job"], t["health"], t["lastError"][:60] if t["lastError"] else "") for t in d["data"]["activeTargets"]]' 2>/dev/null \
    || echo "(install python3 to format; raw response above)"

  echo
  echo "==> Monitoring up."
  echo "    Prometheus:  http://127.0.0.1:${PROM_HOST_PORT}"
  echo "    Grafana:     http://127.0.0.1:${GRAFANA_HOST_PORT} (admin / ${GRAFANA_ADMIN_PASS})"
  echo "    Dashboard:   http://127.0.0.1:${GRAFANA_HOST_PORT}/d/cyberclaw-overview"
}

cmd_monitoring_down() {
  for c in cyberclaw-grafana cyberclaw-prometheus; do
    if podman container exists "${c}"; then
      podman rm -f "${c}" >/dev/null
      echo "==> Removed ${c}"
    fi
  done
}

main() {
  local sub="${1:-help}"
  shift || true
  case "${sub}" in
    build)            cmd_build "$@" ;;
    up)               cmd_up "$@" ;;
    logs)             cmd_logs "$@" ;;
    down)             cmd_down "$@" ;;
    status)           cmd_status "$@" ;;
    smoke)            cmd_smoke "$@" ;;
    restart)          cmd_restart "$@" ;;
    monitoring-up)    cmd_monitoring_up "$@" ;;
    monitoring-down)  cmd_monitoring_down "$@" ;;
    help|*)
      grep -E '^#( |$)' "$0" | sed 's/^# //; s/^#$//'
      ;;
  esac
}

main "$@"
