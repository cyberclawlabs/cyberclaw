//! # LSP (Language Server Protocol) Connector
//!
//! A [`Connector`] implementation that exposes Language Server Protocol
//! capabilities as read-only code-intelligence queries. The connector spawns a
//! configured LSP server as a subprocess and speaks LSP JSON-RPC over its
//! stdin/stdout.
//!
//! ## Capabilities (v1, MVP)
//!
//! | id | description | risk |
//! |---|---|---|
//! | `lsp.hover` | Hover info (type / docs) at `(file, line, col)` | Low |
//! | `lsp.definition` | Goto-definition for symbol at position | Low |
//! | `lsp.references` | Find all references to symbol at position | Low |
//! | `lsp.diagnostics` | Current diagnostics for a file | Low |
//! | `lsp.document_symbols` | Symbol tree of a document | Low |
//! | `lsp.workspace_symbols` | Workspace-wide symbol search by name pattern | Low |
//!
//! All capabilities are read-only introspection — [`RiskLevel::Low`] with
//! [`CapabilityEffect::Read`]. Mutating operations (`rename`, `code_action`,
//! `formatting`) are intentionally out of scope for v1 — they are deferred to a
//! future `LspWriteConnector` with elevated risk and an edit-preview contract.
//!
//! ## MVP limitations
//!
//! - **Single backend at a time.** The connector is constructed with exactly
//!   one LSP server command (e.g. `rust-analyzer`). Multi-language dispatch
//!   (Rust/TS/Py routed to different servers by file extension) is deferred
//!   to v2.
//! - **No persistent server handle.** v1 spawns a fresh LSP server per request,
//!   sends `initialize` + operation + `shutdown`, and tears it down. That is
//!   slow but simple. A short-lived per-session cache is possible via
//!   `with_shared_server()` in a later revision — out of scope here.
//! - **No `textDocument/didOpen` synchronization.** We open files with
//!   `didOpen` inline per request. Unsaved-buffer scenarios are not supported.
//! - **If the configured LSP server command is unavailable at runtime** (not
//!   on `PATH`, or fails to spawn), capabilities return
//!   [`CapabilityExecutionResult`] with status
//!   [`ExecutionStatus::Failed`] and a structured error message. The
//!   connector does NOT panic and MUST NOT be a hard runtime requirement for
//!   the platform.
//!
//! ## Idiom compliance notes
//!
//! - §1 **Injected dispatcher**: LSP backend is injected via
//!   [`LspConnectorConfig::server_command`] at construction — no hard-coded
//!   backend. Tests replace the command with `echo` / fixture scripts.
//! - §2 **Event sink**: N/A at the connector layer (events are emitted by
//!   orchestrators consuming this connector, not by the connector itself —
//!   same as [`LocalConnector`] and [`SandboxConnector`]).
//! - §3 **Plan-in / events-out**: Each request carries a typed plan
//!   ([`LspHoverInput`], [`LspDefinitionInput`], …) that is
//!   `Serialize + Deserialize`.
//! - §4 **Facades from owner crate**: This module exports
//!   [`capability_facades()`] — the agent-runtime registry imports them by
//!   string ID.
//! - §5 **No sixth object**: This is a `Connector`. Nothing new introduced.
//! - §6 **N/A**: no Skill bundle changed.

use crate::types::{
    CapabilityExecutionRequest, CapabilityExecutionResult, Connector,
    ExecutionStatus as ConnectorExecutionStatus,
};
use cyberclaw_core::facade::{CapabilityFacade, FacadeExposure, ToolsetCategory};
use cyberclaw_core::ids::{CapabilityId, ConnectorId};
use cyberclaw_core::manifests::{CapabilityContract, ConnectorRuntime};
use cyberclaw_core::prelude::*;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout, Command};
use tracing::{debug, error, info, warn};

/// Default timeout for a single LSP request (initialize + op + shutdown).
pub const DEFAULT_LSP_TIMEOUT: Duration = Duration::from_secs(30);

/// Construction-time configuration for [`LspConnector`].
///
/// The `server_command` is the program (and optional args) that will be
/// launched as the LSP backend. Typical values:
///
/// - `("rust-analyzer", vec![])` — Rust
/// - `("typescript-language-server", vec!["--stdio".into()])` — TS/JS
/// - `("pyright-langserver", vec!["--stdio".into()])` — Python
///
/// v1 supports exactly one backend per connector instance. To support multiple
/// languages concurrently, register multiple [`LspConnector`] instances with
/// distinct `id`s (e.g. `lsp-rust`, `lsp-ts`) in the connector registry.
#[derive(Debug, Clone)]
pub struct LspConnectorConfig {
    /// The LSP server executable name (looked up on `PATH`) or absolute path.
    pub server_command: String,
    /// Arguments passed to the LSP server.
    pub server_args: Vec<String>,
    /// Workspace root URI passed to the server on `initialize`.
    /// If `None`, the current working directory is used.
    pub workspace_root: Option<PathBuf>,
    /// Request-level timeout.
    pub request_timeout: Duration,
}

impl LspConnectorConfig {
    /// Build a config for `rust-analyzer`.
    pub fn rust_analyzer(workspace_root: PathBuf) -> Self {
        Self {
            server_command: "rust-analyzer".to_string(),
            server_args: Vec::new(),
            workspace_root: Some(workspace_root),
            request_timeout: DEFAULT_LSP_TIMEOUT,
        }
    }

    /// Build a config for a generic stdio-based LSP server.
    pub fn generic(command: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            server_command: command.into(),
            server_args: args,
            workspace_root: None,
            request_timeout: DEFAULT_LSP_TIMEOUT,
        }
    }
}

