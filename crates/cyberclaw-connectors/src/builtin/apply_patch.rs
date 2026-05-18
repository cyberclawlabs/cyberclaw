//! Capability `file:edit:patch` — apply unified diff to files.
//!
//! # Input
//! ```json
//! {
//!   "diff_text": "--- a/foo.txt\n+++ b/foo.txt\n@@ -1,3 +1,3 @@\n ...",
//!   "base_dir":  "/optional/base/dir"    // defaults to workspace root
//! }
//! ```
//!
//! # Output
//! ```json
//! { "files_modified": ["foo.txt"], "files_created": ["new.txt"], "files_deleted": ["old.txt"] }
//! ```
//!
//! # Supported diff operations
//! - **Modify** (`--- a/file` / `+++ b/file`) — context-line matching with hunk apply
//! - **Add**    (`--- /dev/null` / `+++ b/file`) — creates a new file
//! - **Delete** (`--- a/file` / `+++ /dev/null`) — deletes an existing file
//!
//! # Limitations
//! - No 3-way merge (contextual apply only).
//! - Strict context matching: apply fails if context lines do not match.
//! - Binary files are not supported.

use crate::types::CapabilityExecutionRequest;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, info};

// ---------------------------------------------------------------------------
// I/O types
// ---------------------------------------------------------------------------

/// Input for `file:edit:patch`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEditPatchInput {
    /// Unified diff text (UTF-8).
    pub diff_text: String,
    /// Base directory for relative paths in the diff (defaults to workspace root).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_dir: Option<PathBuf>,
}

/// Output for `file:edit:patch`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEditPatchOutput {
    pub files_modified: Vec<String>,
    pub files_created: Vec<String>,
    pub files_deleted: Vec<String>,
}

// ---------------------------------------------------------------------------
// Unified diff data model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum HunkLine {
    Context(String),
    Add(String),
    Remove(String),
}

#[derive(Debug, Clone)]
struct Hunk {
    /// 1-based start line in the original file (from `@@ -N,M @@`).
    orig_start: usize,
    lines: Vec<HunkLine>,
}

#[derive(Debug, Clone, PartialEq)]
enum FileOp {
    Modify,
    Create,
    Delete,
}

#[derive(Debug, Clone)]
struct FileDiff {
    op: FileOp,
    /// Path to the target file (relative to base_dir).
    target_path: String,
    hunks: Vec<Hunk>,
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

fn strip_prefix<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    s.strip_prefix(prefix)
}

/// Strip the `a/` or `b/` git-diff prefix from a path, if present.
fn strip_git_prefix(path: &str) -> &str {
    if let Some(p) = path.strip_prefix("a/").or_else(|| path.strip_prefix("b/")) {
        p
    } else {
        path
    }
}

