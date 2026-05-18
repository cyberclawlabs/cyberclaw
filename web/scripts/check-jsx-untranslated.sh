#!/bin/bash
# Wrapper for jsx-untranslated.py. Runs the AST-lite check that catches
# JSX text nodes / Title-Case English not behind dict() lookups.
#
# Separate file (not inline) because bash heredoc + Python with `<`/`>`
# JSX literals + bash array expansion conflict on every escape rule.

set -u
cd "$(dirname "$0")/.." || exit 2
exec python3 scripts/jsx-untranslated.py
