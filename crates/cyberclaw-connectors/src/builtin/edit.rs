//! Capability `file:edit:replace` — single-file string replacement with uniqueness guard.
//!
//! # Input
//! ```json
//! {
//!   "path":       "<absolute path>",
//!   "old_string": "<exact text to find>",
//!   "new_string": "<replacement text>",
//!   "count_hint": 1          // optional, defaults to 1 (must equal actual occurrences)
//! }
//! ```
//!
//! # Output
//! ```json
//! { "path": "...", "replacements": 1, "backup_path": "..." }
//! ```
//!
//! # Semantics
//! - If `count_hint` is omitted or 1, `old_string` **must** appear exactly once; otherwise
//!   the operation is rejected to prevent accidental multi-site edits.
//! - If `count_hint` is N > 1, exactly N occurrences must be present and all are replaced.
//! - A `.cyberclaw/backups/<sha256_of_path_hex>.bak` file is written before any mutation.

use crate::types::CapabilityExecutionRequest;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, info};

// ---------------------------------------------------------------------------
// I/O types
// ---------------------------------------------------------------------------

/// Input for `file:edit:replace`.
///
/// Accepts both the native `count_hint` form (expect exactly N occurrences) and
/// the claude-code-style `replace_all` boolean form. When `replace_all=true`,
/// every occurrence is replaced and uniqueness is not enforced. When
/// `replace_all=false` (default), the occurrence count must equal `count_hint`
/// (default 1) — this preserves the "fail loudly on ambiguous edit" semantics.
///
/// The schema also accepts `file_path` as an alias for `path` so that
/// LLM-visible facades can match claude-code's Edit tool shape verbatim.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEditReplaceInput {
    /// Absolute path to the target file. Accepts either `path` or `file_path`.
    #[serde(alias = "file_path")]
    pub path: String,
    /// Exact string to locate in the file.
    pub old_string: String,
    /// Replacement string.
    pub new_string: String,
    /// Expected occurrence count (default = 1).  The actual count must match
    /// when `replace_all` is false.
    #[serde(default = "default_count_hint")]
    pub count_hint: usize,
    /// When true, replace every occurrence of `old_string` without requiring
    /// a unique match. Mirrors claude-code's Edit `replace_all` parameter.
    #[serde(default)]
    pub replace_all: bool,
}

fn default_count_hint() -> usize {
    1
}

/// Output for `file:edit:replace`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEditReplaceOutput {
    /// Path that was edited.
    pub path: String,
    /// Number of replacements made.
    pub replacements: usize,
    /// Path of the pre-edit backup file.
    pub backup_path: String,
}

// ---------------------------------------------------------------------------
// Backup helper
// ---------------------------------------------------------------------------

/// Write a backup of `path` under `<workspace_root>/.cyberclaw/backups/`.
///
/// The backup filename is the hex-encoded SHA-256 of the **absolute path string**,
/// so re-running the same edit overwrites the previous backup (latest wins).
fn write_backup(workspace: &Path, file_path: &Path, content: &str) -> anyhow::Result<PathBuf> {
    let hash = {
        let mut hasher = Sha256::new();
        hasher.update(file_path.to_string_lossy().as_bytes());
        hex::encode(hasher.finalize())
    };

    let backup_dir = workspace.join(".cyberclaw").join("backups");
    fs::create_dir_all(&backup_dir)?;

    let backup_path = backup_dir.join(format!("{}.bak", hash));
    fs::write(&backup_path, content)?;
    debug!(
        "Backup written: {} -> {}",
        file_path.display(),
        backup_path.display()
    );
    Ok(backup_path)
}

// ---------------------------------------------------------------------------
// Capability entry point
// ---------------------------------------------------------------------------

