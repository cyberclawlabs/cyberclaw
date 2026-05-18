//! Runtime secret leak detection for execution outputs.
//!
//! Scans runtime output data (tool results, agent responses, logs) for
//! leaked secrets using a two-phase approach:
//!
//! 1. **Aho-Corasick fast scan** - Multi-pattern prefix matching to quickly
//!    eliminate clean content without running expensive regex.
//! 2. **Regex precise match** - Only runs on content where a known prefix
//!    was detected, confirming the full pattern.
//!
//! # Difference from `secret_scanner`
//!
//! - [`SecretScanner`](crate::secret_scanner::SecretScanner) targets static
//!   analysis of code and configuration files (line-by-line, entropy-based).
//! - [`LeakDetector`] targets runtime output streams where speed matters
//!   and the action (block/redact/warn) drives control flow.

use std::ops::Range;

use aho_corasick::AhoCorasick;
use regex::Regex;
use serde::{Deserialize, Serialize};

/// Action to take when a leak is detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeakAction {
    /// Block the output entirely (for critical secrets).
    Block,
    /// Redact the secret, replacing it with `[REDACTED]`.
    Redact,
    /// Log a warning but allow the output.
    Warn,
}

impl std::fmt::Display for LeakAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LeakAction::Block => write!(f, "block"),
            LeakAction::Redact => write!(f, "redact"),
            LeakAction::Warn => write!(f, "warn"),
        }
    }
}

/// Severity of a detected leak.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeakSeverity {
    /// Informational, low risk.
    Low,
    /// Should be reviewed.
    Medium,
    /// Should be blocked or redacted.
    High,
    /// Must be blocked immediately.
    Critical,
}

impl std::fmt::Display for LeakSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LeakSeverity::Low => write!(f, "low"),
            LeakSeverity::Medium => write!(f, "medium"),
            LeakSeverity::High => write!(f, "high"),
            LeakSeverity::Critical => write!(f, "critical"),
        }
    }
}

/// A pattern for detecting secret leaks in runtime output.
#[derive(Debug, Clone)]
pub struct LeakPattern {
    /// Human-readable name for this pattern (e.g. "aws_access_key").
    pub name: String,
    /// Compiled regex for precise matching.
    pub regex: Regex,
    /// Severity when this pattern matches.
    pub severity: LeakSeverity,
    /// Action to take on match.
    pub action: LeakAction,
}

/// A single detected potential secret leak.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeakMatch {
    /// Name of the pattern that matched.
    pub pattern_name: String,
    /// Severity of this match.
    pub severity: LeakSeverity,
    /// Recommended action for this match.
    pub action: LeakAction,
    /// Byte offset range in the scanned content.
    pub location: Range<usize>,
    /// A preview of the match with the secret partially masked.
    pub masked_preview: String,
}

/// Result of scanning content for leaks.
#[derive(Debug)]
pub struct LeakScanResult {
    /// All detected potential leaks.
    pub matches: Vec<LeakMatch>,
    /// The recommended action based on the most severe match.
    pub recommended_action: Option<LeakAction>,
}

impl LeakScanResult {
    /// Check if content is clean (no leaks detected).
    pub fn is_clean(&self) -> bool {
        self.matches.is_empty()
    }

    /// Get the highest severity found.
    pub fn max_severity(&self) -> Option<LeakSeverity> {
        self.matches.iter().map(|m| m.severity).max()
    }

    /// Whether any match requires blocking.
    pub fn should_block(&self) -> bool {
        self.matches.iter().any(|m| m.action == LeakAction::Block)
    }

    /// Content with secrets redacted (for matches marked as Redact).
    pub fn redacted_content(&self, original: &str) -> Option<String> {
        let redact_ranges: Vec<Range<usize>> = self
            .matches
            .iter()
            .filter(|m| m.action == LeakAction::Redact)
            .map(|m| m.location.clone())
            .collect();

        if redact_ranges.is_empty() {
            return None;
        }
        Some(apply_redactions(original, &redact_ranges))
    }
}

