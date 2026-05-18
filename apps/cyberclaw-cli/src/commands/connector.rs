//! Connector 管理命令
//!
//! `list` 通过 HTTP 调用 server 的 `/api/v1/connectors`，
//! 显示服务端 ConnectorRegistry 实际注册的所有 connector。
//! `register` 仍走本地 CliState（保留旧测试语义），后续可单独迁移。
//! W3 修复：原版本读 CLI 进程内的 `state.package_registry`（永远空）。

use crate::cli_state::CliState;
use crate::http_client::{get_json, server_url};
use crate::output::OutputFormat;
use anyhow::bail;
use clap::{Args, Subcommand};
use cyberclaw_core::manifests::PackageKind;
use serde::Deserialize;

#[derive(Debug, Subcommand)]
pub enum ConnectorCommand {
    /// 列出已注册的 Connectors
    List(ConnectorListArgs),
    /// 注册新的 Connector
    Register(ConnectorRegisterArgs),
}

#[derive(Debug, Args)]
pub struct ConnectorListArgs {
    /// 输出格式
    #[arg(short, long, value_enum, default_value = "text")]
    pub format: OutputFormat,
    /// CyberClaw server URL（覆盖 CYBERCLAW_SERVER 环境变量）
    #[arg(long)]
    pub server: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ConnectorListResponse {
    #[serde(default)]
    connectors: Vec<ConnectorView>,
}

#[derive(Debug, Deserialize)]
struct ConnectorView {
    #[serde(default)]
    connector_id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    runtime: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    capability_count: u32,
    #[serde(default)]
    risk_level: String,
    #[serde(default)]
    status: String,
}

#[derive(Debug, Args)]
pub struct ConnectorRegisterArgs {
    /// Connector 包路径
    pub path: String,
}

pub async fn handle_connector_command(
    cmd: ConnectorCommand,
    state: &CliState,
) -> anyhow::Result<()> {
    match cmd {
        ConnectorCommand::List(args) => handle_list(args, state).await,
        ConnectorCommand::Register(args) => handle_register(args, state).await,
    }
}

async fn handle_list(args: ConnectorListArgs, _state: &CliState) -> anyhow::Result<()> {
    let server = server_url(args.server.as_deref());
    let resp: ConnectorListResponse =
        get_json(&server, "/api/v1/connectors").await.map_err(|e| {
            anyhow::anyhow!(
                "❌ Could not fetch connectors: {} (is the server running? \
                 check CYBERCLAW_SERVER, or run `cyberclaw chat` first to log in)",
                e
            )
        })?;

    match args.format {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "connectors": resp.connectors.iter().map(|c| serde_json::json!({
                        "connector_id": c.connector_id,
                        "name": c.name,
                        "runtime": c.runtime,
                        "description": c.description,
                        "capability_count": c.capability_count,
                        "risk_level": c.risk_level,
                        "status": c.status,
                    })).collect::<Vec<_>>(),
                    "total": resp.connectors.len(),
                }))?
            );
        }
        OutputFormat::Text => {
            if resp.connectors.is_empty() {
                println!("No connectors registered.");
            } else {
                println!("Total connectors: {}\n", resp.connectors.len());
                for c in &resp.connectors {
                    println!("ID:           {}", c.connector_id);
                    if !c.name.is_empty() && c.name != c.connector_id {
                        println!("Name:         {}", c.name);
                    }
                    if !c.runtime.is_empty() {
                        println!("Runtime:      {}", c.runtime);
                    }
                    if !c.status.is_empty() {
                        println!("Status:       {}", c.status);
                    }
                    if !c.risk_level.is_empty() {
                        println!("Risk:         {}", c.risk_level);
                    }
                    println!("Capabilities: {}", c.capability_count);
                    if !c.description.is_empty() {
                        let preview: String = c.description.chars().take(120).collect();
                        let suffix = if c.description.chars().count() > 120 {
                            "…"
                        } else {
                            ""
                        };
                        println!("Description:  {}{}", preview, suffix);
                    }
                    println!("---");
                }
            }
        }
    }

    Ok(())
}

async fn handle_register(args: ConnectorRegisterArgs, state: &CliState) -> anyhow::Result<()> {
    let loaded = super::package::resolve_package_reference(&args.path).await?;
    if loaded.manifest.kind != PackageKind::Connector {
        bail!(
            "package '{}' is not a connector (kind={:?})",
            loaded.manifest.id,
            loaded.manifest.kind
        );
    }
    let record = super::package::install_loaded_package(loaded, state).await?;
    println!(
        "Registered connector: {} v{}",
        record.id, record.latest_version
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli_state::CliState;
    use cyberclaw_control_plane::{EcosystemScanner, PackageSource};
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_connector_register_from_manifest_path() {
        let state = CliState::new().expect("state");
        let ecosystem_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../ecosystem");
        let scanner = EcosystemScanner::new(&ecosystem_dir);
        let connectors = scanner
            .scan_kind(PackageKind::Connector)
            .await
            .expect("scan connectors");
        let first = connectors
            .into_iter()
            .next()
            .expect("at least one connector");
        let connector_path = match first.source {
            PackageSource::LocalPath(path) => path,
            other => panic!("unexpected connector source: {:?}", other),
        };
        handle_register(
            ConnectorRegisterArgs {
                path: connector_path.clone(),
            },
            &state,
        )
        .await
        .expect("connector register should succeed");

        let loaded = super::super::package::resolve_package_reference(&connector_path)
            .await
            .expect("resolve");
        let registered = state
            .package_registry
            .get(PackageKind::Connector, &loaded.manifest.id)
            .await
            .expect("registry get")
            .expect("connector should exist");
        assert_eq!(
            registered.state,
            cyberclaw_control_plane::RegistryState::Active
        );
    }
}
