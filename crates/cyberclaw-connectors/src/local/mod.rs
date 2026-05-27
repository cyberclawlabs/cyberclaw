pub mod audio;
// F8 Phase 3 (2026-05-06) — `cmd` / `fs` / `search` were private; expose
// them so `apps/cyberclaw-server` can call each module's
// `capability_facades()` for the §4 facade aggregation. Their internal
// state (LocalConnector dispatch handlers) was already accessed via the
// `Connector` trait, so widening the visibility doesn't expand the
// surface — it only lets external crates read the facade declarations.
pub mod cmd;
// Sprint 3 PART A (2026-05-21, optimization-plan-v1 §R2) — function-level
// code-execution helper for the agent-runtime OutputVerifier chain. Not a
// `Capability` by design (see `exec_runtime` doc-comment); exposed here so
// `cyberclaw-agent-runtime` can call `run_code(..)` from `verify.rs`.
pub mod exec_runtime;
pub mod fs;
mod host;
pub mod image_gen;
pub mod lsp;
pub mod memory;
pub mod search;
mod security;
pub mod slides;
pub mod task;
pub mod verify;
pub mod vision;
pub mod web;
pub mod workdir_checkpoint;

use crate::types::{
    CapabilityExecutionRequest, CapabilityExecutionResult, Connector,
    ExecutionStatus as ConnectorExecutionStatus,
};
use cyberclaw_core::manifests::{CapabilityContract, ConnectorRuntime};
use cyberclaw_core::prelude::*;
use cyberclaw_governance::dangerous_capability_filter::{
    DangerousCapabilityFilter, FilterDecision,
};
use once_cell::sync::Lazy;
use std::path::{Path, PathBuf};
use tracing::{debug, error, info, warn};

/// Module-level singleton for content-side governance checks on write capabilities.
///
/// R13-BUG-01 fix: `DangerousCapabilityFilter::check_write_content` was dead code —
/// the filter existed but was never called in the write execution path. This
/// singleton is used by `execute()` to gate `fs.write` / `fs.edit` / `fs.append`
/// / `fs.multi_edit` / `fs.patch_apply` before any bytes reach the filesystem.
static WRITE_CONTENT_FILTER: Lazy<DangerousCapabilityFilter> =
    Lazy::new(DangerousCapabilityFilter::with_defaults);

/// Extract the primary text content from a write-class capability input for
/// content-side credential scanning.
///
/// Returns `None` when the capability is not a write class or the input has no
/// inspectable string payload (e.g. pure path operations like `fs.delete`).
fn extract_write_content(capability_id: &str, input: &serde_json::Value) -> Option<String> {
    match capability_id {
        // fs.write / fs.append — "content" field is the verbatim bytes to land on disk.
        "fs.write" | "fs.append" => input
            .get("content")
            .and_then(|v| v.as_str())
            .map(|s| s.to_owned()),
        // fs.edit / fs.multi_edit — "new_string" / concatenation of all new_strings
        // represents what will be written. Scanning the replacement text prevents
        // an adversary injecting credentials via a find-and-replace operation.
        "fs.edit" => input
            .get("new_string")
            .and_then(|v| v.as_str())
            .map(|s| s.to_owned()),
        "fs.multiedit" | "fs.multi_edit" => {
            let edits = input.get("edits").and_then(|v| v.as_array())?;
            let combined: String = edits
                .iter()
                .filter_map(|e| e.get("new_string").and_then(|v| v.as_str()))
                .collect::<Vec<_>>()
                .join("\n");
            if combined.is_empty() {
                None
            } else {
                Some(combined)
            }
        }
        // fs.patch_apply — the patch diff itself may embed credential lines in
        // `+` context lines. Scan the full patch string.
        "fs.patch_apply" => input
            .get("patch")
            .and_then(|v| v.as_str())
            .map(|s| s.to_owned()),
        // cmd.run / cmd.run_streaming — the command string is executed via
        // `sh -c`, so a payload like `echo "AKIAIOSFODNN7EXAMPLE" > /tmp/f`
        // bypasses the fs.* gate entirely and lands credentials on disk via
        // shell redirect. Scanning the literal command string closes this
        // bypass path (R13-BUG-01 extension, QA R15).
        "cmd.run" | "cmd.run_streaming" => input
            .get("command")
            .and_then(|v| v.as_str())
            .map(|s| s.to_owned()),
        _ => None,
    }
}

/// Gate write-class capability requests through the content-side governance filter.
///
/// Returns `Err` with a human-readable governance message when `check_write_content`
/// emits [`FilterDecision::Deny`]. `Allow` and `Warn` pass through (warn is already
/// logged inside the filter).
fn check_write_content_gate(capability_id: &str, input: &serde_json::Value) -> anyhow::Result<()> {
    let content = match extract_write_content(capability_id, input) {
        Some(c) => c,
        None => return Ok(()),
    };

    // Use the full capability id prefixed with the connector namespace so
    // check_write_content's contains("fs.write") guard matches correctly.
    let full_cap = format!("connector:local:{}", capability_id);
    match WRITE_CONTENT_FILTER.check_write_content(&full_cap, &content) {
        FilterDecision::Deny {
            rule_id, reason, ..
        } => {
            warn!(
                capability = %capability_id,
                rule_id = %rule_id,
                "governance: write content rejected by DangerousCapabilityFilter"
            );
            // NEW-BUG-01 (Change 1) — QA R19 TC4/TC11 showed governance-denied
            // writes returned a generic "governance: write blocked" message
            // that the LLM read as ambiguous → hallucinated "Done. Written"
            // and fabricated `ls` listings for files that don't exist.
            //
            // The "[GOVERNANCE DENY — <rule>]" prefix + explicit
            // "Do not claim it was written" instruction inside the error
            // message itself is the anti-hallucination signal. The agentic
            // loop surfaces this error verbatim into the LLM's next-turn
            // tool result, so the model now sees an unambiguous deny marker
            // every time content-side governance blocks a write.
            Err(anyhow::anyhow!(
                "[GOVERNANCE DENY — {}] The platform's credential-detection \
                 policy blocked this write because the content matches a \
                 credential pattern (rule: {}). The file was NOT created. \
                 Do not claim it was written. Do not fabricate ls output or \
                 file contents. This is a hard policy gate — you cannot \
                 retry the same content.",
                rule_id,
                reason
            ))
        }
        FilterDecision::Warn { rule_id, reason } => {
            warn!(
                capability = %capability_id,
                rule_id = %rule_id,
                reason = %reason,
                "governance: write content matched warning rule"
            );
            Ok(())
        }
        FilterDecision::Allow => Ok(()),
    }
}

