//! Sprint D4: PortabilityVerifier — static analysis of `SKILL.md` to determine
//! the portability tier of a skill (Tier1 / Tier2 / Tier3).
//!
//! # What this module does
//!
//! Scans a `SKILL.md` (frontmatter + body) without executing anything and
//! produces a [`PortabilityReport`] describing:
//!
//! - The set of `capability_id` strings extracted from the
//!   `allowed-tools` / `metadata.required-capabilities` /
//!   `metadata.capabilities` frontmatter, and any `<cap>capability.id</cap>`
//!   tags found in the body.
//! - The portability **tier** (highest-numbered tier wins; an empty skill is
//!   Tier1) computed via the [`PORTABILITY_RULES`] table.
//! - Warnings for unknown capability ids that don't match any rule.
//!
//! # Tier semantics
//!
//! - **Tier 1** — fully sandboxed, OS-level primitives only:
//!   `cmd.*`, `fs.*`, `search.*`, `lsp.*`. Skills at this tier should run
//!   anywhere CyberClaw runs.
//! - **Tier 2** — needs trusted local services: `http.*`, `database.*`,
//!   `git.*`. Portable across environments that grant network/database access.
//! - **Tier 3** — depends on third-party / paid SaaS APIs:
//!   `openai.*`, `anthropic.*`, `slack.*`, `minimax.*`. Not portable across
//!   environments without provisioning the remote integration.
//!
//! # Where this fits
//!
//! Curator integration is intentionally **deferred** — see
//! [`PortabilityVerifier::scan_path`] callers in any future Curator wiring.
//! This module ships as a self-contained verifier with its own unit tests so
//! Sprint D4 closes without touching the Curator dispatch path.

use std::path::Path;

/// The portability tier of a skill.
///
/// Numerically ordered: `Tier1 < Tier2 < Tier3`. The skill's tier is the
/// **maximum** tier of any capability it references — if a skill uses one
/// `openai.*` call alongside ten `cmd.*` calls, the tier is `Tier3`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PortabilityTier {
    /// Sandboxed primitives only (cmd / fs / search / lsp).
    Tier1,
    /// Local trusted services (http / database / git).
    Tier2,
    /// Third-party SaaS providers (openai / anthropic / slack / minimax).
    Tier3,
}

impl PortabilityTier {
    /// Numeric label, useful for serialisation and display.
    pub fn as_u8(self) -> u8 {
        match self {
            PortabilityTier::Tier1 => 1,
            PortabilityTier::Tier2 => 2,
            PortabilityTier::Tier3 => 3,
        }
    }
}

/// Hard-coded `prefix → tier` table. Order matters: longer prefixes should
/// come first, but in practice the prefixes are disjoint at the dot
/// boundary so the order does not affect correctness.
///
/// To add a capability family, append a new `(prefix, tier)` entry. Tests
/// check that every prefix here is matched by at least one fixture.
pub const PORTABILITY_RULES: &[(&str, PortabilityTier)] = &[
    // --- Tier 1: sandboxed primitives ---
    ("cmd.", PortabilityTier::Tier1),
    ("fs.", PortabilityTier::Tier1),
    ("search.", PortabilityTier::Tier1),
    ("lsp.", PortabilityTier::Tier1),
    // --- Tier 2: trusted local services ---
    ("http.", PortabilityTier::Tier2),
    ("database.", PortabilityTier::Tier2),
    ("git.", PortabilityTier::Tier2),
    // --- Tier 3: third-party SaaS ---
    ("openai.", PortabilityTier::Tier3),
    ("anthropic.", PortabilityTier::Tier3),
    ("slack.", PortabilityTier::Tier3),
    ("minimax.", PortabilityTier::Tier3),
];

/// Result of a single static portability scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortabilityReport {
    /// Highest tier observed across all referenced capabilities. An empty
    /// skill (no capabilities) is reported as [`PortabilityTier::Tier1`]
    /// because such a skill does not need any non-portable resource.
    pub tier: PortabilityTier,
    /// Sorted, deduplicated list of capability ids the skill references.
    pub required_capabilities: Vec<String>,
    /// Free-form warnings — currently used to flag capability ids that
    /// match no rule in [`PORTABILITY_RULES`].
    pub warnings: Vec<String>,
}

impl PortabilityReport {
    /// Convenience: any capability id that did not match a rule.
    pub fn has_unknown_capabilities(&self) -> bool {
        self.warnings
            .iter()
            .any(|w| w.starts_with("unknown capability"))
    }
}

/// Static portability scanner.
#[derive(Debug, Default)]
pub struct PortabilityVerifier;

impl PortabilityVerifier {
    pub fn new() -> Self {
        Self
    }