/// Execute the `file:edit:replace` capability.
///
/// `workspace` is the connector's workspace root, used both for path validation
/// and backup directory placement.
pub fn execute(
    workspace: &Path,
    request: CapabilityExecutionRequest,
) -> anyhow::Result<serde_json::Value> {
    let input: FileEditReplaceInput = serde_json::from_value(request.input)?;
    debug!("file:edit:replace path={}", input.path);

    let path = validate_path(workspace, &input.path)?;
    let original = fs::read_to_string(&path)?;

    // Count occurrences
    let actual_count = original.matches(input.old_string.as_str()).count();

    if actual_count == 0 {
        return Err(anyhow::anyhow!(
            "file:edit:replace: old_string not found in {}",
            input.path
        ));
    }
    // When replace_all=true, uniqueness is not enforced: every occurrence
    // is replaced. When replace_all=false, actual count must equal count_hint
    // (default 1) — this is the claude-code "fail loudly on ambiguous" rule.
    if !input.replace_all && actual_count != input.count_hint {
        return Err(anyhow::anyhow!(
            "file:edit:replace: expected {} occurrence(s) of old_string but found {} in {} (pass replace_all=true to replace every occurrence)",
            input.count_hint,
            actual_count,
            input.path
        ));
    }

    // Backup before mutation
    let backup_path = write_backup(workspace, &path, &original)?;

    // Apply replacements
    let new_content = original.replace(input.old_string.as_str(), input.new_string.as_str());
    fs::write(&path, &new_content)?;

    info!(
        "file:edit:replace: {} replacement(s) in {}",
        actual_count, input.path
    );

    Ok(serde_json::to_value(FileEditReplaceOutput {
        path: input.path,
        replacements: actual_count,
        backup_path: backup_path.to_string_lossy().to_string(),
    })?)
}

// ---------------------------------------------------------------------------
// Path validation (no connector dependency)
// ---------------------------------------------------------------------------

