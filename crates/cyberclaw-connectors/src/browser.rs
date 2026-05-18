//! Browser CDP connector.
//!
//! Provides a small Chrome DevTools Protocol client over WebSocket. The
//! connector attaches to an existing Chromium/Chrome debugging endpoint and
//! exposes browser automation as governed `browser.*` capabilities.

use crate::types::{
    CapabilityExecutionRequest, CapabilityExecutionResult, Connector, ExecutionStatus,
};
use async_trait::async_trait;
use base64::{engine::general_purpose, Engine as _};
use cyberclaw_core::capability::RiskLevel;
use cyberclaw_core::facade::{CapabilityFacade, FacadeExposure, ToolsetCategory};
use cyberclaw_core::ids::{CapabilityId, ConnectorId};
use cyberclaw_core::prelude::*;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

const DEFAULT_DEBUG_URL: &str = "http://127.0.0.1:9222";
const DEFAULT_TIMEOUT_MS: u64 = 30_000;

#[derive(Debug, Clone)]
pub struct BrowserConnectorConfig {
    pub debug_url: String,
    pub page_ws_url: Option<String>,
    pub timeout_ms: u64,
}

impl BrowserConnectorConfig {
    pub fn from_env() -> Self {
        Self {
            debug_url: std::env::var("CYBERCLAW_BROWSER_DEBUG_URL")
                .unwrap_or_else(|_| DEFAULT_DEBUG_URL.to_string()),
            page_ws_url: std::env::var("CYBERCLAW_BROWSER_WS_URL").ok(),
            timeout_ms: std::env::var("CYBERCLAW_BROWSER_TIMEOUT_MS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(DEFAULT_TIMEOUT_MS),
        }
    }
}

#[derive(Debug)]
pub struct BrowserConnector {
    id: ConnectorId,
    workspace: PathBuf,
    config: BrowserConnectorConfig,
}

impl BrowserConnector {
    pub fn new(workspace: PathBuf, config: BrowserConnectorConfig) -> anyhow::Result<Self> {
        Ok(Self {
            id: ConnectorId::from_string("browser".to_string())?,
            workspace,
            config,
        })
    }

    pub fn from_env(workspace: PathBuf) -> anyhow::Result<Self> {
        Self::new(workspace, BrowserConnectorConfig::from_env())
    }

    fn capabilities() -> Vec<CapabilityContract> {
        [
            (
                "browser.navigate",
                "Navigate Browser",
                "Navigate an attached browser page to a URL",
                "BrowserNavigateInput",
                "BrowserNavigateOutput",
            ),
            (
                "browser.click",
                "Click Browser Element",
                "Click a DOM element selected by CSS selector",
                "BrowserClickInput",
                "BrowserActionOutput",
            ),
            (
                "browser.fill",
                "Fill Browser Element",
                "Fill a text-like DOM element selected by CSS selector",
                "BrowserFillInput",
                "BrowserActionOutput",
            ),
            (
                "browser.evaluate",
                "Evaluate Browser Script",
                "Evaluate JavaScript in the attached browser page",
                "BrowserEvaluateInput",
                "BrowserEvaluateOutput",
            ),
            (
                "browser.screenshot",
                "Capture Browser Screenshot",
                "Capture a screenshot from the attached browser page",
                "BrowserScreenshotInput",
                "BrowserScreenshotOutput",
            ),
            (
                "browser.dialog_handle",
                "Handle Browser Dialog",
                "Accept or dismiss the next JavaScript dialog",
                "BrowserDialogHandleInput",
                "BrowserActionOutput",
            ),
        ]
        .into_iter()
        .map(
            |(id, title, description, input_schema, output_schema)| CapabilityContract {
                id: id.to_string(),
                title: title.to_string(),
                description: Some(description.to_string()),
                input_schema: input_schema.to_string(),
                output_schema: output_schema.to_string(),
                risk: RiskLevel::High, // 2026-05-17 (v1.2.9): reverted to honest classification. Native runtime now allowed via network_bound:true (cyberclaw shim talks to external obscura over CDP; real isolation is at obscura process).
                effects: vec![
                    CapabilityEffect::Read,
                    CapabilityEffect::Write,
                    CapabilityEffect::Network,
                    CapabilityEffect::Execute,
                ],
                placement: None,
                timeouts: CapabilityTimeouts {
                    request_ms: Some(60_000),
                },
            },
        )
        .collect()
    }