/// Parse a unified diff string into a list of `FileDiff` entries.
fn parse_diff(diff_text: &str) -> anyhow::Result<Vec<FileDiff>> {
    let mut diffs: Vec<FileDiff> = Vec::new();
    let mut lines = diff_text.lines().peekable();

    while let Some(line) = lines.next() {
        // Start of a new file diff
        if let Some(from_path) = strip_prefix(line, "--- ") {
            // Expect +++ on next line
            let to_line = lines
                .next()
                .ok_or_else(|| anyhow::anyhow!("Unexpected end of diff after '---' line"))?;
            let to_path = strip_prefix(to_line, "+++ ")
                .ok_or_else(|| anyhow::anyhow!("Expected '+++ ' after '--- ', got: {to_line}"))?;

            let from_clean = strip_git_prefix(from_path.trim());
            let to_clean = strip_git_prefix(to_path.trim());

            let op = if from_clean == "/dev/null" {
                FileOp::Create
            } else if to_clean == "/dev/null" {
                FileOp::Delete
            } else {
                FileOp::Modify
            };

            let target_path = if op == FileOp::Create {
                to_clean.to_string()
            } else {
                from_clean.to_string()
            };

            let mut hunks: Vec<Hunk> = Vec::new();

            // Parse hunks
            while let Some(peek) = lines.peek() {
                if peek.starts_with("---") || peek.starts_with("diff ") {
                    break;
                }
                let peek = *peek;
                if let Some(hunk_header) = strip_prefix(peek, "@@ ") {
                    lines.next(); // consume
                    let orig_start = parse_hunk_header(hunk_header)?;
                    let mut hunk_lines: Vec<HunkLine> = Vec::new();

                    while let Some(hline) = lines.peek() {
                        if hline.starts_with("@@")
                            || hline.starts_with("---")
                            || hline.starts_with("diff ")
                        {
                            break;
                        }
                        let hline = lines.next().unwrap();
                        if let Some(rest) = hline.strip_prefix('+') {
                            hunk_lines.push(HunkLine::Add(rest.to_string()));
                        } else if let Some(rest) = hline.strip_prefix('-') {
                            hunk_lines.push(HunkLine::Remove(rest.to_string()));
                        } else if let Some(rest) = hline.strip_prefix(' ') {
                            hunk_lines.push(HunkLine::Context(rest.to_string()));
                        } else if hline.is_empty() || hline == "\\ No newline at end of file" {
                            // skip
                        } else {
                            hunk_lines.push(HunkLine::Context(hline.to_string()));
                        }
                    }
                    hunks.push(Hunk {
                        orig_start,
                        lines: hunk_lines,
                    });
                } else {
                    lines.next(); // skip unknown lines between hunks
                }
            }

            diffs.push(FileDiff {
                op,
                target_path,
                hunks,
            });
        }
        // else: skip preamble lines (git diff --git a/... b/... etc.)
    }

    Ok(diffs)
}

/// Parse `@@ -orig_start,count +new_start,count @@` returning orig_start.
fn parse_hunk_header(header: &str) -> anyhow::Result<usize> {
    // header is everything after "@@ ", e.g.: "-1,3 +1,4 @@ optional text"
    let part = header
        .split_whitespace()
        .next()
        .ok_or_else(|| anyhow::anyhow!("Empty hunk header"))?;
    // part is like "-1,3" or "-1"
    let after_minus = part
        .strip_prefix('-')
        .ok_or_else(|| anyhow::anyhow!("Hunk header missing '-': {part}"))?;
    let start_str = after_minus.split(',').next().unwrap_or(after_minus);
    start_str
        .parse::<usize>()
        .map_err(|e| anyhow::anyhow!("Cannot parse hunk orig_start from '{start_str}': {e}"))
}

// ---------------------------------------------------------------------------
// Applier
// ---------------------------------------------------------------------------

/// Apply a single hunk to `lines` (0-indexed Vec of line strings).
///
/// Returns the modified line vec, or an error if context doesn't match.
fn apply_hunk(orig_lines: &[String], hunk: &Hunk) -> anyhow::Result<Vec<String>> {
    // Convert 1-based hunk start to 0-based index
    let start_idx = if hunk.orig_start == 0 {
        0
    } else {
        hunk.orig_start - 1
    };

    // Walk through hunk lines and validate context / remove lines
    let mut orig_cursor = start_idx;
    let mut result: Vec<String> = orig_lines[..start_idx].to_vec();

    for hline in &hunk.lines {
        match hline {
            HunkLine::Context(expected) => {
                if orig_cursor >= orig_lines.len() {
                    return Err(anyhow::anyhow!(
                        "Context line {} out of bounds (file has {} lines)",
                        orig_cursor + 1,
                        orig_lines.len()
                    ));
                }
                let actual = &orig_lines[orig_cursor];
                if actual != expected {
                    return Err(anyhow::anyhow!(
                        "Context mismatch at line {}: expected {:?}, got {:?}",
                        orig_cursor + 1,
                        expected,
                        actual
                    ));
                }
                result.push(actual.clone());
                orig_cursor += 1;
            }
            HunkLine::Remove(expected) => {
                if orig_cursor >= orig_lines.len() {
                    return Err(anyhow::anyhow!(
                        "Remove line {} out of bounds (file has {} lines)",
                        orig_cursor + 1,
                        orig_lines.len()
                    ));
                }
                let actual = &orig_lines[orig_cursor];
                if actual != expected {
                    return Err(anyhow::anyhow!(
                        "Remove mismatch at line {}: expected {:?}, got {:?}",
                        orig_cursor + 1,
                        expected,
                        actual
                    ));
                }
                // Consume original line without emitting it
                orig_cursor += 1;
            }
            HunkLine::Add(new_line) => {
                result.push(new_line.clone());
                // orig_cursor does NOT advance
            }
        }
    }

    // Append remaining original lines after the hunk
    result.extend_from_slice(&orig_lines[orig_cursor..]);
    Ok(result)
}

