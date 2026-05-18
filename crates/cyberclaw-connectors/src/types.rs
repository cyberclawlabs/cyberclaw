use cyberclaw_core::capability_contract::CapabilityBehaviorContract;
use cyberclaw_core::prelude::*;
use serde::{Deserialize, Serialize};

/// Request to execute a capability through a connector
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityExecutionRequest {
    pub execution_id: ExecutionId,
    pub trace_id: String,
    pub actor: ActorRef,
    pub workspace: WorkspaceRef,
    pub connector_id: ConnectorId,
    pub capability_id: CapabilityId,
    pub input: serde_json::Value,
}

/// Result of executing a capability
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityExecutionResult {
    pub execution_id: ExecutionId,
    pub trace_id: String,
    pub connector_id: ConnectorId,
    pub capability_id: CapabilityId,
    pub output: serde_json::Value,
    pub status: ExecutionStatus,
    pub error: Option<String>,
    /// HIGH #6 FIX: Actual runtime mode used during execution (for post-execution verification)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_runtime: Option<crate::runtime::RuntimeMode>,
}

/// Status of capability execution
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionStatus {
    Success,
    Failed,
    Timeout,
    Cancelled,
}

/// Registry entry for a connector instance
#[derive(Debug, Clone)]
pub struct ConnectorEntry {
    pub id: ConnectorId,
    pub runtime: ConnectorRuntime,
    pub capabilities: Vec<CapabilityContract>,
    pub connector: std::sync::Arc<dyn Connector>,
    /// Optional per-capability behavior contracts for cross-validation
    /// (e.g. destructiveness vs risk level consistency checks).
    /// Key is the capability ID string.
    pub behavior_contracts: std::collections::HashMap<String, CapabilityBehaviorContract>,
}

/// Trait for all connectors
#[async_trait::async_trait]
pub trait Connector: Send + Sync + std::fmt::Debug + 'static {
    /// Get the connector ID
    fn id(&self) -> &ConnectorId;

    /// Get the connector runtime type
    fn runtime(&self) -> ConnectorRuntime;

    /// Get the capabilities exposed by this connector
    fn capabilities(&self) -> Vec<CapabilityContract>;

    /// Execute a capability
    async fn execute(
        &self,
        request: CapabilityExecutionRequest,
    ) -> anyhow::Result<CapabilityExecutionResult>;
}