/// Runtime secret leak detector using Aho-Corasick + regex.
pub struct LeakDetector {
    patterns: Vec<LeakPattern>,
    /// Aho-Corasick automaton for fast prefix scanning.
    prefix_matcher: Option<AhoCorasick>,
    /// Maps each prefix entry to its pattern index.
    known_prefixes: Vec<(String, usize)>,
}

impl LeakDetector {
    /// Create a detector with default patterns covering common secret types.
    pub fn new() -> Self {
        Self::with_patterns(default_patterns())
    }

    /// Create a detector with custom patterns.
    pub fn with_patterns(patterns: Vec<LeakPattern>) -> Self {
        let mut prefixes = Vec::new();
        for (idx, pattern) in patterns.iter().enumerate() {
            if let Some(prefix) = extract_literal_prefix(pattern.regex.as_str()) {
                if prefix.len() >= 3 {
                    prefixes.push((prefix, idx));
                }
            }
        }

        let prefix_matcher = if !prefixes.is_empty() {
            let prefix_strings: Vec<&str> = prefixes.iter().map(|(s, _)| s.as_str()).collect();
            AhoCorasick::builder()
                .ascii_case_insensitive(false)
                .build(&prefix_strings)
                .ok()
        } else {
            None
        };

        Self {
            patterns,
            prefix_matcher,
            known_prefixes: prefixes,
        }
    }

    /// Scan content for potential secret leaks.
    pub fn scan(&self, content: &str) -> LeakScanResult {
        let mut matches = Vec::new();

        // Phase 1: Use Aho-Corasick to find candidate pattern indices.
        let candidate_indices: Vec<usize> = if let Some(ref matcher) = self.prefix_matcher {
            let mut indices = Vec::new();
            for mat in matcher.find_iter(content) {
                let found_prefix = &self.known_prefixes[mat.pattern().as_usize()].0;
                for (other_prefix, other_idx) in &self.known_prefixes {
                    if (other_prefix.starts_with(found_prefix.as_str())
                        || found_prefix.starts_with(other_prefix.as_str()))
                        && !indices.contains(other_idx)
                    {
                        indices.push(*other_idx);
                    }
                }
            }
            // Include patterns without extractable prefixes.
            for (idx, _) in self.patterns.iter().enumerate() {
                if !self.known_prefixes.iter().any(|(_, i)| *i == idx) && !indices.contains(&idx) {
                    indices.push(idx);
                }
            }
            indices
        } else {
            (0..self.patterns.len()).collect()
        };

        // Phase 2: Run regex only on candidate patterns.
        for idx in candidate_indices {
            let pattern = &self.patterns[idx];
            for mat in pattern.regex.find_iter(content) {
                let matched_text = mat.as_str();
                let location = mat.start()..mat.end();

                matches.push(LeakMatch {
                    pattern_name: pattern.name.clone(),
                    severity: pattern.severity,
                    action: pattern.action,
                    location,
                    masked_preview: mask_secret(matched_text),
                });
            }
        }

        // Sort by location for stable output.
        matches.sort_by_key(|m| m.location.start);

        let recommended_action = if matches.iter().any(|m| m.action == LeakAction::Block) {
            Some(LeakAction::Block)
        } else if matches.iter().any(|m| m.action == LeakAction::Redact) {
            Some(LeakAction::Redact)
        } else if !matches.is_empty() {
            Some(LeakAction::Warn)
        } else {
            None
        };

        LeakScanResult {
            matches,
            recommended_action,
        }
    }

    /// Add a custom pattern at runtime.
    ///
    /// Note: the Aho-Corasick prefix matcher is not rebuilt; for full
    /// performance benefit, construct a new detector via `with_patterns`.
    pub fn add_pattern(&mut self, pattern: LeakPattern) {
        self.patterns.push(pattern);
    }

