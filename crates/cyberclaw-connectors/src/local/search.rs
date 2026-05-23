//! # Search Capabilities — `search.grep` and `search.glob`
//!
//! This module implements content-search and file-path-glob capabilities for
//! `LocalConnector`.
//!
//! ## Capabilities exposed
//!
//! | Capability ID  | Description                              | Risk  |
//! |----------------|------------------------------------------|-------|
//! | `search.grep`  | ripgrep-backed regex/literal content search | Low |
//! | `search.glob`  | Glob-pattern file path matching          | Low   |
//!
//! ## Implementation notes
//!
//! `search.grep` shells out to the `rg` binary when available (full feature
//! set: `--type`, multiline, context lines, output modes).  When `rg` is not
//! on `PATH` it falls back to a pure-Rust walkdir+regex implementation that
//! covers the common cases.
//!
//! `search.glob` uses the `glob` crate (already in Cargo.toml) for pattern
//! matching and `walkdir` for directory traversal, with `.gitignore` exclusion
//! applied via a simple heuristic (entries whose path components contain a
//! `.gitignore`-listed prefix are skipped).  For full `.gitignore` semantics
//! the `ignore` crate would be preferred; it is not yet in the workspace so we
//! use `walkdir` + manual ignore detection instead.
//!
//! ## §4 Facade export
//!
//! Per EVOLUTION_IDIOMS §4 ("Facades From Owner Crate"), this module exports
//! [`capability_facades`] which returns a [`SearchFacadeDescriptor`] list.
//! Because `cyberclaw-connectors` cannot import `cyberclaw-agent-runtime`
//! (that would create a dependency cycle), the return type uses a local
//! descriptor struct carrying the same fields as `CapabilityFacade`.  The host
//! binary converts these into `CapabilityFacade` instances during capability
//! composition — identical to the pattern used in `cmd.rs` and `fs.rs`.

use super::LocalConnector;
use crate::types::*;
use cyberclaw_core::capability::RiskLevel;
use cyberclaw_core::facade::{CapabilityFacade, FacadeExposure, ToolsetCategory};
use cyberclaw_core::ids::{CapabilityId, ConnectorId};
use std::process::Command;
use tracing::{debug, error, info, warn};

// ---------------------------------------------------------------------------
// §4 Facade export — real cyberclaw_core::facade types (Phase 2)
// ---------------------------------------------------------------------------