    async fn cdp_client(&self) -> anyhow::Result<CdpClient<WebSocketCdpTransport>> {
        let ws_url = if let Some(url) = &self.config.page_ws_url {
            url.clone()
        } else {
            discover_page_ws_url(&self.config.debug_url, self.config.timeout_ms).await?
        };
        CdpClient::connect(ws_url).await
    }

    fn validate_output_path(
        &self,
        requested: Option<&str>,
        extension: &str,
    ) -> anyhow::Result<PathBuf> {
        let raw = requested.map(PathBuf::from).unwrap_or_else(|| {
            self.workspace.join("browser_screenshots").join(format!(
                "{}.{}",
                uuid::Uuid::new_v4(),
                extension
            ))
        });

        let abs = if raw.is_absolute() {
            raw
        } else {
            self.workspace.join(raw)
        };

        let workspace = self
            .workspace
            .canonicalize()
            .map_err(|e| anyhow::anyhow!("Failed to canonicalize browser workspace: {}", e))?;
        let parent = abs
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Screenshot path has no parent"))?;
        let existing_parent = nearest_existing_ancestor(parent)?;
        let canonical_parent = existing_parent.canonicalize()?;
        if !canonical_parent.starts_with(&workspace) {
            anyhow::bail!("Screenshot path is outside workspace boundary");
        }
        Ok(abs)
    }
}

#[async_trait]
impl Connector for BrowserConnector {
    fn id(&self) -> &ConnectorId {
        &self.id
    }

    fn runtime(&self) -> ConnectorRuntime {
        ConnectorRuntime::Remote
    }

    fn capabilities(&self) -> Vec<CapabilityContract> {
        Self::capabilities()
    }

