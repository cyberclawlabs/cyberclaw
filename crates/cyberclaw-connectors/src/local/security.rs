//! Security capabilities for `LocalConnector`.
//!
//! Currently exposes `security.osv_scan`, which runs `cargo audit --json`
//! against a Cargo.lock and returns a structured vulnerability list.
//!
//! Risk: Low. Effects: Read + Network (queries the OSV / RUSTSEC database
//! over HTTPS). Adding it as Low risk lets `agentic_loop` call it without
//! a review gate; the action is read-only and the output is sanitized
//! before returning.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde_json::Value;
use tokio::process::Command;
use tokio::time::timeout;

use super::LocalConnector;
use crate::types::{CapabilityExecutionRequest, OsvScanInput, OsvScanOutput, OsvVulnerability};

const SCAN_TIMEOUT_SECS: u64 = 60;

pub async fn osv_scan(
    _connector: &LocalConnector,
    request: CapabilityExecutionRequest,
) -> anyhow::Result<Value> {
    let input: OsvScanInput = serde_json::from_value(request.input)?;
    let lockfile = Path::new(&input.lockfile_path);
    if !lockfile.exists() {
        anyhow::bail!("lockfile not found: {}", input.lockfile_path);
    }

    let ecosystem = input.ecosystem.as_deref().unwrap_or_else(|| {
        match lockfile.file_name().and_then(|n| n.to_str()) {
            Some("Cargo.lock") => "cargo",
            Some("package-lock.json") | Some("yarn.lock") => "npm",
            Some("requirements.txt") | Some("Pipfile.lock") => "pypi",
            _ => "unknown",
        }
    });

    if ecosystem != "cargo" {
        // Only Cargo is wired today. Other ecosystems would need a different
        // backend (osv-scanner CLI or the OSV REST API directly).
        let output = OsvScanOutput {
            total: 0,
            vulnerabilities: vec![],
            scanner: "unsupported".to_string(),
            completed: false,
        };
        return Ok(serde_json::to_value(output)?);
    }

    // cargo audit reads Cargo.lock from the directory containing it.
    let workdir = lockfile
        .parent()
        .ok_or_else(|| anyhow::anyhow!("lockfile has no parent directory"))?;

    let cmd_future = Command::new("cargo")
        .arg("audit")
        .arg("--json")
        .current_dir(workdir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();

    let raw = match timeout(Duration::from_secs(SCAN_TIMEOUT_SECS), cmd_future).await {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => anyhow::bail!("failed to spawn cargo audit: {e}"),
        Err(_) => anyhow::bail!("cargo audit timed out after {}s", SCAN_TIMEOUT_SECS),
    };

    let stdout = String::from_utf8_lossy(&raw.stdout);
    let parsed: Value = serde_json::from_str(&stdout).unwrap_or(Value::Null);

    let mut vulns = Vec::new();
    if let Some(arr) = parsed
        .get("vulnerabilities")
        .and_then(|v| v.get("list"))
        .and_then(|v| v.as_array())
    {
        for entry in arr {
            let advisory = entry.get("advisory");
            let pkg = entry.get("package");
            let id = advisory
                .and_then(|a| a.get("id"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let title = advisory
                .and_then(|a| a.get("title"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let pkg_name = pkg
                .and_then(|p| p.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let pkg_version = pkg
                .and_then(|p| p.get("version"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let severity = advisory
                .and_then(|a| a.get("cvss"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            vulns.push(OsvVulnerability {
                package: pkg_name,
                version: pkg_version,
                advisory_id: id,
                title,
                severity,
            });
        }
    }

    let output = OsvScanOutput {
        total: vulns.len(),
        vulnerabilities: vulns,
        scanner: "cargo-audit".to_string(),
        completed: raw.status.success() || raw.status.code() == Some(1), // 1 = vulns found
    };

    Ok(serde_json::to_value(output)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::CapabilityExecutionRequest;
    use cyberclaw_core::identity::{ActorRef, ActorType};
    use cyberclaw_core::ids::{ActorId, CapabilityId, ConnectorId, ExecutionId, WorkspaceId};
    use cyberclaw_core::workspace::{WorkspaceMode, WorkspaceRef};

    fn req_with(input: serde_json::Value) -> CapabilityExecutionRequest {
        CapabilityExecutionRequest {
            execution_id: ExecutionId::new(),
            trace_id: "test-trace".to_string(),
            actor: ActorRef {
                id: ActorId::new(),
                actor_type: ActorType::System,
                tenant_id: None,
                home_node_id: None,
                display_name: "test".to_string(),
            },
            workspace: WorkspaceRef {
                id: WorkspaceId::new(),
                mode: WorkspaceMode::Ephemeral,
                materialization_mode: None,
                home_node_id: None,
                backing_store: None,
                root: "/tmp/test".to_string(),
                writable_roots: vec![],
            },
            connector_id: ConnectorId::from_string("local".to_string()).unwrap(),
            capability_id: CapabilityId::from_string("security.osv_scan".to_string()).unwrap(),
            input,
        }
    }

    #[tokio::test]
    async fn osv_scan_missing_lockfile_errors() {
        let connector = LocalConnector::new(std::path::PathBuf::from("/tmp"));
        let r = req_with(serde_json::json!({
            "lockfile_path": "/nonexistent/Cargo.lock"
        }));
        let result = osv_scan(&connector, r).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("lockfile not found"));
    }

    #[tokio::test]
    async fn osv_scan_unsupported_ecosystem_returns_marker() {
        let tmp = tempfile::TempDir::new().unwrap();
        let lock = tmp.path().join("package-lock.json");
        std::fs::write(&lock, "{}").unwrap();

        let connector = LocalConnector::new(tmp.path().to_path_buf());
        let r = req_with(serde_json::json!({
            "lockfile_path": lock.to_str().unwrap()
        }));
        let result = osv_scan(&connector, r).await.expect("scan ok");
        let output: OsvScanOutput = serde_json::from_value(result).expect("deserializable");
        assert_eq!(output.scanner, "unsupported");
        assert_eq!(output.total, 0);
        assert!(!output.completed);
    }
}
