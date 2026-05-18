//! Declarative TOML-driven output filter pipeline for Capability outputs.
//!
//! This module provides an 8-stage filter pipeline that can be configured via TOML
//! to transform, redact, and truncate capability output before it reaches callers.
//!
//! # Dependency
//!
//! Requires `toml = "0.8"` in `[dependencies]` (available as `toml.workspace = true`).
//!
//! # Example
//!
//! ```toml
//! schema_version = 1
//!
//! [filters.strip-build-noise]
//! description = "Remove verbose compiler output"
//! match_command = "^cargo\\s+build"
//! strip_lines_matching = ["^\\s+Compiling ", "^\\s+Finished"]
//! max_lines = 50
//! ```
//!
//! ```rust
//! use cyberclaw_connectors::toml_filter::{FilterEngine, FilterResult};
//!
//! let toml = r#"
//! schema_version = 1
//! [filters.example]
//! match_command = "^echo"
//! strip_ansi = true
//! "#;
//! let engine = FilterEngine::from_toml(toml).unwrap();
//! let result = engine.apply("echo hello", "hello");
//! assert!(matches!(result, FilterResult::Filtered { .. }));
//! ```

use std::collections::BTreeMap;

use regex::Regex;
use serde::Deserialize;

// ─── Configuration types (deserialized from TOML) ────────────────────────────

/// Top-level filter configuration file.
#[derive(Debug, Deserialize)]
pub struct FilterConfig {
    /// Schema version for forward compatibility (must be 1).
    pub schema_version: u32,
    /// Named filter definitions.
    #[serde(default)]
    pub filters: BTreeMap<String, FilterDef>,
    /// Inline test cases keyed by filter name.
    #[serde(default)]
    pub tests: BTreeMap<String, Vec<FilterTestDef>>,
}

/// A single filter definition with an 8-stage pipeline.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilterDef {
    /// Human-readable description of what this filter does.
    pub description: Option<String>,
    /// Regex pattern matched against the command/capability name.
    pub match_command: String,
    /// Stage 1: strip ANSI escape codes before any other processing.
    #[serde(default)]
    pub strip_ansi: bool,
    /// Stage 2: regex substitutions applied line-by-line.
    #[serde(default)]
    pub replace: Vec<ReplaceRule>,
    /// Stage 3: short-circuit rules on full output match.
    #[serde(default)]
    pub match_output: Vec<MatchOutputRule>,
    /// Stage 4a: discard lines whose content matches any of these patterns.
    #[serde(default)]
    pub strip_lines_matching: Vec<String>,
    /// Stage 4b: keep only lines matching at least one of these patterns.
    ///
    /// If empty, no line-keep filtering is applied.
    #[serde(default)]
    pub keep_lines_matching: Vec<String>,
    /// Stage 5: truncate each line to at most N characters.
    pub truncate_lines_at: Option<usize>,
    /// Stage 6a: keep only the first N lines.
    pub head_lines: Option<usize>,
    /// Stage 6b: keep only the last N lines.
    pub tail_lines: Option<usize>,
    /// Stage 7: absolute cap on total lines returned.
    pub max_lines: Option<usize>,
    /// Stage 8: replacement message when the result is empty after all stages.
    pub on_empty: Option<String>,
}

/// A single regex substitution rule applied per line.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplaceRule {
    /// Regex pattern to match.
    pub pattern: String,
    /// Replacement string (supports `$1` capture group references).
    pub replacement: String,
}

/// A short-circuit rule evaluated against the full output string.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MatchOutputRule {
    /// Regex pattern to match against the entire output.
    pub pattern: String,
    /// Message to return when the pattern matches (and `unless` does not).
    pub message: String,
    /// Optional regex: if this also matches, the rule is skipped.
    #[serde(default)]
    pub unless: Option<String>,
}

/// An inline test case for a named filter.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilterTestDef {
    /// Human-readable name for this test case.
    pub name: String,
    /// Raw input string fed into the filter pipeline.
    pub input: String,
    /// Expected output string after filtering.
    pub expected: String,
}

// ─── Compiled types ──────────────────────────────────────────────────────────

struct CompiledMatchOutputRule {
    pattern: Regex,
    message: String,
    unless: Option<Regex>,
}

