//! Secret leak detection for execution outputs and artifacts.
//!
//! Scans text content for leaked secrets such as API keys, tokens,
//! and private keys using regex patterns and entropy analysis.

use regex::Regex;
use serde::{Deserialize, Serialize};

/// Severity level of a detected secret finding.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretSeverity {
    /// Informational, low risk
    Low,
    /// Medium risk, should be reviewed
    Medium,
    /// High risk, should be blocked
    High,
    /// Critical risk, must be blocked immediately
    Critical,
}

/// A detection pattern type for identifying secrets.
#[derive(Debug, Clone)]
pub enum SecretPattern {
    /// Regex-based pattern matching
    Regex {
        /// Human-readable name for this pattern
        name: String,
        /// The compiled regex pattern
        pattern: Regex,
        /// Severity when this pattern matches
        severity: SecretSeverity,
    },
    /// Entropy-based detection for high-entropy strings (e.g. random tokens)
    Entropy {
        /// Human-readable name for this pattern
        name: String,
        /// Minimum length of the string to check
        min_length: usize,
        /// Minimum Shannon entropy threshold (bits per character)
        min_entropy: f64,
        /// Severity when this pattern matches
        severity: SecretSeverity,
    },
}

impl SecretPattern {
    /// Return the name of this pattern.
    pub fn name(&self) -> &str {
        match self {
            SecretPattern::Regex { name, .. } => name,
            SecretPattern::Entropy { name, .. } => name,
        }
    }

    /// Return the severity of this pattern.
    pub fn severity(&self) -> &SecretSeverity {
        match self {
            SecretPattern::Regex { severity, .. } => severity,
            SecretPattern::Entropy { severity, .. } => severity,
        }
    }
}

/// A single finding from a secret scan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretFinding {
    /// Name of the pattern that matched
    pub pattern_name: String,
    /// Redacted/masked representation of the matched text
    pub matched_text: String,
    /// Line number where the secret was found (1-based)
    pub line_number: usize,
    /// Severity of this finding
    pub severity: SecretSeverity,
}

impl SecretFinding {
    /// Mask a matched string, showing only first 4 and last 4 characters.
    fn mask(text: &str) -> String {
        let chars: Vec<char> = text.chars().collect();
        let len = chars.len();
        if len <= 8 {
            return "*".repeat(len);
        }
        let prefix: String = chars[..4].iter().collect();
        let suffix: String = chars[len - 4..].iter().collect();
        format!("{}...{}", prefix, suffix)
    }

    fn new(
        pattern_name: &str,
        matched: &str,
        line_number: usize,
        severity: SecretSeverity,
    ) -> Self {
        Self {
            pattern_name: pattern_name.to_string(),
            matched_text: Self::mask(matched),
            line_number,
            severity,
        }
    }
}

/// Aggregated result of scanning a piece of content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretScanResult {
    /// Whether any secrets were found
    pub has_secrets: bool,
    /// All individual findings
    pub findings: Vec<SecretFinding>,
    /// The highest severity across all findings, if any
    pub severity: Option<SecretSeverity>,
}

impl SecretScanResult {
    fn from_findings(findings: Vec<SecretFinding>) -> Self {
        let max_severity = findings.iter().map(|f| f.severity.clone()).max();
        let has_secrets = !findings.is_empty();
        Self {
            has_secrets,
            findings,
            severity: max_severity,
        }
    }
}

/// Scanner that detects secrets in text content.
///
/// Holds an ordered list of [`SecretPattern`]s and runs them against
/// each line of the supplied content.
pub struct SecretScanner {
    patterns: Vec<SecretPattern>,
}

impl SecretScanner {
    /// Create a scanner with the given set of patterns.
    pub fn new(patterns: Vec<SecretPattern>) -> Self {
        Self { patterns }
    }

    /// Create a scanner with built-in patterns covering the most common
    /// secret types (AWS keys, GitHub tokens, JWTs, private keys, generic API keys).
    pub fn with_defaults() -> Self {
        let patterns = vec![
            SecretPattern::Regex {
                name: "aws-access-key".to_string(),
                pattern: Regex::new(r"AKIA[0-9A-Z]{16}").expect("valid regex"),
                severity: SecretSeverity::Critical,
            },
            SecretPattern::Regex {
                name: "github-token".to_string(),
                pattern: Regex::new(r"gh[ps]_[A-Za-z0-9_]{36,}").expect("valid regex"),
                severity: SecretSeverity::Critical,
            },
            SecretPattern::Regex {
                name: "jwt-token".to_string(),
                pattern: Regex::new(r"eyJ[A-Za-z0-9_-]+\.eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+")
                    .expect("valid regex"),
                severity: SecretSeverity::Critical,
            },
            SecretPattern::Regex {
                name: "private-key-header".to_string(),
                pattern: Regex::new(r"-----BEGIN (RSA |EC |DSA )?PRIVATE KEY-----")
                    .expect("valid regex"),
                severity: SecretSeverity::Critical,
            },
            SecretPattern::Regex {
                name: "generic-api-key".to_string(),
                pattern: Regex::new(
                    r#"(?i)(api[_-]?key|apikey|secret[_-]?key)\s*[=:]\s*["']?[A-Za-z0-9+/=]{20,}"#,
                )
                .expect("valid regex"),
                severity: SecretSeverity::High,
            },
        ];
        Self { patterns }
    }

    /// Add an additional pattern to this scanner.
    pub fn add_pattern(&mut self, pattern: SecretPattern) {
        self.patterns.push(pattern);
    }