/// Apply all hunks from a `FileDiff::Modify` operation.
fn apply_modify(content: &str, file_diff: &FileDiff) -> anyhow::Result<String> {
    // Split preserving whether file ends with newline
    let ends_with_newline = content.ends_with('\n');
    let mut lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();

    // Apply hunks in reverse order so earlier hunks don't shift line numbers
    // for later hunks.  We must sort by orig_start descending.
    let mut sorted_hunks = file_diff.hunks.clone();
    sorted_hunks.sort_by_key(|b| std::cmp::Reverse(b.orig_start));

    for hunk in &sorted_hunks {
        lines = apply_hunk(&lines, hunk)?;
    }

    let mut result = lines.join("\n");
    if ends_with_newline || result.is_empty() {
        result.push('\n');
    }
    Ok(result)
}

/// Build the full content for a newly-created file from `Add` lines.
fn build_created_content(file_diff: &FileDiff) -> String {
    let mut lines: Vec<String> = Vec::new();
    for hunk in &file_diff.hunks {
        for hline in &hunk.lines {
            if let HunkLine::Add(line) = hline {
                lines.push(line.clone());
            }
        }
    }
    if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n"))
    }
}

// ---------------------------------------------------------------------------
// Capability entry point
// ---------------------------------------------------------------------------

/// Execute the `file:edit:patch` capability.
pub fn execute(
    workspace: &Path,
    request: CapabilityExecutionRequest,
) -> anyhow::Result<serde_json::Value> {
    let input: FileEditPatchInput = serde_json::from_value(request.input)?;
    debug!("file:edit:patch base_dir={:?}", input.base_dir);

    let base = input.base_dir.as_deref().unwrap_or(workspace);

    let diffs = parse_diff(&input.diff_text)?;
    if diffs.is_empty() {
        return Err(anyhow::anyhow!(
            "file:edit:patch: no file diffs found in diff_text"
        ));
    }

    let mut files_modified: Vec<String> = Vec::new();
    let mut files_created: Vec<String> = Vec::new();
    let mut files_deleted: Vec<String> = Vec::new();

    for file_diff in &diffs {
        let file_path = base.join(&file_diff.target_path);
        validate_within_workspace(workspace, &file_path)?;

        match file_diff.op {
            FileOp::Modify => {
                let content = fs::read_to_string(&file_path).map_err(|e| {
                    anyhow::anyhow!("file:edit:patch: cannot read {}: {e}", file_path.display())
                })?;
                let new_content = apply_modify(&content, file_diff)?;
                fs::write(&file_path, &new_content)?;
                files_modified.push(file_diff.target_path.clone());
                info!("file:edit:patch: modified {}", file_diff.target_path);
            }
            FileOp::Create => {
                if let Some(parent) = file_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                let content = build_created_content(file_diff);
                fs::write(&file_path, &content)?;
                files_created.push(file_diff.target_path.clone());
                info!("file:edit:patch: created {}", file_diff.target_path);
            }
            FileOp::Delete => {
                if file_path.exists() {
                    fs::remove_file(&file_path)?;
                }
                files_deleted.push(file_diff.target_path.clone());
                info!("file:edit:patch: deleted {}", file_diff.target_path);
            }
        }
    }

    Ok(serde_json::to_value(FileEditPatchOutput {
        files_modified,
        files_created,
        files_deleted,
    })?)
}

