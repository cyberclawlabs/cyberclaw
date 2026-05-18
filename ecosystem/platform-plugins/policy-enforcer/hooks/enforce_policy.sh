#!/usr/bin/env sh
# policy-enforcer pre-execution hook
#
# Invoked by PluginHookLoader before each capability execution.
# The platform passes execution context via environment variables:
#
#   CYBERCLAW_HOOK_CAPABILITY_ID  — e.g. "cmd.exec"
#   CYBERCLAW_HOOK_CONNECTOR_ID   — e.g. "local"
#   CYBERCLAW_HOOK_ACTOR_ID       — UUID of the calling actor
#   CYBERCLAW_HOOK_TENANT_ID      — tenant scope (empty if single-tenant)
#   CYBERCLAW_HOOK_TRACE_ID       — trace correlation ID
#
# Exit 0  → allow execution to proceed.
# Exit 1  → block execution (failurePolicy: abort surfaces this as an error).
set -eu

CAPABILITY="${CYBERCLAW_HOOK_CAPABILITY_ID:-}"
DENYLIST="${POLICY_ENFORCER_DENYLIST:-}"

if [ -z "$CAPABILITY" ]; then
    printf 'policy-enforcer: CYBERCLAW_HOOK_CAPABILITY_ID not set\n' >&2
    exit 1
fi

if [ -z "$DENYLIST" ]; then
    # No denylist configured — allow all.
    exit 0
fi

# Check whether CAPABILITY appears in the colon-separated denylist.
IFS=':'
for denied in $DENYLIST; do
    if [ "$CAPABILITY" = "$denied" ]; then
        printf 'policy-enforcer: capability "%s" is in denylist — blocked\n' \
            "$CAPABILITY" >&2
        exit 1
    fi
done

exit 0