/// A compiled filter ready for zero-copy execution.
pub struct CompiledFilter {
    /// Name of this filter (from the TOML key).
    pub name: String,
    /// Optional description.
    pub description: Option<String>,
    match_re: Regex,
    strip_ansi: bool,
    replace_rules: Vec<(Regex, String)>,
    match_output_rules: Vec<CompiledMatchOutputRule>,
    strip_line_res: Vec<Regex>,
    keep_line_res: Vec<Regex>,
    truncate_lines_at: Option<usize>,
    head_lines: Option<usize>,
    tail_lines: Option<usize>,
    max_lines: Option<usize>,
    on_empty: Option<String>,
}

impl CompiledFilter {
    /// Apply all 8 pipeline stages to `output` and return the transformed string.
    fn apply_pipeline(&self, output: &str) -> String {
        // Stage 1: strip ANSI escape codes
        let owned;
        let after_ansi: &str = if self.strip_ansi {
            owned = strip_ansi_codes(output);
            &owned
        } else {
            output
        };

        // Stage 2: line-by-line regex substitutions
        let after_replace: String = if self.replace_rules.is_empty() {
            after_ansi.to_string()
        } else {
            after_ansi
                .lines()
                .map(|line| {
                    let mut l = line.to_string();
                    for (re, repl) in &self.replace_rules {
                        l = re.replace_all(&l, repl.as_str()).to_string();
                    }
                    l
                })
                .collect::<Vec<_>>()
                .join("\n")
        };

        // Stage 3: full-output short-circuit match
        for rule in &self.match_output_rules {
            if rule.pattern.is_match(&after_replace) {
                let skip = rule
                    .unless
                    .as_ref()
                    .map(|u| u.is_match(&after_replace))
                    .unwrap_or(false);
                if !skip {
                    return rule.message.clone();
                }
            }
        }

        // Stage 4: line filtering (strip then keep)
        let mut lines: Vec<&str> = after_replace.lines().collect();

        if !self.strip_line_res.is_empty() {
            lines.retain(|l| !self.strip_line_res.iter().any(|re| re.is_match(l)));
        }

        if !self.keep_line_res.is_empty() {
            lines.retain(|l| self.keep_line_res.iter().any(|re| re.is_match(l)));
        }

        // Stage 5: per-line truncation
        let truncated: Vec<String> = if let Some(n) = self.truncate_lines_at {
            lines
                .iter()
                .map(|l| {
                    if l.len() > n {
                        // UTF-8 safe: find the last char boundary at or before n
                        let end = l
                            .char_indices()
                            .map(|(i, _)| i)
                            .take_while(|&i| i <= n)
                            .last()
                            .unwrap_or(0);
                        l[..end].to_string()
                    } else {
                        l.to_string()
                    }
                })
                .collect()
        } else {
            lines.iter().map(|l| l.to_string()).collect()
        };

        // Stage 6: head / tail
        let selected: Vec<String> = match (self.head_lines, self.tail_lines) {
            (Some(h), _) => truncated.into_iter().take(h).collect(),
            (None, Some(t)) => {
                let len = truncated.len();
                let skip = len.saturating_sub(t);
                truncated.into_iter().skip(skip).collect()
            }
            (None, None) => truncated,
        };

        // Stage 7: absolute line cap
        let capped: Vec<String> = if let Some(m) = self.max_lines {
            selected.into_iter().take(m).collect()
        } else {
            selected
        };

        // Stage 8: on_empty fallback
        if capped.is_empty() {
            self.on_empty.clone().unwrap_or_default()
        } else {
            capped.join("\n")
        }
    }
}

// ─── Error types ─────────────────────────────────────────────────────────────

/// Errors that can occur when building or running a [`FilterEngine`].
#[derive(Debug)]
pub enum FilterError {
    /// The TOML source could not be parsed.
    ParseError(String),
    /// A regex pattern inside a filter definition is invalid.
    RegexError { filter: String, source: String },
    /// The `schema_version` field is not supported.
    UnsupportedSchema(u32),
}

impl std::fmt::Display for FilterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FilterError::ParseError(msg) => write!(f, "TOML parse error: {}", msg),
            FilterError::RegexError { filter, source } => {
                write!(f, "Invalid regex in filter '{}': {}", filter, source)
            }
            FilterError::UnsupportedSchema(v) => {
                write!(f, "Schema version {} not supported (expected 1)", v)
            }
        }
    }
}

impl std::error::Error for FilterError {}

// ─── Result types ─────────────────────────────────────────────────────────────

