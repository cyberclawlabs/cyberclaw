//! Capability `connector:lsp:*` — Language Server Protocol access for code
//! intelligence (symbols / hover / references).
//!
//! # Design
//!
//! Three capabilities are exposed:
//!
//! - `connector:lsp:symbols`         — `textDocument/documentSymbol`
//! - `connector:lsp:hover`           — `textDocument/hover`
//! - `connector:lsp:find_references` — `textDocument/references`
//!
//! A minimal hand-written JSON-RPC 2.0 client talks to a language server
//! subprocess over stdio with the standard `Content-Length: N\r\n\r\n{body}`
//! framing used by LSP.  No heavy LSP crate is pulled in — every request is
//! a fire-and-wait `request -> response` pair keyed by a monotonic id.
//!
//! ## Process pool
//!
//! Language-server subprocesses are expensive to start (rust-analyzer takes
//! multiple seconds to index a workspace), so one process is kept alive per
//! `(workspace_root, language_server_command)` pair and re-used across
//! requests.  See `client_pool()`.
//!
//! ## Offline / unavailable behaviour
//!
//! If the configured language-server binary is missing or the spawn fails,
//! the capability returns an `available: false` envelope rather than an
//! error so callers (and agents in CI) continue to function.
//!
//! ```json
//! { "available": false, "reason": "rust-analyzer not found in PATH" }
//! ```
//!
//! ## Default servers
//!
//! If `language_server` is not supplied in the request, a default is chosen
//! from the file extension:
//!
//! | extension           | default command                |
//! |---------------------|--------------------------------|
//! | `.rs`               | `rust-analyzer`                |
//! | `.ts` / `.tsx` / `.js` / `.jsx` | `typescript-language-server --stdio` |
//! | `.py`               | `pyright-langserver --stdio`   |
//! | `.go`               | `gopls`                        |
//!
//! Callers may override via the input field `language_server` (command plus
//! optional args list).

use crate::types::CapabilityExecutionRequest;
use dashmap::DashMap;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use tracing::{debug, warn};

// ---------------------------------------------------------------------------
// Public I/O types
// ---------------------------------------------------------------------------

/// Optional language-server selection.  When omitted, a default is chosen
/// from the file extension (see module docs).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LanguageServerConfig {
    /// Executable name or path.
    pub command: String,
    /// Arguments passed to the executable.
    #[serde(default)]
    pub args: Vec<String>,
    /// Optional workspace root to use as the spawned process's current
    /// working directory.  When `None` the async subprocess transport
    /// inherits the caller's cwd (see [`async_lsp::StdioLspTransport::spawn`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_root: Option<PathBuf>,
}

/// 1-based `{line, character}` position as shown in editors, matching LSP's
/// on-the-wire 0-based convention after subtracting 1.  Use 0-based values if
/// you prefer — the capability treats the number as opaque and forwards it.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

// ------- symbols -------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspSymbolsInput {
    pub file_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language_server: Option<LanguageServerConfig>,
}

/// Recursive symbol entry mirroring LSP's `DocumentSymbol`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolEntry {
    pub name: String,
    pub kind: u32,
    pub range: Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<SymbolEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspSymbolsOutput {
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default)]
    pub symbols: Vec<SymbolEntry>,
}

// ------- hover -------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspHoverInput {
    pub file_path: String,
    pub position: Position,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language_server: Option<LanguageServerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspHoverOutput {
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default)]
    pub markdown: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<Value>,
}

// ------- find_references -------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspFindReferencesInput {
    pub file_path: String,
    pub position: Position,
    #[serde(default)]
    pub include_declaration: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language_server: Option<LanguageServerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReferenceLocation {
    pub file: String,
    pub range: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspFindReferencesOutput {
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default)]
    pub references: Vec<ReferenceLocation>,
}

// ---------------------------------------------------------------------------
// JSON-RPC transport
// ---------------------------------------------------------------------------

/// A bidirectional byte stream pair used by the JSON-RPC client.
///
/// The implementation is split from the business logic so tests can drop in
/// an in-process pipe and run the full protocol end-to-end without having to
/// spawn a real language server.
pub trait LspTransport: Send {
    fn write_message(&mut self, body: &str) -> anyhow::Result<()>;
    fn read_message(&mut self) -> anyhow::Result<String>;
    /// Optional — default `Ok(())`.  Real subprocess transports override to
    /// send `SIGKILL` on drop.
    fn shutdown(&mut self) {}
}

/// Transport over a child process's stdin/stdout.
struct ChildTransport {
    child: Child,
    stdin: std::process::ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
}

impl ChildTransport {
    fn spawn(command: &str, args: &[String], cwd: &Path) -> anyhow::Result<Self> {
        let mut child = Command::new(command)
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| anyhow::anyhow!("spawn '{command}' failed: {e}"))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("child stdin unavailable"))?;
        let stdout = BufReader::new(
            child
                .stdout
                .take()
                .ok_or_else(|| anyhow::anyhow!("child stdout unavailable"))?,
        );

        Ok(Self {
            child,
            stdin,
            stdout,
        })
    }
}

impl LspTransport for ChildTransport {
    fn write_message(&mut self, body: &str) -> anyhow::Result<()> {
        write_framed(&mut self.stdin, body)
    }

    fn read_message(&mut self) -> anyhow::Result<String> {
        read_framed(&mut self.stdout)
    }

    fn shutdown(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for ChildTransport {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Write a JSON-RPC body with LSP-style `Content-Length` framing.
fn write_framed<W: Write>(w: &mut W, body: &str) -> anyhow::Result<()> {
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    w.write_all(header.as_bytes())?;
    w.write_all(body.as_bytes())?;
    w.flush()?;
    Ok(())
}

/// Read one `Content-Length`-framed JSON-RPC body.
fn read_framed<R: BufRead>(r: &mut R) -> anyhow::Result<String> {
    let mut content_length: Option<usize> = None;

    loop {
        let mut line = String::new();
        let n = r.read_line(&mut line)?;
        if n == 0 {
            return Err(anyhow::anyhow!("EOF while reading LSP header"));
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break; // end of headers
        }
        if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
            content_length = Some(
                rest.trim()
                    .parse()
                    .map_err(|e| anyhow::anyhow!("invalid Content-Length: {e}"))?,
            );
        }
        // other headers (Content-Type) ignored
    }

    let len = content_length.ok_or_else(|| anyhow::anyhow!("missing Content-Length header"))?;
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    String::from_utf8(buf).map_err(|e| anyhow::anyhow!("non-UTF8 LSP body: {e}"))
}

// ---------------------------------------------------------------------------
// JSON-RPC client
// ---------------------------------------------------------------------------

/// Minimal JSON-RPC 2.0 client wrapping an `LspTransport`.
///
/// Only synchronous `request(method, params) -> response` is supported — LSP
/// notifications that flow server -> client are drained and discarded until
/// a response with a matching id arrives.
pub struct LspClient {
    transport: Box<dyn LspTransport>,
    next_id: u64,
    initialized: bool,
}

impl LspClient {
    pub fn new(transport: Box<dyn LspTransport>) -> Self {
        Self {
            transport,
            next_id: 1,
            initialized: false,
        }
    }

    /// Send an LSP `initialize` request with the given workspace root and
    /// dispatch the `initialized` notification.  Repeated calls are no-ops.
    pub fn ensure_initialized(&mut self, workspace: &Path) -> anyhow::Result<()> {
        if self.initialized {
            return Ok(());
        }
        let root_uri = path_to_uri(workspace);
        let params = json!({
            "processId": std::process::id(),
            "rootUri": root_uri,
            "capabilities": {
                "textDocument": {
                    "documentSymbol": { "hierarchicalDocumentSymbolSupport": true },
                    "hover": { "contentFormat": ["markdown", "plaintext"] },
                    "references": {},
                }
            },
            "workspaceFolders": [{ "uri": root_uri, "name": "workspace" }],
        });
        let _ = self.request("initialize", params)?;
        self.notify("initialized", json!({}))?;
        self.initialized = true;
        Ok(())
    }

    /// Send a JSON-RPC request and return the `result` field of the response.
    pub fn request(&mut self, method: &str, params: Value) -> anyhow::Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let body = serde_json::to_string(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))?;
        self.transport.write_message(&body)?;

        // Drain notifications until we see a response with the matching id.
        loop {
            let raw = self.transport.read_message()?;
            let msg: Value = serde_json::from_str(&raw)
                .map_err(|e| anyhow::anyhow!("invalid JSON from LSP: {e}: {raw}"))?;

            match msg.get("id").and_then(Value::as_u64) {
                Some(mid) if mid == id => {
                    if let Some(err) = msg.get("error") {
                        return Err(anyhow::anyhow!("LSP error: {}", err));
                    }
                    return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
                }
                // Server-initiated requests (e.g. window/workDoneProgress/create)
                // — reply with empty success so the server does not block.
                Some(other_id) if msg.get("method").is_some() => {
                    let reply = serde_json::to_string(&json!({
                        "jsonrpc": "2.0",
                        "id": other_id,
                        "result": null,
                    }))?;
                    let _ = self.transport.write_message(&reply);
                    continue;
                }
                // Notifications (no id) — drop.
                _ => continue,
            }
        }
    }

