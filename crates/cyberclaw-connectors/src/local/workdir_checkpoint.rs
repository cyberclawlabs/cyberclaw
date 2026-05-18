//! # Workdir Checkpoint Connector
//!
//! Partial-port of Hermes-agent's `checkpoint_manager.py`. Provides shadow-git
//! workspace snapshot + rollback capabilities keyed by `sha256(abs_workdir)`.
//!
//! Scope (PARTIAL — see
//! `docs/implementation/reports/2026-04-18-checkpoint-manager-partial-port.md`):
//!
//! - `workdir.checkpoint` — take shadow-git snapshot of the workdir
//! - `workdir.list`       — list checkpoints (reverse chronological)
//! - `workdir.diff`       — diff historical hash against current or another hash
//! - `workdir.restore`    — rollback to a hash (with pre-rollback safety snapshot)
//!
//! ## Architecture
//!
//! ```text
//! ~/.cyberclaw/checkpoints/{sha256(abs_workdir)[:16]}/  — shadow bare-ish repo
//!   HEAD, refs/, objects/                               — standard git internals
//!   info/exclude                                        — DEFAULT_EXCLUDES
//! ```
//!
//! Git commands are invoked with `GIT_DIR=<shadow_repo>` +
//! `GIT_WORK_TREE=<user_workdir>` so no git state leaks into the user's
//! project directory.
//!
//! ## Guards (ported from Hermes `_validate_commit_hash`)
//!
//! - Commit hashes must be 7–40 hex chars (short or full SHA-1).
//! - Hashes must not start with `-` (prevents git flag injection).
//! - Hashes must exist in the shadow repo (verified via `git cat-file -e`).
//! - Workdir paths must be absolute and free of `..` components.
//!
//! ## NOT a hard security boundary
//!
//! This module relies on the host `git` binary. It offers blast-radius
//! reduction for user-workspace rollback, not sandboxed isolation of
//! adversarial code. Governance layers MUST treat `workdir.checkpoint` and
//! `workdir.restore` as [`RiskLevel::High`].

use crate::types::CapabilityExecutionRequest;
use cyberclaw_core::capability::RiskLevel;
use cyberclaw_core::facade::{CapabilityFacade, FacadeExposure, ToolsetCategory};
use cyberclaw_core::ids::{CapabilityId, ConnectorId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default git subprocess timeout.
const DEFAULT_GIT_TIMEOUT: Duration = Duration::from_secs(30);

/// Hard ceiling on file count in a workdir before we reject a checkpoint.
/// Mirrors `_MAX_FILES` in Hermes.
const DEFAULT_MAX_FILES: u64 = 50_000;

/// Patterns written to the shadow repo's `info/exclude` on init.
pub const DEFAULT_EXCLUDES: &[&str] = &[
    "target/",
    "node_modules/",
    ".git/",
    "*.tmp",
    "dist/",
    "build/",
    "__pycache__/",
];

/// Lower bound we refuse to drop below when killing a hung git.
const KILL_GRACE_MS: u64 = 50;

// ---------------------------------------------------------------------------
// §4 Facade export — real cyberclaw_core::facade types (Phase 2)
// ---------------------------------------------------------------------------

/// Return facade entries for every `workdir.*` capability in this module.
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
                name: "workdir.checkpoint".to_string(),
                description: "Take a shadow-git snapshot of a workdir. Returns the commit \
                              hash, timestamp, file count, and total size."
                    .to_string(),
                connector_id: connector_id.clone(),
                capability_id: CapabilityId::from_string("workdir.checkpoint".to_string()).unwrap(),
                risk_level: RiskLevel::High,
                effects: vec!["write".to_string()],
                read_only: false,
                destructive: true,
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "workdir": {
                            "type": "string",
                            "description": "Absolute path to the workdir to snapshot"
                        },
                        "reason": {
                            "type": "string",
                            "description": "Human-readable reason (used as commit message)"
                        }
                    },
                    "required": ["workdir"]
                })),
                exposure: FacadeExposure::LlmDefault,
            },
            ToolsetCategory::FileSystem,
        ),
        (
            CapabilityFacade {
                name: "workdir.list".to_string(),
                description: "List checkpoints for a workdir in reverse-chronological order."
                    .to_string(),
                connector_id: connector_id.clone(),
                capability_id: CapabilityId::from_string("workdir.list".to_string()).unwrap(),
                risk_level: RiskLevel::Low,
                effects: vec!["read".to_string()],
                read_only: true,
                destructive: false,
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "workdir": { "type": "string" },
                        "limit":   { "type": "integer" }
                    },
                    "required": ["workdir"]
                })),
                exposure: FacadeExposure::LlmDefault,
            },
            ToolsetCategory::FileSystem,
        ),
        (
            CapabilityFacade {
                name: "workdir.diff".to_string(),
                description: "Diff a historical checkpoint against the current workdir, or \
                              against another checkpoint if hash_b is provided."
                    .to_string(),
                connector_id: connector_id.clone(),
                capability_id: CapabilityId::from_string("workdir.diff".to_string()).unwrap(),
                risk_level: RiskLevel::Low,
                effects: vec!["read".to_string()],
                read_only: true,
                destructive: false,
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "workdir": { "type": "string" },
                        "hash_a":  { "type": "string" },
                        "hash_b":  { "type": "string" }
                    },
                    "required": ["workdir", "hash_a"]
                })),
                exposure: FacadeExposure::Internal,
            },
            ToolsetCategory::FileSystem,
        ),
        (
            CapabilityFacade {
                name: "workdir.restore".to_string(),
                description: "Rollback a workdir to a checkpoint. Takes a safety snapshot of \
                              current state first. Optional 'paths' restricts restore to \
                              specific paths within the workdir."
                    .to_string(),
                connector_id: connector_id.clone(),
                capability_id: CapabilityId::from_string("workdir.restore".to_string()).unwrap(),
                risk_level: RiskLevel::High,
                effects: vec!["write".to_string()],
                read_only: false,
                destructive: true,
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "workdir": { "type": "string" },
                        "hash":    { "type": "string" },
                        "paths":   { "type": "array", "items": { "type": "string" } }
                    },
                    "required": ["workdir", "hash"]
                })),
                exposure: FacadeExposure::Internal,
            },
            ToolsetCategory::FileSystem,
        ),
    ]
}