/// LocalConnector - executes capabilities in the native runtime
#[derive(Debug)]
pub struct LocalConnector {
    id: ConnectorId,
    workspace: PathBuf,
    capabilities: Vec<CapabilityContract>,
    /// Sprint 19 W4 — optional container runtime for High-risk handlers
    /// (currently `cmd.exec`). When set, `cmd.exec` runs the whitelisted
    /// command inside an alpine container with NetworkMode::None instead
    /// of bare subprocess. When None, falls back to subprocess (legacy
    /// path retained for tests + hosts without docker/podman).
    pub(crate) container_runtime: Option<std::sync::Arc<crate::runtime::ContainerRuntime>>,
}

impl LocalConnector {
    /// Workspace root that this connector serves. Public so capability
    /// modules (e.g. `image_gen`) can write artifacts under it.
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// Create a new LocalConnector
    pub fn new(workspace: PathBuf) -> Self {
        let id = ConnectorId::from_string("local".to_string()).unwrap();
        let capabilities = Self::build_capabilities();

        Self {
            id,
            workspace,
            capabilities,
            container_runtime: None,
        }
    }

    /// Attach a `ContainerRuntime` so High-risk handlers (cmd.exec) run
    /// inside a sandboxed container instead of bare subprocess.
    ///
    /// Sprint 19 W4. Caller is expected to verify the backend is
    /// available (`ContainerBackend::detect().await.is_some()`) before
    /// constructing the runtime.
    pub fn with_container_runtime(
        mut self,
        runtime: std::sync::Arc<crate::runtime::ContainerRuntime>,
    ) -> Self {
        self.container_runtime = Some(runtime);
        self
    }

