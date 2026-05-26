use super::LocalConnector;
use crate::types::*;
use cyberclaw_core::capability::RiskLevel;
use cyberclaw_core::facade::{CapabilityFacade, FacadeExposure, ToolsetCategory};
use cyberclaw_core::ids::{CapabilityId, ConnectorId};
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use tracing::{debug, info};

// ---------------------------------------------------------------------------
// §4 Facade export — real cyberclaw_core::facade types (Phase 2)
// ---------------------------------------------------------------------------

/// Return facade entries for every `fs.*` capability in this module.
///
/// The host binary registers them under `ToolsetCategory::FileSystem`.
// Public API pre-integration: wired by host binary during capability
// composition. See docs/architecture/idioms/EVOLUTION_IDIOMS.md §4.
#[allow(dead_code)]
pub fn capability_facades() -> Vec<(CapabilityFacade, ToolsetCategory)> {
    let connector_id = ConnectorId::from_string("local".to_string()).unwrap();
    vec![
        (
            CapabilityFacade {
                name: "file_read".to_string(),
                description:
                    "Read the contents of a file. Supports optional byte offset and limit \
                              for partial reads."
                        .to_string(),
                connector_id: connector_id.clone(),
                capability_id: CapabilityId::from_string("fs.read".to_string()).unwrap(),
                risk_level: RiskLevel::Low,
                effects: vec!["read".to_string()],
                read_only: true,
                destructive: false,
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Absolute path to the file" },
                        "offset": { "type": "integer", "description": "Byte offset to start reading from" },
                        "limit": { "type": "integer", "description": "Maximum bytes to read" }
                    },
                    "required": ["path"]
                })),
                exposure: FacadeExposure::LlmDefault,
                workspace_root: None,
            },
            ToolsetCategory::FileSystem,
        ),
        (
            CapabilityFacade {
                name: "file_write".to_string(),
                description: "Create or overwrite a file with the provided content.".to_string(),
                connector_id: connector_id.clone(),
                capability_id: CapabilityId::from_string("fs.write".to_string()).unwrap(),
                risk_level: RiskLevel::Medium,
                effects: vec!["write".to_string()],
                read_only: false,
                destructive: false,
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Absolute path to write" },
                        "content": { "type": "string", "description": "Content to write" },
                        "create_dirs": {
                            "type": "boolean",
                            "description": "Create parent dirs if missing"
                        }
                    },
                    "required": ["path", "content"]
                })),
                exposure: FacadeExposure::LlmDefault,
                workspace_root: None,
            },
            ToolsetCategory::FileSystem,
        ),
        (
            CapabilityFacade {
                name: "file_edit".to_string(),
                description:
                    "Replace the first occurrence of old_string with new_string in a file. \
                              Set replace_all to true to replace every occurrence."
                        .to_string(),
                connector_id: connector_id.clone(),
                capability_id: CapabilityId::from_string("fs.edit".to_string()).unwrap(),
                risk_level: RiskLevel::Medium,
                effects: vec!["write".to_string()],
                read_only: false,
                destructive: false,
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Absolute path to the file" },
                        "old_string": { "type": "string", "description": "Exact text to find" },
                        "new_string": { "type": "string", "description": "Replacement text" },
                        "replace_all": {
                            "type": "boolean",
                            "description": "Replace all occurrences (default false)"
                        }
                    },
                    "required": ["path", "old_string", "new_string"]
                })),
                exposure: FacadeExposure::LlmDefault,
                workspace_root: None,
            },
            ToolsetCategory::FileSystem,
        ),
        (
            CapabilityFacade {
                name: "file_multiedit".to_string(),
                description: "Apply a batch of old_string to new_string replacements to one file \
                              atomically. Operations are applied left-to-right on the in-memory \
                              content; each old_string must be present."
                    .to_string(),
                connector_id: connector_id.clone(),
                capability_id: CapabilityId::from_string("fs.multi_edit".to_string()).unwrap(),
                risk_level: RiskLevel::Medium,
                effects: vec!["write".to_string()],
                read_only: false,
                destructive: false,
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Absolute path to the file" },
                        "edits": {
                            "type": "array",
                            "description": "Ordered list of edit operations",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "old_string": { "type": "string" },
                                    "new_string": { "type": "string" }
                                },
                                "required": ["old_string", "new_string"]
                            }
                        }
                    },
                    "required": ["path", "edits"]
                })),
                exposure: FacadeExposure::LlmDefault,
                workspace_root: None,
            },
            ToolsetCategory::FileSystem,
        ),
        (
            CapabilityFacade {
                name: "file_append".to_string(),
                description: "Append text to the end of an existing file.".to_string(),
                connector_id: connector_id.clone(),
                capability_id: CapabilityId::from_string("fs.append".to_string()).unwrap(),
                risk_level: RiskLevel::Medium,
                effects: vec!["write".to_string()],
                read_only: false,
                destructive: false,
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Absolute path to the file" },
                        "content": { "type": "string", "description": "Text to append" }
                    },
                    "required": ["path", "content"]
                })),
                exposure: FacadeExposure::LlmDefault,
                workspace_root: None,
            },
            ToolsetCategory::FileSystem,
        ),
        (
            CapabilityFacade {
                name: "file_delete".to_string(),
                description: "Delete a file. Pass recursive=true to remove a directory tree."
                    .to_string(),
                connector_id: connector_id.clone(),
                capability_id: CapabilityId::from_string("fs.delete".to_string()).unwrap(),
                risk_level: RiskLevel::High,
                effects: vec!["write".to_string()],
                read_only: false,
                destructive: true,
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Absolute path to delete" },
                        "recursive": {
                            "type": "boolean",
                            "description": "Remove directory recursively (default false)"
                        }
                    },
                    "required": ["path"]
                })),
                exposure: FacadeExposure::LlmDefault,
                workspace_root: None,
            },
            ToolsetCategory::FileSystem,
        ),
        (
            CapabilityFacade {
                name: "file_list".to_string(),
                description: "List the immediate children of a directory.".to_string(),
                connector_id: connector_id.clone(),
                capability_id: CapabilityId::from_string("fs.list_dir".to_string()).unwrap(),
                risk_level: RiskLevel::Low,
                effects: vec!["read".to_string()],
                read_only: true,
                destructive: false,
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Absolute path to the directory"
                        },
                        "include_hidden": {
                            "type": "boolean",
                            "description": "Include hidden entries (default false)"
                        }
                    },
                    "required": ["path"]
                })),
                exposure: FacadeExposure::LlmDefault,
                workspace_root: None,
            },
            ToolsetCategory::FileSystem,
        ),
        (
            CapabilityFacade {
                name: "file_stat".to_string(),
                description:
                    "Return metadata for a path: existence, type, size, mtime, permissions."
                        .to_string(),
                connector_id: connector_id.clone(),
                capability_id: CapabilityId::from_string("fs.stat".to_string()).unwrap(),
                risk_level: RiskLevel::Low,
                effects: vec!["read".to_string()],
                read_only: true,
                destructive: false,
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Absolute path to inspect" }
                    },
                    "required": ["path"]
                })),
                exposure: FacadeExposure::LlmDefault,
                workspace_root: None,
            },
            ToolsetCategory::FileSystem,
        ),
    ]
}