/// Outcome of applying the filter engine to a command and its output.
#[derive(Debug)]
pub enum FilterResult {
    /// A filter matched and transformed the output.
    Filtered {
        /// Name of the filter that matched.
        filter_name: String,
        /// Transformed output text.
        output: String,
        /// Number of lines in the original input.
        input_lines: usize,
        /// Number of lines in the filtered output.
        output_lines: usize,
    },
    /// No filter matched — the output should be passed through unchanged.
    Passthrough,
}

/// Result of running an inline test case.
#[derive(Debug)]
pub struct TestResult {
    /// Name of the filter this test belongs to.
    pub filter_name: String,
    /// Human-readable test case name.
    pub test_name: String,
    /// Whether expected == actual.
    pub passed: bool,
    /// The expected output string.
    pub expected: String,
    /// The actual output string produced by the pipeline.
    pub actual: String,
}

// ─── FilterEngine ─────────────────────────────────────────────────────────────

/// The main filter engine — loads TOML configurations and dispatches output
/// through the matching filter's 8-stage pipeline.
pub struct FilterEngine {
    filters: Vec<CompiledFilter>,
}

impl FilterEngine {
    /// Build a [`FilterEngine`] from a single TOML configuration string.
    ///
    /// # Errors
    ///
    /// Returns [`FilterError::ParseError`] if the TOML is malformed,
    /// [`FilterError::UnsupportedSchema`] if `schema_version != 1`, or
    /// [`FilterError::RegexError`] if any regex fails to compile.
    pub fn from_toml(toml_str: &str) -> Result<Self, FilterError> {
        let config: FilterConfig =
            toml::from_str(toml_str).map_err(|e| FilterError::ParseError(e.to_string()))?;
        Self::from_config(config)
    }

    /// Build a [`FilterEngine`] by merging multiple TOML configuration strings.
    ///
    /// Filters are appended in source order; when dispatching, the first match wins.
    ///
    /// # Errors
    ///
    /// Returns the first error encountered while parsing any source.
    pub fn from_toml_sources(sources: &[&str]) -> Result<Self, FilterError> {
        let mut all_filters = Vec::new();
        for src in sources {
            let config: FilterConfig =
                toml::from_str(src).map_err(|e| FilterError::ParseError(e.to_string()))?;
            if config.schema_version != 1 {
                return Err(FilterError::UnsupportedSchema(config.schema_version));
            }
            let engine = Self::from_config(config)?;
            all_filters.extend(engine.filters);
        }
        Ok(Self {
            filters: all_filters,
        })
    }

    /// Find the first filter whose `match_command` pattern matches `command`,
    /// apply its pipeline to `output`, and return the result.
    ///
    /// Returns [`FilterResult::Passthrough`] when no filter matches.
    pub fn apply(&self, command: &str, output: &str) -> FilterResult {
        for filter in &self.filters {
            if filter.match_re.is_match(command) {
                let transformed = filter.apply_pipeline(output);
                let input_lines = output.lines().count();
                let output_lines = transformed.lines().count();
                return FilterResult::Filtered {
                    filter_name: filter.name.clone(),
                    output: transformed,
                    input_lines,
                    output_lines,
                };
            }
        }
        FilterResult::Passthrough
    }

    /// Execute all inline test cases defined in `config` against this engine.
    ///
    /// Each test applies the named filter (looked up by command pattern match on
    /// `"__test__:<filter_name>"`) and compares the result to the expected string.
    pub fn run_tests(&self, config: &FilterConfig) -> Vec<TestResult> {
        let mut results = Vec::new();
        for (filter_name, test_defs) in &config.tests {
            // Synthesise a command that will match the filter by name prefix.
            let synthetic_cmd = format!("__test__:{}", filter_name);
            // Find the filter directly by name since the synthetic command won't
            // necessarily match match_command. Use the filter's pipeline directly.
            let filter = self.filters.iter().find(|f| &f.name == filter_name);
            for test_def in test_defs {
                let actual = match filter {
                    Some(f) => f.apply_pipeline(&test_def.input),
                    None => {
                        // Fall back to engine dispatch (may return passthrough).
                        match self.apply(&synthetic_cmd, &test_def.input) {
                            FilterResult::Filtered { output, .. } => output,
                            FilterResult::Passthrough => test_def.input.clone(),
                        }
                    }
                };
                let passed = actual == test_def.expected;
                results.push(TestResult {
                    filter_name: filter_name.clone(),
                    test_name: test_def.name.clone(),
                    passed,
                    expected: test_def.expected.clone(),
                    actual,
                });
            }
        }
        results
    }