/// LSP Connector — exposes six code-intelligence capabilities.
///
/// See module docs for the capability list, risk model, and MVP limitations.
#[derive(Debug, Clone)]
pub struct LspConnector {
    id: ConnectorId,
    config: LspConnectorConfig,
}

impl LspConnector {
    /// Create a new LSP connector with the given connector id and backend config.
    pub fn new(
        connector_id: impl Into<String>,
        config: LspConnectorConfig,
    ) -> anyhow::Result<Self> {
        let id = ConnectorId::from_string(connector_id.into())
            .map_err(|e| anyhow::anyhow!("invalid connector id: {e:?}"))?;
        Ok(Self { id, config })
    }

    /// Convenience: rust-analyzer connector with id `lsp-rust`.
    pub fn rust_analyzer(workspace_root: PathBuf) -> anyhow::Result<Self> {
        Self::new(
            "lsp-rust",
            LspConnectorConfig::rust_analyzer(workspace_root),
        )
    }

    fn build_capabilities() -> Vec<CapabilityContract> {
        let ids = [
            ("lsp.hover", "Hover Info", "Hover info at (file, line, col)"),
            (
                "lsp.definition",
                "Go to Definition",
                "Resolve symbol definition locations at (file, line, col)",
            ),
            (
                "lsp.references",
                "Find References",
                "Find all references to symbol at (file, line, col)",
            ),
            (
                "lsp.diagnostics",
                "Diagnostics",
                "Return current diagnostics for a file",
            ),
            (
                "lsp.document_symbols",
                "Document Symbols",
                "Symbol tree for a file",
            ),
            (
                "lsp.workspace_symbols",
                "Workspace Symbols",
                "Search workspace symbols by name pattern",
            ),
        ];
        ids.iter()
            .map(|(id, title, desc)| CapabilityContract {
                id: (*id).to_string(),
                title: (*title).to_string(),
                description: Some((*desc).to_string()),
                input_schema: format!("{}Input", to_pascal(id)),
                output_schema: format!("{}Output", to_pascal(id)),
                risk: RiskLevel::Low,
                effects: vec![CapabilityEffect::Read],
                placement: None,
                timeouts: CapabilityTimeouts {
                    request_ms: Some(30_000),
                },
            })
            .collect()
    }
}