// ---------------------------------------------------------------------------
// Capability implementations
// ---------------------------------------------------------------------------

/// Read file capability implementation
pub fn read(
    connector: &LocalConnector,
    request: CapabilityExecutionRequest,
) -> anyhow::Result<serde_json::Value> {
    let input: FsReadInput = serde_json::from_value(request.input)?;
    let actor = request.actor.clone();
    debug!("Reading file: {}", input.path);

    // Validate path is within workspace
    let path = connector.validate_path_for_actor(&input.path, &actor)?;

    // Open and read file
    let mut file = fs::File::open(&path)?;

    // Handle offset if specified
    if let Some(offset) = input.offset {
        file.seek(SeekFrom::Start(offset))?;
    }

    // Read content with limit if specified
    let content = if let Some(limit) = input.limit {
        let buffer_size = usize::try_from(limit).unwrap_or(usize::MAX);
        let mut buffer = vec![0; buffer_size];
        let bytes_read = file.read(&mut buffer)?;
        buffer.truncate(bytes_read);
        String::from_utf8_lossy(&buffer).to_string()
    } else {
        let mut content = String::new();
        file.read_to_string(&mut content)?;
        content
    };

    let truncated = input.limit.is_some()
        && content.len() == usize::try_from(input.limit.unwrap()).unwrap_or(usize::MAX);

    info!("Read {} bytes from {}", content.len(), input.path);

    let output = FsReadOutput {
        content,
        path: input.path,
        truncated,
    };

    Ok(serde_json::to_value(output)?)
}

/// Write file capability implementation
pub fn write(
    connector: &LocalConnector,
    request: CapabilityExecutionRequest,
) -> anyhow::Result<serde_json::Value> {
    let input: FsWriteInput = serde_json::from_value(request.input)?;
    let actor = request.actor.clone();
    debug!("Writing file: {}", input.path);

    // Validate path is within workspace
    let path = connector.validate_path_for_actor(&input.path, &actor)?;

    // Create parent directories if requested
    if input.create_dirs {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
    }

    // Write content to file
    let mut file = fs::File::create(&path)?;
    let bytes_written = file.write(input.content.as_bytes())?;
    file.flush()?;

    info!("Wrote {} bytes to {}", bytes_written, input.path);

    let output = FsWriteOutput {
        path: input.path,
        bytes_written: bytes_written as u64,
    };

    Ok(serde_json::to_value(output)?)
}

/// Edit file capability implementation
pub fn edit(
    connector: &LocalConnector,
    request: CapabilityExecutionRequest,
) -> anyhow::Result<serde_json::Value> {
    let input: FsEditInput = serde_json::from_value(request.input)?;
    let actor = request.actor.clone();
    debug!("Editing file: {}", input.path);

    // Validate path is within workspace
    let path = connector.validate_path_for_actor(&input.path, &actor)?;

    // Read existing content
    let content = fs::read_to_string(&path)?;

    // Perform replacements
    let (new_content, replacements) = if input.replace_all {
        let matches = content.matches(&input.old_string).count();
        let new = content.replace(&input.old_string, &input.new_string);
        (new, matches as u64)
    } else {
        if let Some(pos) = content.find(&input.old_string) {
            let mut new = String::with_capacity(content.len());
            new.push_str(&content[..pos]);
            new.push_str(&input.new_string);
            new.push_str(&content[pos + input.old_string.len()..]);
            (new, 1)
        } else {
            (content, 0)
        }
    };

    // Write back if changes were made
    if replacements > 0 {
        fs::write(&path, new_content)?;
        info!("Made {} replacements in {}", replacements, input.path);
    } else {
        info!("No replacements made in {}", input.path);
    }

    let output = FsEditOutput {
        path: input.path,
        replacements,
    };

    Ok(serde_json::to_value(output)?)
}