    async fn execute(
        &self,
        request: CapabilityExecutionRequest,
    ) -> anyhow::Result<CapabilityExecutionResult> {
        let output = match request.capability_id.as_str() {
            "browser.navigate" => navigate(self, &request).await,
            "browser.click" => click(self, &request).await,
            "browser.fill" => fill(self, &request).await,
            "browser.evaluate" => evaluate(self, &request).await,
            "browser.screenshot" => screenshot(self, &request).await,
            "browser.dialog_handle" => dialog_handle(self, &request).await,
            other => Err(anyhow::anyhow!("Unknown browser capability: {}", other)),
        };

        match output {
            Ok(output) => Ok(CapabilityExecutionResult {
                execution_id: request.execution_id,
                trace_id: request.trace_id,
                connector_id: request.connector_id,
                capability_id: request.capability_id,
                output,
                status: ExecutionStatus::Success,
                error: None,
                actual_runtime: None,
            }),
            Err(e) => {
                // 2026-05-17 — surface the real CDP failure to logs. Without this,
                // the ApiError::InternalError sanitization at the HTTP layer drops
                // the detail and operators see only "Internal server error".
                tracing::error!(
                    capability = %request.capability_id,
                    error = ?e,
                    "BrowserConnector capability failed"
                );
                Ok(CapabilityExecutionResult {
                    execution_id: request.execution_id,
                    trace_id: request.trace_id,
                    connector_id: request.connector_id,
                    capability_id: request.capability_id,
                    output: json!({ "error": format!("{:#}", e) }),
                    status: ExecutionStatus::Failed,
                    error: Some(format!("{:#}", e)),
                    actual_runtime: None,
                })
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserNavigateInput {
    pub url: String,
    #[serde(default = "default_wait_until")]
    pub wait_until: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserNavigateOutput {
    pub url: String,
    pub frame_id: Option<String>,
    pub loader_id: Option<String>,
    pub waited_for: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserClickInput {
    pub selector: String,
    #[serde(default)]
    pub button: Option<String>,
    #[serde(default)]
    pub click_count: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserFillInput {
    pub selector: String,
    pub text: String,
    #[serde(default = "default_true")]
    pub clear: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserEvaluateInput {
    pub expression: String,
    #[serde(default = "default_true")]
    pub await_promise: bool,
    #[serde(default = "default_true")]
    pub return_by_value: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserEvaluateOutput {
    pub result: Value,
    pub exception: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserScreenshotInput {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default = "default_png")]
    pub format: String,
    #[serde(default)]
    pub quality: Option<u8>,
    #[serde(default)]
    pub full_page: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserScreenshotOutput {
    pub path: String,
    pub bytes_written: u64,
    pub format: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserDialogHandleInput {
    pub accept: bool,
    #[serde(default)]
    pub prompt_text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserActionOutput {
    pub ok: bool,
}

fn default_true() -> bool {
    true
}

fn default_png() -> String {
    "png".to_string()
}

fn default_wait_until() -> String {
    "load".to_string()
}

async fn navigate(
    connector: &BrowserConnector,
    request: &CapabilityExecutionRequest,
) -> anyhow::Result<Value> {
    let input: BrowserNavigateInput = serde_json::from_value(request.input.clone())?;
    validate_http_url(&input.url)?;
    let mut client = connector.cdp_client().await?;
    client.send_command("Page.enable", json!({})).await?;
    let result = client
        .send_command("Page.navigate", json!({ "url": input.url }))
        .await?;
    wait_for_navigation(&mut client, &input.wait_until, connector.config.timeout_ms).await?;
    Ok(serde_json::to_value(BrowserNavigateOutput {
        url: input.url,
        frame_id: result
            .get("frameId")
            .and_then(|v| v.as_str())
            .map(ToString::to_string),
        loader_id: result
            .get("loaderId")
            .and_then(|v| v.as_str())
            .map(ToString::to_string),
        waited_for: input.wait_until,
    })?)
}

async fn click(
    connector: &BrowserConnector,
    request: &CapabilityExecutionRequest,
) -> anyhow::Result<Value> {
    let input: BrowserClickInput = serde_json::from_value(request.input.clone())?;
    let button = input.button.as_deref().unwrap_or("left");
    let click_count = input.click_count.unwrap_or(1);
    let expression = click_script(&input.selector, button, click_count)?;
    let mut client = connector.cdp_client().await?;
    evaluate_script(&mut client, expression, true, false).await?;
    Ok(serde_json::to_value(BrowserActionOutput { ok: true })?)
}

async fn fill(
    connector: &BrowserConnector,
    request: &CapabilityExecutionRequest,
) -> anyhow::Result<Value> {
    let input: BrowserFillInput = serde_json::from_value(request.input.clone())?;
    let expression = fill_script(&input.selector, &input.text, input.clear)?;
    let mut client = connector.cdp_client().await?;
    evaluate_script(&mut client, expression, true, false).await?;
    Ok(serde_json::to_value(BrowserActionOutput { ok: true })?)
}

async fn evaluate(
    connector: &BrowserConnector,
    request: &CapabilityExecutionRequest,
) -> anyhow::Result<Value> {
    let input: BrowserEvaluateInput = serde_json::from_value(request.input.clone())?;
    let mut client = connector.cdp_client().await?;
    let result = evaluate_script(
        &mut client,
        input.expression,
        input.await_promise,
        input.return_by_value,
    )
    .await?;
    Ok(serde_json::to_value(result)?)
}

async fn screenshot(
    connector: &BrowserConnector,
    request: &CapabilityExecutionRequest,
) -> anyhow::Result<Value> {
    let input: BrowserScreenshotInput = serde_json::from_value(request.input.clone())?;
    let format = normalize_screenshot_format(&input.format)?;
    let mut params = json!({
        "format": format,
        "fromSurface": true,
        "captureBeyondViewport": input.full_page,
    });
    if format == "jpeg" {
        if let Some(quality) = input.quality {
            params["quality"] = json!(quality.min(100));
        }
    }
    let mut client = connector.cdp_client().await?;
    let result = client
        .send_command("Page.captureScreenshot", params)
        .await?;
    let b64 = result
        .get("data")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("CDP screenshot response missing data"))?;
    let bytes = general_purpose::STANDARD.decode(b64)?;
    let path = connector.validate_output_path(input.path.as_deref(), &format)?;
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(&path, &bytes).await?;
    Ok(serde_json::to_value(BrowserScreenshotOutput {
        path: path.to_string_lossy().to_string(),
        bytes_written: bytes.len() as u64,
        format,
    })?)
}

async fn dialog_handle(
    connector: &BrowserConnector,
    request: &CapabilityExecutionRequest,
) -> anyhow::Result<Value> {
    let input: BrowserDialogHandleInput = serde_json::from_value(request.input.clone())?;
    let mut client = connector.cdp_client().await?;
    client.send_command("Page.enable", json!({})).await?;
    client
        .send_command(
            "Page.handleJavaScriptDialog",
            json!({
                "accept": input.accept,
                "promptText": input.prompt_text.unwrap_or_default(),
            }),
        )
        .await?;
    Ok(serde_json::to_value(BrowserActionOutput { ok: true })?)
}

async fn evaluate_script<T: CdpTransport + Send>(
    client: &mut CdpClient<T>,
    expression: String,
    await_promise: bool,
    return_by_value: bool,
) -> anyhow::Result<BrowserEvaluateOutput> {
    let result = client
        .send_command(
            "Runtime.evaluate",
            json!({
                "expression": expression,
                "awaitPromise": await_promise,
                "returnByValue": return_by_value,
            }),
        )
        .await?;
    let exception = result.get("exceptionDetails").map(|v| v.to_string());
    if let Some(exception) = exception.clone() {
        anyhow::bail!("Browser evaluation failed: {}", exception);
    }
    Ok(BrowserEvaluateOutput {
        result: result.get("result").cloned().unwrap_or(Value::Null),
        exception,
    })
}

async fn wait_for_navigation<T: CdpTransport + Send>(
    client: &mut CdpClient<T>,
    wait_until: &str,
    timeout_ms: u64,
) -> anyhow::Result<()> {
    let event = match wait_until {
        "none" => return Ok(()),
        "domcontentloaded" => "Page.domContentEventFired",
        "load" | "" => "Page.loadEventFired",
        other => anyhow::bail!("Unsupported wait_until '{}'", other),
    };
    client
        .wait_event(event, Duration::from_millis(timeout_ms))
        .await
        .map(|_| ())
}

fn validate_http_url(url: &str) -> anyhow::Result<()> {
    let parsed = reqwest::Url::parse(url)?;
    match parsed.scheme() {
        "http" | "https" => Ok(()),
        scheme => anyhow::bail!("Unsupported browser URL scheme '{}'", scheme),
    }
}

fn normalize_screenshot_format(format: &str) -> anyhow::Result<String> {
    match format {
        "png" | "" => Ok("png".to_string()),
        "jpeg" | "jpg" => Ok("jpeg".to_string()),
        other => anyhow::bail!("Unsupported screenshot format '{}'", other),
    }
}

fn click_script(selector: &str, button: &str, click_count: u32) -> anyhow::Result<String> {
    if !matches!(button, "left" | "middle" | "right") {
        anyhow::bail!("Unsupported mouse button '{}'", button);
    }
    let selector = serde_json::to_string(selector)?;
    let button = serde_json::to_string(button)?;
    Ok(format!(
        r#"(async () => {{
const el = document.querySelector({selector});
if (!el) throw new Error("selector not found: " + {selector});
el.scrollIntoView({{block: "center", inline: "center"}});
for (let i = 0; i < {click_count}; i++) {{
  el.dispatchEvent(new MouseEvent("click", {{bubbles: true, cancelable: true, button: {button} === "right" ? 2 : {button} === "middle" ? 1 : 0}}));
}}
return true;
}})()"#
    ))
}

fn fill_script(selector: &str, text: &str, clear: bool) -> anyhow::Result<String> {
    let selector = serde_json::to_string(selector)?;
    let text = serde_json::to_string(text)?;
    Ok(format!(
        r#"(async () => {{
const el = document.querySelector({selector});
if (!el) throw new Error("selector not found: " + {selector});
el.scrollIntoView({{block: "center", inline: "center"}});
if ("value" in el) {{
  if ({clear}) el.value = "";
  el.value += {text};
  el.dispatchEvent(new Event("input", {{bubbles: true}}));
  el.dispatchEvent(new Event("change", {{bubbles: true}}));
}} else {{
  el.textContent = {text};
}}
return true;
}})()"#
    ))
}

fn nearest_existing_ancestor(path: &Path) -> anyhow::Result<PathBuf> {
    let mut current = path.to_path_buf();
    loop {
        if current.exists() {
            return Ok(current);
        }
        current = current
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| anyhow::anyhow!("No existing ancestor for path"))?;
    }
}

async fn discover_page_ws_url(debug_url: &str, timeout_ms: u64) -> anyhow::Result<String> {
    let base = debug_url.trim_end_matches('/');
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(timeout_ms))
        .no_proxy()
        .build()?;
    let targets: Value = client
        .get(format!("{}/json", base))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    if let Some(url) = pick_page_ws_url(&targets) {
        return Ok(url);
    }

    let created: Value = client
        .put(format!("{}/json/new?about:blank", base))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    created
        .get("webSocketDebuggerUrl")
        .and_then(|v| v.as_str())
        .map(ToString::to_string)
        .ok_or_else(|| anyhow::anyhow!("Chrome DevTools did not return a page WebSocket URL"))
}

fn pick_page_ws_url(targets: &Value) -> Option<String> {
    targets.as_array().and_then(|items| {
        items
            .iter()
            .find(|item| item.get("type").and_then(|v| v.as_str()) == Some("page"))
            .and_then(|item| item.get("webSocketDebuggerUrl").and_then(|v| v.as_str()))
            .map(ToString::to_string)
    })
}

#[async_trait]
trait CdpTransport {
    async fn send_json(&mut self, value: Value) -> anyhow::Result<()>;
    async fn recv_json(&mut self) -> anyhow::Result<Value>;
}

struct WebSocketCdpTransport {
    stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
}

#[async_trait]
impl CdpTransport for WebSocketCdpTransport {
    async fn send_json(&mut self, value: Value) -> anyhow::Result<()> {
        self.stream.send(WsMessage::Text(value.to_string())).await?;
        Ok(())
    }

    async fn recv_json(&mut self) -> anyhow::Result<Value> {
        loop {
            let msg = self
                .stream
                .next()
                .await
                .ok_or_else(|| anyhow::anyhow!("CDP WebSocket closed"))??;
            match msg {
                WsMessage::Text(text) => return Ok(serde_json::from_str(&text)?),
                WsMessage::Binary(bytes) => return Ok(serde_json::from_slice(&bytes)?),
                WsMessage::Ping(bytes) => {
                    self.stream.send(WsMessage::Pong(bytes)).await?;
                }
                WsMessage::Close(_) => anyhow::bail!("CDP WebSocket closed"),
                _ => {}
            }
        }
    }
}

struct CdpClient<T> {
    transport: T,
    next_id: u64,
    buffered: VecDeque<Value>,
    /// 2026-05-17: standard-CDP sessionId for page commands. Chrome's
    /// page-level WS auto-attaches (sessionId-less Page.navigate works);
    /// strict-CDP servers like obscura require explicit Target.attachToTarget
    /// to get a sessionId, then every Page.*/Runtime.* command must carry it.
    /// `None` for backwards-compatible flat mode (Chrome direct page WS).
    session_id: Option<String>,
}

impl CdpClient<WebSocketCdpTransport> {
    async fn connect(ws_url: String) -> anyhow::Result<Self> {
        let (stream, _) = tokio_tungstenite::connect_async(&ws_url).await?;
        let mut client = Self {
            transport: WebSocketCdpTransport { stream },
            next_id: 1,
            buffered: VecDeque::new(),
            session_id: None,
        };
        // 2026-05-17: ensure a page session exists. Strict-CDP servers
        // (obscura) require explicit attach even when the WS URL is page-
        // specific; Chrome tolerates a missing attach but accepts it too.
        // Bubble the real error so dispatchers see why instead of getting
        // a silent flat-mode fallback that then 500s downstream.
        client
            .ensure_page_session()
            .await
            .map_err(|e| anyhow::anyhow!("CDP attach failed: {e}"))?;
        Ok(client)
    }
}

impl<T: CdpTransport + Send> CdpClient<T> {
    /// Discover (or create) a page target and attach to it, storing the
    /// returned sessionId so subsequent Page.* / Runtime.* commands route
    /// to the right page. Idempotent — does nothing if session_id is set.
    ///
    /// 2026-05-17: handles obscura's lifecycle where a previous client's
    /// WS disconnect can destroy the page-1 target, leaving the browser
    /// with zero pages. We create one on demand.
    async fn ensure_page_session(&mut self) -> anyhow::Result<()> {
        if self.session_id.is_some() {
            return Ok(());
        }
        let resp = self.send_command("Target.getTargets", json!({})).await?;
        let targets = resp
            .get("targetInfos")
            .and_then(|v| v.as_array())
            .ok_or_else(|| anyhow::anyhow!("Target.getTargets missing targetInfos"))?;
        let target_id = if let Some(t) = targets
            .iter()
            .find(|t| t.get("type").and_then(|v| v.as_str()) == Some("page"))
        {
            t.get("targetId")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("page target missing targetId"))?
                .to_string()
        } else {
            // No page exists — create a blank one. Standard CDP per the spec.
            let created = self
                .send_command(
                    "Target.createTarget",
                    json!({ "url": "about:blank" }),
                )
                .await?;
            created
                .get("targetId")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    anyhow::anyhow!("Target.createTarget did not return targetId")
                })?
                .to_string()
        };
        let attach = self
            .send_command(
                "Target.attachToTarget",
                json!({ "targetId": target_id, "flatten": true }),
            )
            .await?;
        let session_id = attach
            .get("sessionId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Target.attachToTarget did not return sessionId"))?
            .to_string();
        self.session_id = Some(session_id);
        Ok(())
    }
}