    /// Build the list of capabilities exposed by LocalConnector
    fn build_capabilities() -> Vec<CapabilityContract> {
        let mut capabilities = vec![
            // File system capabilities
            CapabilityContract {
                id: "fs.read".to_string(),
                title: "Read File".to_string(),
                description: Some("Read contents of a file".to_string()),
                input_schema: "FsReadInput".to_string(),
                output_schema: "FsReadOutput".to_string(),
                risk: RiskLevel::Low,
                effects: vec![CapabilityEffect::Read],
                placement: None,
                timeouts: CapabilityTimeouts {
                    request_ms: Some(5000),
                },
            },
            CapabilityContract {
                id: "fs.write".to_string(),
                title: "Write File".to_string(),
                description: Some("Write contents to a file".to_string()),
                input_schema: "FsWriteInput".to_string(),
                output_schema: "FsWriteOutput".to_string(),
                risk: RiskLevel::Medium,
                effects: vec![CapabilityEffect::Write],
                placement: None,
                timeouts: CapabilityTimeouts {
                    request_ms: Some(5000),
                },
            },
            CapabilityContract {
                id: "fs.edit".to_string(),
                title: "Edit File".to_string(),
                description: Some("Replace text in a file".to_string()),
                input_schema: "FsEditInput".to_string(),
                output_schema: "FsEditOutput".to_string(),
                risk: RiskLevel::Medium,
                effects: vec![CapabilityEffect::Write],
                placement: None,
                timeouts: CapabilityTimeouts {
                    request_ms: Some(5000),
                },
            },
            // Search capabilities
            CapabilityContract {
                id: "search.grep".to_string(),
                title: "Search with Grep".to_string(),
                description: Some("Search for pattern in files".to_string()),
                input_schema: "SearchGrepInput".to_string(),
                output_schema: "SearchGrepOutput".to_string(),
                risk: RiskLevel::Low,
                effects: vec![CapabilityEffect::Read],
                placement: None,
                timeouts: CapabilityTimeouts {
                    request_ms: Some(10000),
                },
            },
            CapabilityContract {
                id: "search.glob".to_string(),
                title: "Search with Glob".to_string(),
                description: Some("Find files matching pattern".to_string()),
                input_schema: "SearchGlobInput".to_string(),
                output_schema: "SearchGlobOutput".to_string(),
                risk: RiskLevel::Low,
                effects: vec![CapabilityEffect::Read],
                placement: None,
                timeouts: CapabilityTimeouts {
                    request_ms: Some(10000),
                },
            },
            CapabilityContract {
                id: "fs.multiedit".to_string(),
                title: "Multi-Edit File".to_string(),
                description: Some(
                    "Apply multiple text replacements in one atomic operation".to_string(),
                ),
                input_schema: "FsMultiEditInput".to_string(),
                output_schema: "FsMultiEditOutput".to_string(),
                risk: RiskLevel::Medium,
                effects: vec![CapabilityEffect::Write],
                placement: None,
                timeouts: CapabilityTimeouts {
                    request_ms: Some(5000),
                },
            },
            CapabilityContract {
                id: "fs.patch_apply".to_string(),
                title: "Apply Unified Diff Patch".to_string(),
                description: Some(
                    "Apply a unified-diff patch to a file (Hermes BT-05)".to_string(),
                ),
                input_schema: "FsPatchApplyInput".to_string(),
                output_schema: "FsPatchApplyOutput".to_string(),
                risk: RiskLevel::Medium,
                effects: vec![CapabilityEffect::Write],
                placement: None,
                timeouts: CapabilityTimeouts {
                    request_ms: Some(5000),
                },
            },
            // D1 follow-up (2026-05-12) — wire previously-orphan fs.* capabilities
            // so LLM `file_list` / `file_stat` / `file_append` / `file_delete`
            // tools land at real LocalConnector handlers instead of bouncing off
            // an "Unknown capability" arm. Source for each handler lives in
            // crates/cyberclaw-connectors/src/local/fs.rs.
            CapabilityContract {
                id: "fs.list_dir".to_string(),
                title: "List Directory".to_string(),
                description: Some("Enumerate immediate children of a directory.".to_string()),
                input_schema: "FsListDirInput".to_string(),
                output_schema: "FsListDirOutput".to_string(),
                risk: RiskLevel::Low,
                effects: vec![CapabilityEffect::Read],
                placement: None,
                timeouts: CapabilityTimeouts {
                    request_ms: Some(5000),
                },
            },
            CapabilityContract {
                id: "fs.append".to_string(),
                title: "Append to File".to_string(),
                description: Some("Append text to the end of an existing file.".to_string()),
                input_schema: "FsAppendInput".to_string(),
                output_schema: "FsAppendOutput".to_string(),
                risk: RiskLevel::Medium,
                effects: vec![CapabilityEffect::Write],
                placement: None,
                timeouts: CapabilityTimeouts {
                    request_ms: Some(5000),
                },
            },
            CapabilityContract {
                id: "fs.stat".to_string(),
                title: "Stat Path".to_string(),
                description: Some(
                    "Return metadata for a path: existence, type, size, mtime, permissions."
                        .to_string(),
                ),
                input_schema: "FsStatInput".to_string(),
                output_schema: "FsStatOutput".to_string(),
                risk: RiskLevel::Low,
                effects: vec![CapabilityEffect::Read],
                placement: None,
                timeouts: CapabilityTimeouts {
                    request_ms: Some(5000),
                },
            },
            CapabilityContract {
                id: "fs.delete".to_string(),
                title: "Delete Path".to_string(),
                description: Some(
                    "Delete a file. Pass recursive=true to remove a directory tree.".to_string(),
                ),
                input_schema: "FsDeleteInput".to_string(),
                output_schema: "FsDeleteOutput".to_string(),
                risk: RiskLevel::High,
                effects: vec![CapabilityEffect::Write],
                placement: None,
                timeouts: CapabilityTimeouts {
                    request_ms: Some(5000),
                },
            },
            CapabilityContract {
                id: "workdir.checkpoint".to_string(),
                title: "Workdir Checkpoint".to_string(),
                description: Some(
                    "Take a shadow-git snapshot of a workdir. Returns hash, timestamp, \
                     file count, and total size."
                        .to_string(),
                ),
                input_schema: "WorkdirCheckpointInput".to_string(),
                output_schema: "WorkdirCheckpointOutput".to_string(),
                risk: RiskLevel::High,
                effects: vec![CapabilityEffect::Write],
                placement: None,
                timeouts: CapabilityTimeouts {
                    request_ms: Some(60_000),
                },
            },
            CapabilityContract {
                id: "workdir.list".to_string(),
                title: "List Workdir Checkpoints".to_string(),
                description: Some(
                    "List checkpoints for a workdir in reverse-chronological order.".to_string(),
                ),
                input_schema: "WorkdirListInput".to_string(),
                output_schema: "WorkdirListOutput".to_string(),
                risk: RiskLevel::Low,
                effects: vec![CapabilityEffect::Read],
                placement: None,
                timeouts: CapabilityTimeouts {
                    request_ms: Some(15_000),
                },
            },
            // Command capabilities
            CapabilityContract {
                id: "cmd.exec".to_string(),
                title: "Execute Command".to_string(),
                description: Some("Execute a shell command".to_string()),
                input_schema: "CmdExecInput".to_string(),
                output_schema: "CmdExecOutput".to_string(),
                risk: RiskLevel::High,
                effects: vec![CapabilityEffect::Execute],
                placement: None,
                timeouts: CapabilityTimeouts {
                    request_ms: Some(30000),
                },
            },
            // R-1 (2026-05-05) — `cmd.run` is the unrestricted alternative to
            // `cmd.exec`. The bash facade routes here so agents can run business
            // validation scripts (python3 etc.). Risk: High; governance is
            // enforced by the PolicyEngine + a host-level command-content gate.
            CapabilityContract {
                id: "cmd.run".to_string(),
                title: "Run Shell Command".to_string(),
                description: Some(
                    "Run an arbitrary shell command via `sh -c` with full host privileges \
                     (governance-gated). Used by the agent `bash` facade."
                        .to_string(),
                ),
                input_schema: "CmdRunInput".to_string(),
                output_schema: "CmdRunOutput".to_string(),
                risk: RiskLevel::High,
                effects: vec![CapabilityEffect::Execute, CapabilityEffect::Write],
                placement: None,
                timeouts: CapabilityTimeouts {
                    request_ms: Some(30000),
                },
            },
            CapabilityContract {
                id: "cmd.run_streaming".to_string(),
                title: "Run Shell Command (Streaming)".to_string(),
                description: Some(
                    "Run a shell command and stream stdout/stderr line-by-line. Same risk \
                     profile as cmd.run."
                        .to_string(),
                ),
                input_schema: "CmdRunStreamingInput".to_string(),
                output_schema: "CmdRunStreamingOutput".to_string(),
                risk: RiskLevel::High,
                effects: vec![CapabilityEffect::Execute, CapabilityEffect::Write],
                placement: None,
                timeouts: CapabilityTimeouts {
                    request_ms: Some(60000),
                },
            },
            CapabilityContract {
                id: "cmd.run_powershell".to_string(),
                title: "Run PowerShell Script".to_string(),
                description: Some(
                    "Run a PowerShell script via `pwsh` (PowerShell 7+) or `powershell.exe` \
                     fallback on Windows."
                        .to_string(),
                ),
                input_schema: "CmdRunPowerShellInput".to_string(),
                output_schema: "CmdRunPowerShellOutput".to_string(),
                risk: RiskLevel::High,
                effects: vec![CapabilityEffect::Execute, CapabilityEffect::Write],
                placement: None,
                timeouts: CapabilityTimeouts {
                    request_ms: Some(30000),
                },
            },
            // Web capabilities
            CapabilityContract {
                id: "web.fetch".to_string(),
                title: "Fetch URL".to_string(),
                description: Some("Fetch web page content over HTTP/HTTPS".to_string()),
                input_schema: "WebFetchInput".to_string(),
                output_schema: "WebFetchOutput".to_string(),
                risk: RiskLevel::Low,
                effects: vec![CapabilityEffect::Read, CapabilityEffect::Network],
                placement: None,
                timeouts: CapabilityTimeouts {
                    request_ms: Some(10000),
                },
            },
            CapabilityContract {
                id: "web.search".to_string(),
                title: "Web Search".to_string(),
                description: Some("Search web content using a configured provider".to_string()),
                input_schema: "WebSearchInput".to_string(),
                output_schema: "WebSearchOutput".to_string(),
                risk: RiskLevel::Low,
                effects: vec![CapabilityEffect::Read, CapabilityEffect::Network],
                placement: None,
                timeouts: CapabilityTimeouts {
                    request_ms: Some(10000),
                },
            },
            // Audio transcription (R-04 — Hermes v0.12 transcription_tools)
            CapabilityContract {
                id: "audio.transcribe".to_string(),
                title: "Transcribe Audio (Whisper)".to_string(),
                description: Some("Transcribe an audio file via OpenAI Whisper".to_string()),
                input_schema: "AudioTranscribeInput".to_string(),
                output_schema: "AudioTranscribeOutput".to_string(),
                risk: RiskLevel::Low,
                effects: vec![CapabilityEffect::Read, CapabilityEffect::Network],
                placement: None,
                timeouts: CapabilityTimeouts {
                    request_ms: Some(120_000),
                },
            },
            // Audio synthesis (R-05 — Hermes v0.12 tts_tool)
            CapabilityContract {
                id: "audio.synthesize".to_string(),
                title: "Synthesize Speech (TTS)".to_string(),
                description: Some(
                    "Generate audio from text via OpenAI / ElevenLabs TTS".to_string(),
                ),
                input_schema: "AudioSynthesizeInput".to_string(),
                output_schema: "AudioSynthesizeOutput".to_string(),
                risk: RiskLevel::Medium,
                effects: vec![CapabilityEffect::Write, CapabilityEffect::Network],
                placement: None,
                timeouts: CapabilityTimeouts {
                    request_ms: Some(120_000),
                },
            },
            // Image generation (R-03 — Hermes v0.12 image_generation_tool)
            CapabilityContract {
                id: "image.generate".to_string(),
                title: "Generate Image".to_string(),
                description: Some(
                    "Generate an image via DALL-E / Stability and write to workspace".to_string(),
                ),
                input_schema: "ImageGenerateInput".to_string(),
                output_schema: "ImageGenerateOutput".to_string(),
                risk: RiskLevel::Medium,
                effects: vec![CapabilityEffect::Write, CapabilityEffect::Network],
                placement: None,
                timeouts: CapabilityTimeouts {
                    request_ms: Some(120_000),
                },
            },
            // Vision capability (R-02 — Hermes v0.12 vision_tools)
            CapabilityContract {
                id: "vision.analyze_image".to_string(),
                title: "Analyze Image (Vision)".to_string(),
                description: Some(
                    "Send an image to a vision-capable LLM and return text".to_string(),
                ),
                input_schema: "VisionAnalyzeInput".to_string(),
                output_schema: "VisionAnalyzeOutput".to_string(),
                risk: RiskLevel::Low,
                effects: vec![CapabilityEffect::Read, CapabilityEffect::Network],
                placement: None,
                timeouts: CapabilityTimeouts {
                    request_ms: Some(45000),
                },
            },
            // B-4 (2026-05-05) — Verifier capability for numeric/business
            // tasks. Re-computes sum/avg/count over a CSV column (optionally
            // grouped) and emits a binary match verdict. Risk: Low because
            // it is read-only against the workspace.
            CapabilityContract {
                id: "verify.numeric_aggregate".to_string(),
                title: "Verify Numeric Aggregate".to_string(),
                description: Some(
                    "Re-compute sum/avg/count over a CSV column and compare against a claimed \
                     value. Used as an LLM self-check gate in numeric/business workflows."
                        .to_string(),
                ),
                input_schema: "VerifyNumericInput".to_string(),
                output_schema: "VerifyNumericOutput".to_string(),
                risk: RiskLevel::Low,
                effects: vec![CapabilityEffect::Read],
                placement: None,
                timeouts: CapabilityTimeouts {
                    request_ms: Some(15000),
                },
            },
            // GA-01 closure (2026-05-06) — slides.render bridges Markdown
            // deck source to a real .pptx via python-pptx subprocess.
            // Closes the grade-C gap where the agent could only produce
            // Markdown; admin/agent now has a Connector→Capability path
            // to a binary-format business deliverable.
            CapabilityContract {
                id: "slides.render".to_string(),
                title: "Render slide deck (Markdown → PPTX)".to_string(),
                description: Some(
                    "Convert a Markdown deck source (--- separators, # title, - bullet lines) \
                     into a real PowerPoint .pptx via python-pptx. Returns the file path, \
                     slide count, and pptx MIME type. Falls back loudly if python3 / \
                     python-pptx is missing rather than silently writing a broken file."
                        .to_string(),
                ),
                input_schema: "SlidesRenderInput".to_string(),
                output_schema: "SlidesRenderOutput".to_string(),
                risk: RiskLevel::Medium,
                effects: vec![CapabilityEffect::Write, CapabilityEffect::Execute],
                placement: None,
                timeouts: CapabilityTimeouts {
                    request_ms: Some(60_000),
                },
            },
            // Security capabilities
            CapabilityContract {
                id: "security.osv_scan".to_string(),
                title: "OSV Vulnerability Scan".to_string(),
                description: Some(
                    "Scan a lockfile for known vulnerabilities via cargo-audit / OSV".to_string(),
                ),
                input_schema: "OsvScanInput".to_string(),
                output_schema: "OsvScanOutput".to_string(),
                risk: RiskLevel::Low,
                effects: vec![CapabilityEffect::Read, CapabilityEffect::Network],
                placement: None,
                timeouts: CapabilityTimeouts {
                    request_ms: Some(60000),
                },
            },
        ];

        capabilities.extend(Self::build_host_capabilities());
        capabilities
    }

