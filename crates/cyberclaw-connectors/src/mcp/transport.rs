//! MCP transport layer implementations

use super::protocol::{McpRequest, McpResponse};
use super::types::TransportConfig;
use anyhow::{Context, Result};
use async_trait::async_trait;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

/// Transport trait for MCP protocol
#[async_trait]
pub trait McpTransport: Send + Sync {
    /// Send a request and receive a response
    async fn send(&self, request: McpRequest) -> Result<McpResponse>;

    /// Close the transport
    async fn close(&mut self) -> Result<()>;
}

/// Standard I/O transport (spawns a child process)
pub struct StdioTransport {
    /// Child process
    child: Mutex<Option<Child>>,
    /// Command to execute
    command: String,
    /// Command arguments
    args: Vec<String>,
    /// Working directory
    workdir: Option<String>,
}

impl StdioTransport {
    /// Create a new stdio transport
    pub fn new(command: String, args: Vec<String>, workdir: Option<String>) -> Self {
        Self {
            child: Mutex::new(None),
            command,
            args,
            workdir,
        }
    }

    /// Start the child process
    async fn ensure_started(&self) -> Result<()> {
        let mut guard = self.child.lock().await;

        if guard.is_none() {
            let mut cmd = Command::new(&self.command);
            cmd.args(&self.args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);

            if let Some(ref workdir) = self.workdir {
                cmd.current_dir(workdir);
            }

            let child = cmd
                .spawn()
                .with_context(|| format!("Failed to spawn MCP server: {}", self.command))?;

            *guard = Some(child);
        }

        Ok(())
    }
}

#[async_trait]
impl McpTransport for StdioTransport {
    async fn send(&self, request: McpRequest) -> Result<McpResponse> {
        self.ensure_started().await?;

        let mut guard = self.child.lock().await;
        let child = guard.as_mut().context("Child process not started")?;

        // Get stdin and stdout
        let stdin = child.stdin.as_mut().context("Failed to get stdin")?;
        let stdout = child.stdout.as_mut().context("Failed to get stdout")?;

        // Serialize request as JSON
        let request_json = serde_json::to_string(&request)?;
        tracing::debug!("MCP Request: {}", request_json);

        // Write request + newline
        stdin.write_all(request_json.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        stdin.flush().await?;

        // Read response line
        let mut reader = BufReader::new(stdout);
        let mut response_line = String::new();
        reader.read_line(&mut response_line).await?;

        tracing::debug!("MCP Response: {}", response_line);

        // Parse response
        let response: McpResponse = serde_json::from_str(&response_line)
            .with_context(|| format!("Failed to parse MCP response: {}", response_line))?;

        Ok(response)
    }

    async fn close(&mut self) -> Result<()> {
        let mut guard = self.child.lock().await;

        if let Some(mut child) = guard.take() {
            // Send SIGTERM
            #[cfg(unix)]
            {
                use nix::sys::signal::{kill, Signal};
                use nix::unistd::Pid;

                if let Some(pid) = child.id() {
                    let _ = kill(Pid::from_raw(pid as i32), Signal::SIGTERM);
                }
            }

            // Wait for exit with timeout
            tokio::select! {
                _ = child.wait() => {}
                _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {
                    // Force kill if not exited
                    let _ = child.kill().await;
                }
            }
        }

        Ok(())
    }
}

/// HTTP/HTTPS transport
pub struct HttpTransport {
    /// HTTP client
    client: reqwest::Client,
    /// Server URL
    url: String,
}

impl HttpTransport {
    /// Create a new HTTP transport
    pub fn new(url: String, headers: std::collections::HashMap<String, String>) -> Result<Self> {
        let client_builder = reqwest::Client::builder();
        #[cfg(test)]
        let client_builder = client_builder.no_proxy();
        let mut client_builder = client_builder;

        // Add custom headers
        let mut header_map = reqwest::header::HeaderMap::new();
        for (key, value) in headers {
            header_map.insert(
                reqwest::header::HeaderName::from_bytes(key.as_bytes())?,
                reqwest::header::HeaderValue::from_str(&value)?,
            );
        }

        client_builder = client_builder.default_headers(header_map);

        let client = client_builder.build()?;

        Ok(Self { client, url })
    }
}

#[async_trait]
impl McpTransport for HttpTransport {
    async fn send(&self, request: McpRequest) -> Result<McpResponse> {
        tracing::debug!("MCP HTTP Request to {}: {:?}", self.url, request);

        let response = self
            .client
            .post(&self.url)
            .json(&request)
            .send()
            .await
            .with_context(|| format!("Failed to send MCP request to {}", self.url))?;

        let status = response.status();
        if !status.is_success() {
            anyhow::bail!("MCP server returned error status: {}", status);
        }

        let response_body = response
            .json::<McpResponse>()
            .await
            .context("Failed to parse MCP response")?;

        tracing::debug!("MCP HTTP Response: {:?}", response_body);

        Ok(response_body)
    }

    async fn close(&mut self) -> Result<()> {
        // HTTP transport doesn't need explicit cleanup
        Ok(())
    }
}

/// Create transport from config
pub fn create_transport(config: &TransportConfig) -> Result<Box<dyn McpTransport>> {
    match config {
        TransportConfig::Stdio {
            command,
            args,
            workdir,
        } => Ok(Box::new(StdioTransport::new(
            command.clone(),
            args.clone(),
            workdir.clone(),
        ))),
        TransportConfig::Http { url, headers } => {
            Ok(Box::new(HttpTransport::new(url.clone(), headers.clone())?))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_stdio_transport() {
        let config = TransportConfig::Stdio {
            command: "echo".to_string(),
            args: vec!["test".to_string()],
            workdir: None,
        };

        let transport = create_transport(&config);
        assert!(transport.is_ok());
    }

    #[test]
    fn test_create_http_transport() {
        let config = TransportConfig::Http {
            url: "http://localhost:8080".to_string(),
            headers: std::collections::HashMap::new(),
        };

        let transport = create_transport(&config);
        assert!(transport.is_ok());
    }

    #[tokio::test]
    async fn test_http_transport_creation() {
        let mut headers = std::collections::HashMap::new();
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        let transport = HttpTransport::new("http://localhost:8080".to_string(), headers);
        assert!(transport.is_ok());
    }
}