impl<T: CdpTransport + Send> CdpClient<T> {
    #[cfg(test)]
    fn with_transport(transport: T) -> Self {
        Self {
            transport,
            next_id: 1,
            buffered: VecDeque::new(),
            session_id: None,
        }
    }

    async fn send_command(&mut self, method: &str, params: Value) -> anyhow::Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        // 2026-05-17: include sessionId when present. Required by strict-CDP
        // servers (obscura) for Page.*/Runtime.*; harmless on Chrome.
        // Bootstrap commands (Target.getTargets, Target.attachToTarget) are
        // browser-domain so they're sent before session_id is populated.
        let mut envelope = json!({
            "id": id,
            "method": method,
            "params": params,
        });
        if let Some(sid) = &self.session_id {
            envelope
                .as_object_mut()
                .expect("envelope is always an object")
                .insert("sessionId".to_string(), Value::String(sid.clone()));
        }
        self.transport.send_json(envelope).await?;

        loop {
            let msg = self.transport.recv_json().await?;
            if msg.get("id").and_then(|v| v.as_u64()) == Some(id) {
                if let Some(error) = msg.get("error") {
                    anyhow::bail!("CDP command '{}' failed: {}", method, error);
                }
                return Ok(msg.get("result").cloned().unwrap_or_else(|| json!({})));
            }
            self.buffered.push_back(msg);
        }
    }

    async fn wait_event(&mut self, method: &str, timeout: Duration) -> anyhow::Result<Value> {
        let wait = async {
            if let Some(pos) = self
                .buffered
                .iter()
                .position(|msg| msg.get("method").and_then(|v| v.as_str()) == Some(method))
            {
                return self
                    .buffered
                    .remove(pos)
                    .ok_or_else(|| anyhow::anyhow!("Buffered CDP event disappeared"));
            }

            loop {
                let msg = self.transport.recv_json().await?;
                if msg.get("method").and_then(|v| v.as_str()) == Some(method) {
                    return Ok(msg);
                }
                self.buffered.push_back(msg);
            }
        };
        tokio::time::timeout(timeout, wait)
            .await
            .map_err(|_| anyhow::anyhow!("Timed out waiting for CDP event {}", method))?
    }
}