/// Multi-edit capability: apply a batch of replacements atomically.
///
/// Each operation requires the `old_string` to be present in the in-memory
/// content at the time it is applied. If any `old_string` is absent the
/// function returns an error and no bytes are written to disk.
// Public API pre-integration: wired by host binary during capability
// composition. See docs/architecture/idioms/EVOLUTION_IDIOMS.md §4.
#[allow(dead_code)]
pub fn multi_edit(
    connector: &LocalConnector,
    request: CapabilityExecutionRequest,
) -> anyhow::Result<serde_json::Value> {
    let input: FsMultiEditInput = serde_json::from_value(request.input)?;
    let actor = request.actor.clone();
    debug!("Multi-editing file: {}", input.path);

    let path = connector.validate_path_for_actor(&input.path, &actor)?;
    let mut content = fs::read_to_string(&path)?;

    let mut per_op: Vec<u64> = Vec::with_capacity(input.edits.len());

    for (idx, op) in input.edits.iter().enumerate() {
        if !content.contains(op.old_string.as_str()) {
            return Err(anyhow::anyhow!(
                "multi_edit op[{}]: old_string not found in {}",
                idx,
                input.path
            ));
        }
        let pos = content.find(op.old_string.as_str()).expect("checked above");
        let mut new_content = String::with_capacity(content.len());
        new_content.push_str(&content[..pos]);
        new_content.push_str(&op.new_string);
        new_content.push_str(&content[pos + op.old_string.len()..]);
        content = new_content;
        per_op.push(1);
    }

    let total_replacements: u64 = per_op.iter().sum();
    fs::write(&path, &content)?;
    info!(
        "multi_edit: {} replacements in {}",
        total_replacements, input.path
    );

    Ok(serde_json::to_value(FsMultiEditOutput {
        path: input.path,
        total_replacements,
        per_op_replacements: per_op,
    })?)
}

/// `fs.patch_apply` — apply a unified-diff patch to a file (Hermes BT-05).
///
/// The patch must be in unified-diff format (`@@ -<start>,<count> +<start>,<count> @@`
/// hunk headers with ` `, `+`, `-` line prefixes). `---` / `+++` headers
/// above the first hunk are tolerated and ignored — the file path comes
/// from `path`, not from the diff.
///
/// All hunks are applied atomically: if any hunk fails to match the
/// surrounding context, the file is left untouched and the call errors.
/// This avoids partial-patch states that are hard to recover from.
#[allow(dead_code)]
pub fn patch_apply(
    connector: &LocalConnector,
    request: CapabilityExecutionRequest,
) -> anyhow::Result<serde_json::Value> {
    let input: FsPatchApplyInput = serde_json::from_value(request.input)?;
    let actor = request.actor.clone();
    debug!("Applying unified-diff patch to: {}", input.path);

    let path = connector.validate_path_for_actor(&input.path, &actor)?;
    let original = fs::read_to_string(&path)?;
    let original_lines: Vec<&str> = original.lines().collect();

    let hunks = parse_unified_diff(&input.patch)?;
    if hunks.is_empty() {
        return Err(anyhow::anyhow!("patch contains no hunks"));
    }

    let mut new_lines: Vec<String> = original_lines.iter().map(|s| s.to_string()).collect();
    let mut lines_added = 0usize;
    let mut lines_removed = 0usize;
    // Track cumulative offset from prior hunks so later hunks land at the
    // right line after additions/deletions shifted the buffer.
    let mut offset: i64 = 0;

    for (i, hunk) in hunks.iter().enumerate() {
        let target_idx = (hunk.old_start as i64 - 1 + offset).max(0) as usize;
        // Verify context: the lines we expect to remove + context must match.
        let mut cursor = target_idx;
        for op in &hunk.ops {
            match op {
                HunkOp::Context(line) | HunkOp::Remove(line) => {
                    let actual = new_lines.get(cursor).map(|s| s.as_str()).unwrap_or("");
                    if actual != line {
                        return Err(anyhow::anyhow!(
                            "hunk[{}]: context mismatch at line {} — expected '{}', found '{}'",
                            i,
                            cursor + 1,
                            line,
                            actual
                        ));
                    }
                    cursor += 1;
                }
                HunkOp::Add(_) => {} // additions don't consume original lines
            }
        }
        // Apply: rebuild the slice [target_idx .. cursor] from the hunk ops.
        let mut replacement: Vec<String> = Vec::new();
        let mut local_added = 0usize;
        let mut local_removed = 0usize;
        for op in &hunk.ops {
            match op {
                HunkOp::Context(line) => replacement.push(line.clone()),
                HunkOp::Add(line) => {
                    replacement.push(line.clone());
                    local_added += 1;
                }
                HunkOp::Remove(_) => local_removed += 1,
            }
        }
        new_lines.splice(target_idx..cursor, replacement.iter().cloned());
        offset += local_added as i64 - local_removed as i64;
        lines_added += local_added;
        lines_removed += local_removed;
    }

    // Preserve trailing newline iff the original had one.
    let mut new_content = new_lines.join("\n");
    if original.ends_with('\n') {
        new_content.push('\n');
    }
    fs::write(&path, &new_content)?;

    info!(
        "patch_apply: {} hunks, +{} -{} in {}",
        hunks.len(),
        lines_added,
        lines_removed,
        input.path
    );

    Ok(serde_json::to_value(FsPatchApplyOutput {
        path: input.path,
        hunks_applied: hunks.len(),
        lines_added,
        lines_removed,
    })?)
}