    // ── private helpers ──────────────────────────────────────────────────────

    fn from_config(config: FilterConfig) -> Result<Self, FilterError> {
        if config.schema_version != 1 {
            return Err(FilterError::UnsupportedSchema(config.schema_version));
        }

        let mut filters = Vec::with_capacity(config.filters.len());

        for (name, def) in config.filters {
            let match_re = compile_regex(&name, &def.match_command)?;

            let replace_rules = def
                .replace
                .iter()
                .map(|r| {
                    let re = compile_regex(&name, &r.pattern)?;
                    Ok((re, r.replacement.clone()))
                })
                .collect::<Result<Vec<_>, FilterError>>()?;

            let match_output_rules = def
                .match_output
                .iter()
                .map(|r| {
                    let pattern = compile_regex(&name, &r.pattern)?;
                    let unless = r
                        .unless
                        .as_deref()
                        .map(|p| compile_regex(&name, p))
                        .transpose()?;
                    Ok(CompiledMatchOutputRule {
                        pattern,
                        message: r.message.clone(),
                        unless,
                    })
                })
                .collect::<Result<Vec<_>, FilterError>>()?;

            let strip_line_res = def
                .strip_lines_matching
                .iter()
                .map(|p| compile_regex(&name, p))
                .collect::<Result<Vec<_>, FilterError>>()?;

            let keep_line_res = def
                .keep_lines_matching
                .iter()
                .map(|p| compile_regex(&name, p))
                .collect::<Result<Vec<_>, FilterError>>()?;

            filters.push(CompiledFilter {
                name,
                description: def.description,
                match_re,
                strip_ansi: def.strip_ansi,
                replace_rules,
                match_output_rules,
                strip_line_res,
                keep_line_res,
                truncate_lines_at: def.truncate_lines_at,
                head_lines: def.head_lines,
                tail_lines: def.tail_lines,
                max_lines: def.max_lines,
                on_empty: def.on_empty,
            });
        }

        Ok(Self { filters })
    }
}

// ─── Internal helpers ─────────────────────────────────────────────────────────

fn compile_regex(filter_name: &str, pattern: &str) -> Result<Regex, FilterError> {
    Regex::new(pattern).map_err(|e| FilterError::RegexError {
        filter: filter_name.to_string(),
        source: e.to_string(),
    })
}