/// §4 — authoritative facade declarations for `browser.*` capabilities.
///
/// High-blast tools (browser_evaluate, browser_dialog_handle) are set to
/// `LlmAdvanced` so they appear in the deferred registry rather than the
/// default active prompt, reducing accidental JS-execution surface.
#[allow(dead_code)]
pub fn capability_facades() -> Vec<(CapabilityFacade, ToolsetCategory)> {
    let connector_id = ConnectorId::from_string("browser".to_string()).unwrap();
    vec![
        (
            CapabilityFacade {
                name: "browser_navigate".to_string(),
                description: "Navigate an attached Chrome DevTools Protocol browser page to an \
                    HTTP or HTTPS URL."
                    .to_string(),
                connector_id: connector_id.clone(),
                capability_id: CapabilityId::from_string("browser.navigate".to_string()).unwrap(),
                risk_level: RiskLevel::High, // 2026-05-17 (v1.2.9): reverted, see capabilities() network_bound rationale
                effects: vec!["execute".to_string()],
                read_only: false,
                destructive: true,
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "url": { "type": "string", "description": "HTTP or HTTPS URL to navigate to" },
                        "wait_until": {
                            "type": "string",
                            "enum": ["load", "domcontentloaded", "none"],
                            "description": "Navigation event to wait for. Default: load."
                        }
                    },
                    "required": ["url"]
                })),
                exposure: FacadeExposure::LlmDefault,
            },
            ToolsetCategory::Web,
        ),
        (
            CapabilityFacade {
                name: "browser_click".to_string(),
                description:
                    "Click a DOM element in the attached browser page using a CSS selector."
                        .to_string(),
                connector_id: connector_id.clone(),
                capability_id: CapabilityId::from_string("browser.click".to_string()).unwrap(),
                risk_level: RiskLevel::High, // 2026-05-17 (v1.2.9): reverted, see capabilities() network_bound rationale
                effects: vec!["execute".to_string()],
                read_only: false,
                destructive: true,
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "selector": { "type": "string", "description": "CSS selector for the target element" },
                        "button": {
                            "type": "string",
                            "enum": ["left", "middle", "right"],
                            "description": "Mouse button. Default: left."
                        },
                        "click_count": { "type": "integer", "description": "Number of click events. Default: 1." }
                    },
                    "required": ["selector"]
                })),
                exposure: FacadeExposure::LlmDefault,
            },
            ToolsetCategory::Web,
        ),
        (
            CapabilityFacade {
                name: "browser_type".to_string(),
                description: "Type text into a focused DOM element in the attached browser page, \
                    simulating key-by-key input events."
                    .to_string(),
                connector_id: connector_id.clone(),
                capability_id: CapabilityId::from_string("browser.type".to_string()).unwrap(),
                risk_level: RiskLevel::High, // 2026-05-17 (v1.2.9): reverted, see capabilities() network_bound rationale
                effects: vec!["execute".to_string()],
                read_only: false,
                destructive: true,
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "selector": { "type": "string", "description": "CSS selector for the target element" },
                        "text": { "type": "string", "description": "Text to type" },
                        "delay_ms": { "type": "integer", "description": "Delay between keystrokes in ms (default 0)" }
                    },
                    "required": ["selector", "text"]
                })),
                exposure: FacadeExposure::LlmDefault,
            },
            ToolsetCategory::Web,
        ),
        (
            CapabilityFacade {
                name: "browser_fill".to_string(),
                description: "Fill a text-like DOM element in the attached browser page using a \
                    CSS selector."
                    .to_string(),
                connector_id: connector_id.clone(),
                capability_id: CapabilityId::from_string("browser.fill".to_string()).unwrap(),
                risk_level: RiskLevel::High, // 2026-05-17 (v1.2.9): reverted, see capabilities() network_bound rationale
                effects: vec!["execute".to_string()],
                read_only: false,
                destructive: true,
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "selector": { "type": "string", "description": "CSS selector for the target element" },
                        "text": { "type": "string", "description": "Text to enter" },
                        "clear": { "type": "boolean", "description": "Clear existing value before filling. Default: true." }
                    },
                    "required": ["selector", "text"]
                })),
                exposure: FacadeExposure::LlmDefault,
            },
            ToolsetCategory::Web,
        ),
        (
            CapabilityFacade {
                name: "browser_evaluate".to_string(),
                description:
                    "Evaluate JavaScript in the attached browser page and return the \
                    Chrome DevTools Protocol result. **High blast radius — executes arbitrary JS.**"
                        .to_string(),
                connector_id: connector_id.clone(),
                capability_id: CapabilityId::from_string("browser.evaluate".to_string()).unwrap(),
                risk_level: RiskLevel::High, // 2026-05-17 (v1.2.9): reverted, see capabilities() network_bound rationale
                effects: vec!["execute".to_string()],
                read_only: false,
                destructive: true,
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "expression": { "type": "string", "description": "JavaScript expression to evaluate" },
                        "await_promise": { "type": "boolean", "description": "Await Promise results. Default: true." },
                        "return_by_value": { "type": "boolean", "description": "Return serializable values by value. Default: true." }
                    },
                    "required": ["expression"]
                })),
                exposure: FacadeExposure::LlmAdvanced,
            },
            ToolsetCategory::Web,
        ),
        (
            CapabilityFacade {
                name: "browser_screenshot".to_string(),
                description: "Capture a screenshot from the attached browser page and write it \
                    inside the CyberClaw workspace."
                    .to_string(),
                connector_id: connector_id.clone(),
                capability_id: CapabilityId::from_string("browser.screenshot".to_string()).unwrap(),
                risk_level: RiskLevel::High, // 2026-05-17 (v1.2.9): reverted, see capabilities() network_bound rationale
                effects: vec!["execute".to_string()],
                read_only: false,
                destructive: true,
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Optional workspace-relative output path" },
                        "format": {
                            "type": "string",
                            "enum": ["png", "jpeg", "jpg"],
                            "description": "Image format. Default: png."
                        },
                        "quality": { "type": "integer", "description": "JPEG quality 0-100" },
                        "full_page": { "type": "boolean", "description": "Capture beyond viewport when supported" }
                    },
                    "required": []
                })),
                exposure: FacadeExposure::LlmDefault,
            },
            ToolsetCategory::Web,
        ),
        (
            CapabilityFacade {
                name: "browser_dialog_handle".to_string(),
                description: "Accept or dismiss the current JavaScript dialog in the attached \
                    browser page. **Internal — typically triggered by evaluate results, \
                    not directly by LLM.**"
                    .to_string(),
                connector_id: connector_id.clone(),
                capability_id: CapabilityId::from_string("browser.dialog_handle".to_string())
                    .unwrap(),
                risk_level: RiskLevel::High, // 2026-05-17 (v1.2.9): reverted, see capabilities() network_bound rationale
                effects: vec!["execute".to_string()],
                read_only: false,
                destructive: true,
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "accept": { "type": "boolean", "description": "Accept when true, dismiss when false" },
                        "prompt_text": { "type": "string", "description": "Optional prompt text for prompt dialogs" }
                    },
                    "required": ["accept"]
                })),
                exposure: FacadeExposure::LlmAdvanced,
            },
            ToolsetCategory::Web,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Debug, Clone)]
    struct MockTransport {
        sent: Arc<Mutex<Vec<Value>>>,
        recv: Arc<Mutex<VecDeque<Value>>>,
    }

    impl MockTransport {
        fn new(messages: Vec<Value>) -> Self {
            Self {
                sent: Arc::new(Mutex::new(Vec::new())),
                recv: Arc::new(Mutex::new(messages.into())),
            }
        }

        fn sent(&self) -> Vec<Value> {
            self.sent.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl CdpTransport for MockTransport {
        async fn send_json(&mut self, value: Value) -> anyhow::Result<()> {
            self.sent.lock().unwrap().push(value);
            Ok(())
        }

        async fn recv_json(&mut self) -> anyhow::Result<Value> {
            self.recv
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| anyhow::anyhow!("mock recv exhausted"))
        }
    }

    #[test]
    fn pick_page_ws_url_selects_page_target() {
        let targets = json!([
            {"type":"service_worker","webSocketDebuggerUrl":"ws://worker"},
            {"type":"page","webSocketDebuggerUrl":"ws://page"}
        ]);
        assert_eq!(pick_page_ws_url(&targets).as_deref(), Some("ws://page"));
    }

    #[test]
    fn pick_page_ws_url_returns_none_when_absent() {
        let targets = json!([{"type":"other"}]);
        assert!(pick_page_ws_url(&targets).is_none());
    }

    #[test]
    fn normalize_screenshot_format_accepts_jpg_alias() {
        assert_eq!(normalize_screenshot_format("jpg").unwrap(), "jpeg");
    }

    #[test]
    fn normalize_screenshot_format_rejects_unknown() {
        assert!(normalize_screenshot_format("webp").is_err());
    }

    #[test]
    fn click_script_escapes_selector() {
        let script = click_script(r#"button[data-x="1"]"#, "left", 1).unwrap();
        assert!(script.contains(r#"button[data-x=\"1\"]"#));
    }

    #[test]
    fn click_script_rejects_unknown_button() {
        assert!(click_script("button", "side", 1).is_err());
    }

    #[test]
    fn fill_script_contains_input_events() {
        let script = fill_script("input[name=q]", "hello", true).unwrap();
        assert!(script.contains("new Event(\"input\""));
        assert!(script.contains("new Event(\"change\""));
    }

    #[tokio::test]
    async fn cdp_client_matches_response_by_id_after_event() {
        let transport = MockTransport::new(vec![
            json!({"method":"Page.loadEventFired","params":{}}),
            json!({"id":1,"result":{"ok":true}}),
        ]);
        let sent = transport.clone();
        let mut client = CdpClient::with_transport(transport);
        let result = client.send_command("Page.enable", json!({})).await.unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(sent.sent()[0]["method"], "Page.enable");
    }

    #[tokio::test]
    async fn cdp_client_wait_event_uses_buffered_event() {
        let transport = MockTransport::new(vec![json!({"method":"Page.loadEventFired"})]);
        let mut client = CdpClient::with_transport(transport);
        let msg = client
            .wait_event("Page.loadEventFired", Duration::from_millis(10))
            .await
            .unwrap();
        assert_eq!(msg["method"], "Page.loadEventFired");
    }

    #[tokio::test]
    async fn evaluate_script_returns_cdp_result() {
        let transport = MockTransport::new(vec![json!({
            "id":1,
            "result":{"result":{"type":"number","value":42}}
        })]);
        let mut client = CdpClient::with_transport(transport);
        let out = evaluate_script(&mut client, "1 + 41".to_string(), true, true)
            .await
            .unwrap();
        assert_eq!(out.result["value"], 42);
    }

    #[tokio::test]
    async fn evaluate_script_fails_on_exception_details() {
        let transport = MockTransport::new(vec![json!({
            "id":1,
            "result":{"exceptionDetails":{"text":"boom"}}
        })]);
        let mut client = CdpClient::with_transport(transport);
        let err = evaluate_script(&mut client, "throw new Error()".to_string(), true, true)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Browser evaluation failed"));
    }
}