    /// Scan a SKILL.md text (frontmatter + body) and produce a
    /// [`PortabilityReport`].
    ///
    /// This function does not parse the YAML strictly — it only extracts
    /// strings that **look like** capability ids (kebab-cased segments with a
    /// dot separator) from a few well-known locations and from any
    /// `<cap>...</cap>` tags in the body. Strict YAML validation is
    /// [`crate::skill_validator`]'s job.
    pub fn scan_skill_md(&self, contents: &str) -> PortabilityReport {
        let caps = extract_capability_ids(contents);
        self.report_from_caps(caps)
    }

    /// Scan a `SKILL.md` on disk. Errors are surfaced as warnings (so
    /// callers always get a [`PortabilityReport`] even when the file is
    /// missing or unreadable).
    pub fn scan_path(&self, path: &Path) -> PortabilityReport {
        match std::fs::read_to_string(path) {
            Ok(s) => self.scan_skill_md(&s),
            Err(e) => PortabilityReport {
                tier: PortabilityTier::Tier1,
                required_capabilities: vec![],
                warnings: vec![format!(
                    "could not read SKILL.md at {}: {}",
                    path.display(),
                    e
                )],
            },
        }
    }

    fn report_from_caps(&self, mut caps: Vec<String>) -> PortabilityReport {
        caps.sort();
        caps.dedup();

        if caps.is_empty() {
            return PortabilityReport {
                tier: PortabilityTier::Tier1,
                required_capabilities: vec![],
                warnings: vec![],
            };
        }

        let mut tier = PortabilityTier::Tier1;
        let mut warnings = Vec::new();
        for cap in &caps {
            match classify_capability(cap) {
                Some(t) => {
                    if t > tier {
                        tier = t;
                    }
                }
                None => warnings.push(format!(
                    "unknown capability '{}' — not covered by any PORTABILITY_RULES prefix",
                    cap
                )),
            }
        }
        PortabilityReport {
            tier,
            required_capabilities: caps,
            warnings,
        }
    }
}

/// Classify a single capability id by matching it against the
/// [`PORTABILITY_RULES`] prefix table. Returns `None` when no prefix matches.
pub fn classify_capability(cap: &str) -> Option<PortabilityTier> {
    for (prefix, tier) in PORTABILITY_RULES {
        if cap.starts_with(prefix) {
            return Some(*tier);
        }
    }
    None
}

/// Extract every capability-id-shaped string from the SKILL.md.
///
/// The extractor is intentionally cheap and rules-light:
///
/// 1. Anything inside `<cap>...</cap>` tags in the body.
/// 2. Comma- or whitespace-separated values under any frontmatter line that
///    looks like `allowed-tools:`, `required-capabilities:`,
///    `capabilities:`, or YAML list `- entry` lines that follow such a key.
/// 3. Hash-prefixed `#cap:` markers anywhere in the body
///    (e.g. `#cap:openai.embeddings`).
///
/// Nothing outside those locations is treated as a capability — we want
/// the scanner to be conservative so that "the SKILL.md mentions
/// `cmd.run`" in a casual prose doesn't bump the tier.
fn extract_capability_ids(contents: &str) -> Vec<String> {
    let mut out = Vec::new();

    // 1. <cap>...</cap> tags.
    let mut rest = contents;
    while let Some(start) = rest.find("<cap>") {
        let after = &rest[start + 5..];
        if let Some(end) = after.find("</cap>") {
            let raw = after[..end].trim();
            if looks_like_capability_id(raw) {
                out.push(raw.to_string());
            }
            rest = &after[end + 6..];
        } else {
            break;
        }
    }

    // 2. Frontmatter list/inline values.
    //    We do a line-based scan rather than a strict YAML parse because
    //    SKILL.md authors are inconsistent and we only need approximate hits.
    let mut active_key: Option<&'static str> = None;
    for line in contents.lines() {
        let trimmed = line.trim_start();
        let lower = trimmed.to_ascii_lowercase();
        // Detect a key line.
        let key = if lower.starts_with("allowed-tools:") {
            Some("allowed-tools")
        } else if lower.starts_with("required-capabilities:") {
            Some("required-capabilities")
        } else if lower.starts_with("capabilities:") {
            Some("capabilities")
        } else {
            None
        };
        if let Some(k) = key {
            active_key = Some(k);
            // Inline list / scalar: `allowed-tools: [a, b]` or
            // `allowed-tools: cmd.run`.
            let after_colon = trimmed.split_once(':').map(|(_, v)| v).unwrap_or("");
            for token in split_inline_values(after_colon) {
                if looks_like_capability_id(&token) {
                    out.push(token);
                }
            }
            continue;
        }
        // Continuation: a line under the active key.
        if let Some(_k) = active_key {
            // YAML list line `  - cmd.run`.
            if let Some(dash) = trimmed.strip_prefix('-') {
                let val = dash.trim().trim_matches(|c: char| c == '"' || c == '\'');
                if looks_like_capability_id(val) {
                    out.push(val.to_string());
                }
                continue;
            }
            // A non-list, non-empty, non-keyed line ends the active key.
            if !trimmed.is_empty() && !trimmed.starts_with('#') && !trimmed.starts_with('-') {
                active_key = None;
            }
        }
    }

    // 3. `#cap:foo.bar` markers in the body.
    for marker in contents.split("#cap:").skip(1) {
        let raw: String = marker
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '_' || *c == '-')
            .collect();
        if looks_like_capability_id(&raw) {
            out.push(raw);
        }
    }

    out
}