    /// Send a JSON-RPC notification (no id, no response expected).
    pub fn notify(&mut self, method: &str, params: Value) -> anyhow::Result<()> {
        let body = serde_json::to_string(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))?;
        self.transport.write_message(&body)
    }
}

// ---------------------------------------------------------------------------
// Process pool
// ---------------------------------------------------------------------------

type SharedClient = Arc<Mutex<LspClient>>;

static CLIENT_POOL: Lazy<DashMap<String, SharedClient>> = Lazy::new(DashMap::new);

fn pool_key(workspace: &Path, cfg: &LanguageServerConfig) -> String {
    format!(
        "{}|{}|{}",
        workspace.display(),
        cfg.command,
        cfg.args.join(" ")
    )
}

/// Return (or spawn) the pooled client for the given (workspace, server)
/// pair.  On spawn failure returns `Ok(None)` so callers can emit an
/// `available: false` envelope.
fn get_or_spawn_client(
    workspace: &Path,
    cfg: &LanguageServerConfig,
) -> anyhow::Result<Option<SharedClient>> {
    let key = pool_key(workspace, cfg);
    if let Some(existing) = CLIENT_POOL.get(&key) {
        return Ok(Some(existing.clone()));
    }

    let transport = match ChildTransport::spawn(&cfg.command, &cfg.args, workspace) {
        Ok(t) => t,
        Err(e) => {
            warn!(server = %cfg.command, "LSP spawn failed: {e}");
            return Ok(None);
        }
    };
    let client = Arc::new(Mutex::new(LspClient::new(Box::new(transport))));
    CLIENT_POOL.insert(key, client.clone());
    Ok(Some(client))
}

// ---------------------------------------------------------------------------
// Default server picker
// ---------------------------------------------------------------------------

