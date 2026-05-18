#!/bin/bash
# Guard rail: catch the SPECIFIC i18n bugs that have actually slipped
# past hand review. Narrow scope — false positives kill adoption.
#
# What this catches (high signal):
#   1. Sidebar labelZh containing English words/phrases that aren't a
#      cyberclaw brand term. Sidebar is uniform structured config, so
#      a label like "Platform Plugin" or "学习 & Curator" is a clear bug.
#   2. `&` anywhere inside a zh string. In Chinese UI copy `&` should
#      always be `与` / `·` / `和`.
#
# What this does NOT try to catch (too noisy):
#   - Page dict() Chinese strings mixing English in parens — that's a
#     legitimate pattern (e.g. "IM 入站（IM Platforms）") and trying to
#     classify each one is combinatorial. Manual review per page.
#
# Run via `npm run check` (also wired into `prebuild`).

set -u

cd "$(dirname "$0")/.." || exit 2

ALLOWED_BRAND_TERMS=(
  "Agent" "Skill" "Connector" "Capability" "Curator" "MoA"
  "PTY" "MCP" "LLM" "JWT" "OAuth" "URL" "API" "JSON" "TOML"
  "Webhook" "SSE" "WebSocket" "Cron" "Kanban" "CyberClaw"
  "IM"
)

violations=0

check_sidebar_label() {
  local lineno="$1" value="$2"
  python3 - "$lineno" "$value" "${ALLOWED_BRAND_TERMS[@]}" <<'PY'
import re, sys
lineno, value = sys.argv[1], sys.argv[2]
allowed = sys.argv[3:]
# Rule 1: forbid raw `&`
if re.search(r'(?<!\w)&(?!amp;|\w)', value):
    print(f"  src/components/Sidebar.tsx:{lineno}: labelZh uses '&' — use 与 / · / 和: {value}")
    sys.exit(2)
# Rule 2: forbid Title-Case multi-letter English phrases not in allowlist.
# Strip backtick code spans and identifiers first.
stripped = re.sub(r'`[^`]*`', '', value)
phrases = re.findall(r"[A-Z][a-zA-Z]{1,}(?:\s+[A-Z][a-zA-Z]{1,})*", stripped)
for p in phrases:
    if p in allowed:
        continue
    if all(tok in allowed for tok in p.split()):
        continue
    print(f"  src/components/Sidebar.tsx:{lineno}: labelZh has non-brand English '{p}': {value}")
    sys.exit(2)
sys.exit(0)
PY
  return $?
}

echo "=== sidebar labelZh purity ==="
while IFS= read -r line; do
  [ -z "$line" ] && continue
  lineno=$(printf '%s' "$line" | cut -d: -f1)
  rest=$(printf '%s' "$line" | cut -d: -f2-)
  value=$(printf '%s' "$rest" | sed -n 's/.*labelZh:[[:space:]]*"\([^"]*\)".*/\1/p')
  [ -z "$value" ] && continue
  if ! check_sidebar_label "$lineno" "$value"; then
    violations=$((violations + 1))
  fi
done < <(grep -nE 'labelZh:[[:space:]]*"' src/components/Sidebar.tsx 2>/dev/null)

echo "=== global '&' in zh strings (page dict + components) ==="
# Find any "..." string in TS/TSX whose content has both a Chinese
# character AND a bare `&` (not `&amp;` and not adjacent to a word).
while IFS= read -r match; do
  [ -z "$match" ] && continue
  echo "  $match"
  violations=$((violations + 1))
done < <(
  grep -rnE '"[^"]*&[^"]*"' src/pages src/components 2>/dev/null \
    | python3 -c '
import sys, re
for line in sys.stdin:
    if ":" not in line:
        continue
    # Reuse the same heuristic — needs Chinese char + bare `&`.
    if not re.search(r"[一-鿿]", line):
        continue
    # Quote span check.
    for m in re.finditer(r"\"([^\"]*)\"", line):
        s = m.group(1)
        if not re.search(r"[一-鿿]", s):
            continue
        if re.search(r"(?<!\w)&(?!amp;|\w)", s):
            file_line = line.split(":")[:2]
            print(f"{file_line[0]}:{file_line[1]}: bare \"&\" inside zh: {s}")
            break
'
)

if [ "$violations" -gt 0 ]; then
  echo
  echo "ERROR: $violations i18n purity violation(s) detected."
  echo "Fixes:"
  echo "  - Sidebar labelZh: translate the English phrase OR add to"
  echo "    ALLOWED_BRAND_TERMS (with a PR justification)."
  echo "  - '&' inside zh: replace with 与 / · / 和."
  exit 1
fi

echo "check-i18n-purity: OK (0 violations)"
exit 0
