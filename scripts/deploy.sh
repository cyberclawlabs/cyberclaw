#!/usr/bin/env bash
# deploy.sh — Build, ship, and restart cyberclaw-server with health-check and rollback.
# Usage: ./deploy.sh [--config <path>] [--skip-build] [--rollback-only]
# Environment:
#   DEPLOY_USER          SSH user (default: ec2-user)
#   DEPLOY_HOST          Remote host (required)
#   DEPLOY_PORT          SSH port (default: 22)
#   DEPLOY_PATH          Remote binary path (default: /opt/cyberclaw-server)
#   DEPLOY_SERVICE      systemd service name (default: cyberclaw-server)
#   DEPLOY_HEALTH_URL   Health check URL (default: http://localhost:8080/health)
#   HEALTH_MAX_WAIT     Seconds to wait for health (default: 30)
#   HEALTH_INTERVAL     Seconds between retries (default: 2)
#   ROLLBACK_ON_FAILURE Redeploy previous binary on failure (default: true)
#   SCP_OPTS            Extra scp flags (default: -o StrictHostKeyChecking=no)

set -euo pipefail

# ── Defaults ──────────────────────────────────────────────────────────────────
DEPLOY_USER="${DEPLOY_USER:-ec2-user}"
DEPLOY_HOST="${DEPLOY_HOST:?DEPLOY_HOST must be set}"
DEPLOY_PORT="${DEPLOY_PORT:-22}"
DEPLOY_PATH="${DEPLOY_PATH:-/opt/cyberclaw-server}"
DEPLOY_SERVICE="${DEPLOY_SERVICE:-cyberclaw-server}"
DEPLOY_HEALTH_URL="${DEPLOY_HEALTH_URL:-http://localhost:8080/health}"
HEALTH_MAX_WAIT="${HEALTH_MAX_WAIT:-30}"
HEALTH_INTERVAL="${HEALTH_INTERVAL:-2}"
ROLLBACK_ON_FAILURE="${ROLLBACK_ON_FAILURE:-true}"
SCP_OPTS="${SCP_OPTS:--o StrictHostKeyChecking=no}"

readonly BIN_NAME="cyberclaw-server"
readonly BUILD_DIR="/Users/max/project/cyberclaw/target/release"
readonly LOCAL_BIN="${BUILD_DIR}/${BIN_NAME}"
readonly ARTIFACT="cyberclaw-server-$(date +%Y%m%d-%H%M%S).tar.gz"

SKIP_BUILD=false
ROLLBACK_ONLY=false
CONFIG=""

# ── Arg parsing ───────────────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-build)  SKIP_BUILD=true; shift ;;
    --rollback-only) ROLLBACK_ONLY=true; shift ;;
    --config)      CONFIG="$2"; shift 2 ;;
    *) echo "Unknown option: $1" >&2; exit 1 ;;
  esac
done

# Load config file if provided
if [[ -n "$CONFIG" && -f "$CONFIG" ]]; then
  # shellcheck disable=SC1090
  source <(grep -E '^[A-Z]' "$CONFIG" | xargs -I{} echo "export {}")
fi

DEPLOY_SSH="${DEPLOY_USER}@${DEPLOY_HOST}"
TIMESTAMP=$(date +%Y%m%d-%H%M%S)
PREV_BAK=""

# ── Helpers ───────────────────────────────────────────────────────────────────
log()  { echo "[$(date +%H:%M:%S)] $*"; }
warn() { echo "[$(date +%H:%M:%S)] WARN: $*" >&2; }
die()  { echo "[$(date +%H:%M:%S)] ERROR: $*" >&2; exit 1; }

ssh_cmd() {
  ssh -p "$DEPLOY_PORT" $SCP_OPTS "$DEPLOY_SSH" "$@"
}

# ── Pre-flight ────────────────────────────────────────────────────────────────
log "=== CyberClaw Deployment Started ==="
log "Host:    ${DEPLOY_SSH}:${DEPLOY_PORT}"
log "Service: ${DEPLOY_SERVICE}"
log "Binary:  ${DEPLOY_PATH}"

