#!/usr/bin/env bash
# CyberClaw container entrypoint.
#
# Bootstraps a demo `users.toml` when (and only when) `SEED_DEMO_USERS=1`
# is set and the file is missing. Production images leave the env unset
# and the server boots into bootstrap-token / wizard onboarding mode
# (see `apps/cyberclaw-server/src/state.rs::need_token`). Staging /
# devloops set `SEED_DEMO_USERS=1` so the SPA's hard-coded login hint
# (`op_ada` — see `web/src/i18n.jsx`) lines up with what the server
# actually accepts.
#
# Idempotent: if the file already exists, we leave it alone. The user
# can extend `users.toml` at runtime; subsequent restarts won't clobber.

set -euo pipefail

USERS_FILE="${HOME}/.cyberclaw/users.toml"

if [[ "${SEED_DEMO_USERS:-0}" == "1" ]]; then
  if [[ ! -f "${USERS_FILE}" ]]; then
    mkdir -p "$(dirname "${USERS_FILE}")"
    cat > "${USERS_FILE}" <<'EOF'
# Demo operators — seeded by entrypoint.sh when SEED_DEMO_USERS=1.
# Production images should leave this absent so the server requires
# bootstrap-token onboarding.

[[users]]
user_id = "op_ada"
display_name = "Operator Ada"
created_at = "2026-04-24T00:00:00Z"
last_login = "2026-04-24T00:00:00Z"
role = "admin"
onboarded_at = "2026-04-26T00:00:00+00:00"
intent_auto_route = false

[[users]]
user_id = "qa-admin"
display_name = "QA Admin"
created_at = "2026-04-24T00:00:00Z"
last_login = "2026-04-24T00:00:00Z"
role = "admin"
onboarded_at = "2026-04-26T00:00:00+00:00"
intent_auto_route = false
EOF
    echo "[entrypoint] seeded demo users.toml at ${USERS_FILE}" >&2
  else
    echo "[entrypoint] users.toml already present — skipping seed" >&2
  fi
fi

exec "$@"