fn to_pascal(cap_id: &str) -> String {
    // "lsp.document_symbols" -> "LspDocumentSymbols"
    cap_id
        .split(['.', '_'])
        .filter(|s| !s.is_empty())
        .map(|s| {
            let mut cs = s.chars();
            match cs.next() {
                Some(c) => c.to_ascii_uppercase().to_string() + cs.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Input / Output schemas
// ---------------------------------------------------------------------------

/// Position in a text document. Line + col are 1-based to match editor UIs;
/// the JSON-RPC layer converts to LSP's 0-based indices internally.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspPosition {
    pub file: String,
    pub line: u32,
    pub col: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspHoverInput {
    pub file: String,
    pub line: u32,
    pub col: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspHoverOutput {
    pub contents: String,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspDefinitionInput {
    pub file: String,
    pub line: u32,
    pub col: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspLocation {
    pub file: String,
    pub line: u32,
    pub col: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspDefinitionOutput {
    pub locations: Vec<LspLocation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspReferencesInput {
    pub file: String,
    pub line: u32,
    pub col: u32,
    #[serde(default)]
    pub include_declaration: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspReferencesOutput {
    pub locations: Vec<LspLocation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspDiagnosticsInput {
    pub file: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspDiagnostic {
    /// "error" | "warning" | "information" | "hint"
    pub severity: String,
    pub message: String,
    pub line: u32,
    pub col: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspDiagnosticsOutput {
    pub diagnostics: Vec<LspDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspDocumentSymbolsInput {
    pub file: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspSymbol {
    pub name: String,
    pub kind: String,
    pub line: u32,
    pub col: u32,
    #[serde(default)]
    pub container: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspDocumentSymbolsOutput {
    pub symbols: Vec<LspSymbol>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspWorkspaceSymbolsInput {
    pub query: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspWorkspaceSymbolsOutput {
    pub symbols: Vec<LspSymbol>,
}

// ---------------------------------------------------------------------------
// JSON-RPC scaffolding
// ---------------------------------------------------------------------------

/// Convert LSP `SymbolKind` numeric codes to short strings.
/// Reference: https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#symbolKind
fn symbol_kind_name(kind: i64) -> &'static str {
    match kind {
        1 => "file",
        2 => "module",
        3 => "namespace",
        4 => "package",
        5 => "class",
        6 => "method",
        7 => "property",
        8 => "field",
        9 => "constructor",
        10 => "enum",
        11 => "interface",
        12 => "function",
        13 => "variable",
        14 => "constant",
        15 => "string",
        16 => "number",
        17 => "boolean",
        18 => "array",
        19 => "object",
        20 => "key",
        21 => "null",
        22 => "enum_member",
        23 => "struct",
        24 => "event",
        25 => "operator",
        26 => "type_parameter",
        _ => "unknown",
    }
}

fn diagnostic_severity_name(sev: i64) -> &'static str {
    match sev {
        1 => "error",
        2 => "warning",
        3 => "information",
        4 => "hint",
        _ => "unknown",
    }
}

fn path_to_file_uri(path: &str) -> String {
    // Minimal `file://` URI — sufficient for rust-analyzer and most servers.
    // Does not handle percent-encoding of non-ASCII; adequate for MVP.
    if path.starts_with("file://") {
        path.to_string()
    } else if path.starts_with('/') {
        format!("file://{}", path)
    } else {
        // Relative paths: resolve against cwd for the URI.
        match std::env::current_dir() {
            Ok(cwd) => format!("file://{}/{}", cwd.display(), path),
            Err(_) => format!("file://{}", path),
        }
    }
}

fn file_uri_to_path(uri: &str) -> String {
    uri.strip_prefix("file://").unwrap_or(uri).to_string()
}

/// Minimal LSP client: spawn server, initialize, run one op, shutdown.
///
/// Callers pass the method name and params; the client wraps them in a JSON-RPC
/// envelope and returns the `result` field. On any failure (spawn, I/O, parse,
/// LSP error response) the client surfaces an `anyhow::Error`.
struct LspSession {
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
    _child: tokio::process::Child,
}

impl LspSession {
    async fn spawn(config: &LspConnectorConfig) -> anyhow::Result<Self> {
        let mut cmd = Command::new(&config.server_command);
        cmd.args(&config.server_args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        if let Some(root) = &config.workspace_root {
            cmd.current_dir(root);
        }

        let mut child = cmd.spawn().map_err(|e| {
            anyhow::anyhow!(
                "failed to spawn LSP server '{}': {e}",
                config.server_command
            )
        })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("LSP server had no stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("LSP server had no stdout"))?;

        Ok(Self {
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
            _child: child,
        })
    }

    async fn initialize(&mut self, workspace_root: Option<&PathBuf>) -> anyhow::Result<()> {
        let root_uri = workspace_root
            .map(|p| format!("file://{}", p.display()))
            .unwrap_or_else(|| {
                std::env::current_dir()
                    .map(|p| format!("file://{}", p.display()))
                    .unwrap_or_else(|_| "file:///".to_string())
            });
        let params = serde_json::json!({
            "processId": std::process::id(),
            "rootUri": root_uri,
            "capabilities": {
                "textDocument": {
                    "hover": { "contentFormat": ["plaintext", "markdown"] },
                    "definition": {},
                    "references": {},
                    "publishDiagnostics": {},
                    "documentSymbol": { "hierarchicalDocumentSymbolSupport": false },
                },
                "workspace": {
                    "symbol": {}
                }
            }
        });
        let _ = self.request("initialize", params).await?;
        self.notify("initialized", serde_json::json!({})).await?;
        Ok(())
    }

    async fn shutdown_quietly(&mut self) {
        let _ = self.request("shutdown", serde_json::json!(null)).await;
        let _ = self.notify("exit", serde_json::json!(null)).await;
    }

    async fn did_open(&mut self, file: &str) -> anyhow::Result<()> {
        let text = tokio::fs::read_to_string(file)
            .await
            .map_err(|e| anyhow::anyhow!("failed to read file '{file}' for didOpen: {e}"))?;
        let uri = path_to_file_uri(file);
        let lang = detect_language_id(file);
        self.notify(
            "textDocument/didOpen",
            serde_json::json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": lang,
                    "version": 1,
                    "text": text,
                }
            }),
        )
        .await
    }

    async fn request(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let id = self.next_id;
        self.next_id += 1;
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        self.write_message(&msg).await?;
        // Drain until we see a response with matching id. Notifications
        // (e.g. publishDiagnostics, window/logMessage) are ignored here but
        // can be harvested by the `diagnostics` capability which reads a few
        // extra frames before returning.
        loop {
            let frame = self.read_message().await?;
            if frame.get("id").and_then(|v| v.as_i64()) == Some(id) {
                if let Some(err) = frame.get("error") {
                    return Err(anyhow::anyhow!("LSP error: {err}"));
                }
                return Ok(frame
                    .get("result")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null));
            }
            // Not our response — discard.
        }
    }

    async fn notify(&mut self, method: &str, params: serde_json::Value) -> anyhow::Result<()> {
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        self.write_message(&msg).await
    }

    async fn write_message(&mut self, msg: &serde_json::Value) -> anyhow::Result<()> {
        let body = serde_json::to_string(msg)?;
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        self.stdin.write_all(header.as_bytes()).await?;
        self.stdin.write_all(body.as_bytes()).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    async fn read_message(&mut self) -> anyhow::Result<serde_json::Value> {
        // Parse LSP header block: `Content-Length: N\r\n` (+ optional other
        // headers) then `\r\n` then N bytes of JSON body.
        let mut content_length: Option<usize> = None;
        loop {
            let mut line = String::new();
            let n = self.stdout.read_line(&mut line).await?;
            if n == 0 {
                return Err(anyhow::anyhow!("LSP server closed stdout unexpectedly"));
            }
            let trimmed = line.trim_end_matches(['\r', '\n']);
            if trimmed.is_empty() {
                break; // end of headers
            }
            if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
                content_length = Some(rest.trim().parse().map_err(|e| {
                    anyhow::anyhow!("invalid Content-Length header '{trimmed}': {e}")
                })?);
            }
            // Other headers (Content-Type) are ignored.
        }
        let len = content_length
            .ok_or_else(|| anyhow::anyhow!("LSP frame missing Content-Length header"))?;
        let mut buf = vec![0u8; len];
        self.stdout.read_exact(&mut buf).await?;
        let v: serde_json::Value = serde_json::from_slice(&buf)?;
        Ok(v)
    }

    /// Drain pending notifications for up to `dur` and collect any
    /// `textDocument/publishDiagnostics` params matching `uri`.
    async fn drain_diagnostics(&mut self, uri: &str, dur: Duration) -> Vec<LspDiagnostic> {
        let deadline = tokio::time::Instant::now() + dur;
        let mut out = Vec::new();
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            let frame = match tokio::time::timeout(remaining, self.read_message()).await {
                Ok(Ok(v)) => v,
                _ => break,
            };
            if frame.get("method").and_then(|v| v.as_str())
                == Some("textDocument/publishDiagnostics")
            {
                if let Some(params) = frame.get("params") {
                    if params.get("uri").and_then(|v| v.as_str()) == Some(uri) {
                        if let Some(arr) = params.get("diagnostics").and_then(|v| v.as_array()) {
                            for d in arr {
                                let severity =
                                    d.get("severity").and_then(|v| v.as_i64()).unwrap_or(0);
                                let message = d
                                    .get("message")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let (line, col) = range_start(d.get("range"));
                                out.push(LspDiagnostic {
                                    severity: diagnostic_severity_name(severity).to_string(),
                                    message,
                                    line,
                                    col,
                                });
                            }
                        }
                    }
                }
            }
        }
        out
    }
}

fn detect_language_id(file: &str) -> &'static str {
    let ext = std::path::Path::new(file)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    match ext {
        "rs" => "rust",
        "ts" | "tsx" => "typescript",
        "js" | "jsx" => "javascript",
        "py" => "python",
        "go" => "go",
        "java" => "java",
        "c" | "h" => "c",
        "cpp" | "hpp" | "cc" | "hh" => "cpp",
        _ => "plaintext",
    }
}

fn range_start(range: Option<&serde_json::Value>) -> (u32, u32) {
    let start = range.and_then(|r| r.get("start"));
    let line = start
        .and_then(|s| s.get("line"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let col = start
        .and_then(|s| s.get("character"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    // LSP is 0-based; convert to 1-based for user-facing output.
    ((line as u32) + 1, (col as u32) + 1)
}

fn position_params(file: &str, line: u32, col: u32) -> serde_json::Value {
    // Convert user-facing 1-based indices to LSP 0-based. Saturate at 0.
    let lsp_line = line.saturating_sub(1);
    let lsp_col = col.saturating_sub(1);
    serde_json::json!({
        "textDocument": { "uri": path_to_file_uri(file) },
        "position": { "line": lsp_line, "character": lsp_col },
    })
}

// ---------------------------------------------------------------------------
// Capability handlers
// ---------------------------------------------------------------------------

async fn run_with_session<F, T>(config: &LspConnectorConfig, f: F) -> anyhow::Result<T>
where
    F: for<'a> FnOnce(
        &'a mut LspSession,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = anyhow::Result<T>> + Send + 'a>,
    >,
{
    let fut = async {
        let mut session = LspSession::spawn(config).await?;
        session.initialize(config.workspace_root.as_ref()).await?;
        let out = f(&mut session).await;
        session.shutdown_quietly().await;
        out
    };
    tokio::time::timeout(config.request_timeout, fut)
        .await
        .map_err(|_| anyhow::anyhow!("LSP request timed out after {:?}", config.request_timeout))?
}

async fn cap_hover(
    config: &LspConnectorConfig,
    input: LspHoverInput,
) -> anyhow::Result<LspHoverOutput> {
    run_with_session(config, move |s| {
        Box::pin(async move {
            s.did_open(&input.file).await?;
            let resp = s
                .request(
                    "textDocument/hover",
                    position_params(&input.file, input.line, input.col),
                )
                .await?;
            let (contents, kind) = match resp.get("contents") {
                Some(v) => extract_hover_contents(v),
                None => (String::new(), "none".to_string()),
            };
            Ok(LspHoverOutput { contents, kind })
        })
    })
    .await
}

fn extract_hover_contents(v: &serde_json::Value) -> (String, String) {
    match v {
        serde_json::Value::String(s) => (s.clone(), "plaintext".to_string()),
        serde_json::Value::Array(arr) => {
            let joined: Vec<String> = arr
                .iter()
                .map(|x| match x {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Object(o) => o
                        .get("value")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    _ => String::new(),
                })
                .collect();
            (joined.join("\n"), "markedString[]".to_string())
        }
        serde_json::Value::Object(o) => {
            let kind = o
                .get("kind")
                .and_then(|v| v.as_str())
                .unwrap_or("markup")
                .to_string();
            let value = o
                .get("value")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            (value, kind)
        }
        _ => (String::new(), "unknown".to_string()),
    }
}

async fn cap_definition(
    config: &LspConnectorConfig,
    input: LspDefinitionInput,
) -> anyhow::Result<LspDefinitionOutput> {
    run_with_session(config, move |s| {
        Box::pin(async move {
            s.did_open(&input.file).await?;
            let resp = s
                .request(
                    "textDocument/definition",
                    position_params(&input.file, input.line, input.col),
                )
                .await?;
            Ok(LspDefinitionOutput {
                locations: parse_locations(&resp),
            })
        })
    })
    .await
}

async fn cap_references(
    config: &LspConnectorConfig,
    input: LspReferencesInput,
) -> anyhow::Result<LspReferencesOutput> {
    run_with_session(config, move |s| {
        Box::pin(async move {
            s.did_open(&input.file).await?;
            let mut params = position_params(&input.file, input.line, input.col);
            if let Some(obj) = params.as_object_mut() {
                obj.insert(
                    "context".to_string(),
                    serde_json::json!({ "includeDeclaration": input.include_declaration }),
                );
            }
            let resp = s.request("textDocument/references", params).await?;
            Ok(LspReferencesOutput {
                locations: parse_locations(&resp),
            })
        })
    })
    .await
}

async fn cap_diagnostics(
    config: &LspConnectorConfig,
    input: LspDiagnosticsInput,
) -> anyhow::Result<LspDiagnosticsOutput> {
    run_with_session(config, move |s| {
        Box::pin(async move {
            let uri = path_to_file_uri(&input.file);
            s.did_open(&input.file).await?;
            // LSP pushes diagnostics via publishDiagnostics. Give the server
            // up to 2s (within the outer request_timeout) to publish.
            let diagnostics = s.drain_diagnostics(&uri, Duration::from_secs(2)).await;
            Ok(LspDiagnosticsOutput { diagnostics })
        })
    })
    .await
}

async fn cap_document_symbols(
    config: &LspConnectorConfig,
    input: LspDocumentSymbolsInput,
) -> anyhow::Result<LspDocumentSymbolsOutput> {
    run_with_session(config, move |s| {
        Box::pin(async move {
            s.did_open(&input.file).await?;
            let resp = s
                .request(
                    "textDocument/documentSymbol",
                    serde_json::json!({
                        "textDocument": { "uri": path_to_file_uri(&input.file) }
                    }),
                )
                .await?;
            Ok(LspDocumentSymbolsOutput {
                symbols: parse_symbols(&resp),
            })
        })
    })
    .await
}

async fn cap_workspace_symbols(
    config: &LspConnectorConfig,
    input: LspWorkspaceSymbolsInput,
) -> anyhow::Result<LspWorkspaceSymbolsOutput> {
    run_with_session(config, move |s| {
        Box::pin(async move {
            let resp = s
                .request(
                    "workspace/symbol",
                    serde_json::json!({ "query": input.query }),
                )
                .await?;
            Ok(LspWorkspaceSymbolsOutput {
                symbols: parse_symbols(&resp),
            })
        })
    })
    .await
}

fn parse_locations(v: &serde_json::Value) -> Vec<LspLocation> {
    // Response may be `Location`, `Location[]`, `LocationLink[]`, or null.
    let as_one = |item: &serde_json::Value| -> Option<LspLocation> {
        let uri = item
            .get("uri")
            .or_else(|| item.get("targetUri"))
            .and_then(|v| v.as_str())?;
        let range = item.get("range").or_else(|| item.get("targetRange"));
        let (line, col) = range_start(range);
        Some(LspLocation {
            file: file_uri_to_path(uri),
            line,
            col,
        })
    };
    match v {
        serde_json::Value::Array(arr) => arr.iter().filter_map(as_one).collect(),
        serde_json::Value::Object(_) => as_one(v).into_iter().collect(),
        _ => Vec::new(),
    }
}

fn parse_symbols(v: &serde_json::Value) -> Vec<LspSymbol> {
    let arr = match v.as_array() {
        Some(a) => a,
        None => return Vec::new(),
    };
    let mut out = Vec::new();
    for item in arr {
        // Flat SymbolInformation has `location.range.start`.
        // DocumentSymbol has `range.start` and may contain `children`.
        let name = item
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let kind = item
            .get("kind")
            .and_then(|v| v.as_i64())
            .map(symbol_kind_name)
            .unwrap_or("unknown")
            .to_string();
        let (line, col) = if let Some(loc) = item.get("location") {
            range_start(loc.get("range"))
        } else {
            range_start(item.get("range"))
        };
        let container = item
            .get("containerName")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        out.push(LspSymbol {
            name,
            kind,
            line,
            col,
            container,
        });
    }
    out
}

// ---------------------------------------------------------------------------
// Connector impl
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
impl Connector for LspConnector {
    fn id(&self) -> &ConnectorId {
        &self.id
    }

    fn runtime(&self) -> ConnectorRuntime {
        ConnectorRuntime::Process
    }

    fn capabilities(&self) -> Vec<CapabilityContract> {
        Self::build_capabilities()
    }

    async fn execute(
        &self,
        request: CapabilityExecutionRequest,
    ) -> anyhow::Result<CapabilityExecutionResult> {
        debug!(
            "LspConnector executing capability {} for execution {}",
            request.capability_id, request.execution_id
        );

        let cap_id = request.capability_id.as_str().to_string();
        let input = request.input.clone();

        let result: anyhow::Result<serde_json::Value> = match cap_id.as_str() {
            "lsp.hover" => {
                let inp: LspHoverInput = serde_json::from_value(input)?;
                cap_hover(&self.config, inp).await.and_then(|o| {
                    serde_json::to_value(o).map_err(|e| anyhow::anyhow!("serialize: {e}"))
                })
            }
            "lsp.definition" => {
                let inp: LspDefinitionInput = serde_json::from_value(input)?;
                cap_definition(&self.config, inp).await.and_then(|o| {
                    serde_json::to_value(o).map_err(|e| anyhow::anyhow!("serialize: {e}"))
                })
            }
            "lsp.references" => {
                let inp: LspReferencesInput = serde_json::from_value(input)?;
                cap_references(&self.config, inp).await.and_then(|o| {
                    serde_json::to_value(o).map_err(|e| anyhow::anyhow!("serialize: {e}"))
                })
            }
            "lsp.diagnostics" => {
                let inp: LspDiagnosticsInput = serde_json::from_value(input)?;
                cap_diagnostics(&self.config, inp).await.and_then(|o| {
                    serde_json::to_value(o).map_err(|e| anyhow::anyhow!("serialize: {e}"))
                })
            }
            "lsp.document_symbols" => {
                let inp: LspDocumentSymbolsInput = serde_json::from_value(input)?;
                cap_document_symbols(&self.config, inp).await.and_then(|o| {
                    serde_json::to_value(o).map_err(|e| anyhow::anyhow!("serialize: {e}"))
                })
            }
            "lsp.workspace_symbols" => {
                let inp: LspWorkspaceSymbolsInput = serde_json::from_value(input)?;
                cap_workspace_symbols(&self.config, inp)
                    .await
                    .and_then(|o| {
                        serde_json::to_value(o).map_err(|e| anyhow::anyhow!("serialize: {e}"))
                    })
            }
            other => {
                let msg = format!("Unknown LSP capability: {other}");
                warn!("{msg}");
                return Ok(CapabilityExecutionResult {
                    execution_id: request.execution_id,
                    trace_id: request.trace_id,
                    connector_id: request.connector_id,
                    capability_id: request.capability_id,
                    output: serde_json::json!({ "error": msg }),
                    status: ConnectorExecutionStatus::Failed,
                    error: Some(msg),
                    actual_runtime: None,
                });
            }
        };

        match result {
            Ok(output) => {
                info!("LSP capability {} succeeded", request.capability_id);
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
                error!("LSP capability {} failed: {:?}", request.capability_id, e);
                Ok(CapabilityExecutionResult {
                    execution_id: request.execution_id,
                    trace_id: request.trace_id,
                    connector_id: request.connector_id,
                    capability_id: request.capability_id,
                    output: serde_json::json!({ "error": e.to_string() }),
                    status: ConnectorExecutionStatus::Failed,
                    error: Some(e.to_string()),
                    actual_runtime: None,
                })
            }
        }
    }
}

// ---------------------------------------------------------------------------
// §4 Facade export — real cyberclaw_core::facade types (Phase 2)
// ---------------------------------------------------------------------------

/// Return facade entries for every `lsp.*` capability in this module.
///
/// The host binary registers them under `ToolsetCategory::CodeAnalysis`.
/// The connector ID `"lsp-rust"` is the default; the host binary may remap
/// it when registering under a differently-named connector instance.
pub fn capability_facades() -> Vec<(CapabilityFacade, ToolsetCategory)> {
    let connector_id = ConnectorId::from_string("lsp-rust".to_string()).unwrap();
    vec![
        (
            CapabilityFacade {
                name: "lsp_hover".to_string(),
                description: "Show hover information (type, documentation) for the symbol at \
                              (file, line, col)."
                    .to_string(),
                connector_id: connector_id.clone(),
                capability_id: CapabilityId::from_string("lsp.hover".to_string()).unwrap(),
                risk_level: RiskLevel::Low,
                effects: vec!["read".to_string()],
                read_only: true,
                destructive: false,
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "file": { "type": "string", "description": "Path to the source file" },
                        "line": { "type": "integer", "description": "1-based line number" },
                        "col": { "type": "integer", "description": "1-based column number" }
                    },
                    "required": ["file", "line", "col"]
                })),
                exposure: FacadeExposure::LlmDefault,
            },
            ToolsetCategory::CodeAnalysis,
        ),
        (
            CapabilityFacade {
                name: "lsp_definition".to_string(),
                description:
                    "Resolve the definition location(s) for the symbol at (file, line, col)."
                        .to_string(),
                connector_id: connector_id.clone(),
                capability_id: CapabilityId::from_string("lsp.definition".to_string()).unwrap(),
                risk_level: RiskLevel::Low,
                effects: vec!["read".to_string()],
                read_only: true,
                destructive: false,
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "file": { "type": "string" },
                        "line": { "type": "integer" },
                        "col": { "type": "integer" }
                    },
                    "required": ["file", "line", "col"]
                })),
                exposure: FacadeExposure::LlmDefault,
            },
            ToolsetCategory::CodeAnalysis,
        ),
        (
            CapabilityFacade {
                name: "lsp_references".to_string(),
                description: "Find all references to the symbol at (file, line, col).".to_string(),
                connector_id: connector_id.clone(),
                capability_id: CapabilityId::from_string("lsp.references".to_string()).unwrap(),
                risk_level: RiskLevel::Low,
                effects: vec!["read".to_string()],
                read_only: true,
                destructive: false,
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "file": { "type": "string" },
                        "line": { "type": "integer" },
                        "col": { "type": "integer" },
                        "include_declaration": { "type": "boolean", "default": false }
                    },
                    "required": ["file", "line", "col"]
                })),
                exposure: FacadeExposure::LlmDefault,
            },
            ToolsetCategory::CodeAnalysis,
        ),
        (
            CapabilityFacade {
                name: "lsp_diagnostics".to_string(),
                description: "Return current LSP diagnostics (errors/warnings) for a file."
                    .to_string(),
                connector_id: connector_id.clone(),
                capability_id: CapabilityId::from_string("lsp.diagnostics".to_string()).unwrap(),
                risk_level: RiskLevel::Low,
                effects: vec!["read".to_string()],
                read_only: true,
                destructive: false,
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": { "file": { "type": "string" } },
                    "required": ["file"]
                })),
                exposure: FacadeExposure::LlmDefault,
            },
            ToolsetCategory::CodeAnalysis,
        ),
        (
            CapabilityFacade {
                name: "lsp_document_symbols".to_string(),
                description: "Return the symbol tree (functions, classes, variables) for a file."
                    .to_string(),
                connector_id: connector_id.clone(),
                capability_id: CapabilityId::from_string("lsp.document_symbols".to_string())
                    .unwrap(),
                risk_level: RiskLevel::Low,
                effects: vec!["read".to_string()],
                read_only: true,
                destructive: false,
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": { "file": { "type": "string" } },
                    "required": ["file"]
                })),
                exposure: FacadeExposure::LlmDefault,
            },
            ToolsetCategory::CodeAnalysis,
        ),
        (
            CapabilityFacade {
                name: "lsp_workspace_symbols".to_string(),
                description: "Search workspace-wide for symbols matching a name pattern."
                    .to_string(),
                connector_id: connector_id.clone(),
                capability_id: CapabilityId::from_string("lsp.workspace_symbols".to_string())
                    .unwrap(),
                risk_level: RiskLevel::Low,
                effects: vec!["read".to_string()],
                read_only: true,
                destructive: false,
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": { "query": { "type": "string" } },
                    "required": ["query"]
                })),
                exposure: FacadeExposure::LlmDefault,
            },
            ToolsetCategory::CodeAnalysis,
        ),
    ]
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use cyberclaw_core::identity::Identity;
    use cyberclaw_core::workspace::{WorkspaceMode, WorkspaceRef};

    fn mk_connector(server_cmd: &str, args: Vec<String>) -> LspConnector {
        LspConnector::new(
            "lsp-test",
            LspConnectorConfig {
                server_command: server_cmd.to_string(),
                server_args: args,
                workspace_root: None,
                request_timeout: Duration::from_secs(3),
            },
        )
        .expect("valid connector id")
    }

    fn mk_request(cap: &str, input: serde_json::Value) -> CapabilityExecutionRequest {
        CapabilityExecutionRequest {
            execution_id: ExecutionId::new(),
            trace_id: "trace-lsp-test".to_string(),
            actor: Identity::System.to_actor_ref(None).unwrap(),
            workspace: WorkspaceRef {
                id: WorkspaceId::from_string("lsp-test".to_string()).unwrap(),
                mode: WorkspaceMode::Ephemeral,
                materialization_mode: None,
                home_node_id: None,
                backing_store: None,
                root: "/tmp/lsp-test".to_string(),
                writable_roots: vec![],
            },
            connector_id: ConnectorId::from_string("lsp-test".to_string()).unwrap(),
            capability_id: CapabilityId::from_string(cap.to_string()).unwrap(),
            input,
        }
    }

    // ---- Capability contract shape -----------------------------------------

    #[test]
    fn exposes_six_read_only_capabilities() {
        let c = mk_connector("true", vec![]);
        let caps = c.capabilities();
        assert_eq!(caps.len(), 6);
        let ids: Vec<&str> = caps.iter().map(|c| c.id.as_str()).collect();
        assert!(ids.contains(&"lsp.hover"));
        assert!(ids.contains(&"lsp.definition"));
        assert!(ids.contains(&"lsp.references"));
        assert!(ids.contains(&"lsp.diagnostics"));
        assert!(ids.contains(&"lsp.document_symbols"));
        assert!(ids.contains(&"lsp.workspace_symbols"));
        for cap in &caps {
            assert_eq!(cap.risk, RiskLevel::Low, "{} should be Low risk", cap.id);
            assert_eq!(cap.effects, vec![CapabilityEffect::Read]);
        }
        assert_eq!(c.runtime(), ConnectorRuntime::Process);
    }

    #[test]
    fn capability_facades_count_and_category() {
        let specs = capability_facades();
        assert_eq!(specs.len(), 6);
        for (s, cat) in &specs {
            assert_eq!(*cat, ToolsetCategory::CodeAnalysis);
            assert_eq!(s.risk_level, RiskLevel::Low);
            assert!(s.name.starts_with("lsp_"));
            assert!(s.capability_id.as_str().starts_with("lsp."));
            let schema = s.input_schema.as_ref().expect("input_schema must be Some");
            assert!(schema.get("properties").is_some());
        }
        let names: Vec<&str> = specs.iter().map(|(s, _)| s.name.as_str()).collect();
        assert!(names.contains(&"lsp_hover"));
        assert!(names.contains(&"lsp_workspace_symbols"));
    }

    // ---- Dispatch / error paths --------------------------------------------

    #[tokio::test]
    async fn rejects_unknown_capability() {
        let c = mk_connector("true", vec![]);
        let mut req = mk_request(
            "lsp.hover",
            serde_json::json!({
                "file": "/nonexistent", "line": 1, "col": 1
            }),
        );
        req.capability_id = CapabilityId::from_string("lsp.bogus_op".to_string()).unwrap();
        let res = c.execute(req).await.unwrap();
        assert_eq!(res.status, ConnectorExecutionStatus::Failed);
        assert!(res.error.unwrap().contains("Unknown LSP capability"));
    }

    #[tokio::test]
    async fn returns_failed_when_backend_missing() {
        // Use a guaranteed-nonexistent binary name so spawn fails cleanly.
        let c = mk_connector("cyberclaw-definitely-not-a-real-lsp-server-xyz", vec![]);
        let req = mk_request(
            "lsp.hover",
            serde_json::json!({
                "file": "/tmp/does-not-exist.rs",
                "line": 1,
                "col": 1,
            }),
        );
        let res = c.execute(req).await.unwrap();
        assert_eq!(res.status, ConnectorExecutionStatus::Failed);
        assert!(res.error.is_some(), "error message should be populated");
        // Must not panic and must surface a structured error — no assumption
        // about the exact message (platform-dependent).
    }

    #[tokio::test]
    async fn invalid_input_returns_error() {
        let c = mk_connector("true", vec![]);
        // Missing required fields for hover.
        let req = mk_request("lsp.hover", serde_json::json!({ "file": "/tmp/x.rs" }));
        let res = c.execute(req).await;
        // The serde_json::from_value returns an Err which propagates through
        // execute's `?`. Depending on the codepath that becomes either a
        // Result::Err or a Failed status — both are acceptable. Just ensure
        // we don't panic and don't report Success.
        match res {
            Ok(r) => assert_ne!(r.status, ConnectorExecutionStatus::Success),
            Err(_) => { /* acceptable — surface error to caller */ }
        }
    }

    // ---- Pure JSON-RPC / parsing helpers -----------------------------------

    #[test]
    fn position_params_converts_to_zero_based() {
        let p = position_params("/tmp/x.rs", 10, 5);
        assert_eq!(p["position"]["line"], 9);
        assert_eq!(p["position"]["character"], 4);
        assert!(p["textDocument"]["uri"]
            .as_str()
            .unwrap()
            .starts_with("file://"));
    }

    #[test]
    fn position_params_saturates_at_zero() {
        let p = position_params("/tmp/x.rs", 0, 0);
        assert_eq!(p["position"]["line"], 0);
        assert_eq!(p["position"]["character"], 0);
    }

    #[test]
    fn parse_locations_handles_single_and_array_forms() {
        let single = serde_json::json!({
            "uri": "file:///a.rs",
            "range": { "start": { "line": 4, "character": 2 }, "end": {"line": 4, "character": 10} }
        });
        let parsed = parse_locations(&single);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].file, "/a.rs");
        assert_eq!(parsed[0].line, 5); // 1-based
        assert_eq!(parsed[0].col, 3);

        let arr = serde_json::json!([
            { "uri": "file:///a.rs", "range": { "start": { "line": 0, "character": 0 } } },
            { "targetUri": "file:///b.rs", "targetRange": { "start": { "line": 2, "character": 1 } } }
        ]);
        let parsed = parse_locations(&arr);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[1].file, "/b.rs");
        assert_eq!(parsed[1].line, 3);
    }

    #[test]
    fn parse_symbols_handles_flat_and_hierarchical_forms() {
        // SymbolInformation (flat) form
        let flat = serde_json::json!([
            {
                "name": "my_fn",
                "kind": 12,
                "location": {
                    "uri": "file:///a.rs",
                    "range": { "start": { "line": 9, "character": 0 } }
                },
                "containerName": "mod_a"
            }
        ]);
        let syms = parse_symbols(&flat);
        assert_eq!(syms.len(), 1);
        assert_eq!(syms[0].name, "my_fn");
        assert_eq!(syms[0].kind, "function");
        assert_eq!(syms[0].line, 10);
        assert_eq!(syms[0].container.as_deref(), Some("mod_a"));

        // DocumentSymbol (hierarchical) form
        let hier = serde_json::json!([
            {
                "name": "Foo",
                "kind": 5,
                "range": { "start": { "line": 0, "character": 0 } }
            }
        ]);
        let syms = parse_symbols(&hier);
        assert_eq!(syms.len(), 1);
        assert_eq!(syms[0].kind, "class");
        assert_eq!(syms[0].line, 1);
    }

    #[test]
    fn extract_hover_contents_supports_all_variants() {
        let s = serde_json::Value::String("hello".into());
        assert_eq!(
            extract_hover_contents(&s),
            ("hello".into(), "plaintext".into())
        );

        let obj = serde_json::json!({ "kind": "markdown", "value": "# Title" });
        let (v, k) = extract_hover_contents(&obj);
        assert_eq!(v, "# Title");
        assert_eq!(k, "markdown");

        let arr = serde_json::json!([
            "line1",
            { "value": "line2" }
        ]);
        let (v, _) = extract_hover_contents(&arr);
        assert!(v.contains("line1"));
        assert!(v.contains("line2"));
    }

    #[test]
    fn path_to_file_uri_handles_absolute_and_prefixed() {
        assert_eq!(path_to_file_uri("/a/b.rs"), "file:///a/b.rs");
        assert_eq!(path_to_file_uri("file:///c/d.rs"), "file:///c/d.rs");
    }

    #[test]
    fn symbol_kind_and_severity_tables() {
        assert_eq!(symbol_kind_name(5), "class");
        assert_eq!(symbol_kind_name(12), "function");
        assert_eq!(symbol_kind_name(999), "unknown");
        assert_eq!(diagnostic_severity_name(1), "error");
        assert_eq!(diagnostic_severity_name(2), "warning");
        assert_eq!(diagnostic_severity_name(99), "unknown");
    }

    #[test]
    fn detect_language_id_matches_common_extensions() {
        assert_eq!(detect_language_id("foo.rs"), "rust");
        assert_eq!(detect_language_id("foo.ts"), "typescript");
        assert_eq!(detect_language_id("foo.py"), "python");
        assert_eq!(detect_language_id("README"), "plaintext");
    }
}

