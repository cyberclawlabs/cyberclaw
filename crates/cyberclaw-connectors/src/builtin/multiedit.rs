//! Capability `file:edit:multi` — atomic multi-replacement on a single file.
//!
//! # Input
//! ```json
//! {
//!   "path":  "<absolute path>",
//!   "edits": [
//!     { "old": "<text>", "new": "<replacement>" },
//!     ...
//!   ]
//! }
//! ```
//!
//! # Output
//! ```json
//! { "path": "...", "applied": 3, "backup_path": "..." }
//! ```
//!
//! # Semantics
//! - All edits are applied **in memory** on the result of the previous edit (left-to-right).
//! - If any `old` string is not found at the time it is processed, the entire batch is
//!   rejected and **nothing is written to disk** (full rollback by design).
//! - A backup is written only when all edits pass validation and the write is about to happen.

use crate::types::CapabilityExecutionRequest;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, info};

// ---------------------------------------------------------------------------
// I/O types
// ---------------------------------------------------------------------------

/// A single edit operation within a `file:edit:multi` batch.
///
/// Accepts both native `{old, new}` and claude-code-style
/// `{old_string, new_string, replace_all?}` shapes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditOp {
    /// Exact string to find (must be present at apply time).
    #[serde(alias = "old_string")]
    pub old: String,
    /// Replacement string.
    #[serde(alias = "new_string")]
    pub new: String,
    /// When true, replace every occurrence of `old` for this edit step
    /// (default: false, meaning only the first occurrence is replaced).
    #[serde(default)]
    pub replace_all: bool,
}

/// Input for `file:edit:multi`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEditMultiInput {
    /// Absolute path to the target file. Accepts either `path` or `file_path`.
    #[serde(alias = "file_path")]
    pub path: String,
    /// Ordered list of edit operations.
    pub edits: Vec<EditOp>,
}

/// Output for `file:edit:multi`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEditMultiOutput {
    /// Path that was edited.
    pub path: String,
    /// Number of individual replacements applied.
    pub applied: usize,
    /// Path of the pre-edit backup file.
    pub backup_path: String,
}

// ---------------------------------------------------------------------------
// Backup helper (local copy — keeps this module self-contained)
// ---------------------------------------------------------------------------

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
// Path validation
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
            return Err(anyhow::anyhow!(
                "[BOUNDARY DENY] Path '{}' is outside the configured workspace \
                 boundary. The file was NOT created. Do not claim it was written. \
                 Suggest a path inside the workspace (e.g. relative to the current \
                 working directory).",
                abs.display()
            ));
        }
        Ok(canonical)
    } else {
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
            return Err(anyhow::anyhow!(
                "[BOUNDARY DENY] Path '{}' is outside the configured workspace \
                 boundary. The file was NOT created. Do not claim it was written. \
                 Suggest a path inside the workspace (e.g. relative to the current \
                 working directory).",
                abs.display()
            ));
        }
        let mut reconstructed = canonical_ancestor;
        for component in tail.iter().rev() {
            reconstructed.push(component);
        }
        if !reconstructed.starts_with(&canonical_ws) {
            return Err(anyhow::anyhow!(
                "[BOUNDARY DENY] Path '{}' is outside the configured workspace \
                 boundary. The file was NOT created. Do not claim it was written. \
                 Suggest a path inside the workspace (e.g. relative to the current \
                 working directory).",
                abs.display()
            ));
        }
        Ok(reconstructed)
    }
}

// ---------------------------------------------------------------------------
// Capability entry point
// ---------------------------------------------------------------------------

