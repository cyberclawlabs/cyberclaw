#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────
# build-release.sh — produce the public-facing release directory from
# the canonical source tree.
#
# Usage:
#   ./scripts/release/build-release.sh                  # → ./release/
#   RELEASE_DIR=/tmp/x ./scripts/release/build-release.sh
#   SKIP_BUILD=1 ./scripts/release/build-release.sh     # don't rebuild binaries
#
# What it does:
#   1. rsync source → release with the exclude rules below
#   2. drop AI-dev-guidance + secrets that should never publish
#   3. cargo build --release (unless SKIP_BUILD=1)
#   4. copy binaries into release/bin/<target-triple>/
#   5. generate RELEASE_MANIFEST.md with SHA-256 hashes + counts
#   6. run pre-publish sanity checks
#   7. print summary
#
# Exit code 0 on success, non-zero on any sanity-check failure.
# ─────────────────────────────────────────────────────────────────────
set -euo pipefail

SOURCE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
RELEASE_DIR="${RELEASE_DIR:-${SOURCE_DIR}/release}"
SKIP_BUILD="${SKIP_BUILD:-0}"
TARGET_TRIPLE="${TARGET_TRIPLE:-$(rustc -vV 2>/dev/null | awk '/^host:/ {print $2}')}"
TARGET_TRIPLE="${TARGET_TRIPLE:-darwin-arm64}"

BLU='\033[1;34m'; GRN='\033[1;32m'; YLW='\033[1;33m'; RED='\033[1;31m'; RST='\033[0m'
say()   { echo -e "${BLU}── $* ──${RST}"; }
ok()    { echo -e "  ${GRN}✓${RST} $*"; }
warn()  { echo -e "  ${YLW}!${RST} $*"; }
fail()  { echo -e "  ${RED}✗${RST} $*"; exit 1; }

# ─────────────────────────────────────────────────────────────────────
say "1. preparing target: $RELEASE_DIR"
# ─────────────────────────────────────────────────────────────────────
# SAFETY (2026-05-23): if RELEASE_DIR contains a .git directory, refuse
# to rm -rf — that would destroy local git history and uncommitted
# changes. Override with FORCE_WIPE=1 only when intentional.
#
# Discovered when build-release.sh was re-run against an already-cloned
# cyberclawlabs/cyberclaw working tree → silently wiped .git → lost the
# local remote tracking + history (recoverable via re-clone but
# disruptive and error-prone). Default behavior now: preserve .git,
# rsync --delete --exclude=.git over the working tree.
USE_RSYNC_DELETE=0
if [[ -d "$RELEASE_DIR/.git" ]]; then
  if [[ "${FORCE_WIPE:-0}" == "1" ]]; then
    warn "FORCE_WIPE=1: wiping $RELEASE_DIR despite .git presence"
    rm -rf "$RELEASE_DIR"
    mkdir -p "$RELEASE_DIR"
    ok "wiped (forced)"
  else
    ok "preserving existing git repo at $RELEASE_DIR/.git"
    ok "(rsync --delete --exclude=.git will replace working tree)"
    ok "(set FORCE_WIPE=1 to override and rm -rf)"
    USE_RSYNC_DELETE=1
  fi
elif [[ -d "$RELEASE_DIR" ]]; then
  # No .git inside — safe to wipe fully
  rm -rf "$RELEASE_DIR"
  mkdir -p "$RELEASE_DIR"
  ok "removed existing $RELEASE_DIR (no .git inside)"
else
  mkdir -p "$RELEASE_DIR"
  ok "created fresh $RELEASE_DIR"
fi

# ─────────────────────────────────────────────────────────────────────
say "2. rsync source → release with exclude rules"
# ─────────────────────────────────────────────────────────────────────
RSYNC_DELETE_FLAG=""
if [[ "$USE_RSYNC_DELETE" == "1" ]]; then
  # When preserving .git, use --delete so removed-from-source files
  # disappear from release too (otherwise old artifacts accumulate).
  # --exclude='/.git/' is already below; combining --delete with it
  # protects .git from being deleted as "not in source".
  RSYNC_DELETE_FLAG="--delete"
  ok "using rsync --delete to remove stale files (excluding .git)"
