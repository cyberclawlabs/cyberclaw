# CyberClaw — release manifest

**Tag:** v1.2.17
**Generated:** 2026-05-23T08:36:28Z
**Total size:** 136M
**Built by:** `scripts/release/build-release.sh`

## Pre-compiled binaries

| Binary | Platform | Size | SHA-256 |
|---|---|---|---|
| `bin/aarch64-apple-darwin/cyberclaw-server` | aarch64-apple-darwin | 40992 KB | `a01460fc30d32174889a61b43348591aa5742789c1b6b9e6fc941691a2f8f43e` |
| `bin/aarch64-apple-darwin/cyberclaw-cli` | aarch64-apple-darwin | 13032 KB | `2b7dbec0a5e5902708168a8b459889b22ec88a7671c666d02af09030fb16c049` |

For other platforms, build from source: `cargo build --release`.

## Component inventory

| Path | What it is |
|---|---|
| `apps/` | cyberclaw-server (HTTP + admin) and cyberclaw-cli source |
| `crates/` | 15 workspace crates |
| `ecosystem/` | 11 agents · 116 skills · 9 connectors · 2 platform-plugins |
| `web/` | Admin Console SPA: 34 .jsx source files + Babel-compiled `web/dist/` |
| `docs/` | Architecture, API, deployment, security, getting-started, user-guide, reference, builders, configuration, modules |
| `bin/` | Pre-compiled binaries |
| `schemas/` | JSON Schemas for manifests |
| `scripts/` | Maintenance scripts |
| `examples/` | Runnable code samples |
| `deploy/` | Deployment recipes |

## Source code stats

- **503** Rust files
- **265740** lines of Rust (production + embedded `#[cfg(test)]` unit tests)
- **34** JSX source files for the WebUI

## What's excluded

| Path | Reason |
|---|---|
| `target/` | Cargo build cache |
| `tmp/` `claw-research/` | Local research dumps |
| `node_modules/` | npm install artifacts |
| `.git/` | Internal sprint history. Run `git init` to start fresh |
| `.env` `.env.test` | Live credentials. Use `.env.example` |
| `.omc/` `.staging/` `.serena/` `.claude/` `.spec-workflow/` | Local agent / runtime state |
| `.playwright-mcp/` `playwright-report/` `test-results/` | E2E artifacts |
| `docs/implementation/` `docs/development/` `docs/superpowers/` | Internal sprint and process docs |
| `scripts/testing/` | Internal QA harness |
| `tests/` (top-level + per-crate) | Test code |
| `web/debug/` `web/uploads/` | Dev artifacts |
| `AGENTS.md` `CLAUDE.md` `claude.md` | AI dev guidance — internal only |
| Stray binaries: `*.png` `*.pptx` `*.docx` `*.xlsx` `*.pdf` `*.zip` at root | Capture artifacts |
| Database files: `*.db` `*.sqlite` `*.profraw` `*.profdata` `*.log` | Runtime state |
| Credentials: `*.pem` `*.key` `secrets/` `credentials/` | Defense in depth |

## How to publish

```
cd /Users/max/project/cyberclaw-release
git init -b main
git add .
git commit -m "Release v1.2.17"
git remote add origin git@github.com:OWNER/cyberclaw.git
git push -u origin main
git tag v1.2.17
git push origin v1.2.17
gh release create v1.2.17 \
  bin/aarch64-apple-darwin/cyberclaw-server \
  bin/aarch64-apple-darwin/cyberclaw-cli \
  --title "v1.2.17" --notes-file CHANGELOG.md
```