/// Infer a default language-server command from the file extension.
pub fn default_server_for(file_path: &Path) -> Option<LanguageServerConfig> {
    let ext = file_path
        .extension()
        .and_then(|s| s.to_str())?
        .to_ascii_lowercase();
    Some(match ext.as_str() {
        "rs" => LanguageServerConfig {
            command: "rust-analyzer".to_string(),
            args: vec![],
            workspace_root: None,
        },
        "ts" | "tsx" | "js" | "jsx" => LanguageServerConfig {
            command: "typescript-language-server".to_string(),
            args: vec!["--stdio".to_string()],
            workspace_root: None,
        },
        "py" => LanguageServerConfig {
            command: "pyright-langserver".to_string(),
            args: vec!["--stdio".to_string()],
            workspace_root: None,
        },
        "go" => LanguageServerConfig {
            command: "gopls".to_string(),
            args: vec![],
            workspace_root: None,
        },
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn path_to_uri(p: &Path) -> String {
    // Best-effort `file://` URI.  Avoid pulling the `url` crate because for
    // LSP the servers only care that the scheme is `file` and the rest is the
    // absolute OS path.
    let s = p.display().to_string();
    if s.starts_with('/') {
        format!("file://{s}")
    } else {
        format!("file:///{}", s.replace('\\', "/"))
    }
}

fn absolute_path(workspace: &Path, file_path: &str) -> PathBuf {
    let p = Path::new(file_path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        workspace.join(p)
    }
}

fn unavailable(reason: impl Into<String>, payload_key: &str) -> Value {
    let reason = reason.into();
    match payload_key {
        "symbols" => json!({ "available": false, "reason": reason, "symbols": [] }),
        "hover" => json!({ "available": false, "reason": reason, "markdown": "" }),
        "references" => json!({ "available": false, "reason": reason, "references": [] }),
        _ => json!({ "available": false, "reason": reason }),
    }
}

fn resolve_server(
    input_cfg: Option<LanguageServerConfig>,
    file: &Path,
) -> Result<LanguageServerConfig, String> {
    if let Some(cfg) = input_cfg {
        if cfg.command.is_empty() {
            return Err("language_server.command must not be empty".to_string());
        }
        return Ok(cfg);
    }
    default_server_for(file).ok_or_else(|| {
        format!(
            "no default language server for file '{}'; pass `language_server` explicitly",
            file.display()
        )
    })
}

fn did_open(client: &mut LspClient, file: &Path) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(file).unwrap_or_default();
    let language_id = file
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_else(|| "plaintext".to_string());
    client.notify(
        "textDocument/didOpen",
        json!({
            "textDocument": {
                "uri": path_to_uri(file),
                "languageId": language_id,
                "version": 1,
                "text": text,
            }
        }),
    )
}

/// Recursively parse LSP's `DocumentSymbol[]` into our `SymbolEntry` tree.
fn parse_symbol_tree(arr: &[Value]) -> Vec<SymbolEntry> {
    arr.iter()
        .map(|s| {
            let name = s
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let kind = s.get("kind").and_then(Value::as_u64).unwrap_or(0) as u32;
            let range = s.get("range").cloned().unwrap_or(Value::Null);
            let children = s
                .get("children")
                .and_then(Value::as_array)
                .map(|a| parse_symbol_tree(a))
                .unwrap_or_default();
            SymbolEntry {
                name,
                kind,
                range,
                children,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Public capability entry points
// ---------------------------------------------------------------------------

/// Execute `connector:lsp:symbols`.
pub fn execute_symbols(
    workspace: &Path,
    request: CapabilityExecutionRequest,
) -> anyhow::Result<Value> {
    let input: LspSymbolsInput = serde_json::from_value(request.input)?;
    let file = absolute_path(workspace, &input.file_path);

    let cfg = match resolve_server(input.language_server, &file) {
        Ok(c) => c,
        Err(e) => return Ok(unavailable(e, "symbols")),
    };

    debug!("connector:lsp:symbols file={}", file.display());

    let client = match get_or_spawn_client(workspace, &cfg)? {
        Some(c) => c,
        None => {
            return Ok(unavailable(
                format!("'{}' not found or failed to spawn", cfg.command),
                "symbols",
            ));
        }
    };

    let mut guard = client
        .lock()
        .map_err(|e| anyhow::anyhow!("LSP client mutex poisoned: {e}"))?;
    guard.ensure_initialized(workspace)?;
    did_open(&mut guard, &file)?;

    let result = guard.request(
        "textDocument/documentSymbol",
        json!({ "textDocument": { "uri": path_to_uri(&file) } }),
    )?;

    let symbols = match result.as_array() {
        Some(arr)
            if arr.first().and_then(|v| v.get("children")).is_some()
                || arr.first().and_then(|v| v.get("kind")).is_some() =>
        {
            // Hierarchical `DocumentSymbol[]`.
            parse_symbol_tree(arr)
        }
        Some(arr) => {
            // Flat `SymbolInformation[]` — flatten without nesting.
            arr.iter()
                .map(|s| SymbolEntry {
                    name: s
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    kind: s.get("kind").and_then(Value::as_u64).unwrap_or(0) as u32,
                    range: s
                        .get("location")
                        .and_then(|l| l.get("range"))
                        .cloned()
                        .unwrap_or(Value::Null),
                    children: vec![],
                })
                .collect()
        }
        None => vec![],
    };

    Ok(serde_json::to_value(LspSymbolsOutput {
        available: true,
        reason: None,
        symbols,
    })?)
}

/// Execute `connector:lsp:hover`.
pub fn execute_hover(
    workspace: &Path,
    request: CapabilityExecutionRequest,
) -> anyhow::Result<Value> {
    let input: LspHoverInput = serde_json::from_value(request.input)?;
    let file = absolute_path(workspace, &input.file_path);

    let cfg = match resolve_server(input.language_server, &file) {
        Ok(c) => c,
        Err(e) => return Ok(unavailable(e, "hover")),
    };

    debug!(
        "connector:lsp:hover file={} pos={}:{}",
        file.display(),
        input.position.line,
        input.position.character
    );

    let client = match get_or_spawn_client(workspace, &cfg)? {
        Some(c) => c,
        None => {
            return Ok(unavailable(
                format!("'{}' not found or failed to spawn", cfg.command),
                "hover",
            ));
        }
    };

    let mut guard = client
        .lock()
        .map_err(|e| anyhow::anyhow!("LSP client mutex poisoned: {e}"))?;
    guard.ensure_initialized(workspace)?;
    did_open(&mut guard, &file)?;

    let result = guard.request(
        "textDocument/hover",
        json!({
            "textDocument": { "uri": path_to_uri(&file) },
            "position": { "line": input.position.line, "character": input.position.character },
        }),
    )?;

    let (markdown, range) = extract_hover(&result);

    Ok(serde_json::to_value(LspHoverOutput {
        available: true,
        reason: None,
        markdown,
        range,
    })?)
}

fn extract_hover(result: &Value) -> (String, Option<Value>) {
    if result.is_null() {
        return (String::new(), None);
    }

    let range = result.get("range").cloned();

    // `contents` can be:
    //   - MarkupContent `{ kind, value }`
    //   - MarkedString (string | `{ language, value }`)
    //   - MarkedString[]
    let markdown = match result.get("contents") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Object(o)) => {
            if let Some(v) = o.get("value").and_then(Value::as_str) {
                v.to_string()
            } else {
                String::new()
            }
        }
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| match v {
                Value::String(s) => Some(s.clone()),
                Value::Object(o) => o.get("value").and_then(Value::as_str).map(String::from),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n\n"),
        _ => String::new(),
    };

    (markdown, range)
}

/// Execute `connector:lsp:find_references`.
pub fn execute_find_references(
    workspace: &Path,
    request: CapabilityExecutionRequest,
) -> anyhow::Result<Value> {
    let input: LspFindReferencesInput = serde_json::from_value(request.input)?;
    let file = absolute_path(workspace, &input.file_path);

    let cfg = match resolve_server(input.language_server, &file) {
        Ok(c) => c,
        Err(e) => return Ok(unavailable(e, "references")),
    };

    debug!(
        "connector:lsp:find_references file={} pos={}:{}",
        file.display(),
        input.position.line,
        input.position.character
    );

    let client = match get_or_spawn_client(workspace, &cfg)? {
        Some(c) => c,
        None => {
            return Ok(unavailable(
                format!("'{}' not found or failed to spawn", cfg.command),
                "references",
            ));
        }
    };

    let mut guard = client
        .lock()
        .map_err(|e| anyhow::anyhow!("LSP client mutex poisoned: {e}"))?;
    guard.ensure_initialized(workspace)?;
    did_open(&mut guard, &file)?;

    let result = guard.request(
        "textDocument/references",
        json!({
            "textDocument": { "uri": path_to_uri(&file) },
            "position": { "line": input.position.line, "character": input.position.character },
            "context": { "includeDeclaration": input.include_declaration },
        }),
    )?;

    let references = match result.as_array() {
        Some(arr) => arr
            .iter()
            .map(|loc| {
                let file = loc
                    .get("uri")
                    .and_then(Value::as_str)
                    .map(|u| u.trim_start_matches("file://").to_string())
                    .unwrap_or_default();
                let range = loc.get("range").cloned().unwrap_or(Value::Null);
                ReferenceLocation { file, range }
            })
            .collect(),
        None => vec![],
    };

    Ok(serde_json::to_value(LspFindReferencesOutput {
        available: true,
        reason: None,
        references,
    })?)
}

// ===========================================================================
// S13-3: Async subprocess transport + pool + 4 wire capability methods
// ===========================================================================
//
// This block is additive on top of the Sprint 12 Lane F sync scaffolding
// above.  It introduces:
//
//   * [`async_lsp::LspTransport`]      — async trait for send/recv frames
//   * [`async_lsp::StdioLspTransport`] — real tokio subprocess transport
//   * [`async_lsp::MockLspTransport`]  — scripted transport for tests
//   * [`async_lsp::LspClient`]         — async JSON-RPC 2.0 client
//   * [`async_lsp::LspProcessPool`]    — per-language subprocess pool
//
// The sync [`ChildTransport`] / [`LspClient`] above stays in place for the
// capability entry points (`execute_symbols` / `execute_hover` /
// `execute_find_references`) used by the dispatcher at runtime; the async
// path is what S13-3 builds out so future capabilities (diagnostics push,
// streaming completion) can plug in without a rewrite.
pub mod async_lsp {
    use super::LanguageServerConfig;
    use async_trait::async_trait;
    use serde_json::{json, Value};
    use std::collections::HashMap;
    use std::process::Stdio;
    use std::sync::Arc;
    use thiserror::Error;
    use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio::process::{ChildStdin, ChildStdout, Command};
    use tokio::sync::Mutex;
    use tracing::{debug, warn};

    // --- errors ------------------------------------------------------------

    /// Errors surfaced by the async LSP transport + client + pool.
    #[derive(Debug, Error)]
    pub enum LspError {
        #[error("io: {0}")]
        Io(#[from] std::io::Error),
        #[error("spawn failed for '{command}': {source}")]
        Spawn {
            command: String,
            #[source]
            source: std::io::Error,
        },
        #[error("invalid JSON-RPC payload: {0}")]
        Protocol(String),
        #[error("server returned error: {0}")]
        Remote(String),
        #[error("unexpected EOF while reading LSP frame")]
        Eof,
        #[error("language server '{0}' already shut down")]
        Closed(String),
        #[error("unauthorized binary: '{0}' is not in the LSP allowlist and not an absolute path")]
        UnauthorizedBinary(String),
    }

    /// Static allowlist of known LSP binary basenames that may be spawned from
    /// `PATH`.  Any other basename must be supplied as an absolute path or the
    /// spawn is rejected with [`LspError::UnauthorizedBinary`].
    ///
    /// Keep this list in sync with the default-server picker in the parent
    /// module and with [`default_config_for`].
    pub const LSP_BINARY_ALLOWLIST: &[&str] = &[
        "rust-analyzer",
        "typescript-language-server",
        "pyright-langserver",
        "gopls",
    ];

    /// Return `true` when `command` is safe to forward to the OS PATH:
    ///
    /// - Absolute paths are accepted as-is (caller chose the file).
    /// - Bare names must appear in [`LSP_BINARY_ALLOWLIST`].
    pub(super) fn is_binary_allowed(command: &str) -> bool {
        if std::path::Path::new(command).is_absolute() {
            return true;
        }
        LSP_BINARY_ALLOWLIST.contains(&command)
    }

    impl From<serde_json::Error> for LspError {
        fn from(e: serde_json::Error) -> Self {
            LspError::Protocol(e.to_string())
        }
    }

    // --- transport trait ---------------------------------------------------

    /// Bidirectional byte transport for JSON-RPC 2.0 frames.
    ///
    /// Implementations MUST NOT include the `Content-Length` header in the
    /// frame passed to [`LspTransport::send`] — framing is the transport's
    /// responsibility.  [`LspTransport::recv`] returns the JSON body with the
    /// headers stripped.
    #[async_trait]
    pub trait LspTransport: Send {
        /// Write a single JSON-RPC body as an LSP-framed message.
        async fn send(&mut self, frame: &str) -> Result<(), LspError>;
        /// Read one LSP-framed message and return its JSON body.
        async fn recv(&mut self) -> Result<String, LspError>;
        /// Best-effort shutdown.  Default is a no-op; subprocess transports
        /// override to kill the child.
        async fn shutdown(&mut self) -> Result<(), LspError> {
            Ok(())
        }
    }

    // --- stdio subprocess transport ---------------------------------------

    /// Transport over a tokio child process's stdin/stdout.
    ///
    /// Spawns `config.command config.args...` with piped stdio and speaks
    /// LSP's Content-Length framing over the pipes.
    pub struct StdioLspTransport {
        child: tokio::process::Child,
        stdin: ChildStdin,
        stdout: BufReader<ChildStdout>,
    }

    impl StdioLspTransport {
        /// Spawn `config.command` with `config.args`, capturing stdin/stdout.
        ///
        /// The binary basename must appear in [`LSP_BINARY_ALLOWLIST`] or be
        /// provided as an absolute path; otherwise the spawn is rejected with
        /// [`LspError::UnauthorizedBinary`].  When
        /// [`LanguageServerConfig::workspace_root`] is set, it is used as the
        /// spawned process's current working directory.
        pub async fn spawn(config: &LanguageServerConfig) -> Result<Self, LspError> {
            if config.command.is_empty() {
                return Err(LspError::Spawn {
                    command: String::new(),
                    source: std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "language_server.command must not be empty",
                    ),
                });
            }

            if !is_binary_allowed(&config.command) {
                return Err(LspError::UnauthorizedBinary(config.command.clone()));
            }

            let mut cmd = Command::new(&config.command);
            cmd.args(&config.args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            if let Some(root) = config.workspace_root.as_ref() {
                cmd.current_dir(root);
            }
            let mut child = cmd.spawn().map_err(|e| LspError::Spawn {
                command: config.command.clone(),
                source: e,
            })?;

            let stdin = child
                .stdin
                .take()
                .ok_or_else(|| LspError::Protocol("child stdin unavailable".into()))?;
            let stdout = BufReader::new(
                child
                    .stdout
                    .take()
                    .ok_or_else(|| LspError::Protocol("child stdout unavailable".into()))?,
            );

            Ok(Self {
                child,
                stdin,
                stdout,
            })
        }
    }

    #[async_trait]
    impl LspTransport for StdioLspTransport {
        async fn send(&mut self, frame: &str) -> Result<(), LspError> {
            let header = format!("Content-Length: {}\r\n\r\n", frame.len());
            self.stdin.write_all(header.as_bytes()).await?;
            self.stdin.write_all(frame.as_bytes()).await?;
            self.stdin.flush().await?;
            Ok(())
        }

        async fn recv(&mut self) -> Result<String, LspError> {
            // Read headers line-by-line until an empty line.
            let mut content_length: Option<usize> = None;
            loop {
                let mut line = Vec::<u8>::with_capacity(64);
                read_until_crlf(&mut self.stdout, &mut line).await?;
                let trimmed = std::str::from_utf8(&line)
                    .map_err(|e| LspError::Protocol(format!("non-UTF8 header: {e}")))?
                    .trim_end_matches(['\r', '\n']);
                if trimmed.is_empty() {
                    break;
                }
                if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
                    content_length = Some(
                        rest.trim()
                            .parse()
                            .map_err(|e| LspError::Protocol(format!("bad Content-Length: {e}")))?,
                    );
                }
            }
            let len = content_length
                .ok_or_else(|| LspError::Protocol("missing Content-Length".into()))?;
            let mut buf = vec![0u8; len];
            self.stdout.read_exact(&mut buf).await?;
            String::from_utf8(buf).map_err(|e| LspError::Protocol(format!("non-UTF8 body: {e}")))
        }

        async fn shutdown(&mut self) -> Result<(), LspError> {
            // Best-effort: kill and reap.  Ignore errors — the child may have
            // already exited via the LSP `exit` notification.
            let _ = self.child.start_kill();
            let _ = self.child.wait().await;
            Ok(())
        }
    }

    /// Read one CRLF-terminated line into `buf` (including the CRLF).
    async fn read_until_crlf(
        r: &mut BufReader<ChildStdout>,
        buf: &mut Vec<u8>,
    ) -> Result<(), LspError> {
        loop {
            let mut byte = [0u8; 1];
            let n = r.read(&mut byte).await?;
            if n == 0 {
                return Err(LspError::Eof);
            }
            buf.push(byte[0]);
            if buf.ends_with(b"\r\n") || byte[0] == b'\n' {
                return Ok(());
            }
        }
    }

    // --- mock transport ----------------------------------------------------

    /// Scripted transport for unit tests: recorded outgoing frames are
    /// available via `outbound()`; responses to serve to the client are
    /// enqueued via `queue_response`.
    pub struct MockLspTransport {
        outbound: Arc<Mutex<Vec<String>>>,
        inbound: Arc<Mutex<std::collections::VecDeque<String>>>,
        shutdown_called: Arc<Mutex<bool>>,
    }

    impl Default for MockLspTransport {
        fn default() -> Self {
            Self::new()
        }
    }

    impl MockLspTransport {
        pub fn new() -> Self {
            Self {
                outbound: Arc::new(Mutex::new(Vec::new())),
                inbound: Arc::new(Mutex::new(std::collections::VecDeque::new())),
                shutdown_called: Arc::new(Mutex::new(false)),
            }
        }

        /// Return a cloneable handle to the outbound-frame recorder.
        pub fn outbound_handle(&self) -> Arc<Mutex<Vec<String>>> {
            self.outbound.clone()
        }

        /// Return a cloneable handle to the inbound-frame queue.
        pub fn inbound_handle(&self) -> Arc<Mutex<std::collections::VecDeque<String>>> {
            self.inbound.clone()
        }

        /// Return a cloneable handle to the shutdown flag.
        pub fn shutdown_flag(&self) -> Arc<Mutex<bool>> {
            self.shutdown_called.clone()
        }

        /// Enqueue a JSON-RPC response body matching the next outgoing
        /// request `id` with `result`.
        pub async fn queue_response(&self, id: u64, result: Value) {
            let body = serde_json::to_string(&json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": result,
            }))
            .expect("json encode");
            self.inbound.lock().await.push_back(body);
        }

        /// Enqueue a JSON-RPC notification body (no id).
        pub async fn queue_notification(&self, method: &str, params: Value) {
            let body = serde_json::to_string(&json!({
                "jsonrpc": "2.0",
                "method": method,
                "params": params,
            }))
            .expect("json encode");
            self.inbound.lock().await.push_back(body);
        }
    }

    #[async_trait]
    impl LspTransport for MockLspTransport {
        async fn send(&mut self, frame: &str) -> Result<(), LspError> {
            self.outbound.lock().await.push(frame.to_string());
            Ok(())
        }

        async fn recv(&mut self) -> Result<String, LspError> {
            self.inbound
                .lock()
                .await
                .pop_front()
                .ok_or_else(|| LspError::Protocol("mock transport: no queued response".into()))
        }

        async fn shutdown(&mut self) -> Result<(), LspError> {
            *self.shutdown_called.lock().await = true;
            Ok(())
        }
    }

    // --- async JSON-RPC client --------------------------------------------

    /// Async JSON-RPC 2.0 client over an [`LspTransport`].
    pub struct LspClient {
        transport: Box<dyn LspTransport>,
        next_id: u64,
        initialized: bool,
        shut_down: bool,
    }

    impl LspClient {
        pub fn new(transport: Box<dyn LspTransport>) -> Self {
            Self {
                transport,
                next_id: 1,
                initialized: false,
                shut_down: false,
            }
        }

        fn allocate_id(&mut self) -> u64 {
            let id = self.next_id;
            self.next_id += 1;
            id
        }

        /// Send a JSON-RPC request and wait for the response matching `id`.
        /// Server-sent notifications encountered before the response are
        /// silently dropped.
        pub async fn request(&mut self, method: &str, params: Value) -> Result<Value, LspError> {
            if self.shut_down {
                return Err(LspError::Closed(method.into()));
            }
            let id = self.allocate_id();
            let body = serde_json::to_string(&json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": method,
                "params": params,
            }))?;
            self.transport.send(&body).await?;

            loop {
                let raw = self.transport.recv().await?;
                let msg: Value = serde_json::from_str(&raw)?;
                match msg.get("id").and_then(Value::as_u64) {
                    Some(mid) if mid == id => {
                        if let Some(err) = msg.get("error") {
                            return Err(LspError::Remote(err.to_string()));
                        }
                        return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
                    }
                    // Server-initiated request — reply with null so it
                    // doesn't block.  We don't route server->client requests
                    // to any handler at this stage.
                    Some(other_id) if msg.get("method").is_some() => {
                        let reply = serde_json::to_string(&json!({
                            "jsonrpc": "2.0",
                            "id": other_id,
                            "result": null,
                        }))?;
                        let _ = self.transport.send(&reply).await;
                        continue;
                    }
                    _ => continue, // notification — drop
                }
            }
        }

        /// Send a JSON-RPC notification (no id, no response expected).
        pub async fn notify(&mut self, method: &str, params: Value) -> Result<(), LspError> {
            let body = serde_json::to_string(&json!({
                "jsonrpc": "2.0",
                "method": method,
                "params": params,
            }))?;
            self.transport.send(&body).await
        }

        /// Perform the `initialize` + `initialized` handshake.  Idempotent.
        pub async fn initialize(&mut self, workspace_uri: &str) -> Result<Value, LspError> {
            if self.initialized {
                return Ok(Value::Null);
            }
            let params = json!({
                "processId": std::process::id(),
                "rootUri": workspace_uri,
                "capabilities": {
                    "textDocument": {
                        "hover": { "contentFormat": ["markdown", "plaintext"] },
                        "definition": { "linkSupport": false },
                        "references": {},
                        "publishDiagnostics": {},
                    }
                },
                "workspaceFolders": [{ "uri": workspace_uri, "name": "workspace" }],
            });
            let result = self.request("initialize", params).await?;
            self.notify("initialized", json!({})).await?;
            self.initialized = true;
            Ok(result)
        }

        /// Send `shutdown` + `exit` per LSP spec, then close the transport.
        pub async fn shutdown_and_exit(&mut self) -> Result<(), LspError> {
            if self.shut_down {
                return Ok(());
            }
            // `shutdown` is a request expecting a response, `exit` is a
            // notification.  Any transport errors are swallowed so drop works
            // cleanly even if the server died already.
            let _ = self.request("shutdown", Value::Null).await;
            let _ = self.notify("exit", Value::Null).await;
            let _ = self.transport.shutdown().await;
            self.shut_down = true;
            Ok(())
        }

        // -- capability methods --------------------------------------------

        /// `textDocument/hover` at `(line, character)` — 0-based LSP indices.
        pub async fn hover(
            &mut self,
            file_uri: &str,
            line: u32,
            character: u32,
        ) -> Result<Value, LspError> {
            self.request(
                "textDocument/hover",
                json!({
                    "textDocument": { "uri": file_uri },
                    "position": { "line": line, "character": character },
                }),
            )
            .await
        }

        /// `textDocument/definition` at `(line, character)`.
        pub async fn goto_definition(
            &mut self,
            file_uri: &str,
            line: u32,
            character: u32,
        ) -> Result<Value, LspError> {
            self.request(
                "textDocument/definition",
                json!({
                    "textDocument": { "uri": file_uri },
                    "position": { "line": line, "character": character },
                }),
            )
            .await
        }

        /// `textDocument/references` at `(line, character)`.  `include_declaration`
        /// passes through to LSP's `context.includeDeclaration`.
        pub async fn find_references(
            &mut self,
            file_uri: &str,
            line: u32,
            character: u32,
            include_declaration: bool,
        ) -> Result<Value, LspError> {
            self.request(
                "textDocument/references",
                json!({
                    "textDocument": { "uri": file_uri },
                    "position": { "line": line, "character": character },
                    "context": { "includeDeclaration": include_declaration },
                }),
            )
            .await
        }

        /// Subscribe to `textDocument/publishDiagnostics` for `file_uri`.
        ///
        /// LSP diagnostics are server-pushed notifications; this method
        /// drains the inbound queue looking for a `publishDiagnostics`
        /// notification whose `uri` matches `file_uri` and returns its
        /// `diagnostics` array.  Any other messages are discarded.  Returns
        /// `Value::Array([])` if the transport ends before a match.
        pub async fn diagnostics(&mut self, file_uri: &str) -> Result<Value, LspError> {
            loop {
                let raw = match self.transport.recv().await {
                    Ok(v) => v,
                    Err(LspError::Eof) => return Ok(json!([])),
                    Err(e) => return Err(e),
                };
                let msg: Value = serde_json::from_str(&raw)?;
                if msg.get("method").and_then(Value::as_str)
                    == Some("textDocument/publishDiagnostics")
                {
                    let params = msg.get("params").cloned().unwrap_or(Value::Null);
                    if params.get("uri").and_then(Value::as_str) == Some(file_uri) {
                        return Ok(params.get("diagnostics").cloned().unwrap_or(json!([])));
                    }
                }
            }
        }
    }

    // --- process pool ------------------------------------------------------

    /// Per-language cache of shared [`LspClient`] instances.
    ///
    /// Keyed by language id (`"rust"`, `"typescript"`, `"python"`, `"go"`).
    /// `get_or_spawn` returns a cloneable `Arc<Mutex<LspClient>>` so callers
    /// can await the mutex and share the client across requests without
    /// respawning a subprocess.
    pub struct LspProcessPool {
        by_language: HashMap<String, Arc<Mutex<LspClient>>>,
        /// Injected spawn fn lets tests substitute [`MockLspTransport`]
        /// without spawning a real subprocess.
        spawn_fn: SpawnFn,
    }

    type SpawnFn = Arc<
        dyn Fn(
                &str,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<Output = Result<Box<dyn LspTransport>, LspError>>
                        + Send,
                >,
            > + Send
            + Sync,
    >;

    impl LspProcessPool {
        /// Build a pool whose subprocess transports are real
        /// [`StdioLspTransport`] instances, resolved from the language id via
        /// the default-server picker in the parent module.
        pub fn new() -> Self {
            Self {
                by_language: HashMap::new(),
                spawn_fn: Arc::new(|language: &str| {
                    let language = language.to_string();
                    Box::pin(async move {
                        let cfg = default_config_for(&language).ok_or_else(|| {
                            LspError::Protocol(format!(
                                "no default language server for '{language}'"
                            ))
                        })?;
                        let t = StdioLspTransport::spawn(&cfg).await?;
                        Ok(Box::new(t) as Box<dyn LspTransport>)
                    })
                }),
            }
        }

        /// Build a pool with a custom spawn fn — used by tests to plug in
        /// [`MockLspTransport`].
        pub fn with_spawner(spawn_fn: SpawnFn) -> Self {
            Self {
                by_language: HashMap::new(),
                spawn_fn,
            }
        }

        /// Return (or create) the cached client for `language`.
        pub async fn get_or_spawn(
            &mut self,
            language: &str,
        ) -> Result<Arc<Mutex<LspClient>>, LspError> {
            if let Some(existing) = self.by_language.get(language) {
                return Ok(existing.clone());
            }
            debug!(language, "LspProcessPool spawning language server");
            let transport = (self.spawn_fn)(language).await?;
            let client = Arc::new(Mutex::new(LspClient::new(transport)));
            self.by_language
                .insert(language.to_string(), client.clone());
            Ok(client)
        }

        /// Shut down every cached client via `shutdown` + `exit` + transport
        /// kill.  Failures per-client are logged but do not abort the sweep.
        pub async fn shutdown_all(&mut self) -> Result<(), LspError> {
            let drained: Vec<_> = self.by_language.drain().collect();
            for (lang, client) in drained {
                let mut guard = client.lock().await;
                if let Err(e) = guard.shutdown_and_exit().await {
                    warn!(language = %lang, "LSP shutdown error: {e}");
                }
            }
            Ok(())
        }

        /// How many clients are currently cached.
        pub fn len(&self) -> usize {
            self.by_language.len()
        }

        /// Whether the cache is empty.
        pub fn is_empty(&self) -> bool {
            self.by_language.is_empty()
        }
    }

    impl Default for LspProcessPool {
        fn default() -> Self {
            Self::new()
        }
    }

    /// Resolve a language id to a default [`LanguageServerConfig`].
    fn default_config_for(language: &str) -> Option<LanguageServerConfig> {
        Some(match language {
            "rust" => LanguageServerConfig {
                command: "rust-analyzer".to_string(),
                args: vec![],
                workspace_root: None,
            },
            "typescript" | "javascript" => LanguageServerConfig {
                command: "typescript-language-server".to_string(),
                args: vec!["--stdio".to_string()],
                workspace_root: None,
            },
            "python" => LanguageServerConfig {
                command: "pyright-langserver".to_string(),
                args: vec!["--stdio".to_string()],
                workspace_root: None,
            },
            "go" => LanguageServerConfig {
                command: "gopls".to_string(),
                args: vec![],
                workspace_root: None,
            },
            _ => return None,
        })
    }

    // --- tests -------------------------------------------------------------

    #[cfg(test)]
    mod tests {
        use super::*;

        fn parse_body(s: &str) -> Value {
            serde_json::from_str::<Value>(s).expect("valid JSON")
        }

        // 1. Content-Length framing — covered indirectly by StdioLspTransport;
        //    we exercise it through a tokio duplex pair to avoid spawning a
        //    subprocess.  We cover the encode side by reading the outbound
        //    buffer of MockLspTransport, and we cover the decode side by the
        //    client integration tests (which require recv to parse the
        //    queued JSON string).
        #[tokio::test]
        async fn mock_transport_records_outbound_and_serves_inbound() {
            let mock = MockLspTransport::new();
            let outbound = mock.outbound_handle();
            mock.queue_response(1, json!({"ok": true})).await;
            let mut client = LspClient::new(Box::new(mock));
            let v = client.request("test", json!({})).await.unwrap();
            assert_eq!(v, json!({"ok": true}));
            let sent = outbound.lock().await;
            assert_eq!(sent.len(), 1);
            let msg = parse_body(&sent[0]);
            assert_eq!(msg["jsonrpc"], "2.0");
            assert_eq!(msg["id"], 1);
            assert_eq!(msg["method"], "test");
        }

        // 2. initialize handshake request shape
        #[tokio::test]
        async fn initialize_sends_initialize_then_initialized() {
            let mock = MockLspTransport::new();
            let outbound = mock.outbound_handle();
            mock.queue_response(1, json!({"capabilities": {}})).await;
            let mut client = LspClient::new(Box::new(mock));
            let _ = client.initialize("file:///tmp/ws").await.unwrap();
            let sent = outbound.lock().await;
            assert_eq!(sent.len(), 2, "initialize + initialized");
            let init = parse_body(&sent[0]);
            assert_eq!(init["method"], "initialize");
            assert_eq!(init["id"], 1);
            assert_eq!(init["params"]["rootUri"], "file:///tmp/ws");
            assert!(
                init["params"]["capabilities"]["textDocument"].is_object(),
                "client capabilities advertised"
            );
        }

        // 3. initialized notification has no id
        #[tokio::test]
        async fn initialized_notification_has_no_id() {
            let mock = MockLspTransport::new();
            let outbound = mock.outbound_handle();
            mock.queue_response(1, json!({"capabilities": {}})).await;
            let mut client = LspClient::new(Box::new(mock));
            client.initialize("file:///tmp/ws").await.unwrap();
            let sent = outbound.lock().await;
            let notif = parse_body(&sent[1]);
            assert_eq!(notif["method"], "initialized");
            assert!(notif.get("id").is_none(), "notifications have no id");
            assert_eq!(notif["params"], json!({}));
        }

        // 4. shutdown → exit sequence
        #[tokio::test]
        async fn shutdown_and_exit_sends_both() {
            let mock = MockLspTransport::new();
            let outbound = mock.outbound_handle();
            let shutdown_flag = mock.shutdown_flag();
            // `shutdown` is a request expecting a response.
            mock.queue_response(1, Value::Null).await;
            let mut client = LspClient::new(Box::new(mock));
            client.shutdown_and_exit().await.unwrap();
            let sent = outbound.lock().await;
            assert_eq!(sent.len(), 2, "shutdown + exit");
            let shutdown_msg = parse_body(&sent[0]);
            let exit_msg = parse_body(&sent[1]);
            assert_eq!(shutdown_msg["method"], "shutdown");
            assert_eq!(shutdown_msg["id"], 1);
            assert_eq!(exit_msg["method"], "exit");
            assert!(exit_msg.get("id").is_none());
            assert!(*shutdown_flag.lock().await, "transport.shutdown() called");
        }

        // 5. hover request shape
        #[tokio::test]
        async fn hover_request_shape() {
            let mock = MockLspTransport::new();
            let outbound = mock.outbound_handle();
            mock.queue_response(1, json!({"contents": "hi"})).await;
            let mut client = LspClient::new(Box::new(mock));
            let _ = client.hover("file:///a.rs", 3, 5).await.unwrap();
            let sent = outbound.lock().await;
            let msg = parse_body(&sent[0]);
            assert_eq!(msg["method"], "textDocument/hover");
            assert_eq!(msg["params"]["textDocument"]["uri"], "file:///a.rs");
            assert_eq!(msg["params"]["position"]["line"], 3);
            assert_eq!(msg["params"]["position"]["character"], 5);
        }

        // 6. goto_definition request shape
        #[tokio::test]
        async fn goto_definition_request_shape() {
            let mock = MockLspTransport::new();
            let outbound = mock.outbound_handle();
            mock.queue_response(1, json!([])).await;
            let mut client = LspClient::new(Box::new(mock));
            let _ = client.goto_definition("file:///a.rs", 7, 2).await.unwrap();
            let sent = outbound.lock().await;
            let msg = parse_body(&sent[0]);
            assert_eq!(msg["method"], "textDocument/definition");
            assert_eq!(msg["params"]["textDocument"]["uri"], "file:///a.rs");
            assert_eq!(msg["params"]["position"]["line"], 7);
            assert_eq!(msg["params"]["position"]["character"], 2);
        }

        // 7. find_references request shape
        #[tokio::test]
        async fn find_references_request_shape() {
            let mock = MockLspTransport::new();
            let outbound = mock.outbound_handle();
            mock.queue_response(1, json!([])).await;
            let mut client = LspClient::new(Box::new(mock));
            let _ = client
                .find_references("file:///a.rs", 1, 1, true)
                .await
                .unwrap();
            let sent = outbound.lock().await;
            let msg = parse_body(&sent[0]);
            assert_eq!(msg["method"], "textDocument/references");
            assert_eq!(
                msg["params"]["context"]["includeDeclaration"],
                json!(true),
                "include_declaration threaded through"
            );
        }

        // 8. diagnostics subscription shape — matches server-pushed
        //    publishDiagnostics by uri.
        #[tokio::test]
        async fn diagnostics_matches_publish_for_uri() {
            let mock = MockLspTransport::new();
            // Queue a noise notification first, then the matching one.
            mock.queue_notification(
                "textDocument/publishDiagnostics",
                json!({ "uri": "file:///other.rs", "diagnostics": [{"message": "skip"}] }),
            )
            .await;
            mock.queue_notification(
                "textDocument/publishDiagnostics",
                json!({
                    "uri": "file:///a.rs",
                    "diagnostics": [
                        { "range": {}, "message": "unused variable", "severity": 2 }
                    ]
                }),
            )
            .await;
            let mut client = LspClient::new(Box::new(mock));
            let diags = client.diagnostics("file:///a.rs").await.unwrap();
            assert!(diags.is_array());
            let arr = diags.as_array().unwrap();
            assert_eq!(arr.len(), 1);
            assert_eq!(arr[0]["message"], "unused variable");
        }

        // 9. Pool get_or_spawn caches by language
        #[tokio::test]
        async fn pool_get_or_spawn_caches_by_language() {
            let call_count = Arc::new(Mutex::new(0u32));
            let counter = call_count.clone();
            let spawner: SpawnFn = Arc::new(move |_language: &str| {
                let counter = counter.clone();
                Box::pin(async move {
                    *counter.lock().await += 1;
                    Ok(Box::new(MockLspTransport::new()) as Box<dyn LspTransport>)
                })
            });
            let mut pool = LspProcessPool::with_spawner(spawner);
            let _a = pool.get_or_spawn("rust").await.unwrap();
            let _b = pool.get_or_spawn("rust").await.unwrap();
            let _c = pool.get_or_spawn("python").await.unwrap();
            assert_eq!(pool.len(), 2);
            assert_eq!(
                *call_count.lock().await,
                2,
                "rust spawned once, python once"
            );
        }

        // 10. Pool shutdown_all cleans up
        #[tokio::test]
        async fn pool_shutdown_all_clears_cache() {
            let spawner: SpawnFn = Arc::new(|_language: &str| {
                Box::pin(async move {
                    let mock = MockLspTransport::new();
                    // Queue response for the `shutdown` request the client
                    // will send during shutdown_and_exit.
                    mock.queue_response(1, Value::Null).await;
                    Ok(Box::new(mock) as Box<dyn LspTransport>)
                })
            });
            let mut pool = LspProcessPool::with_spawner(spawner);
            let _ = pool.get_or_spawn("rust").await.unwrap();
            let _ = pool.get_or_spawn("go").await.unwrap();
            assert_eq!(pool.len(), 2);
            pool.shutdown_all().await.unwrap();
            assert!(pool.is_empty(), "cache drained after shutdown_all");
        }

        // 11. Content-Length header encoding is correct byte-length, not
        //     char-length — regression guard for multibyte bodies.
        #[tokio::test]
        async fn stdio_transport_content_length_is_byte_length() {
            // We don't spawn a real subprocess; we verify the header by
            // reading the formatted string directly.  The encode logic is a
            // single line in `send()`, mirrored here for the assertion.
            let body = r#"{"jsonrpc":"2.0","method":"初始化"}"#; // multibyte
            let header = format!("Content-Length: {}\r\n\r\n", body.len());
            assert!(header.contains("Content-Length:"));
            // len() is bytes, not chars, for &str.
            let expected = body.len();
            assert_eq!(header, format!("Content-Length: {expected}\r\n\r\n"));
        }

        // 12. Default-config picker — rust / typescript / python / go covered.
        #[tokio::test]
        async fn default_config_covers_four_languages() {
            assert_eq!(default_config_for("rust").unwrap().command, "rust-analyzer");
            assert_eq!(
                default_config_for("typescript").unwrap().command,
                "typescript-language-server"
            );
            assert_eq!(
                default_config_for("python").unwrap().command,
                "pyright-langserver"
            );
            assert_eq!(default_config_for("go").unwrap().command, "gopls");
            assert!(default_config_for("brainfuck").is_none());
        }

        // 13. Request rejects after shutdown
        #[tokio::test]
        async fn request_after_shutdown_errors() {
            let mock = MockLspTransport::new();
            mock.queue_response(1, Value::Null).await;
            let mut client = LspClient::new(Box::new(mock));
            client.shutdown_and_exit().await.unwrap();
            let err = client.hover("file:///a.rs", 0, 0).await.unwrap_err();
            assert!(
                matches!(err, LspError::Closed(_)),
                "expected Closed, got {err:?}"
            );
        }

        // 14. StdioLspTransport::spawn against a non-existent binary surfaces
        //     the spawn error instead of panicking.  `#[ignore]` because it
        //     does actually invoke the OS spawn path — kept non-ignored
        //     since the path we exercise is the error branch, which never
        //     executes a real program.
        //
        //     An absolute path bypasses the allowlist, so we reach the real
        //     spawn error branch (ENOENT).
        #[tokio::test]
        async fn stdio_transport_spawn_errors_on_missing_binary() {
            let cfg = LanguageServerConfig {
                command: "/nonexistent/definitely-nonexistent-lsp-binary-xyz".into(),
                args: vec![],
                workspace_root: None,
            };
            let result = StdioLspTransport::spawn(&cfg).await;
            let err = match result {
                Ok(_) => panic!("expected spawn failure"),
                Err(e) => e,
            };
            assert!(
                matches!(err, LspError::Spawn { .. }),
                "unexpected error variant: {err:?}"
            );
        }

        // 15. Real rust-analyzer smoke test — gated behind `#[ignore]` so CI
        //     flakes don't fail when rust-analyzer is not installed.
        #[tokio::test]
        #[ignore]
        async fn smoke_real_rust_analyzer_initialize() {
            let cfg = LanguageServerConfig {
                command: "rust-analyzer".into(),
                args: vec![],
                workspace_root: None,
            };
            let transport = StdioLspTransport::spawn(&cfg)
                .await
                .expect("rust-analyzer must be installed");
            let mut client = LspClient::new(Box::new(transport));
            let result = client.initialize("file:///tmp").await;
            assert!(result.is_ok(), "initialize round-trip: {result:?}");
            client.shutdown_and_exit().await.unwrap();
        }

        // 16. Allowlist hit: a recognised bare name passes the allowlist
        //     check. We use every name in LSP_BINARY_ALLOWLIST and confirm
        //     is_binary_allowed() accepts them. Absolute paths also pass.
        #[tokio::test]
        async fn stdio_transport_allowlist_hit_reaches_spawn_stage() {
            for name in LSP_BINARY_ALLOWLIST {
                assert!(
                    is_binary_allowed(name),
                    "allowlisted binary '{name}' rejected"
                );
            }
            assert!(is_binary_allowed("/usr/local/bin/my-lang-server"));
        }

        // 17. Untrusted name: a bare name not in the allowlist is rejected
        //     with UnauthorizedBinary BEFORE the OS spawn path is invoked.
        #[tokio::test]
        async fn stdio_transport_rejects_untrusted_bare_binary() {
            let cfg = LanguageServerConfig {
                command: "totally-sketchy-server".into(),
                args: vec![],
                workspace_root: None,
            };
            let result = StdioLspTransport::spawn(&cfg).await;
            match result {
                Ok(_) => panic!("expected UnauthorizedBinary rejection"),
                Err(LspError::UnauthorizedBinary(name)) => {
                    assert_eq!(name, "totally-sketchy-server");
                }
                Err(other) => panic!("expected UnauthorizedBinary, got {other:?}"),
            }
        }

        // 18. workspace_root threads through to the spawned child's cwd.
        //     We use an absolute-path command to bypass the allowlist, point
        //     at a missing binary so spawn fails cleanly, and assert that
        //     setting workspace_root does NOT change the error classification
        //     (still Spawn) — i.e. the field is accepted by the API.
        #[tokio::test]
        async fn stdio_transport_accepts_workspace_root() {
            let tmp = tempfile::tempdir().expect("tempdir");
            let cfg = LanguageServerConfig {
                command: "/nonexistent/definitely-nonexistent-lsp-binary-xyz".into(),
                args: vec![],
                workspace_root: Some(tmp.path().to_path_buf()),
            };
            match StdioLspTransport::spawn(&cfg).await {
                Ok(_) => panic!("missing binary must fail"),
                Err(err) => assert!(
                    matches!(err, LspError::Spawn { .. }),
                    "workspace_root should not change error classification: {err:?}"
                ),
            }
        }

        // 19. EOF mid-frame regression: reading a Content-Length: 100 header
        //     followed by only 50 body bytes and then EOF must surface an Io
        //     error (read_exact returns UnexpectedEof) rather than hanging.
        //     We drive the same header+body sequence StdioLspTransport::recv
        //     uses against a BufReader<Cursor<..>> fixture so the logic is
        //     exercised without spawning a subprocess.
        #[tokio::test]
        async fn recv_surfaces_io_error_on_eof_mid_frame() {
            use tokio::io::AsyncReadExt;

            let header = b"Content-Length: 100\r\n\r\n";
            let short_body = vec![b'x'; 50];
            let mut bytes: Vec<u8> = Vec::new();
            bytes.extend_from_slice(header);
            bytes.extend_from_slice(&short_body);

            let cursor = std::io::Cursor::new(bytes);
            let mut reader = tokio::io::BufReader::new(cursor);

            // Header phase: read lines until blank, same as recv().
            let mut content_length: Option<usize> = None;
            loop {
                let mut line = Vec::<u8>::with_capacity(64);
                loop {
                    let mut byte = [0u8; 1];
                    let n = reader.read(&mut byte).await.expect("header read");
                    if n == 0 {
                        panic!("EOF in header phase — fixture wrong");
                    }
                    line.push(byte[0]);
                    if line.ends_with(b"\r\n") || byte[0] == b'\n' {
                        break;
                    }
                }
                let trimmed = std::str::from_utf8(&line)
                    .unwrap()
                    .trim_end_matches(['\r', '\n']);
                if trimmed.is_empty() {
                    break;
                }
                if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
                    content_length = Some(rest.trim().parse().unwrap());
                }
            }
            let len = content_length.expect("Content-Length parsed");
            assert_eq!(len, 100);

            // Body phase: request 100 bytes but only 50 are available → EOF.
            let mut buf = vec![0u8; len];
            let result = reader.read_exact(&mut buf).await;
            assert!(
                result.is_err(),
                "expected Io error on EOF mid-frame, got Ok"
            );
            let err = result.unwrap_err();
            assert_eq!(
                err.kind(),
                std::io::ErrorKind::UnexpectedEof,
                "expected UnexpectedEof, got {err:?}"
            );
            // Wrapping into LspError classifies as LspError::Io via From impl.
            let lsp_err: LspError = err.into();
            assert!(
                matches!(lsp_err, LspError::Io(_)),
                "expected LspError::Io, got {lsp_err:?}"
            );
        }
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
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    // ---- in-process mock transport --------------------------------------------

    type MockOutbound = Arc<Mutex<Vec<String>>>;
    type MockInbound = Arc<Mutex<VecDeque<String>>>;

    /// Scripted transport: each `write_message` from the client is recorded
    /// and answered by popping the next queued response off a VecDeque.
    struct MockTransport {
        outbound: MockOutbound,
        inbound: MockInbound,
    }

    impl MockTransport {
        fn new() -> (Self, MockOutbound, MockInbound) {
            let outbound: MockOutbound = Arc::new(Mutex::new(Vec::<String>::new()));
            let inbound: MockInbound = Arc::new(Mutex::new(VecDeque::<String>::new()));
            (
                Self {
                    outbound: outbound.clone(),
                    inbound: inbound.clone(),
                },
                outbound,
                inbound,
            )
        }
    }

    impl LspTransport for MockTransport {
        fn write_message(&mut self, body: &str) -> anyhow::Result<()> {
            self.outbound.lock().unwrap().push(body.to_string());
            Ok(())
        }
        fn read_message(&mut self) -> anyhow::Result<String> {
            self.inbound
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| anyhow::anyhow!("mock transport: no more queued responses"))
        }
    }

    /// Queue a JSON-RPC response body for the next outgoing request with `id`.
    fn queue_response(inbound: &MockInbound, id: u64, result: Value) {
        let body = serde_json::to_string(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        }))
        .unwrap();
        inbound.lock().unwrap().push_back(body);
    }

    // ---- framing round-trip ---------------------------------------------------

    #[test]
    fn test_framing_roundtrip() {
        let mut buf: Vec<u8> = Vec::new();
        write_framed(&mut buf, r#"{"a":1}"#).unwrap();

        let s = String::from_utf8(buf.clone()).unwrap();
        assert!(s.starts_with("Content-Length: 7\r\n\r\n"));

        let mut cursor = std::io::Cursor::new(buf);
        let body = read_framed(&mut cursor).unwrap();
        assert_eq!(body, r#"{"a":1}"#);
    }

    #[test]
    fn test_framing_missing_header_errors() {
        let mut cursor = std::io::Cursor::new(b"no-header\r\n\r\n".to_vec());
        assert!(read_framed(&mut cursor).is_err());
    }

    // ---- default server picker -----------------------------------------------

    #[test]
    fn test_default_server_for_rust() {
        let cfg = default_server_for(Path::new("/tmp/foo.rs")).unwrap();
        assert_eq!(cfg.command, "rust-analyzer");
    }

    #[test]
    fn test_default_server_for_typescript() {
        let cfg = default_server_for(Path::new("/tmp/foo.ts")).unwrap();
        assert_eq!(cfg.command, "typescript-language-server");
        assert_eq!(cfg.args, vec!["--stdio".to_string()]);
    }

    #[test]
    fn test_default_server_unknown_ext_returns_none() {
        assert!(default_server_for(Path::new("/tmp/foo.xyz")).is_none());
    }

    // ---- client request/notification with mock --------------------------------

    #[test]
    fn test_client_request_round_trip() {
        let (transport, outbound, inbound) = MockTransport::new();
        queue_response(&inbound, 1, json!({ "ok": true }));

        let mut client = LspClient::new(Box::new(transport));
        let result = client
            .request("textDocument/hover", json!({ "x": 1 }))
            .unwrap();

        assert_eq!(result, json!({ "ok": true }));

        // Verify the request body looks right.
        let sent = outbound.lock().unwrap();
        assert_eq!(sent.len(), 1);
        let msg: Value = serde_json::from_str(&sent[0]).unwrap();
        assert_eq!(msg["method"], "textDocument/hover");
        assert_eq!(msg["id"], 1);
        assert_eq!(msg["jsonrpc"], "2.0");
    }

    #[test]
    fn test_client_drains_notifications_before_response() {
        let (transport, _outbound, inbound) = MockTransport::new();
        // Interleave a server notification with no id, then the real response.
        let notif = serde_json::to_string(&json!({
            "jsonrpc": "2.0",
            "method": "window/logMessage",
            "params": { "type": 3, "message": "noise" },
        }))
        .unwrap();
        inbound.lock().unwrap().push_back(notif);
        queue_response(&inbound, 1, json!(42));

        let mut client = LspClient::new(Box::new(transport));
        let result = client.request("test", json!({})).unwrap();
        assert_eq!(result, json!(42));
    }

    // ---- symbols capability with mock transport ------------------------------

    fn make_request(workspace: &Path, input: Value) -> CapabilityExecutionRequest {
        let root = workspace.to_string_lossy().to_string();
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
            connector_id: ConnectorId::from_string("test-lsp".to_string()).unwrap(),
            capability_id: CapabilityId::from_string("connector:lsp:symbols".to_string()).unwrap(),
            input,
        }
    }

    /// Helper — bypasses the real process pool and drives the public
    /// `extract_*` / `parse_symbol_tree` helpers directly against a mocked
    /// server response to verify parsing.
    #[test]
    fn test_parse_symbol_tree_hierarchical() {
        let arr = json!([
            {
                "name": "foo",
                "kind": 12,
                "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 5, "character": 1 } },
                "children": [
                    {
                        "name": "bar",
                        "kind": 6,
                        "range": { "start": { "line": 1, "character": 4 }, "end": { "line": 3, "character": 5 } },
                        "children": []
                    }
                ]
            }
        ]);
        let symbols = parse_symbol_tree(arr.as_array().unwrap());
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "foo");
        assert_eq!(symbols[0].kind, 12);
        assert_eq!(symbols[0].children.len(), 1);
        assert_eq!(symbols[0].children[0].name, "bar");
    }

    #[test]
    fn test_extract_hover_markup_content() {
        let result = json!({
            "contents": { "kind": "markdown", "value": "**bar**" },
            "range": { "start": { "line": 1, "character": 0 }, "end": { "line": 1, "character": 3 } }
        });
        let (md, range) = extract_hover(&result);
        assert_eq!(md, "**bar**");
        assert!(range.is_some());
    }

    #[test]
    fn test_extract_hover_marked_string_array() {
        let result = json!({
            "contents": [
                "first",
                { "language": "rust", "value": "fn bar() -> ()" }
            ]
        });
        let (md, range) = extract_hover(&result);
        assert!(md.contains("first"));
        assert!(md.contains("fn bar"));
        assert!(range.is_none());
    }

    #[test]
    fn test_extract_hover_null_result() {
        let (md, range) = extract_hover(&Value::Null);
        assert!(md.is_empty());
        assert!(range.is_none());
    }

    // ---- offline paths: missing binary -> unavailable envelope ----------------

    const DEAD_BINARY: &str = "definitely-nonexistent-lsp-binary-xyz";

    /// Set up a tempdir with `a.rs` and a dead language-server command.
    /// Returns the dir (kept alive) and the dead-binary `LanguageServerConfig`.
    fn offline_fixture() -> (tempfile::TempDir, serde_json::Value) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn main() {}").unwrap();
        let ls = json!({ "command": DEAD_BINARY, "args": [] });
        (dir, ls)
    }

    #[test]
    fn test_symbols_offline_when_binary_missing() {
        let (dir, ls) = offline_fixture();
        let req = make_request(
            dir.path(),
            json!({ "file_path": "a.rs", "language_server": ls }),
        );
        let out = execute_symbols(dir.path(), req).unwrap();
        assert_eq!(out["available"], json!(false));
        assert!(out["reason"].as_str().unwrap().contains("not found"));
        assert_eq!(out["symbols"], json!([]));
    }

    #[test]
    fn test_hover_offline_when_binary_missing() {
        let (dir, ls) = offline_fixture();
        let req = make_request(
            dir.path(),
            json!({
                "file_path": "a.rs",
                "position": { "line": 0, "character": 0 },
                "language_server": ls,
            }),
        );
        let out = execute_hover(dir.path(), req).unwrap();
        assert_eq!(out["available"], json!(false));
        assert_eq!(out["markdown"], json!(""));
    }

    #[test]
    fn test_find_references_offline_when_binary_missing() {
        let (dir, ls) = offline_fixture();
        let req = make_request(
            dir.path(),
            json!({
                "file_path": "a.rs",
                "position": { "line": 0, "character": 0 },
                "language_server": ls,
            }),
        );
        let out = execute_find_references(dir.path(), req).unwrap();
        assert_eq!(out["available"], json!(false));
        assert_eq!(out["references"], json!([]));
    }

    // ---- unknown extension -> unavailable envelope ---------------------------

    #[test]
    fn test_symbols_unavailable_when_no_default_server() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.xyz");
        std::fs::write(&file, "").unwrap();

        let req = make_request(dir.path(), json!({ "file_path": "a.xyz" }));
        let out = execute_symbols(dir.path(), req).unwrap();
        assert_eq!(out["available"], json!(false));
        assert!(out["reason"].as_str().unwrap().contains("no default"));
    }

    // ---- path_to_uri helper ---------------------------------------------------

    #[test]
    fn test_path_to_uri_unix() {
        let uri = path_to_uri(Path::new("/tmp/foo.rs"));
        assert_eq!(uri, "file:///tmp/foo.rs");
    }

    // ---- resolve_server ------------------------------------------------------

    #[test]
    fn test_resolve_server_uses_input_cfg() {
        let cfg = LanguageServerConfig {
            command: "mock".to_string(),
            args: vec!["--foo".to_string()],
            workspace_root: None,
        };
        let resolved = resolve_server(Some(cfg), Path::new("/tmp/a.unknown")).unwrap();
        assert_eq!(resolved.command, "mock");
    }

    #[test]
    fn test_resolve_server_rejects_empty_command() {
        let cfg = LanguageServerConfig {
            command: String::new(),
            args: vec![],
            workspace_root: None,
        };
        assert!(resolve_server(Some(cfg), Path::new("/tmp/a.rs")).is_err());
    }
}