    fn host_capability(
        id: &str,
        title: &str,
        description: &str,
        effects: Vec<CapabilityEffect>,
    ) -> CapabilityContract {
        CapabilityContract {
            id: id.to_string(),
            title: title.to_string(),
            description: Some(description.to_string()),
            input_schema: format!("{}Input", title.replace(' ', "")),
            output_schema: format!("{}Output", title.replace(' ', "")),
            risk: RiskLevel::Low,
            effects,
            placement: None,
            timeouts: CapabilityTimeouts {
                request_ms: Some(30000),
            },
        }
    }

    fn build_host_capabilities() -> Vec<CapabilityContract> {
        vec![
            Self::host_capability(
                "host.agent.run",
                "Host Agent",
                "Run host-level agent orchestration task",
                vec![CapabilityEffect::Execute],
            ),
            Self::host_capability(
                "host.ask_user_question",
                "Ask User Question",
                "Generate structured user question payload",
                vec![CapabilityEffect::Read],
            ),
            Self::host_capability(
                "host.brief",
                "Brief Message",
                "Generate concise user-facing summary",
                vec![CapabilityEffect::Read],
            ),
            Self::host_capability(
                "host.config",
                "Host Config",
                "Read/update host session config",
                vec![CapabilityEffect::Write],
            ),
            Self::host_capability(
                "host.plan.enter",
                "Enter Plan Mode",
                "Enter host plan mode",
                vec![CapabilityEffect::Write],
            ),
            Self::host_capability(
                "host.plan.exit",
                "Exit Plan Mode",
                "Exit host plan mode",
                vec![CapabilityEffect::Write],
            ),
            Self::host_capability(
                "host.worktree.enter",
                "Enter Worktree",
                "Set active worktree context",
                vec![CapabilityEffect::Write],
            ),
            Self::host_capability(
                "host.worktree.exit",
                "Exit Worktree",
                "Clear active worktree context",
                vec![CapabilityEffect::Write],
            ),
            Self::host_capability(
                "host.lsp",
                "Host LSP",
                "Run lightweight LSP-like operations",
                vec![CapabilityEffect::Read],
            ),
            Self::host_capability(
                "host.mcp.auth",
                "MCP Auth",
                "Persist or query MCP auth tokens",
                vec![CapabilityEffect::Write],
            ),
            Self::host_capability(
                "host.notebook.edit",
                "Notebook Edit",
                "Edit Jupyter notebook cell content",
                vec![CapabilityEffect::Write],
            ),
            Self::host_capability(
                "host.repl",
                "Host REPL",
                "Execute REPL-style command",
                vec![CapabilityEffect::Execute],
            ),
            Self::host_capability(
                "host.remote.trigger",
                "Remote Trigger",
                "Send HTTP trigger request to remote endpoint",
                vec![CapabilityEffect::Network, CapabilityEffect::Write],
            ),
            CapabilityContract {
                id: "host.skill.invoke".to_string(),
                title: "Skill Invoke".to_string(),
                description: Some("Resolve and invoke skill handlers".to_string()),
                input_schema: "SkillInvokeInput".to_string(),
                output_schema: "SkillInvokeOutput".to_string(),
                risk: RiskLevel::Medium,
                effects: vec![CapabilityEffect::Read, CapabilityEffect::Execute],
                placement: None,
                timeouts: CapabilityTimeouts {
                    request_ms: Some(30000),
                },
            },
            Self::host_capability(
                "host.sleep",
                "Sleep",
                "Sleep for a specified duration",
                vec![CapabilityEffect::Read],
            ),
            Self::host_capability(
                "host.synthetic.output",
                "Structured Output",
                "Validate structured output payload",
                vec![CapabilityEffect::Read],
            ),
            Self::host_capability(
                "host.task.create",
                "Task Create",
                "Create a host task entry",
                vec![CapabilityEffect::Write],
            ),
            Self::host_capability(
                "host.task.get",
                "Task Get",
                "Fetch a host task entry",
                vec![CapabilityEffect::Read],
            ),
            Self::host_capability(
                "host.task.list",
                "Task List",
                "List host task entries",
                vec![CapabilityEffect::Read],
            ),
            Self::host_capability(
                "host.task.output",
                "Task Output",
                "Append output to a host task",
                vec![CapabilityEffect::Write],
            ),
            Self::host_capability(
                "host.task.stop",
                "Task Stop",
                "Stop a host task",
                vec![CapabilityEffect::Write],
            ),
            Self::host_capability(
                "host.task.update",
                "Task Update",
                "Update host task metadata",
                vec![CapabilityEffect::Write],
            ),
            Self::host_capability(
                "host.team.create",
                "Team Create",
                "Create a host team",
                vec![CapabilityEffect::Write],
            ),
            Self::host_capability(
                "host.team.delete",
                "Team Delete",
                "Delete a host team",
                vec![CapabilityEffect::Write],
            ),
            Self::host_capability(
                "host.todo.write",
                "Todo Write",
                "Write TODO list entries",
                vec![CapabilityEffect::Write],
            ),
            Self::host_capability(
                "host.tool.search",
                "Tool Search",
                "Search available tool names",
                vec![CapabilityEffect::Read],
            ),
            Self::host_capability(
                "host.cron.create",
                "Cron Create",
                "Create cron schedule entry",
                vec![CapabilityEffect::Write],
            ),
            Self::host_capability(
                "host.cron.delete",
                "Cron Delete",
                "Delete cron schedule entry",
                vec![CapabilityEffect::Write],
            ),
            Self::host_capability(
                "host.cron.list",
                "Cron List",
                "List cron schedule entries",
                vec![CapabilityEffect::Read],
            ),
        ]
    }