/// Return facade entries for all `search.*` capabilities.
///
/// **§4 compliance**: This is the single authoritative source for search facade
/// declarations. `BuiltinToolRegistry::default_facades` MUST NOT duplicate
/// these entries; the host binary registers them via
/// `BuiltinToolRegistry::register`.
// Public API pre-integration: wired by host binary during capability
// composition. See docs/architecture/idioms/EVOLUTION_IDIOMS.md §4.
#[allow(dead_code)]
pub fn capability_facades() -> Vec<(CapabilityFacade, ToolsetCategory)> {
    let connector_id = ConnectorId::from_string("local".to_string()).unwrap();
    vec![
        (
            CapabilityFacade {
                name: "file_search".to_string(),
                description: "Search file contents using a regular expression or literal pattern \
                    (ripgrep-backed). \
                    **CRITICAL — counting tasks**: when the user asks \"how many lines\" or \
                    \"count occurrences\", you MUST set `output_mode=\"count\"` — this returns \
                    accurate per-file match totals. Reporting a count from `output_mode=\"content\"` \
                    is unreliable because results are limited by `head_limit` (default 250). \
                    **CRITICAL — case sensitivity**: default is `case_insensitive=true` \
                    (matches grep -i convention). Most interactive queries want this. \
                    Only pass `case_insensitive=false` when matching a class name, constant \
                    identifier, or env-var key where casing distinguishes intent. \
                    Output modes: `files_with_matches` (default, returns file paths), `content` \
                    (returns matching lines with optional context), and `count` (returns per-file \
                    match counts — use for any 'how many' question). \
                    Results are bounded at `head_limit` (default 250). Risk: Low — read-only."
                    .to_string(),
                connector_id: connector_id.clone(),
                capability_id: CapabilityId::from_string("search.grep".to_string()).unwrap(),
                risk_level: RiskLevel::Low,
                effects: vec!["read".to_string()],
                read_only: true,
                destructive: false,
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "pattern": {
                            "type": "string",
                            "description": "Regex or literal pattern to search for"
                        },
                        "path": {
                            "type": "string",
                            "description": "File or directory to search (defaults to workspace root)"
                        },
                        "glob": {
                            "type": "string",
                            "description": "Glob filter for file names (e.g. \"*.rs\", \"**/*.{ts,tsx}\")"
                        },
                        "file_type": {
                            "type": "string",
                            "description": "File type filter (e.g. \"rust\", \"py\", \"ts\") — maps to rg --type"
                        },
                        "case_insensitive": {
                            "type": "boolean",
                            "description": "Case-insensitive search (DEFAULT TRUE — matches \
                                grep -i convention). Most interactive queries want \
                                case-insensitive ('error' should match Error/ERROR/error). \
                                Pass FALSE explicitly only when matching a class name, \
                                constant identifier, or env-var key where casing matters."
                        },
                        "context_before": {
                            "type": "integer",
                            "description": "Lines of context before each match (content mode only)"
                        },
                        "context_after": {
                            "type": "integer",
                            "description": "Lines of context after each match (content mode only)"
                        },
                        "context": {
                            "type": "integer",
                            "description": "Unified context lines before and after (content mode only)"
                        },
                        "output_mode": {
                            "type": "string",
                            "enum": ["files_with_matches", "content", "count"],
                            "description": "Output mode (default: files_with_matches). \
                                **Use 'count' when user asks 'how many lines/matches/occurrences'** \
                                — returns reliable per-file totals not affected by head_limit. \
                                'content' truncates at head_limit so deriving counts from it is \
                                wrong. 'files_with_matches' is for 'which file contains X'."
                        },
                        "head_limit": {
                            "type": "integer",
                            "description": "Maximum results to return (default 250)"
                        },
                        "multiline": {
                            "type": "boolean",
                            "description": "Enable multiline matching (default false)"
                        }
                    },
                    "required": ["pattern"]
                })),
                exposure: FacadeExposure::LlmDefault,
            },
            ToolsetCategory::FileSystem,
        ),
        (
            CapabilityFacade {
                name: "search_glob".to_string(),
                description: "Find files by glob pattern (e.g. `**/*.rs`, `src/**/*.ts`). \
                    Respects `.gitignore` by default. Returns paths sorted by modification \
                    time (most-recently-modified first) when `sort_by_mtime` is true. \
                    Results are bounded at 250 entries. Risk: Low — read-only."
                    .to_string(),
                connector_id: connector_id.clone(),
                capability_id: CapabilityId::from_string("search.glob".to_string()).unwrap(),
                risk_level: RiskLevel::Low,
                effects: vec!["read".to_string()],
                read_only: true,
                destructive: false,
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "pattern": {
                            "type": "string",
                            "description": "Glob pattern (e.g. \"**/*.rs\", \"src/**/*.ts\")"
                        },
                        "path": {
                            "type": "string",
                            "description": "Directory to search in (defaults to workspace root)"
                        },
                        "sort_by_mtime": {
                            "type": "boolean",
                            "description": "Sort by modification time descending (default false)"
                        }
                    },
                    "required": ["pattern"]
                })),
                exposure: FacadeExposure::LlmDefault,
            },
            ToolsetCategory::FileSystem,
        ),
    ]
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default result cap shared by both capabilities.
const DEFAULT_HEAD_LIMIT: usize = 250;

// ---------------------------------------------------------------------------
// Pattern validation (ReDoS guard — unchanged from original)
// ---------------------------------------------------------------------------

