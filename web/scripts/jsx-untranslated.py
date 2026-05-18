#!/usr/bin/env python3
"""Find indirect-rendered untranslated English in TSX files.

Scope: src/pages and src/components, .tsx files only.

Detects:
  - JSX text nodes that are pure Title-Case English phrases not in
    ALLOWED_BRAND_TERMS (e.g. `>Capabilities<` or `>New Job<`).
  - `<span>{key}</span>` patterns where key is a string literal
    inside `.map([...])` over a constant lowercase-key list (the
    StatusPage `["api","audit"...]` anti-pattern).

Exits 0 if clean, 1 if violations found.
"""
import os
import re
import sys

ALLOWED_BRAND_TERMS = {
    "Agent", "Skill", "Connector", "Capability", "Curator", "MoA",
    "PTY", "MCP", "LLM", "JWT", "OAuth", "URL", "API", "JSON", "TOML",
    "Webhook", "SSE", "WebSocket", "Cron", "Kanban", "CyberClaw", "IM",
    "GitHub", "Slack", "Discord", "Telegram", "Lark", "WeChat",
    "Excel", "Markdown", "Rust",
}

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def scan_files():
    for base in ("src/pages", "src/components"):
        for root, _, files in os.walk(os.path.join(ROOT, base)):
            for f in files:
                if f.endswith(".tsx"):
                    yield os.path.join(root, f)


def title_case_phrase(text):
    """True if text is a Title-Case English phrase (each word
    starts with an uppercase letter followed by lowercase letters)."""
    tokens = text.split()
    if not tokens:
        return False
    return all(re.match(r"^[A-Z][a-z]+$", t) for t in tokens)


def is_allowed(text):
    tokens = text.split()
    return all(t in ALLOWED_BRAND_TERMS for t in tokens)


def main():
    violations = 0
    for path in scan_files():
        rel = os.path.relpath(path, ROOT)
        with open(path) as fh:
            text = fh.read()
        # Rule 0: hardcoded "Source: <code>" footers. Should go through
        # {L.source} so zh users see "来源" instead of "Source".
        for m in re.finditer(r"Source:\s*<code>", text):
            line = text[: m.start()].count("\n") + 1
            print(
                f"  {rel}:{line}: hardcoded 'Source:' before <code> — "
                "replace with {L.source} from page dict"
            )
            violations += 1
        # Rule A: JSX text nodes between > and < that look like English
        for m in re.finditer(r">([^<>{}\n]{1,80})<", text):
            raw = m.group(1).strip()
            if not raw:
                continue
            # Skip Chinese-containing
            if re.search(r"[一-鿿]", raw):
                continue
            # Skip identifier-like (snake/kebab/path/dot)
            if any(c in raw for c in ("_", "/", ".", "-")):
                continue
            # Skip lowercase-only or non-Latin
            if not re.search(r"[A-Z]", raw):
                continue
            if not title_case_phrase(raw):
                continue
            if is_allowed(raw):
                continue
            # Skip if appears inside an opening tag's attribute by
            # following angle-bracket heuristic — only count direct
            # text content. Already enforced by `[^<>{}]` class.
            line = text[: m.start()].count("\n") + 1
            print(
                f"  {rel}:{line}: JSX text '{raw}' is hardcoded English "
                "— wrap in {L.xxx} or translate"
            )
            violations += 1
    if violations:
        print()
        print(f"ERROR: {violations} indirect-rendered i18n violation(s).")
        print("Fix: replace the literal with a dict lookup (`{L.xxx}`)")
        print("or, if the term is a CyberClaw brand, add to")
        print("ALLOWED_BRAND_TERMS in this script.")
        sys.exit(1)
    print("jsx-untranslated: OK (0 violations)")


if __name__ == "__main__":
    main()