    /// Resolve the agent-scoped workspace directory for a given actor.
    ///
    /// Sprint 20 W1 — ADR-0002 Phase 1 (per-agent workspace).
    ///
    /// Returns `<workspace_root>/agents/<actor_id>` for `Human` /
    /// `Agent` actor types, creating the directory on first call.
    /// Falls back to the shared `workspace_root` for `System` /
    /// `Connector` actors that have no useful per-actor identity.
    ///
    /// Phase 1 only changes the *default* working directory used by
    /// capability handlers when no `workdir` is supplied in input.
    /// `validate_path` still uses `workspace_root` — agents can still
    /// escape via explicit absolute paths until Phase 2 lands strict
    /// containment.
    pub fn resolve_agent_workspace(&self, actor: &cyberclaw_core::identity::ActorRef) -> PathBuf {
        use cyberclaw_core::identity::ActorType;
        match actor.actor_type {
            ActorType::Human | ActorType::Agent => {
                let agents_root = self.workspace.join("agents");
                let agent_dir = agents_root.join(actor.id.as_str());
                if let Err(e) = std::fs::create_dir_all(&agent_dir) {
                    debug!(
                        "resolve_agent_workspace: create_dir_all failed for {:?}: {}",
                        agent_dir, e
                    );
                    return self.workspace.clone();
                }
                agent_dir
            }
            _ => self.workspace.clone(),
        }
    }