/// Validate a pattern to prevent ReDoS and excessive complexity.
fn validate_search_pattern(pattern: &str, pattern_type: &str) -> anyhow::Result<()> {
    const MAX_PATTERN_LENGTH: usize = 1000;
    if pattern.len() > MAX_PATTERN_LENGTH {
        error!(
            "{} pattern too long: {} chars (max {})",
            pattern_type,
            pattern.len(),
            MAX_PATTERN_LENGTH
        );
        return Err(anyhow::anyhow!(
            "{} pattern too long ({} characters, maximum {})",
            pattern_type,
            pattern.len(),
            MAX_PATTERN_LENGTH
        ));
    }

    if pattern.trim().is_empty() {
        error!("{} pattern is empty", pattern_type);
        return Err(anyhow::anyhow!("{} pattern cannot be empty", pattern_type));
    }

    let nesting_depth = pattern
        .chars()
        .fold((0usize, 0usize), |(depth, max), c| {
            let new_depth = match c {
                '(' | '[' | '{' => depth + 1,
                ')' | ']' | '}' => depth.saturating_sub(1),
                _ => depth,
            };
            (new_depth, max.max(new_depth))
        })
        .1;

    const MAX_NESTING_DEPTH: usize = 10;
    if nesting_depth > MAX_NESTING_DEPTH {
        error!(
            "{} pattern has excessive nesting depth: {} (max {})",
            pattern_type, nesting_depth, MAX_NESTING_DEPTH
        );
        return Err(anyhow::anyhow!(
            "{} pattern has excessive nesting depth ({}, maximum {})",
            pattern_type,
            nesting_depth,
            MAX_NESTING_DEPTH
        ));
    }

    let quantifier_count = pattern
        .chars()
        .filter(|&c| c == '*' || c == '+' || c == '?')
        .count();

    const MAX_QUANTIFIERS: usize = 20;
    if quantifier_count > MAX_QUANTIFIERS {
        error!(
            "{} pattern has too many quantifiers: {} (max {})",
            pattern_type, quantifier_count, MAX_QUANTIFIERS
        );
        return Err(anyhow::anyhow!(
            "{} pattern has too many quantifiers ({}, maximum {})",
            pattern_type,
            quantifier_count,
            MAX_QUANTIFIERS
        ));
    }

    let dangerous_patterns = [
        "(.*)+", "(.*)*", "(.+)+", "(.+)*", "(a+)+", "(a*)*", "(.*.*)", "(.+.+)",
    ];
    for dangerous in &dangerous_patterns {
        if pattern.contains(dangerous) {
            error!(
                "{} pattern contains dangerous nested quantifier: {}",
                pattern_type, dangerous
            );
            return Err(anyhow::anyhow!(
                "{} pattern contains dangerous nested quantifiers (potential ReDoS attack)",
                pattern_type
            ));
        }
    }

    debug!(
        "{} pattern validated successfully: {}",
        pattern_type, pattern
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// search.grep — ripgrep-backed content search
// ---------------------------------------------------------------------------

/// Grep search capability implementation.
///
/// Prefers the `rg` binary for full feature coverage.  Falls back to a
/// pure-Rust walkdir+regex implementation when `rg` is not on `PATH`.
pub async fn grep(
    connector: &LocalConnector,
    request: CapabilityExecutionRequest,
) -> anyhow::Result<serde_json::Value> {
    let input: SearchGrepInput = serde_json::from_value(request.input)?;
    debug!("search.grep: pattern={:?}", input.pattern);

    validate_search_pattern(&input.pattern, "Grep")?;
    if let Some(ref g) = input.glob {
        validate_search_pattern(g, "Glob filter")?;
    }

    let head_limit = input
        .head_limit
        .or_else(|| input.max_results.map(|m| m as usize))
        .unwrap_or(DEFAULT_HEAD_LIMIT);

    // Try ripgrep first; fall back on error.
    match grep_with_rg(connector, &input, head_limit) {
        Ok(output) => Ok(serde_json::to_value(output)?),
        Err(rg_err) => {
            warn!(
                "rg unavailable or failed ({}), falling back to pure-Rust grep",
                rg_err
            );
            let output = grep_fallback(connector, &input, head_limit)?;
            Ok(serde_json::to_value(output)?)
        }
    }
}

/// Execute grep via the `rg` binary and parse JSON output.
fn grep_with_rg(
    connector: &LocalConnector,
    input: &SearchGrepInput,
    head_limit: usize,
) -> anyhow::Result<SearchGrepOutput> {
    let mut cmd = Command::new("rg");

    // Always emit JSON so we can parse structured output.
    cmd.arg("--json");

    if input.case_insensitive {
        cmd.arg("--ignore-case");
    }

    if input.multiline {
        cmd.arg("--multiline");
        cmd.arg("--multiline-dotall");
    }

    if let Some(ref ft) = input.file_type {
        cmd.arg("--type").arg(ft);
    }

    if let Some(ref g) = input.glob {
        cmd.arg("--glob").arg(g);
    }

    // Context lines (content mode only — harmless to add regardless).
    let ctx = input.context;
    let before = input.context_before.or(ctx);
    let after = input.context_after.or(ctx);
    if let Some(b) = before {
        cmd.arg(format!("-B{}", b));
    }
    if let Some(a) = after {
        cmd.arg(format!("-A{}", a));
    }

    // Keep JSON output for every mode. `rg --json --files-with-matches` and
    // `rg --json --count` intentionally switch back to plain text, so we
    // derive files/counts from JSON match/end events below instead.

    cmd.arg("--").arg(&input.pattern);

    if let Some(ref path) = input.path {
        let validated = connector.validate_path(path)?;
        cmd.arg(validated);
    } else {
        cmd.current_dir(&connector.workspace);
    }

    let output = cmd
        .output()
        .map_err(|e| anyhow::anyhow!("rg binary not found or failed to spawn: {}", e))?;

    // rg exits 1 for "no matches", 2 for real errors.
    if output.status.code() == Some(2) {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow::anyhow!("rg error: {}", stderr.trim()));
    }

    parse_rg_json_output(&output.stdout, &input.output_mode, head_limit)
}

/// Parse ripgrep's `--json` output into `SearchGrepOutput`.
fn parse_rg_json_output(
    stdout: &[u8],
    mode: &GrepOutputMode,
    head_limit: usize,
) -> anyhow::Result<SearchGrepOutput> {
    let text = String::from_utf8_lossy(stdout);

    let mut matches: Vec<SearchMatch> = Vec::new();
    let mut files: Vec<String> = Vec::new();
    let mut counts: Vec<SearchCountEntry> = Vec::new();
    let mut truncated = false;

    // For files_with_matches mode, collect unique file paths from match events.
    let mut seen_files: std::collections::HashSet<String> = std::collections::HashSet::new();

    for line in text.lines() {
        let json: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let msg_type = match json.get("type").and_then(|v| v.as_str()) {
            Some(t) => t,
            None => continue,
        };

        match (mode, msg_type) {
            // content mode — collect individual match lines
            (GrepOutputMode::Content, "match") => {
                let data = &json["data"];
                let file = data["path"]["text"].as_str().unwrap_or("").to_string();
                let line_num =
                    u32::try_from(data["line_number"].as_u64().unwrap_or(0)).unwrap_or(u32::MAX);
                let content = data["lines"]["text"]
                    .as_str()
                    .unwrap_or("")
                    .trim_end()
                    .to_string();
                matches.push(SearchMatch {
                    file,
                    line: line_num,
                    content,
                });
                if matches.len() >= head_limit {
                    truncated = true;
                    break;
                }
            }
            // files_with_matches — collect unique file paths from match events
            (GrepOutputMode::FilesWithMatches, "match") => {
                if let Some(file) = json["data"]["path"]["text"].as_str() {
                    if seen_files.insert(file.to_string()) {
                        files.push(file.to_string());
                        if files.len() >= head_limit {
                            truncated = true;
                            break;
                        }
                    }
                }
            }
            // count mode — collect per-file totals from `end` events.
            (GrepOutputMode::Count, "end") => {
                if let Some(file) = json["data"]["path"]["text"].as_str() {
                    let count = json["data"]["stats"]["matches"]
                        .as_u64()
                        .and_then(|n| u32::try_from(n).ok())
                        .unwrap_or(0);
                    counts.push(SearchCountEntry {
                        file: file.to_string(),
                        count,
                    });
                    if counts.len() >= head_limit {
                        truncated = true;
                        break;
                    }
                }
            }
            _ => {}
        }
    }

    let total = match mode {
        GrepOutputMode::Content => matches.len(),
        GrepOutputMode::FilesWithMatches => files.len(),
        GrepOutputMode::Count => counts.len(),
    };

    info!(
        "search.grep (rg): {} results (mode={:?}, truncated={})",
        total, mode, truncated
    );

    Ok(SearchGrepOutput {
        matches,
        files,
        counts,
        count: u32::try_from(total).unwrap_or(u32::MAX),
        truncated,
    })
}

// ---------------------------------------------------------------------------
// Fallback grep — pure Rust via walkdir + std::process::Command "grep"
// ---------------------------------------------------------------------------

/// Fallback grep using the system `grep` binary when `rg` is unavailable.
///
/// Supports the most common use cases (content search with optional case
/// insensitivity and glob filter).  Context lines and `file_type` filtering
/// are not supported in fallback mode.
fn grep_fallback(
    connector: &LocalConnector,
    input: &SearchGrepInput,
    head_limit: usize,
) -> anyhow::Result<SearchGrepOutput> {
    validate_search_pattern(&input.pattern, "Grep fallback")?;

    let mut cmd = Command::new("grep");
    cmd.arg("-r").arg("-n");

    if input.case_insensitive {
        cmd.arg("-i");
    }

    // Glob filter via --include must come before the pattern/path arguments
    // (macOS grep rejects options placed after the path operand).
    if let Some(ref g) = input.glob {
        cmd.arg(format!("--include={}", g));
    }

    cmd.arg("--").arg(&input.pattern);

    if let Some(ref path) = input.path {
        let validated = connector.validate_path(path)?;
        cmd.arg(validated);
    } else {
        cmd.current_dir(&connector.workspace);
        cmd.arg(".");
    }

    let output = cmd.output()?;
    let stdout = String::from_utf8_lossy(&output.stdout);

    let mut matches: Vec<SearchMatch> = Vec::new();
    let mut files_seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut files: Vec<String> = Vec::new();
    // GAP-fix (2026-05-23, v1.2 P2): Count mode previously shared the
    // FilesWithMatches branch, so it returned 1-per-file instead of
    // per-file line totals. B-L2-2 (count import lines across .py files)
    // was returning 4 (= 4 files) instead of 7 (= sum of import lines per
    // file). Track per-file line counts here and surface them in the
    // `counts` vec + `count` total.
    let mut per_file_count: std::collections::HashMap<String, u32> =
        std::collections::HashMap::new();
    let mut truncated = false;

    for line in stdout.lines() {
        if let Some((file_part, rest)) = line.split_once(':') {
            if let Some((line_num_str, content)) = rest.split_once(':') {
                if let Ok(line_num) = line_num_str.parse::<u32>() {
                    match &input.output_mode {
                        GrepOutputMode::Content => {
                            matches.push(SearchMatch {
                                file: file_part.to_string(),
                                line: line_num,
                                content: content.to_string(),
                            });
                            if matches.len() >= head_limit {
                                truncated = true;
                                break;
                            }
                        }
                        GrepOutputMode::FilesWithMatches => {
                            if files_seen.insert(file_part.to_string()) {
                                files.push(file_part.to_string());
                                if files.len() >= head_limit {
                                    truncated = true;
                                    break;
                                }
                            }
                        }
                        GrepOutputMode::Count => {
                            *per_file_count.entry(file_part.to_string()).or_insert(0) += 1;
                        }
                    }
                }
            }
        }
    }

    // Materialise per_file_count → counts vec (sorted by file for determinism)
    let counts: Vec<SearchCountEntry> = {
        let mut v: Vec<SearchCountEntry> = per_file_count
            .into_iter()
            .map(|(file, count)| SearchCountEntry { file, count })
            .collect();
        v.sort_by(|a, b| a.file.cmp(&b.file));
        v
    };

    let total = match &input.output_mode {
        GrepOutputMode::Content => matches.len(),
        GrepOutputMode::FilesWithMatches => files.len(),
        GrepOutputMode::Count => counts.iter().map(|c| c.count as usize).sum(),
    };

    Ok(SearchGrepOutput {
        matches,
        files,
        counts,
        count: u32::try_from(total).unwrap_or(u32::MAX),
        truncated,
    })
}

// ---------------------------------------------------------------------------
// search.glob — glob-pattern file matching
// ---------------------------------------------------------------------------

/// Glob search capability implementation.
///
/// Uses the `glob` crate for pattern matching over the directory tree.
/// Respects `.gitignore` by skipping paths that share a prefix with any
/// entry listed in a `.gitignore` file found in the search root.
///
/// Results are bounded at 250 entries by default.  Pass `sort_by_mtime: true`
/// to receive files sorted by modification time (most-recently-modified first).
pub fn glob_search(
    connector: &LocalConnector,
    request: CapabilityExecutionRequest,
) -> anyhow::Result<serde_json::Value> {
    let input: SearchGlobInput = serde_json::from_value(request.input)?;
    debug!("search.glob: pattern={:?}", input.pattern);

    validate_search_pattern(&input.pattern, "Glob")?;

    let head_limit = input
        .max_results
        .map(|m| m as usize)
        .unwrap_or(DEFAULT_HEAD_LIMIT);

    let search_root = if let Some(ref path) = input.path {
        connector.validate_path(path)?
    } else {
        connector.workspace.clone()
    };

    // Build the gitignore exclusion list from the search root.
    let gitignore_patterns = load_gitignore_prefixes(&search_root);

    // Expand the user pattern: if it contains no path separator, prepend
    // `**/` so it matches anywhere in the tree (same as the original impl).
    let glob_pattern = if input.pattern.contains('/') {
        input.pattern.clone()
    } else {
        format!("**/{}", input.pattern)
    };

    let full_pattern = search_root.join(&glob_pattern);
    let pattern_str = full_pattern
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Invalid UTF-8 in glob pattern path"))?;

    debug!("search.glob: expanded pattern={}", pattern_str);

    let mut entries: Vec<(std::path::PathBuf, Option<std::time::SystemTime>)> = Vec::new();
    let mut truncated = false;

    match glob::glob(pattern_str) {
        Ok(iter) => {
            for entry in iter {
                match entry {
                    Ok(path) => {
                        if !path.is_file() {
                            continue;
                        }

                        // Strip workspace prefix for the relative path used
                        // in gitignore checking.
                        let relative = match path.strip_prefix(&connector.workspace) {
                            Ok(r) => r.to_path_buf(),
                            Err(_) => {
                                warn!("search.glob: skipping path outside workspace: {:?}", path);
                                continue;
                            }
                        };

                        // Gitignore exclusion check.
                        if is_gitignored(&relative, &gitignore_patterns) {
                            debug!("search.glob: gitignored {:?}", relative);
                            continue;
                        }

                        let mtime = if input.sort_by_mtime {
                            path.metadata().ok().and_then(|m| m.modified().ok())
                        } else {
                            None
                        };

                        entries.push((relative, mtime));

                        if !input.sort_by_mtime && entries.len() >= head_limit {
                            truncated = true;
                            break;
                        }
                    }
                    Err(e) => {
                        warn!("search.glob: entry error: {}", e);
                    }
                }
            }
        }
        Err(e) => {
            error!("search.glob: invalid pattern '{}': {}", input.pattern, e);
            return Err(anyhow::anyhow!("Invalid glob pattern: {}", e));
        }
    }

    // Sort by mtime if requested.
    if input.sort_by_mtime {
        entries.sort_by_key(|b| std::cmp::Reverse(b.1));
        if entries.len() > head_limit {
            entries.truncate(head_limit);
            truncated = true;
        }
    }

    let files: Vec<String> = entries
        .iter()
        .map(|(p, _)| p.to_string_lossy().to_string())
        .collect();

    info!(
        "search.glob: {} files matched pattern={:?} (truncated={})",
        files.len(),
        input.pattern,
        truncated
    );

    Ok(serde_json::to_value(SearchGlobOutput {
        count: u32::try_from(files.len()).unwrap_or(u32::MAX),
        files,
        truncated,
    })?)
}

// ---------------------------------------------------------------------------
// .gitignore helper
// ---------------------------------------------------------------------------

/// Load non-comment, non-empty lines from `<root>/.gitignore`.
///
/// Only the root-level `.gitignore` is consulted.  This is intentionally
/// simple: it handles the most common single-repo case without pulling in the
/// `ignore` crate.
fn load_gitignore_prefixes(root: &std::path::Path) -> Vec<String> {
    let gi_path = root.join(".gitignore");
    match std::fs::read_to_string(&gi_path) {
        Ok(content) => content
            .lines()
            .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
            .map(|l| l.trim_start_matches('/').to_string())
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Return `true` if `relative_path` matches any gitignore entry.
///
/// The check is: any path component (or the path string itself) starts with
/// or equals a gitignore pattern entry.  This handles the common cases of
/// `target/`, `node_modules/`, `.env`, etc.
fn is_gitignored(relative: &std::path::Path, patterns: &[String]) -> bool {
    let path_str = relative.to_string_lossy();
    for pattern in patterns {
        // Remove trailing slash from gitignore entries like `target/`.
        let trimmed = pattern.trim_end_matches('/');
        if path_str.starts_with(trimmed) {
            return true;
        }
        // Also check individual path components.
        for component in relative.components() {
            if let std::path::Component::Normal(c) = component {
                if c.to_string_lossy() == trimmed {
                    return true;
                }
            }
        }
    }
    false
}

// ===========================================================================
// Public dispatch wrappers (called by LocalConnector::execute)
// ===========================================================================

/// Dispatch `search.glob` from the connector's `execute()` method.
///
/// Named `glob` to preserve the original symbol that `local/mod.rs` references.
pub fn glob(
    connector: &LocalConnector,
    request: CapabilityExecutionRequest,
) -> anyhow::Result<serde_json::Value> {
    glob_search(connector, request)
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::io::Write;
    use tempfile::TempDir;

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn make_connector(tmp: &TempDir) -> LocalConnector {
        LocalConnector::new(tmp.path().to_path_buf())
    }

    fn make_request(
        connector: &LocalConnector,
        capability: &str,
        input: serde_json::Value,
    ) -> CapabilityExecutionRequest {
        CapabilityExecutionRequest {
            execution_id: cyberclaw_core::ids::ExecutionId::new(),
            trace_id: "test-trace".to_string(),
            actor: cyberclaw_core::identity::ActorRef {
                id: cyberclaw_core::ids::ActorId::new(),
                actor_type: cyberclaw_core::identity::ActorType::System,
                tenant_id: None,
                home_node_id: None,
                display_name: "test".to_string(),
            },
            workspace: cyberclaw_core::workspace::WorkspaceRef {
                id: cyberclaw_core::ids::WorkspaceId::new(),
                mode: cyberclaw_core::workspace::WorkspaceMode::Isolated,
                materialization_mode: None,
                home_node_id: None,
                backing_store: None,
                root: connector.workspace.to_string_lossy().to_string(),
                writable_roots: vec![connector.workspace.to_string_lossy().to_string()],
            },
            connector_id: cyberclaw_core::ids::ConnectorId::from_string("local".to_string())
                .unwrap(),
            capability_id: cyberclaw_core::ids::CapabilityId::from_string(capability.to_string())
                .unwrap(),
            input,
        }
    }

    /// Write `content` to `<tmp>/<rel_path>`, creating parent dirs.
    fn write_file(tmp: &TempDir, rel_path: &str, content: &str) {
        let path = tmp.path().join(rel_path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
    }

    // -----------------------------------------------------------------------
    // Facade tests
    // -----------------------------------------------------------------------

    #[test]
    fn facades_returns_two_entries() {
        let facades = capability_facades();
        assert_eq!(facades.len(), 2, "expected search.grep and search.glob");
    }

    #[test]
    fn facades_capability_ids_are_distinct() {
        let ids: Vec<String> = capability_facades()
            .into_iter()
            .map(|(d, _)| d.capability_id.as_str().to_string())
            .collect();
        let unique: HashSet<&str> = ids.iter().map(|s| s.as_str()).collect();
        assert_eq!(ids.len(), unique.len(), "capability IDs must be distinct");
    }

    #[test]
    fn facades_both_low_risk_read_only() {
        for (desc, _cat) in capability_facades() {
            assert_eq!(
                desc.risk_level,
                RiskLevel::Low,
                "{} must be RiskLevel::Low",
                desc.capability_id.as_str()
            );
            assert!(
                desc.effects.iter().any(|e| e == "read"),
                "{} must declare read effect",
                desc.capability_id.as_str()
            );
            assert_eq!(desc.connector_id.as_str(), "local");
        }
    }

    #[test]
    fn facades_input_schemas_valid() {
        for (desc, _cat) in capability_facades() {
            let schema = desc
                .input_schema
                .as_ref()
                .expect("input_schema must be Some");
            assert!(
                schema.is_object(),
                "{}: input_schema must be a JSON object",
                desc.capability_id.as_str()
            );
            assert_eq!(
                schema.get("type").and_then(|v| v.as_str()),
                Some("object"),
                "{}: input_schema must have type=object",
                desc.capability_id.as_str()
            );
        }
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    /// Return true if a real `rg` binary (not a shell wrapper) is reachable
    /// from the test process's PATH.  Some capabilities (`file_type` filtering)
    /// require the full ripgrep feature set and cannot be exercised via the
    /// `grep` fallback.
    fn rg_binary_available() -> bool {
        std::process::Command::new("rg")
            .arg("--version")
            .output()
            .map(|o| {
                let stdout = String::from_utf8_lossy(&o.stdout);
                o.status.success() && stdout.contains("ripgrep")
            })
            .unwrap_or(false)
    }

    // -----------------------------------------------------------------------
    // search.grep tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn grep_basic_pattern_match() {
        let tmp = TempDir::new().unwrap();
        write_file(&tmp, "hello.txt", "hello world\nfoo bar\n");
        write_file(&tmp, "other.txt", "no match here\n");

        let connector = make_connector(&tmp);
        let req = make_request(
            &connector,
            "search.grep",
            serde_json::json!({
                "pattern": "hello",
                "output_mode": "content"
            }),
        );
        let result = grep(&connector, req).await.unwrap();
        let output: SearchGrepOutput = serde_json::from_value(result).unwrap();
        assert!(output.count > 0, "expected at least one match");
        assert!(
            output.matches.iter().any(|m| m.content.contains("hello")),
            "match content should contain 'hello'"
        );
    }

    #[tokio::test]
    async fn grep_case_insensitive() {
        let tmp = TempDir::new().unwrap();
        write_file(&tmp, "mixed.txt", "Hello World\nHELLO WORLD\nhello world\n");

        let connector = make_connector(&tmp);
        let req = make_request(
            &connector,
            "search.grep",
            serde_json::json!({
                "pattern": "hello",
                "case_insensitive": true,
                "output_mode": "content"
            }),
        );
        let result = grep(&connector, req).await.unwrap();
        let output: SearchGrepOutput = serde_json::from_value(result).unwrap();
        // All three lines should match
        assert!(
            output.matches.len() >= 3,
            "case-insensitive should match all 3 lines, got {}",
            output.matches.len()
        );
    }

    #[tokio::test]
    async fn grep_with_file_type_filter() {
        // `file_type` is a ripgrep-only feature; the grep fallback has no
        // equivalent.  Skip this test when the real `rg` binary is absent.
        if !rg_binary_available() {
            return;
        }

        let tmp = TempDir::new().unwrap();
        write_file(&tmp, "main.rs", "fn main() { println!(\"rust\"); }\n");
        write_file(&tmp, "script.py", "def main(): print('python')\n");

        let connector = make_connector(&tmp);
        let req = make_request(
            &connector,
            "search.grep",
            serde_json::json!({
                "pattern": "main",
                "file_type": "rust",
                "output_mode": "files_with_matches"
            }),
        );
        let result = grep(&connector, req).await.unwrap();
        let output: SearchGrepOutput = serde_json::from_value(result).unwrap();
        // Should only find the .rs file, not .py
        let rs_found = output.files.iter().any(|f| f.ends_with(".rs"));
        let py_found = output.files.iter().any(|f| f.ends_with(".py"));
        assert!(rs_found, "should find main.rs");
        assert!(!py_found, "should not find script.py with rust type filter");
    }

    #[tokio::test]
    async fn grep_with_glob_filter() {
        let tmp = TempDir::new().unwrap();
        write_file(&tmp, "src/lib.rs", "pub fn foo() {}\n");
        write_file(&tmp, "src/main.ts", "export function foo() {}\n");

        let connector = make_connector(&tmp);
        let req = make_request(
            &connector,
            "search.grep",
            serde_json::json!({
                "pattern": "foo",
                "glob": "*.rs",
                "output_mode": "files_with_matches"
            }),
        );
        let result = grep(&connector, req).await.unwrap();
        let output: SearchGrepOutput = serde_json::from_value(result).unwrap();
        let ts_found = output.files.iter().any(|f| f.ends_with(".ts"));
        assert!(!ts_found, "glob *.rs should exclude .ts files");
        let rs_found = output.files.iter().any(|f| f.ends_with(".rs"));
        assert!(rs_found, "glob *.rs should include .rs files");
    }

    #[tokio::test]
    async fn grep_output_mode_count() {
        let tmp = TempDir::new().unwrap();
        write_file(&tmp, "a.txt", "foo\nfoo\nbar\n");
        write_file(&tmp, "b.txt", "baz\n");

        let connector = make_connector(&tmp);
        let req = make_request(
            &connector,
            "search.grep",
            serde_json::json!({
                "pattern": "foo",
                "output_mode": "count"
            }),
        );
        // count mode should not error
        let result = grep(&connector, req).await.unwrap();
        let output: SearchGrepOutput = serde_json::from_value(result).unwrap();
        // The result should be parseable; count >= 0
        let _ = output.count;
    }

    /// v1.2 P2 regression — Count mode must return per-line totals (sum
    /// across files), NOT file count. Before the 2026-05-23 fix, fallback
    /// grep treated Count same as FilesWithMatches, so this test would
    /// return 2 (= 2 files containing "import") instead of 5 (= total
    /// import lines across both files). Discovered via B-L2-2 business
    /// matrix: "count import lines across .py files" expected 7, cb
    /// answered 4 (4 files).
    #[tokio::test]
    async fn grep_count_mode_returns_per_line_total_not_file_count() {
        let tmp = TempDir::new().unwrap();
        write_file(
            &tmp,
            "auth.py",
            "import jwt\nimport os\nfrom logging import getLogger\n",
        );
        write_file(&tmp, "billing.py", "import stripe\n");
        write_file(&tmp, "checkout.py", "import json\nimport requests\n");
        // total "^import" lines: auth=2, billing=1, checkout=2 → 5
        let connector = make_connector(&tmp);
        let req = make_request(
            &connector,
            "search.grep",
            serde_json::json!({
                "pattern": "^import ",
                "output_mode": "count"
            }),
        );
        let result = grep(&connector, req).await.unwrap();
        let output: SearchGrepOutput = serde_json::from_value(result).unwrap();
        assert_eq!(
            output.count, 5,
            "Count mode must sum per-line matches (5), not file count (3). \
             Got count={}, counts={:?}",
            output.count, output.counts
        );
        // counts vec must have 3 entries (one per file)
        assert_eq!(output.counts.len(), 3, "expected per-file counts for 3 files");
        // Per-file values: auth.py=2, billing.py=1, checkout.py=2
        let by_file: std::collections::HashMap<&str, u32> = output
            .counts
            .iter()
            .map(|c| (c.file.rsplit('/').next().unwrap_or(""), c.count))
            .collect();
        assert_eq!(by_file.get("auth.py").copied().unwrap_or(0), 2);
        assert_eq!(by_file.get("billing.py").copied().unwrap_or(0), 1);
        assert_eq!(by_file.get("checkout.py").copied().unwrap_or(0), 2);
    }

    // -----------------------------------------------------------------------
    // search.glob tests
    // -----------------------------------------------------------------------

    #[test]
    fn glob_basic_pattern() {
        let tmp = TempDir::new().unwrap();
        write_file(&tmp, "src/main.rs", "fn main() {}\n");
        write_file(&tmp, "src/lib.rs", "pub fn foo() {}\n");
        write_file(&tmp, "build.py", "# build\n");

        let connector = make_connector(&tmp);
        let req = make_request(
            &connector,
            "search.glob",
            serde_json::json!({ "pattern": "**/*.rs" }),
        );
        let result = glob(&connector, req).unwrap();
        let output: SearchGlobOutput = serde_json::from_value(result).unwrap();
        assert!(
            output.count >= 2,
            "expected at least 2 .rs files, got {}",
            output.count
        );
        assert!(
            output.files.iter().all(|f| f.ends_with(".rs")),
            "all results should be .rs files"
        );
    }

    #[test]
    fn glob_with_mtime_sort() {
        let tmp = TempDir::new().unwrap();
        // Create files with small sleep to ensure different mtimes.
        write_file(&tmp, "older.rs", "// old\n");
        std::thread::sleep(std::time::Duration::from_millis(10));
        write_file(&tmp, "newer.rs", "// new\n");

        let connector = make_connector(&tmp);
        let req = make_request(
            &connector,
            "search.glob",
            serde_json::json!({
                "pattern": "**/*.rs",
                "sort_by_mtime": true
            }),
        );
        let result = glob(&connector, req).unwrap();
        let output: SearchGlobOutput = serde_json::from_value(result).unwrap();
        assert!(output.count >= 2, "expected both .rs files");
        // First file should be newer.rs (most recently modified).
        let first = output.files.first().unwrap();
        assert!(
            first.ends_with("newer.rs"),
            "most-recently-modified should be first, got: {:?}",
            first
        );
    }

    #[test]
    fn glob_respects_gitignore() {
        let tmp = TempDir::new().unwrap();

        // Write a .gitignore that excludes the `target/` directory.
        write_file(&tmp, ".gitignore", "target/\n");
        write_file(&tmp, "target/debug/artifact.rs", "// ignored\n");
        write_file(&tmp, "src/main.rs", "fn main() {}\n");

        let connector = make_connector(&tmp);
        let req = make_request(
            &connector,
            "search.glob",
            serde_json::json!({ "pattern": "**/*.rs" }),
        );
        let result = glob(&connector, req).unwrap();
        let output: SearchGlobOutput = serde_json::from_value(result).unwrap();

        let target_found = output.files.iter().any(|f| f.contains("target"));
        assert!(
            !target_found,
            "target/ should be excluded by .gitignore, got files: {:?}",
            output.files
        );
        let src_found = output.files.iter().any(|f| f.contains("main.rs"));
        assert!(src_found, "src/main.rs should be included");
    }
}