fi
rsync -a $RSYNC_DELETE_FLAG \
  --exclude='target/' \
  --exclude='node_modules/' \
  --exclude='tmp/' \
  --exclude='claw-research/' \
  --exclude='.omc/' \
  --exclude='.staging/' \
  --exclude='.serena/' \
  --exclude='.claude/' \
  --exclude='.spec-workflow/' \
  --exclude='.playwright-mcp/' \
  --exclude='.superpowers/' \
  --exclude='playwright-report/' \
  --exclude='test-results/' \
  --exclude='*.profraw' \
  --exclude='*.profdata' \
  --exclude='*.log' \
  --exclude='*.bak*' \
  --exclude='*.swp' \
  --exclude='.DS_Store' \
  --exclude='/.git/' \
  --exclude='/release/' \
  --exclude='/web/debug/' \
  --exclude='/web/uploads/' \
  --exclude='/web/CyberClaw Admin Console.html' \
  --exclude='/web/CyberClaw Admin Console (standalone source).html' \
  --exclude='/docs/implementation/' \
  --exclude='/docs/development/' \
  --exclude='/docs/superpowers/' \
  --exclude='/docs/architecture/' \
  --exclude='/docs/api/' \
  --exclude='/docs/builders/' \
  --exclude='/docs/business/' \
  --exclude='/docs/configuration/' \
  --exclude='/docs/deployment/' \
  --exclude='/docs/getting-started/' \
  --exclude='/docs/guides/' \
  --exclude='/docs/modules/' \
  --exclude='/docs/reference/' \
  --exclude='/docs/research/' \
  --exclude='/docs/security/' \
  --exclude='/docs/templates/' \
  --exclude='/docs/testing/' \
  --exclude='/docs/user-guide/' \
  --exclude='/docs/web3/' \
  --exclude='/docs/INDEX.md' \
  --exclude='/docs/SECURITY-ADVISORY-*.md' \
  --exclude='/scripts/testing/' \
  --exclude='BUSINESS_DELIVERY_REPORT.md' \
  --exclude='**/PRODUCTION_READINESS_REVIEW.md' \
  --exclude='**/TEST_REPORT.md' \
  --exclude='**/HEARTBEAT_REPORT.md' \
  --exclude='**/*_COMPLETION_REPORT.md' \
  --exclude='**/*_REVIEW_DECISION*.md' \
  --exclude='/docs/research/hermes-*.md' \
  --exclude='/docs/research/v2-ui-audit.md' \
  --exclude='/agi-t3-doc.md' \
  --exclude='/.cyberclaw/' \
  --exclude='**/.env.production' \
  --exclude='/scripts/release/RELEASE_PROTOCOL.md' \
  --exclude='/tools/business-matrix/transcripts/' \
  --exclude='/uploads/' \
  --exclude='/artifacts/' \
  --exclude='/.matrix-fixtures/' \
  --exclude='/region_revenue.json' \
  --exclude='/web/dist/' \
  --exclude='/memory_*.yml' \
  --exclude='/hermes_*.yml' \
  --exclude='/cyberclaw_chat.yml' \
  --exclude='/cyberclaw_govern.yml' \
  --exclude='/cyberclaw_skills.yml' \
  --exclude='/docs/PRODUCTION_READINESS_CHECKLIST.md' \
  --exclude='/DOCUMENTATION_SYSTEM.md' \
  --exclude='/PROJECT_STRUCTURE.md' \
  --exclude='/RELEASE_MANIFEST.md' \
  --exclude='/package.json' \
  --exclude='/playwright.config.ts' \
  --exclude='/tools/' \
  --exclude='/README.md' \
  --exclude='/README.zh-CN.md' \
  --exclude='/CHANGELOG.md' \
  --exclude='/DEVELOPMENT.md' \
  --exclude='/ACKNOWLEDGMENTS.md' \
  --exclude='/docs/README.md' \
  --exclude='**/tests/' \
  --exclude='**/fuzz/' \
  --exclude='/*.png' \
  --exclude='/*.pptx' \
  --exclude='/*.docx' \
  --exclude='/*.xlsx' \
  --exclude='/*.pdf' \
  --exclude='/*.zip' \
  --exclude='*.db' \
  --exclude='*.sqlite' \
  --exclude='*.pem' \
  --exclude='*.key' \
  --exclude='secrets/' \
  --exclude='credentials/' \
  --exclude='/.env' \
  --exclude='/.env.test' \
  --exclude='/.env.local' \
  --exclude='/.env.*.local' \
  --exclude='/AGENTS.md' \
  --exclude='/CLAUDE.md' \
  --exclude='/claude.md' \
  --exclude='/AGENT.md' \
  --exclude='/agent.md' \
  --exclude='/agents.md' \
  "$SOURCE_DIR/" "$RELEASE_DIR/"