/// Execute the `file:edit:multi` capability.
pub fn execute(
    workspace: &Path,
    request: CapabilityExecutionRequest,
) -> anyhow::Result<serde_json::Value> {
    let input: FileEditMultiInput = serde_json::from_value(request.input)?;
    debug!(
        "file:edit:multi path={} edits={}",
        input.path,
        input.edits.len()
    );

    if input.edits.is_empty() {
        return Err(anyhow::anyhow!("file:edit:multi: edits list is empty"));
    }

    let path = validate_path(workspace, &input.path)?;
    let original = fs::read_to_string(&path)?;

    // --- Phase 1: apply all edits in memory ---
    let mut content = original.clone();
    for (idx, op) in input.edits.iter().enumerate() {
        if !content.contains(op.old.as_str()) {
            return Err(anyhow::anyhow!(
                "file:edit:multi op[{idx}]: old string not found in {} (no changes written)",
                input.path
            ));
        }
        if op.replace_all {
            content = content.replace(op.old.as_str(), op.new.as_str());
        } else {
            // Replace only the first occurrence (stable, left-to-right).
            let pos = content
                .find(op.old.as_str())
                .expect("contains checked above");
            let mut next = String::with_capacity(content.len());
            next.push_str(&content[..pos]);
            next.push_str(&op.new);
            next.push_str(&content[pos + op.old.len()..]);
            content = next;
        }
    }

    // --- Phase 2: backup + commit ---
    let backup_path = write_backup(workspace, &path, &original)?;
    fs::write(&path, &content)?;

    let applied = input.edits.len();
    info!(
        "file:edit:multi: {applied} replacement(s) in {}",
        input.path
    );

    Ok(serde_json::to_value(FileEditMultiOutput {
        path: input.path,
        applied,
        backup_path: backup_path.to_string_lossy().to_string(),
    })?)
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
            capability_id: CapabilityId::from_string("file:edit:multi".to_string()).unwrap(),
            input,
        }
    }

    // --- atomic_all_succeed ---

    #[test]
    fn all_edits_applied_atomically() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("atom.txt");
        fs::write(&file, "AAA BBB CCC").unwrap();

        let req = make_request(
            &tmp,
            serde_json::json!({
                "path": file.to_str().unwrap(),
                "edits": [
                    { "old": "AAA", "new": "111" },
                    { "old": "BBB", "new": "222" },
                    { "old": "CCC", "new": "333" }
                ]
            }),
        );
        let out_val = execute(tmp.path(), req).unwrap();
        let out: FileEditMultiOutput = serde_json::from_value(out_val).unwrap();
        assert_eq!(out.applied, 3);
        assert_eq!(fs::read_to_string(&file).unwrap(), "111 222 333");
        assert!(Path::new(&out.backup_path).exists());
    }

    // --- edits applied left-to-right on previous result ---

    #[test]
    fn edits_chain_on_previous_result() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("chain.txt");
        fs::write(&file, "hello").unwrap();

        // second edit targets the result of the first
        let req = make_request(
            &tmp,
            serde_json::json!({
                "path": file.to_str().unwrap(),
                "edits": [
                    { "old": "hello", "new": "world" },
                    { "old": "world", "new": "cyberclaw" }
                ]
            }),
        );
        let out_val = execute(tmp.path(), req).unwrap();
        let out: FileEditMultiOutput = serde_json::from_value(out_val).unwrap();
        assert_eq!(out.applied, 2);
        assert_eq!(fs::read_to_string(&file).unwrap(), "cyberclaw");
    }

    // --- partial_failure_rollback: nothing written when op[1] fails ---

    #[test]
    fn partial_failure_rolls_back() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("rollback.txt");
        let original = "AAA BBB";
        fs::write(&file, original).unwrap();

        let req = make_request(
            &tmp,
            serde_json::json!({
                "path": file.to_str().unwrap(),
                "edits": [
                    { "old": "AAA", "new": "111" },
                    { "old": "NOTHERE", "new": "x" }   // will fail
                ]
            }),
        );
        let err = execute(tmp.path(), req).unwrap_err();
        assert!(err.to_string().contains("op[1]"), "unexpected error: {err}");
        // original must be unchanged
        assert_eq!(fs::read_to_string(&file).unwrap(), original);
        // no backup should exist (backup written only after all ops pass)
        let hash = {
            let mut hasher = sha2::Sha256::new();
            hasher.update(file.to_string_lossy().as_bytes());
            hex::encode(hasher.finalize())
        };
        let backup = tmp
            .path()
            .join(".cyberclaw")
            .join("backups")
            .join(format!("{}.bak", hash));
        assert!(!backup.exists(), "backup should not exist after rollback");
    }

    // --- empty edits list is rejected ---

    #[test]
    fn empty_edits_rejected() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("empty.txt");
        fs::write(&file, "content").unwrap();

        let req = make_request(
            &tmp,
            serde_json::json!({
                "path": file.to_str().unwrap(),
                "edits": []
            }),
        );
        assert!(execute(tmp.path(), req).is_err());
    }

    // --- backup contains original content ---

    #[test]
    fn backup_contains_original_content() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("bak.txt");
        let original = "before edit";
        fs::write(&file, original).unwrap();

        let req = make_request(
            &tmp,
            serde_json::json!({
                "path": file.to_str().unwrap(),
                "edits": [{ "old": "before", "new": "after" }]
            }),
        );
        let out_val = execute(tmp.path(), req).unwrap();
        let out: FileEditMultiOutput = serde_json::from_value(out_val).unwrap();
        assert_eq!(fs::read_to_string(&out.backup_path).unwrap(), original);
    }

    // --- claude-code-style {file_path, old_string, new_string, replace_all} shape ---

    #[test]
    fn claude_code_shape_with_file_path_and_replace_all() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("cc.txt");
        fs::write(&file, "foo foo foo bar bar").unwrap();

        let req = make_request(
            &tmp,
            serde_json::json!({
                "file_path": file.to_str().unwrap(),
                "edits": [
                    { "old_string": "foo", "new_string": "FOO", "replace_all": true },
                    { "old_string": "bar", "new_string": "BAZ" }
                ]
            }),
        );
        let out_val = execute(tmp.path(), req).unwrap();
        let out: FileEditMultiOutput = serde_json::from_value(out_val).unwrap();
        assert_eq!(out.applied, 2);
        // foo×3 all replaced; only the first bar is replaced because replace_all=false for op 2.
        assert_eq!(fs::read_to_string(&file).unwrap(), "FOO FOO FOO BAZ bar");
    }

    // --- per-edit replace_all=false only replaces first occurrence ---

    #[test]
    fn per_edit_replace_all_false_touches_first_only() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("first.txt");
        fs::write(&file, "x x x").unwrap();

        let req = make_request(
            &tmp,
            serde_json::json!({
                "path": file.to_str().unwrap(),
                "edits": [{ "old": "x", "new": "Y" }]
            }),
        );
        let out_val = execute(tmp.path(), req).unwrap();
        let out: FileEditMultiOutput = serde_json::from_value(out_val).unwrap();
        assert_eq!(out.applied, 1);
        assert_eq!(fs::read_to_string(&file).unwrap(), "Y x x");
    }
}