    /// Sprint 20 W1 — ADR-0002 Phase 2 entry point.
    ///
    /// Validates a path against the **agent-scoped** workspace
    /// when `CYBERCLAW_WORKSPACE_STRICT_AGENT_ISOLATION=true` is set
    /// in env (production-correct). Otherwise falls through to the
    /// legacy `validate_path` which uses the shared workspace root
    /// (preserves existing test + single-tenant semantics).
    ///
    /// Returns `Err` when strict mode is on AND the resolved path
    /// escapes the agent's subdir — even via explicit absolute path
    /// or `../` traversal.
    pub(crate) fn validate_path_for_actor(
        &self,
        path: &str,
        actor: &cyberclaw_core::identity::ActorRef,
    ) -> anyhow::Result<PathBuf> {
        let strict = std::env::var("CYBERCLAW_WORKSPACE_STRICT_AGENT_ISOLATION")
            .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(false);

        if !strict {
            // Legacy path — every actor validates against shared
            // workspace root. Phase 1 default workdir derivation
            // still applies for cmd.exec, but explicit paths can
            // escape an agent's subdir.
            return self.validate_path(path);
        }

        // Strict mode: validate against the agent's subdir.
        let agent_root = self.resolve_agent_workspace(actor);
        Self::validate_path_against_root(path, &agent_root)
    }

    /// Validate that a path is within the workspace
    ///
    /// Security improvements:
    /// - Validates all paths regardless of whether they exist (prevents TOCTOU)
    /// - Checks for path traversal patterns (.. components)
    /// - Uses canonicalization of nearest existing ancestor
    /// - Reconstructs safe absolute path for non-existent files
    fn validate_path(&self, path: &str) -> anyhow::Result<PathBuf> {
        Self::validate_path_against_root(path, &self.workspace)
    }

    /// Validate `path` is contained within `root`. Used by both the
    /// legacy [`Self::validate_path`] (root = shared workspace) and
    /// the strict [`Self::validate_path_for_actor`] (root = agent
    /// subdir). Free function so the caller picks the root.
    fn validate_path_against_root(path: &str, root: &Path) -> anyhow::Result<PathBuf> {
        let abs_path = if Path::new(path).is_absolute() {
            PathBuf::from(path)
        } else {
            root.join(path)
        };

        // Step 1: Check for explicit path traversal patterns
        // This prevents attacks using ".." even before filesystem checks
        for component in abs_path.components() {
            if component == std::path::Component::ParentDir {
                error!("Path traversal attempt (.. component): {}", path);
                return Err(anyhow::anyhow!(
                    "Path contains '..' components which are not allowed"
                ));
            }
        }

        // Step 2: Canonicalize root (must exist)
        let canonical_workspace = root
            .canonicalize()
            .map_err(|e| anyhow::anyhow!("Failed to canonicalize workspace: {}", e))?;

        // Step 3: Handle existing vs non-existing paths
        let validated_path = if abs_path.exists() {
            // For existing paths, canonicalize directly
            let canonical = abs_path
                .canonicalize()
                .map_err(|e| anyhow::anyhow!("Failed to canonicalize path: {}", e))?;

            // Verify it's within workspace
            if !canonical.starts_with(&canonical_workspace) {
                error!("Path is outside workspace: {}", path);
                // NEW-BUG-01 (Change 1) — same anti-hallucination treatment as
                // governance D010 deny. The LLM sees this error verbatim in the
                // next-turn tool result; without an explicit "NOT created" /
                // "Do not claim" marker it hallucinates write success.
                return Err(anyhow::anyhow!(
                    "[BOUNDARY DENY] Path '{}' is outside the configured workspace \
                     boundary. The file was NOT created. Do not claim it was written. \
                     Suggest a path inside the workspace (e.g. relative to the current \
                     working directory).",
                    path
                ));
            }

            canonical
        } else {
            // For non-existing paths, find nearest existing ancestor
            let mut ancestor = abs_path.clone();
            let mut remaining_components = Vec::new();

            // Walk up until we find something that exists
            while !ancestor.exists() {
                if let Some(file_name) = ancestor.file_name() {
                    remaining_components.push(file_name.to_os_string());
                }
                if let Some(parent) = ancestor.parent() {
                    ancestor = parent.to_path_buf();
                } else {
                    // No existing ancestor found at all
                    error!("No existing ancestor found for path: {}", path);
                    return Err(anyhow::anyhow!(
                        "Cannot validate path: no existing ancestor"
                    ));
                }
            }

            // Canonicalize the existing ancestor
            let canonical_ancestor = ancestor
                .canonicalize()
                .map_err(|e| anyhow::anyhow!("Failed to canonicalize ancestor: {}", e))?;

            // Verify ancestor is within workspace
            if !canonical_ancestor.starts_with(&canonical_workspace) {
                error!("Path ancestor is outside workspace: {}", path);
                return Err(anyhow::anyhow!(
                    "[BOUNDARY DENY] Path '{}' is outside the configured workspace \
                     boundary. The file was NOT created. Do not claim it was written. \
                     Suggest a path inside the workspace (e.g. relative to the current \
                     working directory).",
                    path
                ));
            }

            // Reconstruct the full path by appending remaining components
            let mut reconstructed = canonical_ancestor;
            for component in remaining_components.iter().rev() {
                reconstructed.push(component);
            }

            // Final sanity check: ensure reconstructed path still starts with workspace
            // This catches edge cases where component reconstruction could escape
            if !reconstructed.starts_with(&canonical_workspace) {
                error!("Reconstructed path is outside workspace: {}", path);
                return Err(anyhow::anyhow!(
                    "[BOUNDARY DENY] Path '{}' is outside the configured workspace \
                     boundary. The file was NOT created. Do not claim it was written. \
                     Suggest a path inside the workspace (e.g. relative to the current \
                     working directory).",
                    path
                ));
            }

            reconstructed
        };

        debug!("Path validated successfully: {}", path);
        Ok(validated_path)
    }
}

#[async_trait::async_trait]
impl Connector for LocalConnector {
    fn id(&self) -> &ConnectorId {
        &self.id
    }

    fn runtime(&self) -> ConnectorRuntime {
        ConnectorRuntime::Native
    }

    fn capabilities(&self) -> Vec<CapabilityContract> {
        self.capabilities.clone()
    }