/// Remove ANSI escape sequences (e.g. colour codes) from a string.
fn strip_ansi_codes(input: &str) -> String {
    // Matches ESC [ ... letter sequences (CSI sequences) and ESC letter (Fe sequences).
    static ANSI_RE: once_cell::sync::Lazy<Regex> = once_cell::sync::Lazy::new(|| {
        Regex::new(r"\x1b\[[0-9;]*[a-zA-Z]").expect("ANSI regex must compile")
    });
    ANSI_RE.replace_all(input, "").to_string()
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── helpers ──────────────────────────────────────────────────────────────

    fn engine_from(toml_str: &str) -> FilterEngine {
        FilterEngine::from_toml(toml_str).expect("valid TOML config")
    }

    // ── test cases ───────────────────────────────────────────────────────────

    /// Parse a minimal valid TOML configuration without errors.
    #[test]
    fn test_parse_basic_config() {
        let toml = r#"
schema_version = 1
[filters.basic]
match_command = "^cargo"
strip_ansi = true
"#;
        let config: FilterConfig = toml::from_str(toml).expect("should parse");
        assert_eq!(config.schema_version, 1);
        assert!(config.filters.contains_key("basic"));
    }

    /// Stage 1: ANSI codes are stripped before other stages.
    #[test]
    fn test_strip_ansi() {
        let toml = r#"
schema_version = 1
[filters.ansi]
match_command = "^cmd"
strip_ansi = true
"#;
        let engine = engine_from(toml);
        let input = "\x1b[32mGreen text\x1b[0m and plain";
        match engine.apply("cmd run", input) {
            FilterResult::Filtered { output, .. } => {
                assert!(!output.contains("\x1b["), "ANSI codes should be removed");
                assert!(output.contains("Green text"));
            }
            FilterResult::Passthrough => panic!("expected a match"),
        }
    }

    /// Stage 2: regex replace rules transform matching lines.
    #[test]
    fn test_replace_rules() {
        let toml = r#"
schema_version = 1
[filters.replace]
match_command = "^build"
[[filters.replace.replace]]
pattern = "error: "
replacement = "ERR: "
"#;
        let engine = engine_from(toml);
        match engine.apply("build all", "error: something went wrong") {
            FilterResult::Filtered { output, .. } => {
                assert!(output.contains("ERR: "), "replacement should apply");
                assert!(!output.contains("error: "));
            }
            FilterResult::Passthrough => panic!("expected a match"),
        }
    }

    /// Stage 3: full-output match triggers short-circuit return.
    #[test]
    fn test_match_output_shortcircuit() {
        let toml = r#"
schema_version = 1
[filters.sc]
match_command = "^deploy"
[[filters.sc.match_output]]
pattern = "Build successful"
message = "OK"
"#;
        let engine = engine_from(toml);
        match engine.apply("deploy app", "Build successful\nSome extra lines") {
            FilterResult::Filtered { output, .. } => assert_eq!(output, "OK"),
            FilterResult::Passthrough => panic!("expected a match"),
        }
    }

    /// Stage 3: `unless` field prevents short-circuit when the guard matches.
    #[test]
    fn test_match_output_unless() {
        let toml = r#"
schema_version = 1
[filters.unless]
match_command = "^test"
[[filters.unless.match_output]]
pattern = "FAILED"
message = "some tests failed"
unless = "0 failures"
"#;
        let engine = engine_from(toml);
        // "0 failures" is present → unless fires → no short-circuit
        match engine.apply("test run", "FAILED\n0 failures") {
            FilterResult::Filtered { output, .. } => {
                assert_ne!(
                    output, "some tests failed",
                    "unless should prevent short-circuit"
                );
            }
            FilterResult::Passthrough => panic!("expected a match"),
        }
        // "0 failures" absent → short-circuit fires
        match engine.apply("test run", "FAILED\n3 failures") {
            FilterResult::Filtered { output, .. } => {
                assert_eq!(output, "some tests failed");
            }
            FilterResult::Passthrough => panic!("expected a match"),
        }
    }

    /// Stage 4a: lines matching strip patterns are removed.
    #[test]
    fn test_strip_lines() {
        let toml = r#"
schema_version = 1
[filters.strip]
match_command = "^make"
strip_lines_matching = ["^\\s*#"]
"#;
        let engine = engine_from(toml);
        let input = "# comment\nreal line\n  # another comment\nfinal";
        match engine.apply("make all", input) {
            FilterResult::Filtered { output, .. } => {
                assert!(!output.contains("# comment"));
                assert!(output.contains("real line"));
                assert!(output.contains("final"));
            }
            FilterResult::Passthrough => panic!("expected a match"),
        }
    }

    /// Stage 4b: only lines matching keep patterns are retained.
    #[test]
    fn test_keep_lines() {
        let toml = r#"
schema_version = 1
[filters.keep]
match_command = "^log"
keep_lines_matching = ["ERROR", "WARN"]
"#;
        let engine = engine_from(toml);
        let input = "INFO: all good\nERROR: oops\nDEBUG: verbose\nWARN: careful";
        match engine.apply("log tail", input) {
            FilterResult::Filtered { output, .. } => {
                assert!(output.contains("ERROR"));
                assert!(output.contains("WARN"));
                assert!(!output.contains("INFO"));
                assert!(!output.contains("DEBUG"));
            }
            FilterResult::Passthrough => panic!("expected a match"),
        }
    }

    /// Stage 5: lines longer than `truncate_lines_at` are cut.
    #[test]
    fn test_truncate_lines() {
        let toml = r#"
schema_version = 1
[filters.trunc]
match_command = "^run"
truncate_lines_at = 10
"#;
        let engine = engine_from(toml);
        let input = "short\nthis line is definitely longer than ten characters";
        match engine.apply("run cmd", input) {
            FilterResult::Filtered { output, .. } => {
                for line in output.lines() {
                    assert!(line.len() <= 10, "line '{}' exceeds 10 chars", line);
                }
            }
            FilterResult::Passthrough => panic!("expected a match"),
        }
    }

    /// Stage 6: `head_lines` and `tail_lines` select correct subsets.
    #[test]
    fn test_head_tail_lines() {
        let input = "a\nb\nc\nd\ne";

        // head_lines = 2 → first 2
        let toml_head = r#"
schema_version = 1
[filters.h]
match_command = "^cmd"
head_lines = 2
"#;
        let engine_head = engine_from(toml_head);
        match engine_head.apply("cmd x", input) {
            FilterResult::Filtered { output, .. } => {
                let lines: Vec<_> = output.lines().collect();
                assert_eq!(lines, vec!["a", "b"]);
            }
            FilterResult::Passthrough => panic!("expected a match"),
        }

        // tail_lines = 2 → last 2
        let toml_tail = r#"
schema_version = 1
[filters.t]
match_command = "^cmd"
tail_lines = 2
"#;
        let engine_tail = engine_from(toml_tail);
        match engine_tail.apply("cmd x", input) {
            FilterResult::Filtered { output, .. } => {
                let lines: Vec<_> = output.lines().collect();
                assert_eq!(lines, vec!["d", "e"]);
            }
            FilterResult::Passthrough => panic!("expected a match"),
        }
    }

    /// Stage 7: `max_lines` caps the total output line count.
    #[test]
    fn test_max_lines() {
        let toml = r#"
schema_version = 1
[filters.cap]
match_command = "^verbose"
max_lines = 3
"#;
        let engine = engine_from(toml);
        let input = (0..10)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        match engine.apply("verbose output", &input) {
            FilterResult::Filtered { output, .. } => {
                assert_eq!(output.lines().count(), 3);
            }
            FilterResult::Passthrough => panic!("expected a match"),
        }
    }

    /// Stage 8: `on_empty` is returned when the pipeline yields no lines.
    #[test]
    fn test_on_empty() {
        let toml = r#"
schema_version = 1
[filters.empty]
match_command = "^nothing"
strip_lines_matching = [".*"]
on_empty = "(no output)"
"#;
        let engine = engine_from(toml);
        match engine.apply("nothing here", "line1\nline2") {
            FilterResult::Filtered { output, .. } => assert_eq!(output, "(no output)"),
            FilterResult::Passthrough => panic!("expected a match"),
        }
    }

    /// When no filter matches, [`FilterResult::Passthrough`] is returned.
    #[test]
    fn test_no_match_passthrough() {
        let toml = r#"
schema_version = 1
[filters.specific]
match_command = "^only-this-command"
strip_ansi = true
"#;
        let engine = engine_from(toml);
        assert!(matches!(
            engine.apply("some-other-command", "output"),
            FilterResult::Passthrough
        ));
    }

    /// Inline test cases defined in `[tests]` are executed and pass.
    #[test]
    fn test_inline_tests() {
        let toml = r#"
schema_version = 1
[filters.greet]
match_command = "^greet"
[[filters.greet.replace]]
pattern = "Hello"
replacement = "Hi"

[[tests.greet]]
name = "basic substitution"
input = "Hello world"
expected = "Hi world"
"#;
        let config: FilterConfig = toml::from_str(toml).expect("valid");
        let engine = FilterEngine::from_toml(toml).expect("valid");
        let results = engine.run_tests(&config);
        assert_eq!(results.len(), 1);
        assert!(
            results[0].passed,
            "inline test should pass: {:?}",
            results[0]
        );
    }

    /// Multiple TOML sources are merged; filters from all sources are available.
    #[test]
    fn test_multi_source_merge() {
        let src_a = r#"
schema_version = 1
[filters.alpha]
match_command = "^alpha"
strip_ansi = true
"#;
        let src_b = r#"
schema_version = 1
[filters.beta]
match_command = "^beta"
max_lines = 1
"#;
        let engine = FilterEngine::from_toml_sources(&[src_a, src_b]).expect("valid");
        assert!(matches!(
            engine.apply("alpha cmd", "out"),
            FilterResult::Filtered { .. }
        ));
        assert!(matches!(
            engine.apply("beta cmd", "out"),
            FilterResult::Filtered { .. }
        ));
        assert!(matches!(
            engine.apply("gamma cmd", "out"),
            FilterResult::Passthrough
        ));
    }

    /// Unsupported schema versions are rejected.
    #[test]
    fn test_unsupported_schema_version() {
        let toml = r#"
schema_version = 99
[filters.x]
match_command = ".*"
"#;
        assert!(matches!(
            FilterEngine::from_toml(toml),
            Err(FilterError::UnsupportedSchema(99))
        ));
    }

    /// Invalid regex patterns produce a [`FilterError::RegexError`].
    #[test]
    fn test_invalid_regex_error() {
        let toml = r#"
schema_version = 1
[filters.bad]
match_command = "["
"#;
        assert!(matches!(
            FilterEngine::from_toml(toml),
            Err(FilterError::RegexError { .. })
        ));
    }
}