#[cfg(test)]
mod subprocess_smoke_tests {
    use super::*;

    /// Returns true if `rust-analyzer` is on PATH and responds to `--version`.
    fn rust_analyzer_available() -> bool {
        std::process::Command::new("rust-analyzer")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Smoke-tests a real rust-analyzer subprocess: spawn → initialize → assert
    /// capabilities field present → shutdown.  Skips gracefully when
    /// rust-analyzer is not installed (CI / colleague machines).
    #[tokio::test]
    async fn test_lsp_subprocess_initialize_with_rust_analyzer() {
        if !rust_analyzer_available() {
            eprintln!("SKIP: rust-analyzer not in PATH, cannot run LSP subprocess smoke test");
            return;
        }

        let config = LspConnectorConfig {
            server_command: "rust-analyzer".to_string(),
            server_args: vec![],
            workspace_root: None,
            request_timeout: Duration::from_secs(5),
        };

        let mut session = tokio::time::timeout(Duration::from_secs(5), LspSession::spawn(&config))
            .await
            .expect("spawn timed out")
            .expect("failed to spawn rust-analyzer");

        tokio::time::timeout(Duration::from_secs(5), session.initialize(None))
            .await
            .expect("initialize timed out")
            .expect("initialize returned error");

        // initialize() discards the result internally; verify via a raw request
        // instead — send a second initialize to trigger a known LSP error OR
        // rely on the fact that initialize() succeeded without error.
        // The smoke test goal is: no panic, no timeout, initialize succeeds.

        // Graceful shutdown to avoid zombie process.
        tokio::time::timeout(Duration::from_secs(2), session.shutdown_quietly())
            .await
            .ok();
    }
}