    /// Get the number of registered patterns.
    pub fn pattern_count(&self) -> usize {
        self.patterns.len()
    }
}

impl Default for LeakDetector {
    fn default() -> Self {
        Self::new()
    }
}

/// Mask a secret for safe display, showing first 4 and last 4 characters.
fn mask_secret(secret: &str) -> String {
    let chars: Vec<char> = secret.chars().collect();
    let len = chars.len();
    if len <= 8 {
        return "*".repeat(len);
    }
    let prefix: String = chars[..4].iter().collect();
    let suffix: String = chars[len - 4..].iter().collect();
    let middle_len = (len - 8).min(8);
    format!("{}{}{}", prefix, "*".repeat(middle_len), suffix)
}

/// Apply redaction ranges to content, replacing matched regions with `[REDACTED]`.
fn apply_redactions(content: &str, ranges: &[Range<usize>]) -> String {
    if ranges.is_empty() {
        return content.to_string();
    }

    let mut sorted = ranges.to_vec();
    sorted.sort_by_key(|r| r.start);

    // Merge overlapping ranges to prevent garbled output
    let mut merged: Vec<Range<usize>> = Vec::new();
    for range in &sorted {
        if let Some(last) = merged.last_mut() {
            if range.start <= last.end {
                last.end = last.end.max(range.end);
                continue;
            }
        }
        merged.push(range.clone());
    }

    let mut result = String::with_capacity(content.len());
    let mut last_end = 0;

    for range in &merged {
        if range.start > last_end {
            result.push_str(&content[last_end..range.start]);
        }
        result.push_str("[REDACTED]");
        last_end = range.end;
    }

    if last_end < content.len() {
        result.push_str(&content[last_end..]);
    }

    result
}

/// Extract a literal prefix from a regex pattern string.
///
/// Returns `None` if no prefix of at least 3 characters can be extracted.
fn extract_literal_prefix(pattern: &str) -> Option<String> {
    let mut prefix = String::new();

    for ch in pattern.chars() {
        match ch {
            '[' | '(' | '.' | '*' | '+' | '?' | '{' | '|' | '^' | '$' | '\\' => break,
            _ => prefix.push(ch),
        }
    }

    if prefix.len() >= 3 {
        Some(prefix)
    } else {
        None
    }
}