// ---------------------------------------------------------------------------
// Input / output schemas
// ---------------------------------------------------------------------------

/// Input for `workdir.checkpoint`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkdirCheckpointInput {
    /// Absolute path to the workdir to snapshot.
    pub workdir: String,
    /// Optional reason string used as the commit message.
    #[serde(default)]
    pub reason: Option<String>,
}

/// Output for `workdir.checkpoint`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkdirCheckpointOutput {
    /// Full 40-char SHA-1 commit hash.
    pub hash: String,
    /// ISO-8601 timestamp of the commit.
    pub timestamp: String,
    /// Number of files staged in the snapshot.
    pub file_count: u32,
    /// Total bytes staged in the snapshot.
    pub size_bytes: u64,
}

/// Input for `workdir.list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkdirListInput {
    pub workdir: String,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// One entry in the list-checkpoints output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointEntry {
    pub hash: String,
    /// ISO-8601 commit time.
    pub timestamp_iso: String,
    pub message: String,
    /// File count at the time the snapshot was taken. `None` if unknown
    /// (e.g. the shadow repo was created by an older version).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_count: Option<u32>,
    /// Snapshot size in bytes at the time the snapshot was taken.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
}

/// Output for `workdir.list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkdirListOutput {
    pub checkpoints: Vec<CheckpointEntry>,
}

/// Input for `workdir.diff`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkdirDiffInput {
    pub workdir: String,
    pub hash_a: String,
    #[serde(default)]
    pub hash_b: Option<String>,
}

/// Output for `workdir.diff`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkdirDiffOutput {
    /// Raw unified-diff text.
    pub diff: String,
}

/// Input for `workdir.restore`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkdirRestoreInput {
    pub workdir: String,
    pub hash: String,
    #[serde(default)]
    pub paths: Option<Vec<String>>,
}

/// Output for `workdir.restore`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkdirRestoreOutput {
    pub restored: bool,
    /// Hash of the pre-rollback safety snapshot that was taken first.
    pub safety_snapshot_hash: String,
}

// ---------------------------------------------------------------------------
// Config — test-tunable knobs
// ---------------------------------------------------------------------------

/// Runtime config for the checkpoint module. Tests use [`Config::for_testing`]
/// to lower the file cap and retarget the checkpoint root to a tempdir.
#[derive(Debug, Clone)]
pub struct Config {
    /// Root dir for shadow repos. Defaults to `~/.cyberclaw/checkpoints/`.
    pub checkpoint_root: PathBuf,
    /// Per-workdir file cap. Mirrors Hermes `_MAX_FILES`.
    pub max_files: u64,
    /// Git subprocess timeout.
    pub git_timeout: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            checkpoint_root: default_checkpoint_root(),
            max_files: DEFAULT_MAX_FILES,
            git_timeout: DEFAULT_GIT_TIMEOUT,
        }
    }
}

impl Config {
    /// Tight-capped config for unit tests: ephemeral root, low file cap.
    #[cfg(test)]
    fn for_testing(root: PathBuf, max_files: u64) -> Self {
        Self {
            checkpoint_root: root,
            max_files,
            git_timeout: DEFAULT_GIT_TIMEOUT,
        }
    }
}

fn default_checkpoint_root() -> PathBuf {
    let home = std::env::var("HOME")
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    home.join(".cyberclaw").join("checkpoints")
}

// ---------------------------------------------------------------------------
// Path / hash validation
// ---------------------------------------------------------------------------

/// Reject hash values that would be interpreted as git flags or that don't
/// match SHA-1 hex. Ported from Hermes `_validate_commit_hash`.
fn validate_commit_hash(hash: &str) -> anyhow::Result<()> {
    let trimmed = hash.trim();
    if trimmed.is_empty() {
        anyhow::bail!("commit hash is empty");
    }
    if trimmed.starts_with('-') {
        anyhow::bail!("commit hash must not start with '-': {trimmed:?}");
    }
    let len = trimmed.len();
    if !(7..=40).contains(&len) {
        anyhow::bail!("commit hash must be 7-40 hex chars, got {len}");
    }
    if !trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
        anyhow::bail!("commit hash must be hex: {trimmed:?}");
    }
    Ok(())
}

/// Reject workdirs that contain `..`, or that are not absolute. The platform
/// declares workdirs explicitly, so we refuse to guess.
fn validate_workdir(workdir: &str) -> anyhow::Result<PathBuf> {
    if workdir.is_empty() {
        anyhow::bail!("workdir is empty");
    }
    let p = Path::new(workdir);
    if !p.is_absolute() {
        anyhow::bail!("workdir must be an absolute path: {workdir}");
    }
    for component in p.components() {
        if component == std::path::Component::ParentDir {
            anyhow::bail!("workdir must not contain '..' components: {workdir}");
        }
    }
    // Canonicalize if possible (it may not exist yet in edge cases, but
    // Hermes itself demands an existing dir — we do too).
    let canonical = p
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("failed to canonicalize workdir {workdir:?}: {e}"))?;
    if !canonical.is_dir() {
        anyhow::bail!("workdir is not a directory: {}", canonical.display());
    }
    Ok(canonical)
}