    /// Scan `content` for secrets.
    ///
    /// Returns a [`SecretScanResult`] containing all findings.
    pub fn scan(&self, content: &str) -> SecretScanResult {
        let mut findings = Vec::new();

        for (line_idx, line) in content.lines().enumerate() {
            let line_number = line_idx + 1;
            for pattern in &self.patterns {
                match pattern {
                    SecretPattern::Regex {
                        name,
                        pattern: re,
                        severity,
                    } => {
                        for mat in re.find_iter(line) {
                            findings.push(SecretFinding::new(
                                name,
                                mat.as_str(),
                                line_number,
                                severity.clone(),
                            ));
                        }
                    }
                    SecretPattern::Entropy {
                        name,
                        min_length,
                        min_entropy,
                        severity,
                    } => {
                        // Check each whitespace-delimited token on the line.
                        for token in line.split_whitespace() {
                            if token.len() >= *min_length {
                                let entropy = shannon_entropy(token);
                                if entropy >= *min_entropy {
                                    findings.push(SecretFinding::new(
                                        name,
                                        token,
                                        line_number,
                                        severity.clone(),
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }

        SecretScanResult::from_findings(findings)
    }
}

impl Default for SecretScanner {
    fn default() -> Self {
        Self::with_defaults()
    }
}

/// Calculate the Shannon entropy of a string in bits per character.
fn shannon_entropy(s: &str) -> f64 {
    if s.is_empty() {
        return 0.0;
    }
    let len = s.len() as f64;
    let mut counts = [0u32; 256];
    for byte in s.bytes() {
        counts[byte as usize] += 1;
    }
    counts
        .iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = c as f64 / len;
            -p * p.log2()
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scanner() -> SecretScanner {
        SecretScanner::with_defaults()
    }

    #[test]
    fn test_detects_aws_key() {
        let content = "export AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE123\nsome other line";
        let result = scanner().scan(content);
        assert!(result.has_secrets, "should detect AWS access key");
        assert!(
            result
                .findings
                .iter()
                .any(|f| f.pattern_name == "aws-access-key"),
            "finding should be named aws-access-key"
        );
        assert_eq!(result.findings[0].line_number, 1);
        assert_eq!(result.severity, Some(SecretSeverity::Critical));
    }

    #[test]
    fn test_detects_github_token() {
        let content = "GITHUB_TOKEN=ghp_abcdefghijklmnopqrstuvwxyzABCDEF1234567890";
        let result = scanner().scan(content);
        assert!(result.has_secrets, "should detect GitHub token");
        assert!(
            result
                .findings
                .iter()
                .any(|f| f.pattern_name == "github-token"),
            "finding should be named github-token"
        );
        assert_eq!(result.severity, Some(SecretSeverity::Critical));
    }

    #[test]
    fn test_detects_jwt() {
        let jwt = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.\
                   eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiaWF0IjoxNTE2MjM5MDIyfQ.\
                   SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";
        let content = format!("Authorization: Bearer {}", jwt);
        let result = scanner().scan(&content);
        assert!(result.has_secrets, "should detect JWT");
        assert!(
            result
                .findings
                .iter()
                .any(|f| f.pattern_name == "jwt-token"),
            "finding should be named jwt-token"
        );
        assert_eq!(result.severity, Some(SecretSeverity::Critical));
    }

    #[test]
    fn test_detects_private_key() {
        let content =
            "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA...\n-----END RSA PRIVATE KEY-----";
        let result = scanner().scan(content);
        assert!(result.has_secrets, "should detect private key header");
        assert!(
            result
                .findings
                .iter()
                .any(|f| f.pattern_name == "private-key-header"),
            "finding should be named private-key-header"
        );
        assert_eq!(result.severity, Some(SecretSeverity::Critical));
    }

    #[test]
    fn test_safe_content_no_false_positives() {
        let content = "Hello, world!\nThis is a normal log line.\nStatus: OK\nUser: alice";
        let result = scanner().scan(content);
        assert!(
            !result.has_secrets,
            "safe content should not trigger findings, got: {:?}",
            result.findings
        );
        assert_eq!(result.severity, None);
    }

    #[test]
    fn test_finding_text_is_masked() {
        let content = "export AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE123";
        let result = scanner().scan(content);
        assert!(result.has_secrets);
        let finding = &result.findings[0];
        // Masked text should not reveal the full secret
        assert!(
            finding.matched_text.contains("..."),
            "matched_text should be masked, got: {}",
            finding.matched_text
        );
        assert!(
            !finding.matched_text.contains("AKIAIOSFODNN7EXAMPLE123"),
            "masked text must not contain the raw secret"
        );
    }

    #[test]
    fn test_entropy_pattern() {
        let mut s = SecretScanner::new(vec![SecretPattern::Entropy {
            name: "high-entropy-token".to_string(),
            min_length: 20,
            min_entropy: 3.5,
            severity: SecretSeverity::Medium,
        }]);
        // A highly random base64-like string should trigger entropy detection
        let secret = "aB3xQ9zR1mK7pL2nW8vY4cF6jH0tE5sD";
        let content = format!("token={}", secret);
        let result = s.scan(&content);
        assert!(result.has_secrets, "high-entropy string should be detected");

        // A short repeating string should not trigger
        s = SecretScanner::new(vec![SecretPattern::Entropy {
            name: "high-entropy-token".to_string(),
            min_length: 20,
            min_entropy: 3.5,
            severity: SecretSeverity::Medium,
        }]);
        let result2 = s.scan("aaaaaaaaaaaaaaaaaaaaaa");
        assert!(
            !result2.has_secrets,
            "low-entropy string should not trigger"
        );
    }
}