#[derive(Debug)]
enum HunkOp {
    Context(String),
    Add(String),
    Remove(String),
}

#[derive(Debug)]
struct Hunk {
    old_start: u64,
    ops: Vec<HunkOp>,
}

fn parse_unified_diff(diff: &str) -> anyhow::Result<Vec<Hunk>> {
    let mut hunks = Vec::new();
    let mut current: Option<Hunk> = None;
    for raw in diff.lines() {
        if raw.starts_with("--- ") || raw.starts_with("+++ ") {
            // File-header lines are ignored — we use the explicit `path`
            // input instead. This makes the same patch reusable across
            // files via the connector API.
            continue;
        }
        if let Some(rest) = raw.strip_prefix("@@") {
            if let Some(h) = current.take() {
                hunks.push(h);
            }
            // Format: " -<start>[,<count>] +<start>[,<count>] @@ <optional context>"
            let after = rest.trim_start();
            let mut parts = after.split_whitespace();
            let old_part = parts
                .next()
                .ok_or_else(|| anyhow::anyhow!("malformed hunk header: '{}'", raw))?;
            let old_start_str = old_part
                .strip_prefix('-')
                .and_then(|s| s.split(',').next())
                .ok_or_else(|| anyhow::anyhow!("malformed hunk header: '{}'", raw))?;
            let old_start: u64 = old_start_str
                .parse()
                .map_err(|_| anyhow::anyhow!("malformed hunk header: '{}'", raw))?;
            current = Some(Hunk {
                old_start: old_start.max(1),
                ops: Vec::new(),
            });
            continue;
        }
        let h = match current.as_mut() {
            Some(h) => h,
            None => continue, // Tolerate noise before first hunk.
        };
        if let Some(line) = raw.strip_prefix(' ') {
            h.ops.push(HunkOp::Context(line.to_string()));
        } else if let Some(line) = raw.strip_prefix('+') {
            h.ops.push(HunkOp::Add(line.to_string()));
        } else if let Some(line) = raw.strip_prefix('-') {
            h.ops.push(HunkOp::Remove(line.to_string()));
        } else if raw.is_empty() {
            // Blank lines in patches conventionally mean a single empty
            // context line.
            h.ops.push(HunkOp::Context(String::new()));
        }
        // Other lines (e.g. "\ No newline at end of file") are ignored.
    }
    if let Some(h) = current {
        hunks.push(h);
    }
    Ok(hunks)
}

/// Append capability: append text to the end of an existing file.
///
/// D1 follow-up (2026-05-12): wired into [`LocalConnector::execute`] under
/// `fs.append`; no longer dead.
pub fn append(
    connector: &LocalConnector,
    request: CapabilityExecutionRequest,
) -> anyhow::Result<serde_json::Value> {
    let input: FsAppendInput = serde_json::from_value(request.input)?;
    let actor = request.actor.clone();
    debug!("Appending to file: {}", input.path);

    let path = connector.validate_path_for_actor(&input.path, &actor)?;

    let mut file = fs::OpenOptions::new().append(true).open(&path)?;
    let bytes_appended = file.write(input.content.as_bytes())?;
    file.flush()?;

    info!("Appended {} bytes to {}", bytes_appended, input.path);

    Ok(serde_json::to_value(FsAppendOutput {
        path: input.path,
        bytes_appended: bytes_appended as u64,
    })?)
}

/// Delete capability: remove a file or directory.
///
/// D1 follow-up (2026-05-12): wired into [`LocalConnector::execute`] under
/// `fs.delete`; no longer dead. Governance protects this via the
/// `DangerousCapabilityFilter` rule for `connector:local:fs.delete`.
pub fn delete(
    connector: &LocalConnector,
    request: CapabilityExecutionRequest,
) -> anyhow::Result<serde_json::Value> {
    let input: FsDeleteInput = serde_json::from_value(request.input)?;
    let actor = request.actor.clone();
    debug!("Deleting: {}", input.path);

    let path = connector.validate_path_for_actor(&input.path, &actor)?;

    if !path.exists() {
        return Ok(serde_json::to_value(FsDeleteOutput {
            path: input.path,
            deleted: false,
        })?);
    }

    if path.is_dir() {
        if input.recursive {
            fs::remove_dir_all(&path)?;
        } else {
            return Err(anyhow::anyhow!(
                "Path is a directory; set recursive=true to delete: {}",
                input.path
            ));
        }
    } else {
        fs::remove_file(&path)?;
    }

    info!("Deleted {}", input.path);

    Ok(serde_json::to_value(FsDeleteOutput {
        path: input.path,
        deleted: true,
    })?)
}

