#!/bin/bash
# Guard rail: forbid native browser dialogs in user-facing pages.
#
# Rationale: window.confirm / window.alert / window.prompt are
#   - unstyled (break theme consistency)
#   - silently no-op'd or returned false in some PWA / Electron /
#     browser-policy contexts (so the action never reaches the user)
#   - block the main thread
#
# CyberClaw has a themed Modal component (web/src/components/Modal.tsx)
# that covers all these cases — see ChatPage and UploadsPage for the
# state-driven confirm pattern.
#
# This script is run as a pre-build gate via `npm run check`.
# Add new exceptions to ALLOWLIST below ONLY with a written justification.

set -u

cd "$(dirname "$0")/.." || exit 2

# Scope: v2 stack (TypeScript only). Legacy `pages_*.jsx` files are dead
# code that linger in src/ but aren't imported by AppV2.tsx — they will
# be removed in a future cleanup pass. Restricting the scan to .ts/.tsx
# keeps this gate honest without spurious failures from unused files.
SCAN_GLOBS=(
  "src/pages"
  "src/components"
  "src/AppV2.tsx"
  "src/main.tsx"
  "src/plugins"
  "src/lib"
)

ALLOWLIST=()  # Add file paths here ONLY with a written justification.

PATTERN='\b(window\.)?(confirm|alert|prompt)\s*\('
# Strip line comments + backtick code spans from each grep line BEFORE
# checking — otherwise comments like "// replace bare confirm()" or
# template strings like \`confirm(...)\` trigger false positives.
matches=$(grep -rnE --include="*.ts" --include="*.tsx" "$PATTERN" "${SCAN_GLOBS[@]}" 2>/dev/null \
  | python3 -c '
import sys, re
PAT = re.compile(r"\b(window\.)?(confirm|alert|prompt)\s*\(")
for raw in sys.stdin:
    parts = raw.split(":", 2)
    if len(parts) < 3:
        continue
    content = parts[2]
    no_comment = re.split(r"//", content, maxsplit=1)[0]
    no_backtick = re.sub(r"`[^`]*`", "", no_comment)
    if PAT.search(no_backtick):
        sys.stdout.write(raw)
' || true)

if [ -n "$matches" ]; then
  # Filter out allowlisted files. Use `${arr[@]+...}` so an empty
  # ALLOWLIST doesn't trip `set -u`.
  filtered="$matches"
  for allowed in ${ALLOWLIST[@]+"${ALLOWLIST[@]}"}; do
    filtered=$(echo "$filtered" | grep -vF "$allowed" || true)
  done

  if [ -n "$filtered" ]; then
    echo "ERROR: native browser dialog detected — use Modal component instead."
    echo
    echo "$filtered"
    echo
    echo "Hint: see web/src/components/Modal.tsx and the state-driven"
    echo "confirm pattern in ChatPage / UploadsPage (pendingDelete state)."
    exit 1
  fi
fi

echo "check-no-native-dialogs: OK (0 matches in v2 TS stack)"
exit 0