    async fn execute(
        &self,
        request: CapabilityExecutionRequest,
    ) -> anyhow::Result<CapabilityExecutionResult> {
        debug!(
            "LocalConnector executing capability {} for execution {}",
            request.capability_id, request.execution_id
        );

        // R13-BUG-01 fix: gate write-class capabilities through content-side
        // credential scan BEFORE any bytes reach the filesystem. The filter
        // is a module-level singleton; this call is synchronous and cheap.
        let write_caps = [
            "fs.write",
            "fs.edit",
            "fs.multiedit",
            "fs.patch_apply",
            "fs.append",
            // R13-BUG-01 extension (QA R15): cmd.run / cmd.run_streaming can
            // write credentials to disk via shell redirect without touching
            // any fs.* capability. Scanning the command string closes the bypass.
            "cmd.run",
            "cmd.run_streaming",
        ];
        if write_caps.contains(&request.capability_id.as_str()) {
            if let Err(e) = check_write_content_gate(request.capability_id.as_str(), &request.input)
            {
                return Ok(CapabilityExecutionResult {
                    execution_id: request.execution_id,
                    trace_id: request.trace_id,
                    connector_id: request.connector_id,
                    capability_id: request.capability_id,
                    output: serde_json::json!({ "error": e.to_string() }),
                    status: ConnectorExecutionStatus::Failed,
                    error: Some(e.to_string()),
                    actual_runtime: None,
                });
            }
        }

        let result = match request.capability_id.as_str() {
            "fs.read" => self::fs::read(self, request.clone()),
            "fs.write" => self::fs::write(self, request.clone()),
            "fs.edit" => self::fs::edit(self, request.clone()),
            "fs.multiedit" => self::fs::multi_edit(self, request.clone()),
            "fs.patch_apply" => self::fs::patch_apply(self, request.clone()),
            // D1 follow-up (2026-05-12) — previously-orphan fs/workdir arms.
            "fs.list_dir" => self::fs::list_dir(self, request.clone()),
            "fs.append" => self::fs::append(self, request.clone()),
            "fs.stat" => self::fs::stat(self, request.clone()),
            "fs.delete" => self::fs::delete(self, request.clone()),
            "workdir.checkpoint" => self::workdir_checkpoint::checkpoint(request.clone()),
            "workdir.list" => self::workdir_checkpoint::list(request.clone()),
            "search.grep" => self::search::grep(self, request.clone()).await,
            "search.glob" => self::search::glob(self, request.clone()),
            "cmd.exec" => self::cmd::exec(self, request.clone()).await,
            "cmd.run" => self::cmd::run(self, request.clone()).await,
            "cmd.run_streaming" => self::cmd::run_streaming(self, request.clone()).await,
            "cmd.run_powershell" => self::cmd::run_powershell(self, request.clone()).await,
            "slides.render" => self::slides::render(self, request.clone()).await,
            "web.fetch" => self::web::fetch(self, request.clone()).await,
            "web.search" => self::web::search(self, request.clone()).await,
            "security.osv_scan" => self::security::osv_scan(self, request.clone()).await,
            "verify.numeric_aggregate" => {
                self::verify::numeric_aggregate(self, request.clone()).await
            }
            "vision.analyze_image" => self::vision::analyze_image(self, request.clone()).await,
            "image.generate" => self::image_gen::generate(self, request.clone()).await,
            "audio.transcribe" => self::audio::transcribe(self, request.clone()).await,
            "audio.synthesize" => self::audio::synthesize(self, request.clone()).await,
            capability_id if capability_id.starts_with("host.") => {
                self::host::execute(self, request.clone()).await
            }
            _ => {
                let error_msg = format!("Unknown capability: {}", request.capability_id);
                error!("{}", error_msg);
                return Ok(CapabilityExecutionResult {
                    execution_id: request.execution_id,
                    trace_id: request.trace_id,
                    connector_id: request.connector_id,
                    capability_id: request.capability_id,
                    output: serde_json::json!({
                        "error": error_msg.clone()
                    }),
                    status: ConnectorExecutionStatus::Failed,
                    error: Some(error_msg),
                    actual_runtime: None,
                });
            }
        };

        match result {
            Ok(output) => {
                info!("Capability {} executed successfully", request.capability_id);
                Ok(CapabilityExecutionResult {
                    execution_id: request.execution_id,
                    trace_id: request.trace_id,
                    connector_id: request.connector_id,
                    capability_id: request.capability_id,
                    output,
                    status: ConnectorExecutionStatus::Success,
                    error: None,
                    actual_runtime: None,
                })
            }
            Err(e) => {
                error!("Capability {} failed: {:?}", request.capability_id, e);
                Ok(CapabilityExecutionResult {
                    execution_id: request.execution_id,
                    trace_id: request.trace_id,
                    connector_id: request.connector_id,
                    capability_id: request.capability_id,
                    output: serde_json::json!({
                        "error": e.to_string()
                    }),
                    status: ConnectorExecutionStatus::Failed,
                    error: Some(e.to_string()),
                    actual_runtime: None,
                })
            }
        }
    }
}

#[cfg(test)]
mod resolve_agent_workspace_tests {
    use super::*;
    use cyberclaw_core::identity::{ActorRef, ActorType};
    use cyberclaw_core::ids::ActorId;

    fn actor(actor_type: ActorType) -> ActorRef {
        ActorRef {
            id: ActorId::from_string("agent_alpha".to_string()).unwrap(),
            actor_type,
            tenant_id: None,
            home_node_id: None,
            display_name: "agent_alpha".to_string(),
        }
    }

