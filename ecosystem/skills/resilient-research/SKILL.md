---
name: resilient-research
description: Web research methodology with tool-fallback reflexes. When one tool returns empty/error, automatically substitute another tool from the palette before reporting failure.
source: cyberclaw (created via /api/v1/skills/create, 2026-05-03)
adapted-for: CyberClaw (Sprint 21, 2026-05-03)
level: 2
status: active
---

<!--
CyberClaw skill — methodology, not executable. Loaded by SkillRuntime
into the agent's system prompt during planning. Real execution still
flows Connector → Capability via the tool palette.

Origin: discovered during interactive QA on rc1 that agents would
report "web_search returned 0 results" and stop, instead of falling
back to web_fetch on a known canonical URL. This skill bakes the
fallback reflex into methodology so any agent that binds it inherits
the behavior.
-->

## When triggered

Any prompt asking for research, lookups, or "find information about X" — including indirect phrasings like "what is the URL of...", "summarize the latest...", "is there a project that does Y".

## Tool fallback reflexes (apply WITHOUT being told)

1. **`web_search` empty** → try **`web_fetch`** on a known canonical URL for the topic (e.g., the project's github / official site). DuckDuckGo Instant Answer returns 0 for most queries; this is expected, not a failure.
2. **`web_fetch` 4xx/5xx** → try a different URL path or the project's GitHub README via `web_fetch` on `https://raw.githubusercontent.com/<org>/<repo>/main/README.md`.
3. **`bash` command rejected by whitelist** → use the equivalent specialized tool: `file_list` instead of `ls -la`, `file_search` instead of `grep`, `file_read` instead of `cat`.

## Reporting contract

Always report:
- Which tools you tried (in order)
- What each returned (success / empty / error code)
- Which one finally succeeded
- A summary table is preferred when 2+ tools were used

Never report empty without saying what fallbacks you attempted. Empty results from one tool are a search-backend limitation, not a research failure.

## Anti-pattern

- ❌ "web_search returned no results" — and stop. Hides the platform's actual capability behind one tool's API limitation.
- ✅ "web_search returned 0 (DuckDuckGo Instant Answer is narrow); fell back to web_fetch on https://www.rust-lang.org/ which returned 200 with the canonical title and version 1.95.0."

## Verified behavior

Tested 2026-05-03 against MiniMax-M2.7-HighSpeed in staging:

**Without this skill bound:** agent emits 2 iterations, reports "No results found", stops.

**With this skill bound:** agent emits 3 iterations: (1) web_search → 0 results, (2) web_fetch → 200 with full HTML, (3) reports URL + version + maintainer details + localization list. Output structured per the reporting contract above.

The skill changes the agent's tool-selection behavior — same prompt, different reflexes. This is the foundational pattern for any operator who wants to give CyberClaw agents domain-specific reflexes without rewriting every caller's prompt.