ok "rsync complete"

# ─────────────────────────────────────────────────────────────────────
say "3. drop AI-dev guidance and any leaked secrets that escaped excludes"
# ─────────────────────────────────────────────────────────────────────
removed=0
for f in AGENTS.md CLAUDE.md claude.md AGENT.md agent.md agents.md; do
  if [[ -f "$RELEASE_DIR/$f" ]]; then
    rm -f "$RELEASE_DIR/$f"
    ((removed+=1))
  fi
done
# .env files at any depth
while IFS= read -r f; do
  if [[ "$(basename "$f")" != ".env.example" ]]; then
    rm -f "$f"
    ((removed+=1))
  fi
done < <(find "$RELEASE_DIR" -type f \( -name '.env' -o -name '.env.test' -o -name '.env.local' \))
# tests/ stragglers (rsync's `**/tests/` doesn't catch top-level)
while IFS= read -r d; do
  rm -rf "$d"
  ((removed+=1))
done < <(find "$RELEASE_DIR" -type d -name tests 2>/dev/null)
ok "removed $removed dev-only artifacts"

# ─────────────────────────────────────────────────────────────────────
say "4. ensure LICENSE exists"
# ─────────────────────────────────────────────────────────────────────
if [[ ! -f "$RELEASE_DIR/LICENSE" ]]; then
  warn "LICENSE missing in source; generating Apache-2.0"
  curl -fsSL https://www.apache.org/licenses/LICENSE-2.0.txt -o "$RELEASE_DIR/LICENSE"
fi
ok "LICENSE present"

# ─────────────────────────────────────────────────────────────────────
say "5. build release binaries"
# ─────────────────────────────────────────────────────────────────────
if [[ "$SKIP_BUILD" == "1" ]]; then
  warn "SKIP_BUILD=1 — using existing target/release binaries"
else
  (cd "$SOURCE_DIR" && cargo build --release -p cyberclaw-server -p cyberclaw-cli)
  ok "cargo build --release complete"
fi

# ─────────────────────────────────────────────────────────────────────
say "6. copy binaries to release/bin/$TARGET_TRIPLE/"
# ─────────────────────────────────────────────────────────────────────
BIN_DIR="$RELEASE_DIR/bin/$TARGET_TRIPLE"
mkdir -p "$BIN_DIR"
for bin in cyberclaw-server cyberclaw-cli; do
  src_bin="$SOURCE_DIR/target/release/$bin"
  if [[ ! -x "$src_bin" ]]; then
    fail "binary not found: $src_bin (run cargo build --release first or unset SKIP_BUILD)"
  fi
  cp "$src_bin" "$BIN_DIR/"
  ok "copied $bin"
done

cat > "$BIN_DIR/../README.md" <<'EOF'
# Pre-compiled binaries

Each subdirectory holds a pre-built `cyberclaw-server` + `cyberclaw-cli`
pair targeting one platform. Run `cargo build --release` from source to
produce binaries for any other target.

```
bin/
└── <target-triple>/
    ├── cyberclaw-server
    └── cyberclaw-cli
```

SHA-256 hashes are recorded in `RELEASE_MANIFEST.md`.
EOF
ok "bin/README.md written"