    #[test]
    fn agent_actor_gets_subdir() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = LocalConnector::new(tmp.path().to_path_buf());
        let ws = conn.resolve_agent_workspace(&actor(ActorType::Agent));
        assert_eq!(ws, tmp.path().join("agents").join("agent_alpha"));
        assert!(ws.exists(), "agent workspace dir should be auto-created");
    }

    #[test]
    fn human_actor_gets_subdir() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = LocalConnector::new(tmp.path().to_path_buf());
        let ws = conn.resolve_agent_workspace(&actor(ActorType::Human));
        assert_eq!(ws, tmp.path().join("agents").join("agent_alpha"));
        assert!(ws.exists());
    }

    #[test]
    fn system_actor_falls_back_to_root() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = LocalConnector::new(tmp.path().to_path_buf());
        let ws = conn.resolve_agent_workspace(&actor(ActorType::System));
        assert_eq!(ws, tmp.path());
        // Phase 1: NO subdir created for system actors.
        assert!(!tmp.path().join("agents").exists());
    }

    #[test]
    fn strict_mode_rejects_path_outside_agent_subdir() {
        // ADR-0002 Phase 2 invariant: when strict mode is on, agent A
        // cannot read/write paths that resolve outside its own subdir,
        // even when the path is inside the shared workspace_root.
        let tmp = tempfile::tempdir().unwrap();
        let conn = LocalConnector::new(tmp.path().to_path_buf());
        let alpha = actor(ActorType::Agent);
        let mut beta = actor(ActorType::Agent);
        beta.id = ActorId::from_string("agent_beta".to_string()).unwrap();

        // Pre-create both subdirs + a file in beta's dir.
        let _ = conn.resolve_agent_workspace(&alpha);
        let beta_root = conn.resolve_agent_workspace(&beta);
        std::fs::write(beta_root.join("secret.txt"), b"hi").unwrap();
        let beta_secret = beta_root.join("secret.txt");

        // Direct validate_path_against_root with the agent's subdir
        // (this is what strict mode does internally).
        let alpha_root = conn.resolve_agent_workspace(&alpha);

        // Within alpha's own subdir → ok (file doesn't need to exist)
        let ok = LocalConnector::validate_path_against_root("file.txt", &alpha_root);
        assert!(ok.is_ok(), "alpha can use its own subdir");

        // Beta's secret via absolute path → REJECTED
        let blocked =
            LocalConnector::validate_path_against_root(beta_secret.to_str().unwrap(), &alpha_root);
        assert!(
            blocked.is_err(),
            "alpha must NOT be able to address beta's file via abs path"
        );
        let err = blocked.unwrap_err().to_string();
        assert!(
            err.contains("[BOUNDARY DENY]")
                || err.contains("outside workspace")
                || err.contains(".."),
            "error should explain containment violation: {err}"
        );

        // Path traversal attempt → REJECTED
        let traversal =
            LocalConnector::validate_path_against_root("../agent_beta/secret.txt", &alpha_root);
        assert!(traversal.is_err(), "..-traversal must be rejected");
    }

    #[test]
    fn two_agents_get_separate_subdirs() {
        // Phase 1 invariant: different actor ids → different default
        // workspaces. (Strict containment of explicit paths is Phase 2.)
        let tmp = tempfile::tempdir().unwrap();
        let conn = LocalConnector::new(tmp.path().to_path_buf());

        let a = ActorRef {
            id: ActorId::from_string("agent_a".to_string()).unwrap(),
            actor_type: ActorType::Agent,
            tenant_id: None,
            home_node_id: None,
            display_name: "a".into(),
        };
        let b = ActorRef {
            id: ActorId::from_string("agent_b".to_string()).unwrap(),
            actor_type: ActorType::Agent,
            tenant_id: None,
            home_node_id: None,
            display_name: "b".into(),
        };

        let wa = conn.resolve_agent_workspace(&a);
        let wb = conn.resolve_agent_workspace(&b);
        assert_ne!(wa, wb);
        assert!(wa.starts_with(tmp.path().join("agents")));
        assert!(wb.starts_with(tmp.path().join("agents")));
    }
}

#[cfg(test)]
mod governance_deny_format_tests {
    use super::*;

    /// NEW-BUG-01 (Change 1) — when content-side governance denies a write,
    /// the error message returned to the agentic loop must carry the
    /// unambiguous "[GOVERNANCE DENY" prefix and an explicit
    /// "Do not claim it was written" instruction so the LLM can recognise
    /// a hard policy gate and stop fabricating success messages / fake
    /// `ls` listings.
    ///
    /// R19 TC4/TC11 reproduced the hallucination: server silently turned
    /// a content-side filter rejection into `LocalActionResult::Failed`
    /// with a generic "governance: write blocked" string, and the model
    /// turned around to claim "Done. Written to /tmp/x" + fabricated
    /// directory listings.
    #[test]
    fn governance_deny_error_carries_unambiguous_marker() {
        // Use an AWS access key pattern that the default
        // DangerousCapabilityFilter rules reject. The input shape mirrors
        // what fs.write receives at the connector boundary.
        let input = serde_json::json!({
            "path": "secrets.txt",
            "content": "AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE\nAWS_SECRET_ACCESS_KEY=wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"
        });

        let result = check_write_content_gate("fs.write", &input);
        assert!(
            result.is_err(),
            "content-side governance filter must deny known AWS credential pattern"
        );

        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("[GOVERNANCE DENY"),
            "error must start with the unambiguous [GOVERNANCE DENY marker so \
             the LLM can recognise a hard policy gate (got: {msg})"
        );
        assert!(
            msg.contains("Do not claim it was written"),
            "error must include explicit anti-hallucination instruction \
             (got: {msg})"
        );
        assert!(
            msg.contains("NOT created"),
            "error must explicitly state the file was NOT created (got: {msg})"
        );
    }

    /// NEW-BUG-01 (Change 1) — workspace boundary rejection must carry the
    /// same anti-hallucination markers as governance D010 deny.
    ///
    /// Reproduces R20 TC4: fs.write /tmp/wordcount-cb.txt failed with bare
    /// "Path is outside workspace boundary" → LLM hallucinated write success.
    /// After this fix the error includes "[BOUNDARY DENY]", "NOT created", and
    /// "Do not claim it was written" so the model receives an unambiguous signal.
    #[test]
    fn workspace_boundary_error_carries_unambiguous_marker() {
        use tempfile::TempDir;

        let tmp = TempDir::new().expect("temp dir");
        // Attempt to validate an absolute path outside the temp workspace
        // (e.g. /tmp/wordcount-cb.txt is outside tmp.path())
        let outside_path = "/tmp/cyberclaw-test-boundary-marker.txt";
        let result = LocalConnector::validate_path_against_root(outside_path, tmp.path());

        assert!(result.is_err(), "path outside workspace must be rejected");

        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("[BOUNDARY DENY]"),
            "workspace boundary error must carry [BOUNDARY DENY] marker so the \
             LLM recognises a hard boundary rejection (got: {msg})"
        );
        assert!(
            msg.contains("NOT created"),
            "workspace boundary error must explicitly state the file was NOT \
             created (got: {msg})"
        );
        assert!(
            msg.contains("Do not claim it was written"),
            "workspace boundary error must include explicit anti-hallucination \
             instruction (got: {msg})"
        );
        // grep-compat: downstream log parsing still matches on the original
        // substring — preserved inside the new message via path interpolation
        // but the critical guard is the [BOUNDARY DENY] prefix above.
    }
}