// ---------------------------------------------------------------------------
// Path safety
// ---------------------------------------------------------------------------

fn validate_within_workspace(workspace: &Path, path: &Path) -> anyhow::Result<()> {
    // Check for ".." before any canonicalization
    for component in path.components() {
        if component == std::path::Component::ParentDir {
            return Err(anyhow::anyhow!(
                "Path contains '..' components which are not allowed"
            ));
        }
    }
    let canonical_ws = workspace
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("Cannot canonicalize workspace: {e}"))?;
    // For the file itself we only canonicalize if it exists
    let check_path = if path.exists() {
        path.canonicalize()
            .map_err(|e| anyhow::anyhow!("Cannot canonicalize path: {e}"))?
    } else {
        // Check nearest existing ancestor
        let mut ancestor = path.to_path_buf();
        while !ancestor.exists() {
            match ancestor.parent() {
                Some(p) => ancestor = p.to_path_buf(),
                None => {
                    return Err(anyhow::anyhow!(
                        "Cannot validate path: no existing ancestor"
                    ))
                }
            }
        }
        ancestor
            .canonicalize()
            .map_err(|e| anyhow::anyhow!("Cannot canonicalize ancestor: {e}"))?
    };
    if !check_path.starts_with(&canonical_ws) {
        return Err(anyhow::anyhow!("Path is outside workspace boundary"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use cyberclaw_core::identity::Identity;
    use cyberclaw_core::prelude::*;
    use cyberclaw_core::workspace::{WorkspaceMode, WorkspaceRef};
    use tempfile::TempDir;

    fn make_request(workspace: &TempDir, input: serde_json::Value) -> CapabilityExecutionRequest {
        let root = workspace.path().to_string_lossy().to_string();
        CapabilityExecutionRequest {
            execution_id: ExecutionId::new(),
            trace_id: "test-trace".to_string(),
            actor: Identity::System.to_actor_ref(None).unwrap(),
            workspace: WorkspaceRef {
                id: WorkspaceId::from_string("test-ws".to_string()).unwrap(),
                mode: WorkspaceMode::Ephemeral,
                materialization_mode: None,
                home_node_id: None,
                backing_store: None,
                root,
                writable_roots: vec![],
            },
            connector_id: ConnectorId::from_string("local".to_string()).unwrap(),
            capability_id: CapabilityId::from_string("file:edit:patch".to_string()).unwrap(),
            input,
        }
    }

    // --- simple_modify ---

    #[test]
    fn simple_modify_replaces_lines() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("simple.txt");
        fs::write(&file, "line1\nline2\nline3\n").unwrap();

        let diff = "--- a/simple.txt\n+++ b/simple.txt\n@@ -1,3 +1,3 @@\n line1\n-line2\n+replaced\n line3\n".to_string();
        let req = make_request(
            &tmp,
            serde_json::json!({
                "diff_text": diff,
                "base_dir": tmp.path().to_str().unwrap()
            }),
        );
        let out_val = execute(tmp.path(), req).unwrap();
        let out: FileEditPatchOutput = serde_json::from_value(out_val).unwrap();
        assert_eq!(out.files_modified, vec!["simple.txt"]);
        assert!(out.files_created.is_empty());
        assert!(out.files_deleted.is_empty());
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "line1\nreplaced\nline3\n"
        );
    }

    // --- add_file ---

    #[test]
    fn add_file_creates_new_file() {
        let tmp = TempDir::new().unwrap();

        let diff =
            "--- /dev/null\n+++ b/newfile.txt\n@@ -0,0 +1,2 @@\n+hello\n+world\n".to_string();
        let req = make_request(
            &tmp,
            serde_json::json!({
                "diff_text": diff,
                "base_dir": tmp.path().to_str().unwrap()
            }),
        );
        let out_val = execute(tmp.path(), req).unwrap();
        let out: FileEditPatchOutput = serde_json::from_value(out_val).unwrap();
        assert_eq!(out.files_created, vec!["newfile.txt"]);
        let content = fs::read_to_string(tmp.path().join("newfile.txt")).unwrap();
        assert_eq!(content, "hello\nworld\n");
    }

    // --- delete_file ---

    #[test]
    fn delete_file_removes_it() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("todelete.txt");
        fs::write(&file, "bye\n").unwrap();

        let diff = "--- a/todelete.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-bye\n".to_string();
        let req = make_request(
            &tmp,
            serde_json::json!({
                "diff_text": diff,
                "base_dir": tmp.path().to_str().unwrap()
            }),
        );
        let out_val = execute(tmp.path(), req).unwrap();
        let out: FileEditPatchOutput = serde_json::from_value(out_val).unwrap();
        assert_eq!(out.files_deleted, vec!["todelete.txt"]);
        assert!(!file.exists());
    }

    // --- malformed_diff ---

    #[test]
    fn malformed_diff_no_plus_plus_line_errors() {
        let tmp = TempDir::new().unwrap();
        let req = make_request(
            &tmp,
            serde_json::json!({
                "diff_text": "--- a/file.txt\nthis is not +++ line\n"
            }),
        );
        assert!(execute(tmp.path(), req).is_err());
    }

    #[test]
    fn empty_diff_text_errors() {
        let tmp = TempDir::new().unwrap();
        let req = make_request(&tmp, serde_json::json!({ "diff_text": "" }));
        assert!(execute(tmp.path(), req).is_err());
    }

    // --- context_mismatch_errors ---

    #[test]
    fn context_mismatch_returns_error() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("ctx.txt");
        fs::write(&file, "line1\nline2\nline3\n").unwrap();

        // Context line says "WRONG" but file has "line1"
        let diff = "--- a/ctx.txt\n+++ b/ctx.txt\n@@ -1,2 +1,2 @@\n WRONG\n-line2\n+replaced\n"
            .to_string();
        let req = make_request(
            &tmp,
            serde_json::json!({
                "diff_text": diff,
                "base_dir": tmp.path().to_str().unwrap()
            }),
        );
        assert!(execute(tmp.path(), req).is_err());
        // file must be unchanged
        assert_eq!(fs::read_to_string(&file).unwrap(), "line1\nline2\nline3\n");
    }

    // --- multi-hunk patch ---

    #[test]
    fn multi_hunk_patch_applies_correctly() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("mh.txt");
        fs::write(&file, "a\nb\nc\nd\ne\n").unwrap();

        // Two hunks: change line 1 and line 5
        let diff = concat!(
            "--- a/mh.txt\n",
            "+++ b/mh.txt\n",
            "@@ -1,1 +1,1 @@\n",
            "-a\n",
            "+A\n",
            "@@ -5,1 +5,1 @@\n",
            "-e\n",
            "+E\n",
        );
        let req = make_request(
            &tmp,
            serde_json::json!({
                "diff_text": diff,
                "base_dir": tmp.path().to_str().unwrap()
            }),
        );
        let out_val = execute(tmp.path(), req).unwrap();
        let out: FileEditPatchOutput = serde_json::from_value(out_val).unwrap();
        assert_eq!(out.files_modified, vec!["mh.txt"]);
        assert_eq!(fs::read_to_string(&file).unwrap(), "A\nb\nc\nd\nE\n");
    }
}