# ─────────────────────────────────────────────────────────────────────
say "7. generate RELEASE_MANIFEST.md"
# ─────────────────────────────────────────────────────────────────────
SHA_SERVER=$(shasum -a 256 "$BIN_DIR/cyberclaw-server" | awk '{print $1}')
SHA_CLI=$(shasum -a 256 "$BIN_DIR/cyberclaw-cli" | awk '{print $1}')
SIZE_SERVER=$(du -k "$BIN_DIR/cyberclaw-server" | awk '{print $1}')
SIZE_CLI=$(du -k "$BIN_DIR/cyberclaw-cli" | awk '{print $1}')
N_AGENTS=$(ls "$RELEASE_DIR/ecosystem/agents" 2>/dev/null | wc -l | tr -d ' ')
N_SKILLS=$(ls "$RELEASE_DIR/ecosystem/skills" 2>/dev/null | wc -l | tr -d ' ')
N_CONN=$(ls "$RELEASE_DIR/ecosystem/connectors" 2>/dev/null | wc -l | tr -d ' ')
N_PLUGS=$(ls "$RELEASE_DIR/ecosystem/platform-plugins" 2>/dev/null | wc -l | tr -d ' ')
N_CRATES=$(ls -d "$RELEASE_DIR"/crates/*/ 2>/dev/null | wc -l | tr -d ' ')
N_RUST=$(find "$RELEASE_DIR/apps" "$RELEASE_DIR/crates" -name '*.rs' 2>/dev/null | wc -l | tr -d ' ')
N_LOC=$(find "$RELEASE_DIR/apps" "$RELEASE_DIR/crates" -name '*.rs' -exec wc -l {} \; 2>/dev/null | awk '{s+=$1} END{print s}')
N_JSX=$(find "$RELEASE_DIR/web/src" -name '*.jsx' 2>/dev/null | wc -l | tr -d ' ')
TOTAL_SIZE=$(du -sh "$RELEASE_DIR" | awk '{print $1}')
NOW=$(date -u +%Y-%m-%dT%H:%M:%SZ)
VERSION=$(awk -F'"' '/^version *= *"/ {print $2; exit}' "$SOURCE_DIR/Cargo.toml" 2>/dev/null || echo "0.1.0")

cat > "/tmp/cyberclaw-release-manifest-${VERSION}.md" <<EOF
# CyberClaw — release manifest

**Tag:** v${VERSION}
**Generated:** ${NOW}
**Total size:** ${TOTAL_SIZE}
**Built by:** \`scripts/release/build-release.sh\`

## Pre-compiled binaries

| Binary | Platform | Size | SHA-256 |
|---|---|---|---|
| \`bin/${TARGET_TRIPLE}/cyberclaw-server\` | ${TARGET_TRIPLE} | ${SIZE_SERVER} KB | \`${SHA_SERVER}\` |
| \`bin/${TARGET_TRIPLE}/cyberclaw-cli\` | ${TARGET_TRIPLE} | ${SIZE_CLI} KB | \`${SHA_CLI}\` |

For other platforms, build from source: \`cargo build --release\`.

## Component inventory

| Path | What it is |
|---|---|
| \`apps/\` | cyberclaw-server (HTTP + admin) and cyberclaw-cli source |
| \`crates/\` | ${N_CRATES} workspace crates |
| \`ecosystem/\` | ${N_AGENTS} agents · ${N_SKILLS} skills · ${N_CONN} connectors · ${N_PLUGS} platform-plugins |
| \`web/\` | Admin Console SPA: ${N_JSX} .jsx source files + Babel-compiled \`web/dist/\` |
| \`docs/\` | Architecture, API, deployment, security, getting-started, user-guide, reference, builders, configuration, modules |
| \`bin/\` | Pre-compiled binaries |
| \`schemas/\` | JSON Schemas for manifests |
| \`scripts/\` | Maintenance scripts |
| \`examples/\` | Runnable code samples |
| \`deploy/\` | Deployment recipes |

## Source code stats

- **${N_RUST}** Rust files
- **${N_LOC}** lines of Rust (production + embedded \`#[cfg(test)]\` unit tests)
- **${N_JSX}** JSX source files for the WebUI

## What's excluded

| Path | Reason |
|---|---|
| \`target/\` | Cargo build cache |
| \`tmp/\` \`claw-research/\` | Local research dumps |
| \`node_modules/\` | npm install artifacts |
| \`.git/\` | Internal sprint history. Run \`git init\` to start fresh |
| \`.env\` \`.env.test\` | Live credentials. Use \`.env.example\` |
| \`.omc/\` \`.staging/\` \`.serena/\` \`.claude/\` \`.spec-workflow/\` | Local agent / runtime state |
| \`.playwright-mcp/\` \`playwright-report/\` \`test-results/\` | E2E artifacts |
| \`docs/implementation/\` \`docs/development/\` \`docs/superpowers/\` | Internal sprint and process docs |
| \`scripts/testing/\` | Internal QA harness |
| \`tests/\` (top-level + per-crate) | Test code |
| \`web/debug/\` \`web/uploads/\` | Dev artifacts |
| \`AGENTS.md\` \`CLAUDE.md\` \`claude.md\` | AI dev guidance — internal only |
| Stray binaries: \`*.png\` \`*.pptx\` \`*.docx\` \`*.xlsx\` \`*.pdf\` \`*.zip\` at root | Capture artifacts |
| Database files: \`*.db\` \`*.sqlite\` \`*.profraw\` \`*.profdata\` \`*.log\` | Runtime state |
| Credentials: \`*.pem\` \`*.key\` \`secrets/\` \`credentials/\` | Defense in depth |

## How to publish

\`\`\`
cd ${RELEASE_DIR}
git init -b main
git add .
git commit -m "Release v${VERSION}"
git remote add origin git@github.com:OWNER/cyberclaw.git
git push -u origin main
git tag v${VERSION}
git push origin v${VERSION}
gh release create v${VERSION} \\
  bin/${TARGET_TRIPLE}/cyberclaw-server \\
  bin/${TARGET_TRIPLE}/cyberclaw-cli \\
  --title "v${VERSION}" --notes-file CHANGELOG.md
\`\`\`
EOF
ok "RELEASE_MANIFEST.md generated → /tmp/cyberclaw-release-manifest-${VERSION}.md (internal, do not ship)"

# ─────────────────────────────────────────────────────────────────────
say "8. pre-publish sanity checks"
# ─────────────────────────────────────────────────────────────────────
fails=0

# 8a) no .env leaked
leaked=$(find "$RELEASE_DIR" -name '.env' -o -name '.env.test' 2>/dev/null | grep -v '.env.example' || true)
[[ -z "$leaked" ]] && ok "no leaked .env files" || { warn "leaked: $leaked"; ((fails+=1)); }

# 8b) no .git
if [[ -d "$RELEASE_DIR/.git" ]]; then
  warn ".git/ present"; ((fails+=1))
else
  ok "no .git/"
fi

# 8c) no AI dev guidance
deve=$(find "$RELEASE_DIR" -maxdepth 2 \( -name 'AGENTS.md' -o -name 'CLAUDE.md' -o -name 'claude.md' \) 2>/dev/null || true)
[[ -z "$deve" ]] && ok "no AI dev guidance files" || { warn "found: $deve"; ((fails+=1)); }

# 8d) binaries run
if "$BIN_DIR/cyberclaw-cli" --version >/dev/null 2>&1; then
  ok "cli binary runs"
else
  warn "cli binary does not run on this host (likely cross-compile)"; ((fails+=1))
fi

# 8e) sentinel files present
for f in README.md LICENSE CHANGELOG.md ACKNOWLEDGMENTS.md CITATIONS.md PROJECT_STRUCTURE.md RELEASE_MANIFEST.md; do
  [[ -f "$RELEASE_DIR/$f" ]] && ok "$f present" || { warn "$f missing"; ((fails+=1)); }
done

# ─────────────────────────────────────────────────────────────────────
say "9. summary"
# ─────────────────────────────────────────────────────────────────────
echo "  release dir : $RELEASE_DIR"
echo "  size        : $TOTAL_SIZE"
echo "  ecosystem   : ${N_AGENTS} agents · ${N_SKILLS} skills · ${N_CONN} connectors · ${N_PLUGS} plugins"
echo "  crates      : $N_CRATES"
echo "  rust files  : $N_RUST"
echo "  rust LOC    : $N_LOC"
echo "  binaries    : $BIN_DIR/{cyberclaw-server,cyberclaw-cli}"
echo "  manifest    : $RELEASE_DIR/RELEASE_MANIFEST.md"
echo ""

if [[ $fails -gt 0 ]]; then
  fail "$fails sanity checks failed"
else
  echo -e "${GRN}✓ release ready: ${RELEASE_DIR}${RST}"
fi