# ── Build ─────────────────────────────────────────────────────────────────────
if [[ "$SKIP_BUILD" == "false" ]]; then
  log "Building release binary..."
  cd /Users/max/project/cyberclaw

  cargo build --release -p cyberclaw-server 2>&1 | tail -5

  [[ -f "$LOCAL_BIN" ]] || die "Build failed: binary not found at ${LOCAL_BIN}"
  log "Build OK — size: $(du -h "$LOCAL_BIN" | cut -f1)"
else
  log "Skipping build (--skip-build)"
  [[ -f "$LOCAL_BIN" ]] || die "Binary not found at ${LOCAL_BIN} (did you forget to build first?)"
fi

# ── Snapshot previous binary for rollback ────────────────────────────────────
if [[ "$ROLLBACK_ON_FAILURE" == "true" ]]; then
  log "Snapshotting current remote binary..."
  PREV_BAK="$DEPLOY_PATH.bak.${TIMESTAMP}"
  ssh_cmd "cp ${DEPLOY_PATH} ${PREV_BAK} && echo OK" | grep -q OK \
    || warn "Could not snapshot previous binary (may not exist yet)"
fi

# ── Upload ────────────────────────────────────────────────────────────────────
log "Uploading binary via SCP..."
scp -P "$DEPLOY_PORT" $SCP_OPTS "$LOCAL_BIN" "${DEPLOY_SSH}:${DEPLOY_PATH}.new"

# ── Atomic swap ───────────────────────────────────────────────────────────────
log "Installing new binary (atomic swap)..."
ssh_cmd "\
  mv ${DEPLOY_PATH}.new ${DEPLOY_PATH} && \
  chmod +x ${DEPLOY_PATH} && \
  echo OK" | grep -q OK \
  || die "Failed to install binary on remote"

# ── Restart service ───────────────────────────────────────────────────────────
log "Restarting systemd service: ${DEPLOY_SERVICE}..."
ssh_cmd "sudo systemctl restart ${DEPLOY_SERVICE}" \
  || die "systemctl restart failed"

# ── Health check ─────────────────────────────────────────────────────────────
log "Waiting up to ${HEALTH_MAX_WAIT}s for /health at ${DEPLOY_HEALTH_URL}..."

elapsed=0
healthy=false
while (( elapsed < HEALTH_MAX_WAIT )); do
  if curl -sf --max-time 3 "${DEPLOY_HEALTH_URL}" > /dev/null 2>&1; then
    healthy=true
    break
  fi
  sleep "$HEALTH_INTERVAL"
  elapsed=$(( elapsed + HEALTH_INTERVAL ))
  echo -n "."
done
echo ""

if [[ "$healthy" == "true" ]]; then
  log "Health check PASSED after ${elapsed}s"
else
  warn "Health check FAILED after ${HEALTH_MAX_WAIT}s"
  if [[ "$ROLLBACK_ON_FAILURE" == "true" && -n "$PREV_BAK" ]]; then
    log "Rolling back to ${PREV_BAK}..."
    ssh_cmd "sudo systemctl stop ${DEPLOY_SERVICE} && \
              cp ${PREV_BAK} ${DEPLOY_PATH} && \
              sudo systemctl start ${DEPLOY_SERVICE}" \
      || die "Rollback failed — manual intervention required!"

    sleep 5
    if curl -sf --max-time 5 "${DEPLOY_HEALTH_URL}" > /dev/null 2>&1; then
      log "Rollback successful — service restored"
    else
      die "Rollback completed but service is still unhealthy — MANUAL INTERVENTION REQUIRED"
    fi
  else
    die "Deployment failed and rollback is disabled"
  fi
fi

# ── Cleanup old backups (keep last 5) ───────────────────────────────────────
log "Cleaning up old backups..."
ssh_cmd "ls -t ${DEPLOY_PATH}.bak.* 2>/dev/null | tail -n +6 | xargs -r rm -f" || true

log "=== Deployment Complete ==="