/// List-dir capability: enumerate immediate children of a directory.
///
/// D1 follow-up (2026-05-12): wired into [`LocalConnector::execute`] under
/// `fs.list_dir`; no longer dead.
pub fn list_dir(
    connector: &LocalConnector,
    request: CapabilityExecutionRequest,
) -> anyhow::Result<serde_json::Value> {
    let input: FsListDirInput = serde_json::from_value(request.input)?;
    let actor = request.actor.clone();
    debug!("Listing directory: {}", input.path);

    let path = connector.validate_path_for_actor(&input.path, &actor)?;

    if !path.is_dir() {
        return Err(anyhow::anyhow!("Not a directory: {}", input.path));
    }

    let mut entries: Vec<FsDirEntry> = Vec::new();

    for entry in fs::read_dir(&path)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();

        if !input.include_hidden && name.starts_with('.') {
            continue;
        }

        let meta = entry.metadata()?;
        entries.push(FsDirEntry {
            path: entry.path().to_string_lossy().to_string(),
            name,
            is_dir: meta.is_dir(),
            size_bytes: if meta.is_file() { meta.len() } else { 0 },
        });
    }

    entries.sort_by(|a, b| a.name.cmp(&b.name));

    let count = entries.len() as u32;
    info!("Listed {} entries in {}", count, input.path);

    Ok(serde_json::to_value(FsListDirOutput {
        path: input.path,
        entries,
        count,
    })?)
}