/// Split an inline frontmatter value like `[a, b, c]` or `a, b` into raw
/// tokens. Square brackets are stripped, then comma- or whitespace-separated
/// tokens are returned with surrounding quotes removed.
fn split_inline_values(raw: &str) -> Vec<String> {
    let cleaned = raw.trim().trim_start_matches('[').trim_end_matches(']');
    cleaned
        .split(|c: char| c == ',' || c.is_whitespace())
        .map(|s| {
            s.trim()
                .trim_matches(|c: char| c == '"' || c == '\'')
                .to_string()
        })
        .filter(|s| !s.is_empty())
        .collect()
}

/// Cheap capability-id sniff: must contain a `.`, must be all
/// `[a-z0-9._-]`, must not start or end with `.`. Anything else is a false
/// positive (likely prose or a path).
fn looks_like_capability_id(raw: &str) -> bool {
    if raw.is_empty() || raw.len() > 128 {
        return false;
    }
    if !raw.contains('.') {
        return false;
    }
    if raw.starts_with('.') || raw.ends_with('.') {
        return false;
    }
    raw.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verify(s: &str) -> PortabilityReport {
        PortabilityVerifier::new().scan_skill_md(s)
    }

    // --------------------------------------------------------------------
    // Tier classification
    // --------------------------------------------------------------------

    #[test]
    fn classify_capability_covers_all_three_tiers() {
        assert_eq!(classify_capability("cmd.run"), Some(PortabilityTier::Tier1));
        assert_eq!(classify_capability("fs.read"), Some(PortabilityTier::Tier1));
        assert_eq!(
            classify_capability("search.glob"),
            Some(PortabilityTier::Tier1)
        );
        assert_eq!(
            classify_capability("lsp.diagnostics"),
            Some(PortabilityTier::Tier1)
        );

        assert_eq!(
            classify_capability("http.get"),
            Some(PortabilityTier::Tier2)
        );
        assert_eq!(
            classify_capability("database.query"),
            Some(PortabilityTier::Tier2)
        );
        assert_eq!(
            classify_capability("git.diff"),
            Some(PortabilityTier::Tier2)
        );

        assert_eq!(
            classify_capability("openai.embed"),
            Some(PortabilityTier::Tier3)
        );
        assert_eq!(
            classify_capability("anthropic.chat"),
            Some(PortabilityTier::Tier3)
        );
        assert_eq!(
            classify_capability("slack.post"),
            Some(PortabilityTier::Tier3)
        );
        assert_eq!(
            classify_capability("minimax.tts"),
            Some(PortabilityTier::Tier3)
        );

        assert_eq!(classify_capability("foo.bar"), None);
    }

    #[test]
    fn empty_skill_is_tier_one() {
        let r = verify("---\nname: empty\ndescription: nothing here\n---\n\n# Empty\n");
        assert_eq!(r.tier, PortabilityTier::Tier1);
        assert!(r.required_capabilities.is_empty());
        assert!(r.warnings.is_empty());
    }

    // --------------------------------------------------------------------
    // Single-tier extraction
    // --------------------------------------------------------------------

    #[test]
    fn allowed_tools_inline_list_extracts_caps_tier1() {
        let md = "---\nname: t1\ndescription: t\nallowed-tools: [cmd.run, fs.read]\n---\n";
        let r = verify(md);
        assert_eq!(r.tier, PortabilityTier::Tier1);
        assert_eq!(r.required_capabilities, vec!["cmd.run", "fs.read"]);
        assert!(r.warnings.is_empty());
    }

    #[test]
    fn allowed_tools_yaml_list_extracts_caps_tier2() {
        let md = "---\nname: t2\ndescription: t\nallowed-tools:\n  - http.get\n  - git.diff\n---\n";
        let r = verify(md);
        assert_eq!(r.tier, PortabilityTier::Tier2);
        assert_eq!(r.required_capabilities, vec!["git.diff", "http.get"]);
    }

    #[test]
    fn cap_tag_in_body_extracts_caps_tier3() {
        let md = r#"---
name: chat-bot
description: writes essays
---

This skill calls <cap>openai.chat</cap> and <cap>anthropic.chat</cap>.
"#;
        let r = verify(md);
        assert_eq!(r.tier, PortabilityTier::Tier3);
        assert_eq!(
            r.required_capabilities,
            vec!["anthropic.chat", "openai.chat"]
        );
    }

    // --------------------------------------------------------------------
    // Mixed tiers — highest wins
    // --------------------------------------------------------------------

    #[test]
    fn mixed_tier_wins_with_tier3() {
        let md = r#"---
name: hybrid
description: both
allowed-tools:
  - cmd.run
  - http.get
  - openai.embed
---

Body uses <cap>fs.read</cap>.
"#;
        let r = verify(md);
        assert_eq!(r.tier, PortabilityTier::Tier3);
        // All four caps should be captured and sorted.
        assert_eq!(
            r.required_capabilities,
            vec!["cmd.run", "fs.read", "http.get", "openai.embed"]
        );
        assert!(r.warnings.is_empty());
    }

    #[test]
    fn mixed_tier_wins_with_tier2_only() {
        let md = "---\nname: net\ndescription: x\nallowed-tools: [cmd.run, http.post]\n---\n";
        let r = verify(md);
        assert_eq!(r.tier, PortabilityTier::Tier2);
    }

    // --------------------------------------------------------------------
    // Unknown capabilities surface as warnings, do NOT bump tier
    // --------------------------------------------------------------------

    #[test]
    fn unknown_capability_emits_warning() {
        let md = "---\nname: u\ndescription: x\nallowed-tools: [foo.bar, cmd.run]\n---\n";
        let r = verify(md);
        // cmd.run is Tier1; foo.bar is unknown.
        assert_eq!(r.tier, PortabilityTier::Tier1);
        assert!(r.has_unknown_capabilities());
        assert_eq!(r.required_capabilities, vec!["cmd.run", "foo.bar"]);
        assert_eq!(r.warnings.len(), 1);
        assert!(r.warnings[0].contains("foo.bar"));
    }

    // --------------------------------------------------------------------
    // Markers / dedup / file IO
    // --------------------------------------------------------------------

    #[test]
    fn hash_cap_marker_in_body_is_picked_up() {
        let md = "---\nname: m\ndescription: x\n---\n\nUse #cap:slack.post here.\n";
        let r = verify(md);
        assert_eq!(r.tier, PortabilityTier::Tier3);
        assert_eq!(r.required_capabilities, vec!["slack.post"]);
    }

    #[test]
    fn duplicate_capabilities_are_deduplicated() {
        let md = r#"---
name: dup
description: x
allowed-tools: [cmd.run, cmd.run]
---

Body: <cap>cmd.run</cap> #cap:cmd.run again.
"#;
        let r = verify(md);
        assert_eq!(r.required_capabilities, vec!["cmd.run"]);
    }

    #[test]
    fn scan_path_handles_missing_file() {
        let v = PortabilityVerifier::new();
        let r = v.scan_path(Path::new("/tmp/_nope_d4_portability_test.md"));
        assert_eq!(r.tier, PortabilityTier::Tier1);
        assert_eq!(r.warnings.len(), 1);
        assert!(r.warnings[0].contains("could not read"));
    }

    #[test]
    fn scan_path_reads_real_file() {
        // Sprint D4: file-backed path mirrors scan_skill_md when readable.
        let tmp = tempfile::TempDir::new().unwrap();
        let p = tmp.path().join("SKILL.md");
        std::fs::write(
            &p,
            "---\nname: io\ndescription: x\nallowed-tools: [git.diff]\n---\n",
        )
        .unwrap();
        let r = PortabilityVerifier::new().scan_path(&p);
        assert_eq!(r.tier, PortabilityTier::Tier2);
        assert_eq!(r.required_capabilities, vec!["git.diff"]);
    }

    // --------------------------------------------------------------------
    // Robustness — bad input does not panic
    // --------------------------------------------------------------------

    #[test]
    fn malformed_cap_tag_does_not_panic() {
        let md = "---\nname: bad\ndescription: x\n---\n\nThis has <cap>nope without close tag.";
        let r = verify(md);
        // Unclosed tag → no extracted cap → tier 1.
        assert_eq!(r.tier, PortabilityTier::Tier1);
        assert!(r.required_capabilities.is_empty());
    }

    #[test]
    fn looks_like_capability_id_rejects_non_caps() {
        assert!(!looks_like_capability_id("no-dot"));
        assert!(!looks_like_capability_id(".leading"));
        assert!(!looks_like_capability_id("trailing."));
        assert!(!looks_like_capability_id("UPPER.case"));
        assert!(looks_like_capability_id("cmd.run"));
        assert!(looks_like_capability_id("openai.chat-completions"));
    }
}