fn validate_path(workspace: &Path, path_str: &str) -> anyhow::Result<PathBuf> {
    let abs = if Path::new(path_str).is_absolute() {
        PathBuf::from(path_str)
    } else {
        workspace.join(path_str)
    };

    for component in abs.components() {
        if component == std::path::Component::ParentDir {
            return Err(anyhow::anyhow!(
                "Path contains '..' components which are not allowed"
            ));
        }
    }

    let canonical_ws = workspace
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("Cannot canonicalize workspace: {e}"))?;

    if abs.exists() {
        let canonical = abs
            .canonicalize()
            .map_err(|e| anyhow::anyhow!("Cannot canonicalize path: {e}"))?;
        if !canonical.starts_with(&canonical_ws) {
            return Err(anyhow::anyhow!("Path is outside workspace boundary"));
        }
        Ok(canonical)
    } else {
        // Walk up to nearest existing ancestor
        let mut ancestor = abs.clone();
        let mut tail: Vec<std::ffi::OsString> = Vec::new();
        while !ancestor.exists() {
            if let Some(name) = ancestor.file_name() {
                tail.push(name.to_os_string());
            }
            match ancestor.parent() {
                Some(p) => ancestor = p.to_path_buf(),
                None => {
                    return Err(anyhow::anyhow!(
                        "Cannot validate path: no existing ancestor"
                    ))
                }
            }
        }
        let canonical_ancestor = ancestor
            .canonicalize()
            .map_err(|e| anyhow::anyhow!("Cannot canonicalize ancestor: {e}"))?;
        if !canonical_ancestor.starts_with(&canonical_ws) {
            return Err(anyhow::anyhow!("Path is outside workspace boundary"));
        }
        let mut reconstructed = canonical_ancestor;
        for component in tail.iter().rev() {
            reconstructed.push(component);
        }
        if !reconstructed.starts_with(&canonical_ws) {
            return Err(anyhow::anyhow!("Path is outside workspace boundary"));
        }
        Ok(reconstructed)
    }
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
            capability_id: CapabilityId::from_string("file:edit:replace".to_string()).unwrap(),
            input,
        }
    }

    // --- happy path ---

    #[test]
    fn happy_path_single_replacement() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("hello.txt");
        fs::write(&file, "Hello World").unwrap();

        let req = make_request(
            &tmp,
            serde_json::json!({
                "path": file.to_str().unwrap(),
                "old_string": "World",
                "new_string": "CyberClaw"
            }),
        );
        let out_val = execute(tmp.path(), req).unwrap();
        let out: FileEditReplaceOutput = serde_json::from_value(out_val).unwrap();
        assert_eq!(out.replacements, 1);
        assert_eq!(fs::read_to_string(&file).unwrap(), "Hello CyberClaw");
        // backup must exist
        assert!(Path::new(&out.backup_path).exists());
    }

    // --- not_unique: two occurrences, count_hint=1 → rejected ---

    #[test]
    fn rejected_when_not_unique() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("dup.txt");
        fs::write(&file, "foo foo").unwrap();

        let req = make_request(
            &tmp,
            serde_json::json!({
                "path": file.to_str().unwrap(),
                "old_string": "foo",
                "new_string": "bar"
            }),
        );
        let err = execute(tmp.path(), req).unwrap_err();
        assert!(
            err.to_string().contains("expected 1 occurrence"),
            "unexpected error: {err}"
        );
        // file must be unchanged
        assert_eq!(fs::read_to_string(&file).unwrap(), "foo foo");
    }

    // --- not_found ---

    #[test]
    fn rejected_when_not_found() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("nf.txt");
        fs::write(&file, "hello").unwrap();

        let req = make_request(
            &tmp,
            serde_json::json!({
                "path": file.to_str().unwrap(),
                "old_string": "NOTHERE",
                "new_string": "x"
            }),
        );
        assert!(execute(tmp.path(), req).is_err());
    }

    // --- count_hint > 1 succeeds when exact match ---

    #[test]
    fn count_hint_multi_replaces_all() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("multi.txt");
        fs::write(&file, "x x x").unwrap();

        let req = make_request(
            &tmp,
            serde_json::json!({
                "path": file.to_str().unwrap(),
                "old_string": "x",
                "new_string": "y",
                "count_hint": 3
            }),
        );
        let out_val = execute(tmp.path(), req).unwrap();
        let out: FileEditReplaceOutput = serde_json::from_value(out_val).unwrap();
        assert_eq!(out.replacements, 3);
        assert_eq!(fs::read_to_string(&file).unwrap(), "y y y");
    }

    // --- multiline replacement ---

    #[test]
    fn multiline_replacement_works() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("ml.txt");
        fs::write(&file, "line1\nline2\nline3\n").unwrap();

        let req = make_request(
            &tmp,
            serde_json::json!({
                "path": file.to_str().unwrap(),
                "old_string": "line1\nline2",
                "new_string": "replaced"
            }),
        );
        let out_val = execute(tmp.path(), req).unwrap();
        let out: FileEditReplaceOutput = serde_json::from_value(out_val).unwrap();
        assert_eq!(out.replacements, 1);
        assert_eq!(fs::read_to_string(&file).unwrap(), "replaced\nline3\n");
    }

    // --- backup content matches original ---

    #[test]
    fn backup_contains_original_content() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("bak.txt");
        let original = "original content";
        fs::write(&file, original).unwrap();

        let req = make_request(
            &tmp,
            serde_json::json!({
                "path": file.to_str().unwrap(),
                "old_string": "original",
                "new_string": "new"
            }),
        );
        let out_val = execute(tmp.path(), req).unwrap();
        let out: FileEditReplaceOutput = serde_json::from_value(out_val).unwrap();
        let backup_content = fs::read_to_string(&out.backup_path).unwrap();
        assert_eq!(backup_content, original);
    }

    // --- replace_all=true replaces every occurrence without uniqueness check ---

    #[test]
    fn replace_all_true_replaces_every_occurrence() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("ra.txt");
        fs::write(&file, "cat dog cat bird cat").unwrap();

        let req = make_request(
            &tmp,
            serde_json::json!({
                "path": file.to_str().unwrap(),
                "old_string": "cat",
                "new_string": "LION",
                "replace_all": true
            }),
        );
        let out_val = execute(tmp.path(), req).unwrap();
        let out: FileEditReplaceOutput = serde_json::from_value(out_val).unwrap();
        assert_eq!(out.replacements, 3);
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "LION dog LION bird LION"
        );
    }

    // --- path escape prevention: '..' components are rejected ---

    #[test]
    fn path_escape_via_parent_dir_is_rejected() {
        let tmp = TempDir::new().unwrap();
        // Target path pretending to climb out of the workspace.
        let evil = format!("{}/../../../etc/passwd", tmp.path().to_string_lossy());

        let req = make_request(
            &tmp,
            serde_json::json!({
                "path": evil,
                "old_string": "x",
                "new_string": "y"
            }),
        );
        let err = execute(tmp.path(), req).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("'..'") || msg.contains("outside workspace"),
            "expected path-escape rejection, got: {msg}"
        );
    }

    // --- claude-code-style file_path alias is accepted ---

    #[test]
    fn file_path_alias_is_accepted() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("alias.txt");
        fs::write(&file, "hello alias").unwrap();

        // Use 'file_path' (claude-code shape), not 'path'.
        let req = make_request(
            &tmp,
            serde_json::json!({
                "file_path": file.to_str().unwrap(),
                "old_string": "alias",
                "new_string": "world"
            }),
        );
        let out_val = execute(tmp.path(), req).unwrap();
        let out: FileEditReplaceOutput = serde_json::from_value(out_val).unwrap();
        assert_eq!(out.replacements, 1);
        assert_eq!(fs::read_to_string(&file).unwrap(), "hello world");
    }
}