/// Stat capability: return metadata for a path.
///
/// D1 follow-up (2026-05-12): wired into [`LocalConnector::execute`] under
/// `fs.stat`; no longer dead.
pub fn stat(
    connector: &LocalConnector,
    request: CapabilityExecutionRequest,
) -> anyhow::Result<serde_json::Value> {
    let input: FsStatInput = serde_json::from_value(request.input)?;
    let actor = request.actor.clone();
    debug!("Stat: {}", input.path);

    let path = connector.validate_path_for_actor(&input.path, &actor)?;

    if !path.exists() {
        return Ok(serde_json::to_value(FsStatOutput {
            path: input.path,
            exists: false,
            is_dir: false,
            is_file: false,
            size_bytes: 0,
            modified_secs: None,
            permissions_octal: None,
        })?);
    }

    let meta = fs::metadata(&path)?;
    let size_bytes = if meta.is_file() { meta.len() } else { 0 };

    let modified_secs = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64);

    #[cfg(unix)]
    let permissions_octal = {
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode() & 0o777;
        Some(format!("{:03o}", mode))
    };

    #[cfg(not(unix))]
    let permissions_octal: Option<String> = None;

    info!(
        "Stat {} -- exists=true is_dir={}",
        input.path,
        meta.is_dir()
    );

    Ok(serde_json::to_value(FsStatOutput {
        path: input.path,
        exists: true,
        is_dir: meta.is_dir(),
        is_file: meta.is_file(),
        size_bytes,
        modified_secs,
        permissions_octal,
    })?)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use cyberclaw_core::identity::Identity;
    use cyberclaw_core::workspace::{WorkspaceMode, WorkspaceRef};
    use tempfile::TempDir;

    fn make_connector(workspace: &TempDir) -> LocalConnector {
        LocalConnector::new(workspace.path().to_path_buf())
    }

    fn make_request(
        connector: &LocalConnector,
        capability: &str,
        input: serde_json::Value,
    ) -> CapabilityExecutionRequest {
        let workspace_root = connector.workspace.to_string_lossy().to_string();
        CapabilityExecutionRequest {
            execution_id: cyberclaw_core::prelude::ExecutionId::new(),
            trace_id: "test-trace".to_string(),
            actor: Identity::System.to_actor_ref(None).unwrap(),
            workspace: WorkspaceRef {
                id: cyberclaw_core::prelude::WorkspaceId::from_string("test-workspace".to_string())
                    .unwrap(),
                mode: WorkspaceMode::Ephemeral,
                materialization_mode: None,
                home_node_id: None,
                backing_store: None,
                root: workspace_root,
                writable_roots: vec![],
            },
            connector_id: cyberclaw_core::prelude::ConnectorId::from_string("local".to_string())
                .unwrap(),
            capability_id: cyberclaw_core::prelude::CapabilityId::from_string(
                capability.to_string(),
            )
            .unwrap(),
            input,
        }
    }

    fn write_file(dir: &TempDir, name: &str, content: &str) {
        fs::write(dir.path().join(name), content).unwrap();
    }

    // --- capability_facades() -- §4 idiom compliance ---

    #[test]
    fn facades_covers_all_eight_capabilities() {
        let facades = capability_facades();
        let names: Vec<String> = facades.iter().map(|(f, _)| f.name.clone()).collect();
        for cap in &[
            "file_read",
            "file_write",
            "file_edit",
            "file_multiedit",
            "file_append",
            "file_delete",
            "file_list",
            "file_stat",
        ] {
            assert!(names.iter().any(|n| n == cap), "missing facade: {cap}");
        }
        assert_eq!(facades.len(), 8);
    }

    #[test]
    fn facades_connector_id_is_local() {
        for (spec, _) in capability_facades() {
            assert_eq!(
                spec.connector_id.as_str(),
                "local",
                "connector_id wrong for {}",
                spec.name
            );
        }
    }

    #[test]
    fn facades_delete_is_high_risk() {
        let (spec, _) = capability_facades()
            .into_iter()
            .find(|(s, _)| s.name == "file_delete")
            .expect("file_delete facade missing");
        assert_eq!(spec.risk_level, RiskLevel::High);
    }

    #[test]
    fn facades_category_hint_is_filesystem() {
        for (spec, cat) in capability_facades() {
            assert_eq!(
                cat,
                ToolsetCategory::FileSystem,
                "wrong category for {}",
                spec.name
            );
        }
    }

    #[test]
    fn facades_input_schemas_are_valid_objects() {
        for (spec, _) in capability_facades() {
            let schema = spec
                .input_schema
                .as_ref()
                .expect("input_schema must be Some");
            assert!(
                schema.is_object(),
                "input_schema not an object for {}",
                spec.name
            );
            assert!(
                schema["properties"].is_object(),
                "missing properties for {}",
                spec.name
            );
        }
    }

    // --- multi_edit ---

    #[test]
    fn multi_edit_applies_operations_left_to_right() {
        let tmp = TempDir::new().unwrap();
        write_file(&tmp, "me.txt", "AAA BBB CCC");
        let conn = make_connector(&tmp);
        let path = tmp.path().join("me.txt");
        let req = make_request(
            &conn,
            "fs.multi_edit",
            serde_json::json!({
                "path": path.to_str().unwrap(),
                "edits": [
                    { "old_string": "AAA", "new_string": "111" },
                    { "old_string": "BBB", "new_string": "222" }
                ]
            }),
        );
        let result = multi_edit(&conn, req).unwrap();
        let out: FsMultiEditOutput = serde_json::from_value(result).unwrap();
        assert_eq!(out.total_replacements, 2);
        assert_eq!(out.per_op_replacements, vec![1, 1]);
        assert_eq!(fs::read_to_string(&path).unwrap(), "111 222 CCC");
    }

    #[test]
    fn multi_edit_errors_when_old_string_absent() {
        let tmp = TempDir::new().unwrap();
        write_file(&tmp, "me2.txt", "hello world");
        let conn = make_connector(&tmp);
        let path = tmp.path().join("me2.txt");
        let req = make_request(
            &conn,
            "fs.multi_edit",
            serde_json::json!({
                "path": path.to_str().unwrap(),
                "edits": [{ "old_string": "NOTHERE", "new_string": "x" }]
            }),
        );
        assert!(multi_edit(&conn, req).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "hello world");
    }

    // --- append ---

    #[test]
    fn append_adds_content_to_existing_file() {
        let tmp = TempDir::new().unwrap();
        write_file(&tmp, "app.txt", "line1\n");
        let conn = make_connector(&tmp);
        let path = tmp.path().join("app.txt");
        let req = make_request(
            &conn,
            "fs.append",
            serde_json::json!({ "path": path.to_str().unwrap(), "content": "line2\n" }),
        );
        let result = append(&conn, req).unwrap();
        let out: FsAppendOutput = serde_json::from_value(result).unwrap();
        assert_eq!(out.bytes_appended, 6);
        assert_eq!(fs::read_to_string(&path).unwrap(), "line1\nline2\n");
    }

    #[test]
    fn append_errors_on_nonexistent_file() {
        let tmp = TempDir::new().unwrap();
        let conn = make_connector(&tmp);
        let missing = tmp.path().join("ghost.txt");
        let req = make_request(
            &conn,
            "fs.append",
            serde_json::json!({ "path": missing.to_str().unwrap(), "content": "x" }),
        );
        assert!(append(&conn, req).is_err());
    }

    // --- delete ---

    #[test]
    fn delete_removes_existing_file() {
        let tmp = TempDir::new().unwrap();
        write_file(&tmp, "del.txt", "bye");
        let conn = make_connector(&tmp);
        let path = tmp.path().join("del.txt");
        let req = make_request(
            &conn,
            "fs.delete",
            serde_json::json!({ "path": path.to_str().unwrap() }),
        );
        let result = delete(&conn, req).unwrap();
        let out: FsDeleteOutput = serde_json::from_value(result).unwrap();
        assert!(out.deleted);
        assert!(!path.exists());
    }

    #[test]
    fn delete_returns_not_deleted_for_missing_path() {
        let tmp = TempDir::new().unwrap();
        let conn = make_connector(&tmp);
        let missing = tmp.path().join("nope.txt");
        let req = make_request(
            &conn,
            "fs.delete",
            serde_json::json!({ "path": missing.to_str().unwrap() }),
        );
        let result = delete(&conn, req).unwrap();
        let out: FsDeleteOutput = serde_json::from_value(result).unwrap();
        assert!(!out.deleted);
    }

    #[test]
    fn delete_dir_requires_recursive_flag() {
        let tmp = TempDir::new().unwrap();
        let sub = tmp.path().join("subdir");
        fs::create_dir(&sub).unwrap();
        let conn = make_connector(&tmp);
        let req = make_request(
            &conn,
            "fs.delete",
            serde_json::json!({ "path": sub.to_str().unwrap() }),
        );
        assert!(delete(&conn, req).is_err());
    }

    #[test]
    fn delete_dir_recursive_works() {
        let tmp = TempDir::new().unwrap();
        let sub = tmp.path().join("rmdir");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("f.txt"), "data").unwrap();
        let conn = make_connector(&tmp);
        let req = make_request(
            &conn,
            "fs.delete",
            serde_json::json!({ "path": sub.to_str().unwrap(), "recursive": true }),
        );
        let result = delete(&conn, req).unwrap();
        let out: FsDeleteOutput = serde_json::from_value(result).unwrap();
        assert!(out.deleted);
        assert!(!sub.exists());
    }

    // --- list_dir ---

    #[test]
    fn list_dir_returns_entries_sorted() {
        let tmp = TempDir::new().unwrap();
        write_file(&tmp, "b.txt", "b");
        write_file(&tmp, "a.txt", "a");
        let conn = make_connector(&tmp);
        let req = make_request(
            &conn,
            "fs.list_dir",
            serde_json::json!({ "path": tmp.path().to_str().unwrap() }),
        );
        let result = list_dir(&conn, req).unwrap();
        let out: FsListDirOutput = serde_json::from_value(result).unwrap();
        assert_eq!(out.count, 2);
        assert_eq!(out.entries[0].name, "a.txt");
        assert_eq!(out.entries[1].name, "b.txt");
        assert!(!out.entries[0].is_dir);
    }

    #[test]
    fn list_dir_excludes_hidden_by_default() {
        let tmp = TempDir::new().unwrap();
        write_file(&tmp, "visible.txt", "v");
        write_file(&tmp, ".hidden", "h");
        let conn = make_connector(&tmp);
        let req = make_request(
            &conn,
            "fs.list_dir",
            serde_json::json!({ "path": tmp.path().to_str().unwrap() }),
        );
        let result = list_dir(&conn, req).unwrap();
        let out: FsListDirOutput = serde_json::from_value(result).unwrap();
        assert_eq!(out.count, 1);
        assert_eq!(out.entries[0].name, "visible.txt");
    }

    #[test]
    fn list_dir_includes_hidden_when_flag_set() {
        let tmp = TempDir::new().unwrap();
        write_file(&tmp, "vis.txt", "v");
        write_file(&tmp, ".dot", "h");
        let conn = make_connector(&tmp);
        let req = make_request(
            &conn,
            "fs.list_dir",
            serde_json::json!({
                "path": tmp.path().to_str().unwrap(),
                "include_hidden": true
            }),
        );
        let result = list_dir(&conn, req).unwrap();
        let out: FsListDirOutput = serde_json::from_value(result).unwrap();
        assert_eq!(out.count, 2);
    }

    #[test]
    fn list_dir_errors_on_file_path() {
        let tmp = TempDir::new().unwrap();
        write_file(&tmp, "file.txt", "x");
        let conn = make_connector(&tmp);
        let path = tmp.path().join("file.txt");
        let req = make_request(
            &conn,
            "fs.list_dir",
            serde_json::json!({ "path": path.to_str().unwrap() }),
        );
        assert!(list_dir(&conn, req).is_err());
    }

    // --- stat ---

    #[test]
    fn stat_returns_metadata_for_existing_file() {
        let tmp = TempDir::new().unwrap();
        write_file(&tmp, "stat.txt", "hello");
        let conn = make_connector(&tmp);
        let path = tmp.path().join("stat.txt");
        let req = make_request(
            &conn,
            "fs.stat",
            serde_json::json!({ "path": path.to_str().unwrap() }),
        );
        let result = stat(&conn, req).unwrap();
        let out: FsStatOutput = serde_json::from_value(result).unwrap();
        assert!(out.exists);
        assert!(out.is_file);
        assert!(!out.is_dir);
        assert_eq!(out.size_bytes, 5);
        assert!(out.modified_secs.is_some());
    }

    #[test]
    fn stat_returns_exists_false_for_missing_path() {
        let tmp = TempDir::new().unwrap();
        let conn = make_connector(&tmp);
        let missing = tmp.path().join("ghost.txt");
        let req = make_request(
            &conn,
            "fs.stat",
            serde_json::json!({ "path": missing.to_str().unwrap() }),
        );
        let result = stat(&conn, req).unwrap();
        let out: FsStatOutput = serde_json::from_value(result).unwrap();
        assert!(!out.exists);
        assert_eq!(out.size_bytes, 0);
    }

    // --- R13-BUG-01 integration tests: write-content governance gate ---
    //
    // These tests call fs::write / fs::append / fs::edit through the
    // LocalConnector::execute() dispatcher to verify the governance gate is
    // wired in the actual runtime path (not just unit-testing the method).

    #[tokio::test]
    async fn test_fs_write_aws_key_rejected_by_runtime() {
        // "Write a file aws-config.txt with content: aws_access_key_id = AKIAIOSFODNN7EXAMPLE..."
        // must be blocked before File::create() is called. The file must NOT exist after.
        use crate::types::Connector;
        let tmp = TempDir::new().unwrap();
        let conn = make_connector(&tmp);
        let target = tmp.path().join("aws-config.txt");

        let request = make_request(
            &conn,
            "fs.write",
            serde_json::json!({
                "path": target.to_str().unwrap(),
                "content": "aws_access_key_id = AKIAIOSFODNN7EXAMPLE\naws_secret_access_key = wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY\n",
                "create_dirs": false
            }),
        );

        let result = conn.execute(request).await.unwrap();
        // Governance rejection must surface as Failed status.
        assert_eq!(
            result.status,
            crate::types::ExecutionStatus::Failed,
            "fs.write with AWS credentials must be rejected by governance"
        );
        let err = result.error.unwrap_or_default();
        assert!(
            err.contains("D010") || err.contains("governance") || err.contains("credential"),
            "error message must reference governance/credential denial: {err}"
        );
        // Critical: the file must NOT have been created on disk.
        assert!(
            !target.exists(),
            "aws-config.txt must NOT be written to disk when governance rejects"
        );
    }

    #[tokio::test]
    async fn test_fs_write_safe_content_passes() {
        // Regression: non-credential content must still write successfully.
        use crate::types::Connector;
        let tmp = TempDir::new().unwrap();
        let conn = make_connector(&tmp);
        let target = tmp.path().join("safe.txt");

        let request = make_request(
            &conn,
            "fs.write",
            serde_json::json!({
                "path": target.to_str().unwrap(),
                "content": "hello world\nThis is plain text with no secrets.\n",
                "create_dirs": false
            }),
        );

        let result = conn.execute(request).await.unwrap();
        assert_eq!(
            result.status,
            crate::types::ExecutionStatus::Success,
            "safe content must write successfully; got error: {:?}",
            result.error
        );
        assert!(target.exists(), "safe.txt must exist on disk after write");
        let on_disk = fs::read_to_string(&target).unwrap();
        assert!(on_disk.contains("hello world"));
    }

    // --- BT-05: fs.patch_apply (unified diff) ---

    #[test]
    fn patch_apply_single_hunk_addition() {
        let dir = TempDir::new().unwrap();
        write_file(&dir, "main.rs", "fn main() {\n}\n");
        let connector = make_connector(&dir);

        let patch = "@@ -1,2 +1,3 @@\n fn main() {\n+    println!(\"patched\");\n }\n";
        let req = make_request(
            &connector,
            "fs.patch_apply",
            serde_json::json!({"path": "main.rs", "patch": patch}),
        );
        let result = patch_apply(&connector, req).expect("patch_apply ok");
        let out: FsPatchApplyOutput = serde_json::from_value(result).unwrap();
        assert_eq!(out.hunks_applied, 1);
        assert_eq!(out.lines_added, 1);
        assert_eq!(out.lines_removed, 0);

        let content = fs::read_to_string(dir.path().join("main.rs")).unwrap();
        assert_eq!(content, "fn main() {\n    println!(\"patched\");\n}\n");
    }

    #[test]
    fn patch_apply_replacement() {
        let dir = TempDir::new().unwrap();
        write_file(&dir, "x.rs", "let port = 8080;\nlet host = \"a\";\n");
        let connector = make_connector(&dir);

        let patch = "@@ -1,2 +1,2 @@\n-let port = 8080;\n+let port = 9090;\n let host = \"a\";\n";
        let req = make_request(
            &connector,
            "fs.patch_apply",
            serde_json::json!({"path": "x.rs", "patch": patch}),
        );
        let result = patch_apply(&connector, req).expect("patch ok");
        let out: FsPatchApplyOutput = serde_json::from_value(result).unwrap();
        assert_eq!(out.lines_added, 1);
        assert_eq!(out.lines_removed, 1);

        let content = fs::read_to_string(dir.path().join("x.rs")).unwrap();
        assert_eq!(content, "let port = 9090;\nlet host = \"a\";\n");
    }

    #[test]
    fn patch_apply_multi_hunk() {
        let dir = TempDir::new().unwrap();
        write_file(
            &dir,
            "multi.rs",
            "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\n",
        );
        let connector = make_connector(&dir);

        // Two hunks: replace line2, then insert after line6.
        let patch = "\
@@ -1,3 +1,3 @@
 line1
-line2
+LINE2
 line3
@@ -5,3 +5,4 @@
 line5
 line6
+EXTRA
 line7
";
        let req = make_request(
            &connector,
            "fs.patch_apply",
            serde_json::json!({"path": "multi.rs", "patch": patch}),
        );
        let result = patch_apply(&connector, req).expect("patch ok");
        let out: FsPatchApplyOutput = serde_json::from_value(result).unwrap();
        assert_eq!(out.hunks_applied, 2);
        assert_eq!(out.lines_added, 2);
        assert_eq!(out.lines_removed, 1);

        let content = fs::read_to_string(dir.path().join("multi.rs")).unwrap();
        assert_eq!(
            content,
            "line1\nLINE2\nline3\nline4\nline5\nline6\nEXTRA\nline7\nline8\n"
        );
    }

    #[test]
    fn patch_apply_context_mismatch_errors_atomically() {
        let dir = TempDir::new().unwrap();
        write_file(&dir, "f.rs", "expected\nother\n");
        let connector = make_connector(&dir);

        let patch = "@@ -1,2 +1,2 @@\n DIFFERENT\n-other\n+OTHER\n";
        let req = make_request(
            &connector,
            "fs.patch_apply",
            serde_json::json!({"path": "f.rs", "patch": patch}),
        );
        let err = patch_apply(&connector, req).expect_err("expected mismatch error");
        assert!(err.to_string().contains("context mismatch"));

        // File is untouched after failure.
        let content = fs::read_to_string(dir.path().join("f.rs")).unwrap();
        assert_eq!(content, "expected\nother\n");
    }
}