/// Input/output schemas for LocalConnector capabilities
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsReadInput {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsReadOutput {
    pub content: String,
    pub path: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsWriteInput {
    pub path: String,
    pub content: String,
    #[serde(default)]
    pub create_dirs: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsWriteOutput {
    pub path: String,
    pub bytes_written: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsEditInput {
    pub path: String,
    pub old_string: String,
    pub new_string: String,
    #[serde(default)]
    pub replace_all: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsEditOutput {
    pub path: String,
    pub replacements: u64,
}

/// One edit operation within a `fs.multi_edit` batch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsEditOp {
    pub old_string: String,
    pub new_string: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsMultiEditInput {
    pub path: String,
    /// Ordered list of edit operations applied atomically left-to-right.
    pub edits: Vec<FsEditOp>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsMultiEditOutput {
    pub path: String,
    pub total_replacements: u64,
    pub per_op_replacements: Vec<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsAppendInput {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsAppendOutput {
    pub path: String,
    pub bytes_appended: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsDeleteInput {
    pub path: String,
    #[serde(default)]
    pub recursive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsDeleteOutput {
    pub path: String,
    pub deleted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsListDirInput {
    pub path: String,
    #[serde(default)]
    pub include_hidden: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsDirEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsListDirOutput {
    pub path: String,
    pub entries: Vec<FsDirEntry>,
    pub count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsStatInput {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsStatOutput {
    pub path: String,
    pub exists: bool,
    pub is_dir: bool,
    pub is_file: bool,
    pub size_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_secs: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permissions_octal: Option<String>,
}

/// Output mode for `search.grep`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GrepOutputMode {
    /// Return file paths that contain at least one match (default).
    #[default]
    FilesWithMatches,
    /// Return matching lines with optional context.
    Content,
    /// Return per-file match counts.
    Count,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchGrepInput {
    /// Regex or literal pattern to search for.
    pub pattern: String,
    /// File or directory path to search in (defaults to workspace root).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Glob filter applied to file names (e.g. `"*.rs"`, `"**/*.{ts,tsx}"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub glob: Option<String>,
    /// Restrict search to a known file type (e.g. `"rust"`, `"py"`, `"ts"`).
    /// Maps to `rg --type`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_type: Option<String>,
    /// Case-insensitive search (`rg -i`).
    #[serde(default)]
    pub case_insensitive: bool,
    /// Lines of context before each match (`rg -B`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_before: Option<u32>,
    /// Lines of context after each match (`rg -A`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_after: Option<u32>,
    /// Unified context lines before and after (`rg -C`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<u32>,
    /// Output mode (default: `files_with_matches`).
    #[serde(default)]
    pub output_mode: GrepOutputMode,
    /// Maximum number of results to return (default 250).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head_limit: Option<usize>,
    /// Enable multiline matching (`.` matches `\n`).
    #[serde(default)]
    pub multiline: bool,
    // Legacy field kept for backwards compat — maps to head_limit when set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_results: Option<u32>,
}

/// One content-mode match line (file + line number + text).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchMatch {
    pub file: String,
    pub line: u32,
    pub content: String,
}

/// One count-mode entry (file + match count).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchCountEntry {
    pub file: String,
    pub count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchGrepOutput {
    /// Populated when `output_mode` is `content`.
    pub matches: Vec<SearchMatch>,
    /// Populated when `output_mode` is `files_with_matches`.
    pub files: Vec<String>,
    /// Populated when `output_mode` is `count`.
    pub counts: Vec<SearchCountEntry>,
    /// Total number of results returned (entries in whichever field is populated).
    pub count: u32,
    /// `true` when results were truncated at `head_limit`.
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchGlobInput {
    /// Glob pattern (e.g. `"**/*.rs"`, `"src/**/*.ts"`).
    pub pattern: String,
    /// Directory to search in (defaults to workspace root).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Sort results by modification time descending (most-recently-modified first).
    #[serde(default)]
    pub sort_by_mtime: bool,
    // Legacy field kept for backwards compat.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_results: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchGlobOutput {
    pub files: Vec<String>,
    pub count: u32,
    /// `true` when results were truncated at the default 250-entry limit.
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CmdExecInput {
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CmdExecOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub timed_out: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebFetchInput {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebFetchOutput {
    pub url: String,
    pub status_code: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    pub body: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSearchInput {
    pub query: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_results: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_domains: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_domains: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSearchResult {
    pub title: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSearchOutput {
    pub query: String,
    pub provider: String,
    pub results: Vec<WebSearchResult>,
    pub total: u32,
}

// ---------------------------------------------------------------------------
// cmd.run — unrestricted host-level shell execution
// ---------------------------------------------------------------------------

/// Input for `cmd.run`: full host-privilege shell execution (no whitelist).
///
/// **Risk level: High** — the command runs as the host process user with no
/// capability restrictions, environment scrubbing, or filesystem isolation.
/// Contrast with `sandbox.execute_isolated`, which uses a tempdir + scrubbed
/// env for a reduced blast radius.  Governance layers must treat `cmd.run`
/// requests with the same scrutiny as raw `sudo` access.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CmdRunInput {
    /// Shell command string. Executed via `sh -c` so pipes, redirects, and
    /// compound expressions are supported. Callers are responsible for safe
    /// argument escaping.
    pub command: String,
    /// Optional working directory. Defaults to the connector workspace root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,
    /// Execution timeout in milliseconds. Defaults to 30 000 ms.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// Optional environment variable overrides merged on top of the inherited
    /// host environment.  All host env vars pass through unless explicitly
    /// overridden here — unlike `sandbox.execute_isolated`, there is no scrub.
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
    /// Optional stdin data piped into the child process.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdin: Option<String>,
}

/// Output of `cmd.run`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CmdRunOutput {
    /// Captured standard output (UTF-8, lossy).
    pub stdout: String,
    /// Captured standard error (UTF-8, lossy).
    pub stderr: String,
    /// Process exit code. `-1` if the process was killed (timeout or signal).
    pub exit_code: i32,
    /// `true` when the process was killed due to the timeout budget.
    pub timed_out: bool,
    /// Wall-clock elapsed milliseconds (measured by the connector).
    pub elapsed_ms: u64,
}

// ---------------------------------------------------------------------------
// cmd.run_streaming — streaming output variant
// ---------------------------------------------------------------------------

/// Input for `cmd.run_streaming`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CmdRunStreamingInput {
    /// Shell command string (executed via `sh -c`).
    pub command: String,
    /// Optional working directory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,
    /// Execution timeout in milliseconds. Defaults to 60 000 ms.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// Optional environment variable overrides.
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
    /// Maximum combined stdout + stderr bytes to buffer before truncation.
    /// Defaults to 1 MiB.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_bytes: Option<usize>,
}

/// Output of `cmd.run_streaming`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CmdRunStreamingOutput {
    /// Interleaved output lines in arrival order.
    pub lines: Vec<StreamLine>,
    /// Complete stdout (UTF-8, lossy).
    pub stdout: String,
    /// Complete stderr (UTF-8, lossy).
    pub stderr: String,
    /// Process exit code.
    pub exit_code: i32,
    /// `true` when killed due to timeout.
    pub timed_out: bool,
    /// `true` when output was truncated at `max_output_bytes`.
    pub truncated: bool,
    /// Wall-clock elapsed milliseconds.
    pub elapsed_ms: u64,
}

/// A single line of streamed output tagged with its origin stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamLine {
    /// `"stdout"` or `"stderr"`.
    pub stream: String,
    /// The line content (newline stripped).
    pub line: String,
}

// ---------------------------------------------------------------------------
// cmd.run_powershell — PowerShell variant
// ---------------------------------------------------------------------------

/// Input for `cmd.run_powershell`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CmdRunPowerShellInput {
    /// PowerShell script or command string.
    pub script: String,
    /// Optional working directory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,
    /// Execution timeout in milliseconds. Defaults to 30 000 ms.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// Optional environment variable overrides.
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
}

/// Output of `cmd.run_powershell`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CmdRunPowerShellOutput {
    /// Captured standard output.
    pub stdout: String,
    /// Captured standard error.
    pub stderr: String,
    /// Process exit code.
    pub exit_code: i32,
    /// `true` when killed due to timeout.
    pub timed_out: bool,
    /// Wall-clock elapsed milliseconds.
    pub elapsed_ms: u64,
    /// Path to the PowerShell executable that was used.
    pub powershell_path: String,
}

/// Input for `fs.patch_apply` capability — applies a unified-diff
/// patch to a file (Hermes BT-05).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsPatchApplyInput {
    /// Path to the file to patch.
    pub path: String,
    /// Unified diff text (the part starting at `@@` hunk headers).
    /// `---` / `+++` headers above the first hunk are tolerated and
    /// ignored — the file path comes from `path`, not from the diff.
    pub patch: String,
}

/// Output of `fs.patch_apply`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsPatchApplyOutput {
    /// Path to the patched file.
    pub path: String,
    /// Number of hunks successfully applied.
    pub hunks_applied: usize,
    /// Lines added across all hunks.
    pub lines_added: usize,
    /// Lines removed across all hunks.
    pub lines_removed: usize,
}

/// Input for `security.osv_scan` capability.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OsvScanInput {
    /// Path to the lockfile to scan (Cargo.lock, package-lock.json, etc.).
    pub lockfile_path: String,
    /// Optional ecosystem hint. Defaults to auto-detect from filename.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ecosystem: Option<String>,
}

/// A single vulnerability finding from `security.osv_scan`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OsvVulnerability {
    /// Package name with the vulnerability.
    pub package: String,
    /// Affected version installed in the lockfile.
    pub version: String,
    /// CVE / RUSTSEC / GHSA identifier.
    pub advisory_id: String,
    /// Short description.
    pub title: String,
    /// CVSS severity if available.
    pub severity: Option<String>,
}

/// Output of `security.osv_scan`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OsvScanOutput {
    /// Total number of vulnerabilities found.
    pub total: usize,
    /// List of vulnerabilities.
    pub vulnerabilities: Vec<OsvVulnerability>,
    /// Scanner backend used (e.g., "cargo-audit").
    pub scanner: String,
    /// `true` if the scan completed without errors.
    pub completed: bool,
}