/// Validate one relative restore path. Rejects absolute paths, `..`
/// components, and any path that escapes `workdir` after join. Mirrors
/// Hermes `_validate_file_path`.
fn validate_restore_path(rel: &str, workdir: &Path) -> anyhow::Result<()> {
    if rel.is_empty() {
        anyhow::bail!("restore path is empty");
    }
    let p = Path::new(rel);
    if p.is_absolute() {
        anyhow::bail!("restore path must be relative: {rel:?}");
    }
    for component in p.components() {
        if component == std::path::Component::ParentDir {
            anyhow::bail!("restore path must not contain '..': {rel:?}");
        }
    }
    // Join-and-check: the joined path's lexical components must stay under
    // workdir even before the filesystem exists.
    let joined: PathBuf = workdir.join(p);
    if !joined.starts_with(workdir) {
        anyhow::bail!("restore path escapes workdir: {rel:?}");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Shadow repo location
// ---------------------------------------------------------------------------

/// Compute the shadow-repo dir for a canonical workdir path.
fn shadow_repo_for(canonical_workdir: &Path, cfg: &Config) -> PathBuf {
    let abs = canonical_workdir.to_string_lossy();
    let mut hasher = Sha256::new();
    hasher.update(abs.as_bytes());
    let digest = hasher.finalize();
    let hex_str = hex::encode(digest);
    cfg.checkpoint_root.join(&hex_str[..16])
}

// ---------------------------------------------------------------------------
// git subprocess helpers
// ---------------------------------------------------------------------------

/// Result of one git invocation.
struct GitOutput {
    stdout: String,
    stderr: String,
    success: bool,
    code: Option<i32>,
}

/// Invoke git with `GIT_DIR` + `GIT_WORK_TREE` set, enforcing a hard timeout.
/// Optional `stdin` is piped in. Stdout/stderr are captured in full (git's
/// own output is bounded).
fn run_git(
    args: &[&str],
    shadow_repo: &Path,
    workdir: &Path,
    cfg: &Config,
) -> anyhow::Result<GitOutput> {
    run_git_with_stdin(args, shadow_repo, workdir, cfg, None)
}

fn run_git_with_stdin(
    args: &[&str],
    shadow_repo: &Path,
    workdir: &Path,
    cfg: &Config,
    stdin_data: Option<&str>,
) -> anyhow::Result<GitOutput> {
    if !workdir.is_dir() {
        anyhow::bail!("workdir not found: {}", workdir.display());
    }

    let mut cmd = Command::new("git");
    cmd.args(args)
        .env("GIT_DIR", shadow_repo)
        .env("GIT_WORK_TREE", workdir)
        // Keep git deterministic across user configs.
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_NAMESPACE")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
        .current_dir(workdir)
        .stdin(if stdin_data.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    debug!("running git {args:?} (GIT_DIR={})", shadow_repo.display());
    let started = Instant::now();
    let mut child = cmd
        .spawn()
        .map_err(|e| anyhow::anyhow!("failed to spawn git: {e}"))?;

    if let Some(data) = stdin_data {
        if let Some(mut pipe) = child.stdin.take() {
            if let Err(e) = pipe.write_all(data.as_bytes()) {
                warn!("failed to write git stdin: {e}");
            }
            drop(pipe);
        }
    }

    // Timeout watcher — mirrors sandbox/process_sandbox.rs pattern.
    let done = Arc::new(AtomicBool::new(false));
    let timed_out = Arc::new(Mutex::new(false));
    let timeout = cfg.git_timeout.max(Duration::from_millis(KILL_GRACE_MS));
    let pid = child.id();
    let watcher_done = Arc::clone(&done);
    let watcher_timed_out = Arc::clone(&timed_out);
    let watcher = thread::spawn(move || {
        let slice = Duration::from_millis(25);
        let mut remaining = timeout;
        loop {
            let step = remaining.min(slice);
            thread::sleep(step);
            if watcher_done.load(Ordering::Acquire) {
                return;
            }
            remaining = remaining.saturating_sub(step);
            if remaining.is_zero() {
                if let Ok(mut flag) = watcher_timed_out.lock() {
                    *flag = true;
                }
                kill_by_pid(pid);
                return;
            }
        }
    });

    let output = child
        .wait_with_output()
        .map_err(|e| anyhow::anyhow!("failed to wait on git: {e}"))?;
    done.store(true, Ordering::Release);
    let _ = watcher.join();

    let elapsed_ms = started.elapsed().as_millis();
    let was_timed_out = timed_out.lock().map(|g| *g).unwrap_or(false);
    if was_timed_out {
        anyhow::bail!(
            "git {args:?} timed out after {}ms",
            cfg.git_timeout.as_millis()
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let code = output.status.code();
    let success = output.status.success();
    debug!(
        "git {args:?} -> rc={:?} elapsed_ms={elapsed_ms} stdout_len={} stderr_len={}",
        code,
        stdout.len(),
        stderr.len()
    );
    Ok(GitOutput {
        stdout,
        stderr,
        success,
        code,
    })
}

#[cfg(unix)]
fn kill_by_pid(pid: u32) {
    use libc::{kill, pid_t, SIGKILL};
    // SAFETY: pid was returned by Command::id() for a live process. If the
    // process has already exited, kill() will fail with ESRCH which we
    // intentionally ignore.
    unsafe {
        let _ = kill(pid as pid_t, SIGKILL);
    }
}

#[cfg(not(unix))]
fn kill_by_pid(_pid: u32) {
    // Best effort on non-unix: the timeout watcher's `wait()` eventually
    // returns after git's own internal timeouts. No-op here.
}

// ---------------------------------------------------------------------------
// Shadow repo initialization
// ---------------------------------------------------------------------------

fn init_shadow_repo(shadow_repo: &Path, workdir: &Path, cfg: &Config) -> anyhow::Result<()> {
    if shadow_repo.join("HEAD").exists() {
        return Ok(());
    }
    std::fs::create_dir_all(shadow_repo)
        .map_err(|e| anyhow::anyhow!("failed to create shadow repo dir: {e}"))?;

    let out = run_git(
        &["init", "--quiet", "--initial-branch=main"],
        shadow_repo,
        workdir,
        cfg,
    )?;
    if !out.success {
        // Retry without --initial-branch for older git versions.
        let out2 = run_git(&["init", "--quiet"], shadow_repo, workdir, cfg)?;
        if !out2.success {
            anyhow::bail!("git init failed: rc={:?} stderr={}", out2.code, out2.stderr);
        }
    }

    let _ = run_git(
        &["config", "user.email", "checkpoint@cyberclaw.local"],
        shadow_repo,
        workdir,
        cfg,
    )?;
    let _ = run_git(
        &["config", "user.name", "CyberClaw Checkpoint"],
        shadow_repo,
        workdir,
        cfg,
    )?;
    // Silence "no commits" warnings on very first diff.
    let _ = run_git(
        &["config", "advice.detachedHead", "false"],
        shadow_repo,
        workdir,
        cfg,
    )?;

    let info_dir = shadow_repo.join("info");
    std::fs::create_dir_all(&info_dir)
        .map_err(|e| anyhow::anyhow!("failed to create info dir: {e}"))?;
    let exclude_body: String = DEFAULT_EXCLUDES.iter().map(|p| format!("{p}\n")).collect();
    std::fs::write(info_dir.join("exclude"), exclude_body)
        .map_err(|e| anyhow::anyhow!("failed to write info/exclude: {e}"))?;

    info!(
        "initialized shadow repo at {} for workdir {}",
        shadow_repo.display(),
        workdir.display()
    );
    Ok(())
}

/// Walk the workdir and count files, short-circuiting when the configured
/// cap is exceeded. Honors `DEFAULT_EXCLUDES` so the count matches what git
/// would stage.
fn count_files_with_cap(workdir: &Path, cfg: &Config) -> u64 {
    let mut count: u64 = 0;
    for entry in walkdir::WalkDir::new(workdir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !is_excluded_entry(e))
    {
        let Ok(entry) = entry else {
            continue;
        };
        if entry.file_type().is_file() {
            count += 1;
            if count > cfg.max_files {
                return count;
            }
        }
    }
    count
}

fn is_excluded_entry(entry: &walkdir::DirEntry) -> bool {
    let name = entry.file_name().to_string_lossy();
    matches!(
        name.as_ref(),
        "target" | "node_modules" | ".git" | "dist" | "build" | "__pycache__"
    )
}

// ---------------------------------------------------------------------------
// Capability implementations
// ---------------------------------------------------------------------------

/// Implementation of `workdir.checkpoint`.
pub fn checkpoint(request: CapabilityExecutionRequest) -> anyhow::Result<serde_json::Value> {
    checkpoint_with_config(request, &Config::default())
}

fn checkpoint_with_config(
    request: CapabilityExecutionRequest,
    cfg: &Config,
) -> anyhow::Result<serde_json::Value> {
    let input: WorkdirCheckpointInput = serde_json::from_value(request.input)?;
    let workdir = validate_workdir(&input.workdir)?;
    let reason = input
        .reason
        .clone()
        .unwrap_or_else(|| "manual checkpoint".to_string());

    let file_count_pre = count_files_with_cap(&workdir, cfg);
    if file_count_pre > cfg.max_files {
        anyhow::bail!(
            "workdir has more than {} files (cap); refusing to checkpoint",
            cfg.max_files
        );
    }

    let shadow = shadow_repo_for(&workdir, cfg);
    init_shadow_repo(&shadow, &workdir, cfg)?;

    // Stage everything.
    let add_out = run_git(&["add", "-A"], &shadow, &workdir, cfg)?;
    if !add_out.success {
        anyhow::bail!(
            "git add -A failed: rc={:?} stderr={}",
            add_out.code,
            add_out.stderr
        );
    }

    // Commit — allow empty so re-checkpointing a clean workdir still returns
    // a hash the caller can pin on.
    let commit_out = run_git(
        &[
            "commit",
            "--allow-empty",
            "--allow-empty-message",
            "-m",
            &reason,
        ],
        &shadow,
        &workdir,
        cfg,
    )?;
    if !commit_out.success {
        anyhow::bail!(
            "git commit failed: rc={:?} stderr={}",
            commit_out.code,
            commit_out.stderr
        );
    }

    // Resolve HEAD for the new hash + timestamp.
    let show_out = run_git(&["log", "-1", "--format=%H|%cI"], &shadow, &workdir, cfg)?;
    if !show_out.success {
        anyhow::bail!(
            "git log failed: rc={:?} stderr={}",
            show_out.code,
            show_out.stderr
        );
    }
    let line = show_out.stdout.trim();
    let mut parts = line.split('|');
    let hash = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("empty git log output"))?
        .to_string();
    let timestamp = parts.next().unwrap_or("").to_string();

    // Stats.
    let ls_out = run_git(&["ls-files"], &shadow, &workdir, cfg)?;
    let file_count = if ls_out.success {
        ls_out.stdout.lines().count() as u32
    } else {
        0
    };
    let size_bytes = compute_tracked_size(&ls_out.stdout, &workdir);

    let output = WorkdirCheckpointOutput {
        hash,
        timestamp,
        file_count,
        size_bytes,
    };
    info!(
        "workdir.checkpoint {} hash={} files={} size={}",
        workdir.display(),
        output.hash,
        output.file_count,
        output.size_bytes
    );
    Ok(serde_json::to_value(output)?)
}

fn compute_tracked_size(ls_files_stdout: &str, workdir: &Path) -> u64 {
    let mut total: u64 = 0;
    for rel in ls_files_stdout.lines() {
        if rel.is_empty() {
            continue;
        }
        let p = workdir.join(rel);
        if let Ok(meta) = std::fs::metadata(&p) {
            if meta.is_file() {
                total = total.saturating_add(meta.len());
            }
        }
    }
    total
}

/// Implementation of `workdir.list`.
pub fn list(request: CapabilityExecutionRequest) -> anyhow::Result<serde_json::Value> {
    list_with_config(request, &Config::default())
}

fn list_with_config(
    request: CapabilityExecutionRequest,
    cfg: &Config,
) -> anyhow::Result<serde_json::Value> {
    let input: WorkdirListInput = serde_json::from_value(request.input)?;
    let workdir = validate_workdir(&input.workdir)?;
    let limit = input.limit.unwrap_or(50);

    let shadow = shadow_repo_for(&workdir, cfg);
    if !shadow.join("HEAD").exists() {
        return Ok(serde_json::to_value(WorkdirListOutput {
            checkpoints: vec![],
        })?);
    }

    // `%H|%cI|%s` — stable separator.
    let limit_str = format!("--max-count={limit}");
    let out = run_git(
        &["log", "--format=%H|%cI|%s", &limit_str],
        &shadow,
        &workdir,
        cfg,
    )?;
    if !out.success {
        // No commits yet → git log returns non-zero. Treat as empty.
        return Ok(serde_json::to_value(WorkdirListOutput {
            checkpoints: vec![],
        })?);
    }

    let mut entries = Vec::new();
    for line in out.stdout.lines() {
        let mut parts = line.splitn(3, '|');
        let hash = parts.next().unwrap_or("").to_string();
        let ts = parts.next().unwrap_or("").to_string();
        let msg = parts.next().unwrap_or("").to_string();
        if hash.is_empty() {
            continue;
        }
        entries.push(CheckpointEntry {
            hash,
            timestamp_iso: ts,
            message: msg,
            file_count: None,
            size_bytes: None,
        });
    }
    Ok(serde_json::to_value(WorkdirListOutput {
        checkpoints: entries,
    })?)
}

/// Implementation of `workdir.diff`.
pub fn diff(request: CapabilityExecutionRequest) -> anyhow::Result<serde_json::Value> {
    diff_with_config(request, &Config::default())
}

fn diff_with_config(
    request: CapabilityExecutionRequest,
    cfg: &Config,
) -> anyhow::Result<serde_json::Value> {
    let input: WorkdirDiffInput = serde_json::from_value(request.input)?;
    let workdir = validate_workdir(&input.workdir)?;
    validate_commit_hash(&input.hash_a)?;
    if let Some(b) = input.hash_b.as_ref() {
        validate_commit_hash(b)?;
    }

    let shadow = shadow_repo_for(&workdir, cfg);
    if !shadow.join("HEAD").exists() {
        anyhow::bail!("no checkpoints exist for {}", workdir.display());
    }

    // Verify hash(es) exist.
    let check_a = run_git(&["cat-file", "-e", &input.hash_a], &shadow, &workdir, cfg)?;
    if !check_a.success {
        anyhow::bail!("checkpoint {} not found in shadow repo", input.hash_a);
    }
    if let Some(b) = input.hash_b.as_ref() {
        let check_b = run_git(&["cat-file", "-e", b], &shadow, &workdir, cfg)?;
        if !check_b.success {
            anyhow::bail!("checkpoint {b} not found in shadow repo");
        }
    }

    let diff_out = if let Some(b) = input.hash_b.as_ref() {
        run_git(
            &["diff", "--no-color", &input.hash_a, b],
            &shadow,
            &workdir,
            cfg,
        )?
    } else {
        // Stage current working tree first so diff reflects current state.
        let _ = run_git(&["add", "-A"], &shadow, &workdir, cfg)?;
        let out = run_git(
            &["diff", "--no-color", "--cached", &input.hash_a],
            &shadow,
            &workdir,
            cfg,
        )?;
        // Unstage to leave shadow index clean.
        let _ = run_git(&["reset", "--quiet", "HEAD"], &shadow, &workdir, cfg);
        out
    };

    // git diff exits non-zero if any differences are found *with --quiet*,
    // but we don't pass --quiet, so any non-zero here is a real error.
    if !diff_out.success && diff_out.code != Some(0) && !diff_out.stderr.is_empty() {
        // Some git builds exit with 1 when there's no common ancestor; treat
        // stderr content as the signal.
        if !diff_out.stderr.trim().is_empty() {
            error!(
                "git diff failed: rc={:?} stderr={}",
                diff_out.code, diff_out.stderr
            );
            anyhow::bail!("git diff failed: {}", diff_out.stderr);
        }
    }

    Ok(serde_json::to_value(WorkdirDiffOutput {
        diff: diff_out.stdout,
    })?)
}

/// Implementation of `workdir.restore`.
pub fn restore(request: CapabilityExecutionRequest) -> anyhow::Result<serde_json::Value> {
    restore_with_config(request, &Config::default())
}

fn restore_with_config(
    request: CapabilityExecutionRequest,
    cfg: &Config,
) -> anyhow::Result<serde_json::Value> {
    // Keep a template for forging the safety-snapshot request — this avoids
    // the partial-move of `request` when we destructure `request.input` below.
    let request_template = request.clone();
    let input: WorkdirRestoreInput = serde_json::from_value(request.input)?;
    let workdir = validate_workdir(&input.workdir)?;
    validate_commit_hash(&input.hash)?;

    if let Some(paths) = input.paths.as_ref() {
        for p in paths {
            validate_restore_path(p, &workdir)?;
        }
    }

    let shadow = shadow_repo_for(&workdir, cfg);
    if !shadow.join("HEAD").exists() {
        anyhow::bail!("no checkpoints exist for {}", workdir.display());
    }

    // Reject unknown commit.
    let check = run_git(&["cat-file", "-e", &input.hash], &shadow, &workdir, cfg)?;
    if !check.success {
        anyhow::bail!("checkpoint {} not found in shadow repo", input.hash);
    }

    // Safety snapshot FIRST.
    let safety_request = CapabilityExecutionRequest {
        input: serde_json::json!({
            "workdir": workdir.to_string_lossy(),
            "reason": format!("pre-restore (to {})", &input.hash[..input.hash.len().min(8)]),
        }),
        ..request_template
    };
    let safety_value = checkpoint_with_config(safety_request, cfg)?;
    let safety_out: WorkdirCheckpointOutput = serde_json::from_value(safety_value)
        .map_err(|e| anyhow::anyhow!("failed to decode safety snapshot output: {e}"))?;

    // Restore — full dir or specific paths.
    let mut args: Vec<String> = vec!["checkout".to_string(), input.hash.clone(), "--".to_string()];
    if let Some(paths) = input.paths.as_ref() {
        if paths.is_empty() {
            args.push(".".to_string());
        } else {
            args.extend(paths.iter().cloned());
        }
    } else {
        args.push(".".to_string());
    }
    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let co_out = run_git(&arg_refs, &shadow, &workdir, cfg)?;
    if !co_out.success {
        anyhow::bail!(
            "git checkout failed: rc={:?} stderr={}",
            co_out.code,
            co_out.stderr
        );
    }

    Ok(serde_json::to_value(WorkdirRestoreOutput {
        restored: true,
        safety_snapshot_hash: safety_out.hash,
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

    /// Detect whether `git` is available in $PATH. Tests are gated on this so
    /// the suite doesn't break on CI images without git.
    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn make_request(capability: &str, input: serde_json::Value) -> CapabilityExecutionRequest {
        CapabilityExecutionRequest {
            execution_id: ExecutionId::new(),
            trace_id: "test-trace-workdir-checkpoint".to_string(),
            actor: Identity::System.to_actor_ref(None).unwrap(),
            workspace: WorkspaceRef {
                id: WorkspaceId::from_string("test-workspace".to_string()).unwrap(),
                mode: WorkspaceMode::Ephemeral,
                materialization_mode: None,
                home_node_id: None,
                backing_store: None,
                root: "/tmp".to_string(),
                writable_roots: vec![],
            },
            connector_id: ConnectorId::from_string("local".to_string()).unwrap(),
            capability_id: CapabilityId::from_string(capability.to_string()).unwrap(),
            input,
        }
    }

    /// Fresh test workdir + fresh checkpoint root, plus a tight file cap.
    fn new_fixture(max_files: u64) -> (TempDir, TempDir, Config) {
        let workdir = TempDir::new().unwrap();
        let ckpt_root = TempDir::new().unwrap();
        let cfg = Config::for_testing(ckpt_root.path().to_path_buf(), max_files);
        (workdir, ckpt_root, cfg)
    }

    fn canonical_of(dir: &TempDir) -> String {
        dir.path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .to_string()
    }

    // -----------------------------------------------------------------
    // Facades — §4 idiom compliance
    // -----------------------------------------------------------------

    #[test]
    fn facades_cover_all_four_capabilities() {
        let facades = capability_facades();
        let names: Vec<&str> = facades.iter().map(|(f, _)| f.name.as_str()).collect();
        for cap in &[
            "workdir.checkpoint",
            "workdir.list",
            "workdir.diff",
            "workdir.restore",
        ] {
            assert!(names.contains(cap), "missing facade: {cap}");
        }
        assert_eq!(facades.len(), 4);
    }

    #[test]
    fn facades_connector_id_is_local() {
        for (f, _) in capability_facades() {
            assert_eq!(
                f.connector_id.as_str(),
                "local",
                "wrong connector_id for {}",
                f.name
            );
        }
    }

    #[test]
    fn facades_checkpoint_and_restore_are_high_risk() {
        let facades = capability_facades();
        let ck = facades
            .iter()
            .find(|(f, _)| f.name == "workdir.checkpoint")
            .unwrap();
        let rs = facades
            .iter()
            .find(|(f, _)| f.name == "workdir.restore")
            .unwrap();
        assert_eq!(ck.0.risk_level, RiskLevel::High);
        assert_eq!(rs.0.risk_level, RiskLevel::High);
    }

    // -----------------------------------------------------------------
    // Hash / path guards — do not require git
    // -----------------------------------------------------------------

    #[test]
    fn validate_commit_hash_accepts_short_and_full() {
        assert!(validate_commit_hash("abcdef1").is_ok());
        assert!(validate_commit_hash("abcdef1234567890abcdef1234567890abcdef12").is_ok());
    }

    #[test]
    fn validate_commit_hash_rejects_flag_injection() {
        assert!(validate_commit_hash("-p").is_err());
        assert!(validate_commit_hash("--patch").is_err());
    }

    #[test]
    fn validate_commit_hash_rejects_bad_length_and_chars() {
        assert!(validate_commit_hash("").is_err());
        assert!(validate_commit_hash("abc").is_err()); // too short
        assert!(validate_commit_hash(&"a".repeat(64)).is_err()); // too long (>40)
        assert!(validate_commit_hash("ghijklm").is_err()); // non-hex
    }

    #[test]
    fn path_traversal_blocked() {
        // Workdir with `..` components
        let bad = "/tmp/../etc";
        assert!(validate_workdir(bad).is_err());
        // Relative path
        assert!(validate_workdir("relative/path").is_err());
        // Empty path
        assert!(validate_workdir("").is_err());

        // Restore path guards
        let tmp = TempDir::new().unwrap();
        let wd = tmp.path();
        assert!(validate_restore_path("", wd).is_err());
        assert!(validate_restore_path("/abs/path", wd).is_err());
        assert!(validate_restore_path("../escape", wd).is_err());
        assert!(validate_restore_path("nested/../escape", wd).is_err());
        assert!(validate_restore_path("ok/nested.txt", wd).is_ok());
    }

    // -----------------------------------------------------------------
    // Integration tests — require `git` on PATH
    // -----------------------------------------------------------------

    #[test]
    fn checkpoint_creates_shadow_repo_on_first_call() {
        if !git_available() {
            eprintln!("skipping: git not available");
            return;
        }
        let (workdir, ckpt_root, cfg) = new_fixture(DEFAULT_MAX_FILES);
        std::fs::write(workdir.path().join("hello.txt"), "hi").unwrap();

        let req = make_request(
            "workdir.checkpoint",
            serde_json::json!({
                "workdir": canonical_of(&workdir),
                "reason": "first"
            }),
        );
        let out = checkpoint_with_config(req, &cfg).unwrap();
        let parsed: WorkdirCheckpointOutput = serde_json::from_value(out).unwrap();

        // Shadow repo exists.
        let canonical_wd = workdir.path().canonicalize().unwrap();
        let shadow = shadow_repo_for(&canonical_wd, &cfg);
        assert!(shadow.join("HEAD").exists(), "shadow repo HEAD missing");
        assert!(
            shadow.starts_with(ckpt_root.path()),
            "shadow repo outside test root"
        );
        assert!(!parsed.hash.is_empty());
        assert!(parsed.file_count >= 1);
    }

    #[test]
    fn checkpoint_returns_valid_sha1_hash() {
        if !git_available() {
            eprintln!("skipping: git not available");
            return;
        }
        let (workdir, _ckpt, cfg) = new_fixture(DEFAULT_MAX_FILES);
        std::fs::write(workdir.path().join("a.txt"), "aaa").unwrap();

        let req = make_request(
            "workdir.checkpoint",
            serde_json::json!({ "workdir": canonical_of(&workdir) }),
        );
        let out = checkpoint_with_config(req, &cfg).unwrap();
        let parsed: WorkdirCheckpointOutput = serde_json::from_value(out).unwrap();

        // SHA-1: 40 hex chars.
        assert_eq!(parsed.hash.len(), 40);
        assert!(parsed.hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn checkpoint_respects_excludes_list() {
        if !git_available() {
            eprintln!("skipping: git not available");
            return;
        }
        let (workdir, _ckpt, cfg) = new_fixture(DEFAULT_MAX_FILES);
        // One tracked file, one in an excluded dir.
        std::fs::write(workdir.path().join("main.rs"), "fn main() {}").unwrap();
        std::fs::create_dir_all(workdir.path().join("target")).unwrap();
        std::fs::write(workdir.path().join("target/debug.o"), "BINARY").unwrap();
        std::fs::create_dir_all(workdir.path().join("node_modules")).unwrap();
        std::fs::write(workdir.path().join("node_modules/lib.js"), "m").unwrap();

        let req = make_request(
            "workdir.checkpoint",
            serde_json::json!({ "workdir": canonical_of(&workdir) }),
        );
        let out = checkpoint_with_config(req, &cfg).unwrap();
        let parsed: WorkdirCheckpointOutput = serde_json::from_value(out).unwrap();
        // Only main.rs should be tracked.
        assert_eq!(
            parsed.file_count, 1,
            "excluded files leaked into checkpoint: file_count={}",
            parsed.file_count
        );
    }

    #[test]
    fn list_returns_checkpoints_in_reverse_chronological() {
        if !git_available() {
            eprintln!("skipping: git not available");
            return;
        }
        let (workdir, _ckpt, cfg) = new_fixture(DEFAULT_MAX_FILES);
        std::fs::write(workdir.path().join("f.txt"), "1").unwrap();
        let wd = canonical_of(&workdir);

        let r1 = make_request(
            "workdir.checkpoint",
            serde_json::json!({ "workdir": wd.clone(), "reason": "first" }),
        );
        let ck1: WorkdirCheckpointOutput =
            serde_json::from_value(checkpoint_with_config(r1, &cfg).unwrap()).unwrap();

        std::fs::write(workdir.path().join("f.txt"), "2").unwrap();
        let r2 = make_request(
            "workdir.checkpoint",
            serde_json::json!({ "workdir": wd.clone(), "reason": "second" }),
        );
        let ck2: WorkdirCheckpointOutput =
            serde_json::from_value(checkpoint_with_config(r2, &cfg).unwrap()).unwrap();

        let list_req = make_request("workdir.list", serde_json::json!({ "workdir": wd }));
        let out: WorkdirListOutput =
            serde_json::from_value(list_with_config(list_req, &cfg).unwrap()).unwrap();
        assert!(out.checkpoints.len() >= 2);
        // Most recent first.
        assert_eq!(out.checkpoints[0].hash, ck2.hash);
        assert_eq!(out.checkpoints[1].hash, ck1.hash);
        assert_eq!(out.checkpoints[0].message, "second");
        assert_eq!(out.checkpoints[1].message, "first");
    }

    #[test]
    fn list_respects_limit() {
        if !git_available() {
            eprintln!("skipping: git not available");
            return;
        }
        let (workdir, _ckpt, cfg) = new_fixture(DEFAULT_MAX_FILES);
        let wd = canonical_of(&workdir);
        for i in 0..3 {
            std::fs::write(workdir.path().join("f.txt"), format!("v{i}")).unwrap();
            let req = make_request(
                "workdir.checkpoint",
                serde_json::json!({ "workdir": wd.clone(), "reason": format!("c{i}") }),
            );
            let _ = checkpoint_with_config(req, &cfg).unwrap();
        }
        let list_req = make_request(
            "workdir.list",
            serde_json::json!({ "workdir": wd, "limit": 2 }),
        );
        let out: WorkdirListOutput =
            serde_json::from_value(list_with_config(list_req, &cfg).unwrap()).unwrap();
        assert_eq!(out.checkpoints.len(), 2);
    }

    #[test]
    fn diff_between_two_hashes() {
        if !git_available() {
            eprintln!("skipping: git not available");
            return;
        }
        let (workdir, _ckpt, cfg) = new_fixture(DEFAULT_MAX_FILES);
        let wd = canonical_of(&workdir);
        std::fs::write(workdir.path().join("x.txt"), "v1").unwrap();
        let r1 = make_request(
            "workdir.checkpoint",
            serde_json::json!({ "workdir": wd.clone() }),
        );
        let h1: WorkdirCheckpointOutput =
            serde_json::from_value(checkpoint_with_config(r1, &cfg).unwrap()).unwrap();

        std::fs::write(workdir.path().join("x.txt"), "v2").unwrap();
        let r2 = make_request(
            "workdir.checkpoint",
            serde_json::json!({ "workdir": wd.clone() }),
        );
        let h2: WorkdirCheckpointOutput =
            serde_json::from_value(checkpoint_with_config(r2, &cfg).unwrap()).unwrap();

        let diff_req = make_request(
            "workdir.diff",
            serde_json::json!({
                "workdir": wd,
                "hash_a": h1.hash,
                "hash_b": h2.hash,
            }),
        );
        let out: WorkdirDiffOutput =
            serde_json::from_value(diff_with_config(diff_req, &cfg).unwrap()).unwrap();
        assert!(
            out.diff.contains("-v1") && out.diff.contains("+v2"),
            "diff missing transition v1→v2: {}",
            out.diff
        );
    }

    #[test]
    fn diff_against_current_workdir() {
        if !git_available() {
            eprintln!("skipping: git not available");
            return;
        }
        let (workdir, _ckpt, cfg) = new_fixture(DEFAULT_MAX_FILES);
        let wd = canonical_of(&workdir);
        std::fs::write(workdir.path().join("y.txt"), "base").unwrap();
        let r = make_request(
            "workdir.checkpoint",
            serde_json::json!({ "workdir": wd.clone() }),
        );
        let ck: WorkdirCheckpointOutput =
            serde_json::from_value(checkpoint_with_config(r, &cfg).unwrap()).unwrap();

        // Modify without checkpointing; diff-against-current should show it.
        std::fs::write(workdir.path().join("y.txt"), "mutated").unwrap();

        let dreq = make_request(
            "workdir.diff",
            serde_json::json!({ "workdir": wd, "hash_a": ck.hash }),
        );
        let out: WorkdirDiffOutput =
            serde_json::from_value(diff_with_config(dreq, &cfg).unwrap()).unwrap();
        assert!(
            out.diff.contains("base") || out.diff.contains("mutated"),
            "expected diff to mention base/mutated, got: {}",
            out.diff
        );
    }

    #[test]
    fn restore_takes_safety_snapshot_first() {
        if !git_available() {
            eprintln!("skipping: git not available");
            return;
        }
        let (workdir, _ckpt, cfg) = new_fixture(DEFAULT_MAX_FILES);
        let wd = canonical_of(&workdir);
        std::fs::write(workdir.path().join("r.txt"), "orig").unwrap();
        let r1 = make_request(
            "workdir.checkpoint",
            serde_json::json!({ "workdir": wd.clone() }),
        );
        let ck1: WorkdirCheckpointOutput =
            serde_json::from_value(checkpoint_with_config(r1, &cfg).unwrap()).unwrap();

        std::fs::write(workdir.path().join("r.txt"), "local-unsaved").unwrap();

        let rreq = make_request(
            "workdir.restore",
            serde_json::json!({ "workdir": wd.clone(), "hash": ck1.hash }),
        );
        let out: WorkdirRestoreOutput =
            serde_json::from_value(restore_with_config(rreq, &cfg).unwrap()).unwrap();
        assert!(out.restored);
        assert_eq!(out.safety_snapshot_hash.len(), 40);

        // Content reverted.
        let after = std::fs::read_to_string(workdir.path().join("r.txt")).unwrap();
        assert_eq!(after, "orig");

        // Safety snapshot now listed in checkpoints.
        let list_req = make_request("workdir.list", serde_json::json!({ "workdir": wd }));
        let listed: WorkdirListOutput =
            serde_json::from_value(list_with_config(list_req, &cfg).unwrap()).unwrap();
        assert!(listed
            .checkpoints
            .iter()
            .any(|e| e.hash == out.safety_snapshot_hash));
    }

    #[test]
    fn restore_with_path_filter() {
        if !git_available() {
            eprintln!("skipping: git not available");
            return;
        }
        let (workdir, _ckpt, cfg) = new_fixture(DEFAULT_MAX_FILES);
        let wd = canonical_of(&workdir);
        std::fs::write(workdir.path().join("a.txt"), "A1").unwrap();
        std::fs::write(workdir.path().join("b.txt"), "B1").unwrap();
        let r1 = make_request(
            "workdir.checkpoint",
            serde_json::json!({ "workdir": wd.clone() }),
        );
        let ck: WorkdirCheckpointOutput =
            serde_json::from_value(checkpoint_with_config(r1, &cfg).unwrap()).unwrap();

        // Mutate both, then restore only a.txt.
        std::fs::write(workdir.path().join("a.txt"), "A2").unwrap();
        std::fs::write(workdir.path().join("b.txt"), "B2").unwrap();

        let rreq = make_request(
            "workdir.restore",
            serde_json::json!({
                "workdir": wd,
                "hash": ck.hash,
                "paths": ["a.txt"],
            }),
        );
        let out: WorkdirRestoreOutput =
            serde_json::from_value(restore_with_config(rreq, &cfg).unwrap()).unwrap();
        assert!(out.restored);

        assert_eq!(
            std::fs::read_to_string(workdir.path().join("a.txt")).unwrap(),
            "A1"
        );
        assert_eq!(
            std::fs::read_to_string(workdir.path().join("b.txt")).unwrap(),
            "B2",
            "b.txt should NOT have been touched by path-filtered restore"
        );
    }

    #[test]
    fn restore_rejects_invalid_hash() {
        if !git_available() {
            eprintln!("skipping: git not available");
            return;
        }
        let (workdir, _ckpt, cfg) = new_fixture(DEFAULT_MAX_FILES);
        let wd = canonical_of(&workdir);
        std::fs::write(workdir.path().join("f.txt"), "x").unwrap();
        // Take one valid checkpoint so the shadow repo exists.
        let r1 = make_request(
            "workdir.checkpoint",
            serde_json::json!({ "workdir": wd.clone() }),
        );
        let _ = checkpoint_with_config(r1, &cfg).unwrap();

        // 1) Syntactically invalid hash → rejected by guard.
        let bad = make_request(
            "workdir.restore",
            serde_json::json!({ "workdir": wd.clone(), "hash": "-p" }),
        );
        assert!(restore_with_config(bad, &cfg).is_err());

        // 2) Syntactically valid but non-existent → rejected by cat-file.
        let missing = make_request(
            "workdir.restore",
            serde_json::json!({
                "workdir": wd,
                "hash": "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
            }),
        );
        assert!(restore_with_config(missing, &cfg).is_err());
    }

    #[test]
    fn checkpoint_rejects_too_many_files() {
        if !git_available() {
            eprintln!("skipping: git not available");
            return;
        }
        // Use a tiny cap of 3. Synthesize 5 files.
        let (workdir, _ckpt, cfg) = new_fixture(3);
        for i in 0..5 {
            std::fs::write(workdir.path().join(format!("f{i}.txt")), "x").unwrap();
        }
        let req = make_request(
            "workdir.checkpoint",
            serde_json::json!({ "workdir": canonical_of(&workdir) }),
        );
        let result = checkpoint_with_config(req, &cfg);
        assert!(result.is_err(), "expected file-cap rejection");
        let err_str = format!("{}", result.unwrap_err());
        assert!(err_str.contains("cap"), "unexpected error text: {err_str}");
    }
}