/// Build the default set of leak detection patterns.
fn default_patterns() -> Vec<LeakPattern> {
    vec![
        // AWS Access Key ID
        LeakPattern {
            name: "aws_access_key".to_string(),
            regex: Regex::new(r"AKIA[0-9A-Z]{16}").expect("valid regex"),
            severity: LeakSeverity::Critical,
            action: LeakAction::Block,
        },
        // GitHub tokens (classic PAT, OAuth, etc.)
        LeakPattern {
            name: "github_token".to_string(),
            regex: Regex::new(r"gh[pousr]_[A-Za-z0-9_]{36,}").expect("valid regex"),
            severity: LeakSeverity::Critical,
            action: LeakAction::Block,
        },
        // GitHub fine-grained PAT
        LeakPattern {
            name: "github_fine_grained_pat".to_string(),
            regex: Regex::new(r"github_pat_[a-zA-Z0-9]{22}_[a-zA-Z0-9]{59}").expect("valid regex"),
            severity: LeakSeverity::Critical,
            action: LeakAction::Block,
        },
        // JWT tokens
        LeakPattern {
            name: "jwt_token".to_string(),
            regex: Regex::new(r"eyJ[A-Za-z0-9_-]+\.eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+")
                .expect("valid regex"),
            severity: LeakSeverity::High,
            action: LeakAction::Redact,
        },
        // PEM private keys (RSA)
        LeakPattern {
            name: "pem_private_key".to_string(),
            regex: Regex::new(r"-----BEGIN\s+(?:RSA\s+)?PRIVATE\s+KEY-----").expect("valid regex"),
            severity: LeakSeverity::Critical,
            action: LeakAction::Block,
        },
        // SSH/EC/DSA private keys
        LeakPattern {
            name: "ssh_private_key".to_string(),
            regex: Regex::new(r"-----BEGIN\s+(?:OPENSSH|EC|DSA)\s+PRIVATE\s+KEY-----")
                .expect("valid regex"),
            severity: LeakSeverity::Critical,
            action: LeakAction::Block,
        },
        // Password in URL (e.g. https://user:pass@host)
        LeakPattern {
            name: "password_in_url".to_string(),
            regex: Regex::new(r"://[^/\s]+:[^/\s]+@[^/\s]+").expect("valid regex"),
            severity: LeakSeverity::High,
            action: LeakAction::Redact,
        },
        // Generic API key assignment
        LeakPattern {
            name: "generic_api_key".to_string(),
            regex: Regex::new(
                r#"(?i)(api[_-]?key|apikey|secret[_-]?key)\s*[=:]\s*["']?[A-Za-z0-9+/=]{20,}"#,
            )
            .expect("valid regex"),
            severity: LeakSeverity::High,
            action: LeakAction::Redact,
        },
        // Slack tokens
        LeakPattern {
            name: "slack_token".to_string(),
            regex: Regex::new(r"xox[baprs]-[0-9a-zA-Z-]{10,}").expect("valid regex"),
            severity: LeakSeverity::High,
            action: LeakAction::Block,
        },
        // Stripe API keys
        LeakPattern {
            name: "stripe_api_key".to_string(),
            regex: Regex::new(r"sk_(?:live|test)_[a-zA-Z0-9]{24,}").expect("valid regex"),
            severity: LeakSeverity::Critical,
            action: LeakAction::Block,
        },
        // Google API keys
        LeakPattern {
            name: "google_api_key".to_string(),
            regex: Regex::new(r"AIza[0-9A-Za-z_-]{35}").expect("valid regex"),
            severity: LeakSeverity::High,
            action: LeakAction::Block,
        },
        // Bearer tokens (redact, may be intentional in some contexts)
        LeakPattern {
            name: "bearer_token".to_string(),
            regex: Regex::new(r"Bearer\s+[a-zA-Z0-9_-]{20,}").expect("valid regex"),
            severity: LeakSeverity::High,
            action: LeakAction::Redact,
        },
        // High-entropy 64-char hex strings (potential secrets/hashes)
        LeakPattern {
            name: "high_entropy_hex".to_string(),
            regex: Regex::new(r"\b[a-fA-F0-9]{64}\b").expect("valid regex"),
            severity: LeakSeverity::Medium,
            action: LeakAction::Warn,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detector() -> LeakDetector {
        LeakDetector::new()
    }

    // ── Detection tests ──────────────────────────────────────────

    #[test]
    fn detects_aws_access_key() {
        let result = detector().scan("export AWS_KEY=AKIAIOSFODNN7EXAMPLE");
        assert!(!result.is_clean());
        assert!(result.should_block());
        assert!(result
            .matches
            .iter()
            .any(|m| m.pattern_name == "aws_access_key"));
    }

    #[test]
    fn detects_github_token() {
        let token = format!("ghp_{}", "x".repeat(36));
        let result = detector().scan(&format!("TOKEN={token}"));
        assert!(!result.is_clean());
        assert!(result
            .matches
            .iter()
            .any(|m| m.pattern_name == "github_token"));
    }

    #[test]
    fn detects_github_fine_grained_pat() {
        let token = format!("github_pat_{}_{}", "a".repeat(22), "b".repeat(59));
        let result = detector().scan(&token);
        assert!(!result.is_clean());
        assert!(result
            .matches
            .iter()
            .any(|m| m.pattern_name == "github_fine_grained_pat"));
    }

    #[test]
    fn detects_jwt_token() {
        let jwt = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.\
                   eyJzdWIiOiIxMjM0NTY3ODkwIn0.\
                   SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";
        let result = detector().scan(&format!("Authorization: Bearer {jwt}"));
        assert!(!result.is_clean());
        assert!(result.matches.iter().any(|m| m.pattern_name == "jwt_token"));
    }

    #[test]
    fn detects_pem_private_key() {
        let content = "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA...";
        let result = detector().scan(content);
        assert!(!result.is_clean());
        assert!(result.should_block());
        assert!(result
            .matches
            .iter()
            .any(|m| m.pattern_name == "pem_private_key"));
    }

    #[test]
    fn detects_ssh_private_key() {
        let content = "-----BEGIN OPENSSH PRIVATE KEY-----\nbase64data==";
        let result = detector().scan(content);
        assert!(!result.is_clean());
        assert!(result
            .matches
            .iter()
            .any(|m| m.pattern_name == "ssh_private_key"));
    }

    #[test]
    fn detects_password_in_url() {
        let content = "DATABASE_URL=postgres://admin:s3cret@db.example.com:5432/mydb";
        let result = detector().scan(content);
        assert!(!result.is_clean());
        assert!(result
            .matches
            .iter()
            .any(|m| m.pattern_name == "password_in_url"));
    }

    #[test]
    fn detects_generic_api_key() {
        let content = r#"config = { api_key: "aB3xQ9zR1mK7pL2nW8vY4cF6" }"#;
        let result = detector().scan(content);
        assert!(!result.is_clean());
        assert!(result
            .matches
            .iter()
            .any(|m| m.pattern_name == "generic_api_key"));
    }

    #[test]
    fn detects_slack_token() {
        let content = "xoxb-1234567890-abcdefghij";
        let result = detector().scan(content);
        assert!(!result.is_clean());
        assert!(result
            .matches
            .iter()
            .any(|m| m.pattern_name == "slack_token"));
    }

    #[test]
    fn detects_stripe_key() {
        let content = format!("sk_{}_aAbBcCdDfFgGhHjJkKmMnNpPqQ", "live");
        let result = detector().scan(&content);
        assert!(!result.is_clean());
        assert!(result
            .matches
            .iter()
            .any(|m| m.pattern_name == "stripe_api_key"));
    }

    #[test]
    fn detects_google_api_key() {
        let key = format!("AIza{}", "x".repeat(35));
        let result = detector().scan(&key);
        assert!(!result.is_clean());
        assert!(result
            .matches
            .iter()
            .any(|m| m.pattern_name == "google_api_key"));
    }

    // ── Clean content tests ──────────────────────────────────────

    #[test]
    fn clean_content_no_false_positives() {
        let texts = [
            "Hello, world! This is a normal log line.",
            "Status: OK, User: alice",
            "The API returns a JSON response",
            "Use ssh to connect to the server",
            "sk-this-is-too-short",
        ];
        for text in texts {
            let result = detector().scan(text);
            assert!(!result.should_block(), "clean text falsely blocked: {text}");
        }
    }

    // ── Masking tests ────────────────────────────────────────────

    #[test]
    fn mask_secret_short_value() {
        assert_eq!(mask_secret("abc"), "***");
        assert_eq!(mask_secret("12345678"), "********");
    }

    #[test]
    fn mask_secret_long_value() {
        let masked = mask_secret("sk-test1234567890abcdef");
        assert!(masked.starts_with("sk-t"));
        assert!(masked.ends_with("cdef"));
        assert!(masked.contains('*'));
    }

    #[test]
    fn matched_text_is_masked() {
        let result = detector().scan("export AWS_KEY=AKIAIOSFODNN7EXAMPLE");
        assert!(!result.is_clean());
        let finding = &result.matches[0];
        assert!(
            finding.masked_preview.contains('*'),
            "preview should be masked: {}",
            finding.masked_preview
        );
        assert!(
            !finding.masked_preview.contains("AKIAIOSFODNN7EXAMPLE"),
            "full secret must not appear in preview"
        );
    }

    // ── Redaction tests ──────────────────────────────────────────

    #[test]
    fn redact_bearer_token() {
        let content = "Header: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9_longtokenvalue";
        let result = detector().scan(content);
        assert!(!result.is_clean());

        let redacted = result.redacted_content(content);
        assert!(redacted.is_some());
        let redacted = redacted.unwrap();
        assert!(redacted.contains("[REDACTED]"));
    }

    // ── Multiple matches ─────────────────────────────────────────

    #[test]
    fn multiple_different_secrets() {
        let content = format!(
            "AWS: AKIAIOSFODNN7EXAMPLE and GitHub: ghp_{}",
            "x".repeat(36)
        );
        let result = detector().scan(&content);
        assert!(
            result.matches.len() >= 2,
            "expected 2+ matches, got {}",
            result.matches.len()
        );
    }

    // ── Severity ordering ────────────────────────────────────────

    #[test]
    fn severity_ordering() {
        assert!(LeakSeverity::Critical > LeakSeverity::High);
        assert!(LeakSeverity::High > LeakSeverity::Medium);
        assert!(LeakSeverity::Medium > LeakSeverity::Low);
    }

    // ── Recommended action logic ─────────────────────────────────

    #[test]
    fn recommended_action_is_block_when_critical() {
        let result = detector().scan("AKIAIOSFODNN7EXAMPLE");
        assert_eq!(result.recommended_action, Some(LeakAction::Block));
    }

    #[test]
    fn recommended_action_is_none_for_clean() {
        let result = detector().scan("just normal text");
        assert_eq!(result.recommended_action, None);
    }

    // ── Performance guard ────────────────────────────────────────

    #[test]
    fn scan_100kb_clean_text_under_100ms() {
        let payload = "The quick brown fox jumps over the lazy dog. ".repeat(2500);
        assert!(payload.len() > 100_000);

        let start = std::time::Instant::now();
        let result = detector().scan(&payload);
        let elapsed = start.elapsed();

        assert!(result.is_clean());
        // Debug builds are significantly slower due to lack of optimizations.
        // Use a relaxed threshold for debug, strict for release.
        let threshold_ms: u128 = if cfg!(debug_assertions) { 300 } else { 100 };
        assert!(
            elapsed.as_millis() < threshold_ms,
            "scan took {}ms on 100KB clean text (threshold: {}ms)",
            elapsed.as_millis(),
            threshold_ms,
        );
    }

    // ── Edge cases ───────────────────────────────────────────────

    #[test]
    fn empty_content() {
        let result = detector().scan("");
        assert!(result.is_clean());
        assert_eq!(result.recommended_action, None);
    }

    #[test]
    fn secret_at_different_positions() {
        let key = "AKIAIOSFODNN7EXAMPLE";

        let result = detector().scan(key);
        assert!(!result.is_clean(), "key at start");

        let result = detector().scan(&format!("prefix {key} suffix"));
        assert!(!result.is_clean(), "key in middle");

        let result = detector().scan(&format!("end: {key}"));
        assert!(!result.is_clean(), "key at end");
    }

    #[test]
    fn custom_pattern() {
        let mut det = LeakDetector::with_patterns(vec![]);
        det.add_pattern(LeakPattern {
            name: "custom_token".to_string(),
            regex: Regex::new(r"mytoken_[a-z]{10,}").expect("valid regex"),
            severity: LeakSeverity::Medium,
            action: LeakAction::Warn,
        });
        let result = det.scan("auth: mytoken_abcdefghijkl");
        assert!(!result.is_clean());
        assert_eq!(result.matches[0].pattern_name, "custom_token");
    }

    #[test]
    fn apply_redactions_non_overlapping() {
        let content = "prefix SECRET1 middle SECRET2 suffix";
        let ranges = vec![7..14, 22..29];
        let redacted = apply_redactions(content, &ranges);
        assert_eq!(redacted, "prefix [REDACTED] middle [REDACTED] suffix");
    }
}
