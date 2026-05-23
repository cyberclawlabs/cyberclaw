//! Package 管理命令
//!
//! v1.x — all subcommands talk to `cyberclaw-server`'s `/api/v2/packages`
//! surface so the registry is shared across CLI invocations and survives
//! a CLI exit. Previously this command path wrote to a process-local
//! `InMemoryRegistry` and "successfully" installed packages evaporated
//! when the CLI process exited.

use crate::cli_state::CliState;
use crate::http_client::{delete, get_json, post_json, server_url};
use crate::output::OutputFormat;
use clap::{Args, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};

/// 包类型过滤参数（用于 --kind 标志）
#[derive(Debug, Clone, ValueEnum)]
pub enum PackageKindFilter {
    Agent,
    Skill,
    Connector,
    Plugin,
}

impl PackageKindFilter {
    fn as_query_str(&self) -> &'static str {
        match self {
            PackageKindFilter::Agent => "agent",
            PackageKindFilter::Skill => "skill",
            PackageKindFilter::Connector => "connector",
            PackageKindFilter::Plugin => "plugin",
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum PackageCommand {
    /// 列出所有已安装的包
    List(PackageListArgs),
    /// 安装新包
    Install(PackageInstallArgs),
    /// 卸载包
    Uninstall(PackageUninstallArgs),
    /// 更新包到最新版本（重新读取 manifest）
    Update(PackageUpdateArgs),
}

#[derive(Debug, Args)]
pub struct PackageListArgs {
    /// 按包类型过滤 (agent/skill/connector/plugin)
    #[arg(short, long, value_enum)]
    pub kind: Option<PackageKindFilter>,

    /// 输出格式
    #[arg(short, long, value_enum, default_value = "text")]
    pub format: OutputFormat,

    /// CyberClaw server URL（覆盖 CYBERCLAW_SERVER 环境变量）
    #[arg(long)]
    pub server: Option<String>,
}

#[derive(Debug, Args)]
pub struct PackageInstallArgs {
    /// 包路径（包含 manifest.yaml 的目录）
    pub package: String,

    /// CyberClaw server URL（覆盖 CYBERCLAW_SERVER 环境变量）
    #[arg(long)]
    pub server: Option<String>,
}

#[derive(Debug, Args)]
pub struct PackageUninstallArgs {
    /// 要卸载的包 ID（如 `cyberclaw/master-agent`）
    pub package: String,

    /// 包类型（agent/skill/connector/plugin）。
    /// 不指定时会从已注册列表中按 id 自动匹配。
    #[arg(short, long, value_enum)]
    pub kind: Option<PackageKindFilter>,

    /// CyberClaw server URL（覆盖 CYBERCLAW_SERVER 环境变量）
    #[arg(long)]
    pub server: Option<String>,
}

#[derive(Debug, Args)]
pub struct PackageUpdateArgs {
    /// 要更新的包 ID（必填；通过 install 命令重读 manifest）
    pub package: String,

    /// 包路径（包含 manifest.yaml 的目录）— 用于重读
    pub path: String,

    /// CyberClaw server URL（覆盖 CYBERCLAW_SERVER 环境变量）
    #[arg(long)]
    pub server: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PackageView {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    id: String,
    #[serde(default)]
    latest_version: String,
    #[serde(default)]
    active_version: Option<String>,
    #[serde(default)]
    state: String,
    #[serde(default)]
    source: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    capability_count: u32,
}

#[derive(Debug, Deserialize)]
struct PackagesResponse {
    #[serde(default)]
    packages: Vec<PackageView>,
    #[serde(default)]
    total: usize,
}

#[derive(Debug, Serialize)]
struct InstallBody {
    path: String,
}

#[derive(Debug, Deserialize)]
struct InstallResponse {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    id: String,
    #[serde(default)]
    version: String,
}

pub async fn handle_package_command(cmd: PackageCommand, _state: &CliState) -> anyhow::Result<()> {
    match cmd {
        PackageCommand::List(args) => handle_list(args).await,
        PackageCommand::Install(args) => handle_install(args).await,
        PackageCommand::Uninstall(args) => handle_uninstall(args).await,
        PackageCommand::Update(args) => handle_update(args).await,
    }
}

async fn handle_list(args: PackageListArgs) -> anyhow::Result<()> {
    let server = server_url(args.server.as_deref());
    let path = match &args.kind {
        Some(k) => format!("/api/v2/packages?kind={}", k.as_query_str()),
        None => "/api/v2/packages".to_string(),
    };
    let resp: PackagesResponse = get_json(&server, &path).await.map_err(|e| {
        anyhow::anyhow!(
            "❌ Could not fetch packages: {} (is the server running? \
             check CYBERCLAW_SERVER, or run `cyberclaw chat` first to log in)",
            e
        )
    })?;

    match args.format {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "packages": resp.packages.iter().map(|p| serde_json::json!({
                        "kind": p.kind,
                        "id": p.id,
                        "latest_version": p.latest_version,
                        "active_version": p.active_version,
                        "state": p.state,
                        "source": p.source,
                        "summary": p.summary,
                        "capability_count": p.capability_count,
                    })).collect::<Vec<_>>(),
                    "total": resp.total,
                }))?
            );
        }
        OutputFormat::Text => {
            if resp.packages.is_empty() {
                println!("No packages installed.");
            } else {
                println!("Total packages: {}\n", resp.total);
                for p in &resp.packages {
                    println!("ID:           {}", p.id);
                    println!("Kind:         {}", p.kind);
                    println!("Version:      {}", p.latest_version);
                    if let Some(active) = &p.active_version {
                        println!("Active:       {}", active);
                    }
                    println!("State:        {}", p.state);
                    println!("Source:       {}", p.source);
                    if p.capability_count > 0 {
                        println!("Capabilities: {}", p.capability_count);
                    }
                    if !p.summary.is_empty() {
                        let preview: String = p.summary.chars().take(120).collect();
                        let suffix = if p.summary.chars().count() > 120 {
                            "…"
                        } else {
                            ""
                        };
                        println!("Summary:      {}{}", preview, suffix);
                    }
                    println!("---");
                }
            }
        }
    }
    Ok(())
}

async fn handle_install(args: PackageInstallArgs) -> anyhow::Result<()> {
    let server = server_url(args.server.as_deref());
    let abs_path = absolutize(&args.package)?;
    let body = InstallBody { path: abs_path };
    let resp: InstallResponse = post_json(&server, "/api/v2/packages", &body)
        .await
        .map_err(|e| anyhow::anyhow!("❌ install failed: {}", e))?;
    println!(
        "Installed package: {} v{} ({})",
        resp.id, resp.version, resp.kind
    );
    Ok(())
}

async fn handle_uninstall(args: PackageUninstallArgs) -> anyhow::Result<()> {
    let server = server_url(args.server.as_deref());

    let kind_label = match args.kind.as_ref() {
        Some(k) => k.as_query_str().to_string(),
        None => find_kind_for_id(&server, &args.package).await?,
    };

    let path = format!(
        "/api/v2/packages/{}/{}",
        urlencode(&kind_label),
        urlencode(&args.package)
    );
    delete(&server, &path)
        .await
        .map_err(|e| anyhow::anyhow!("❌ uninstall failed: {}", e))?;
    println!("Uninstalled package: {} ({})", args.package, kind_label);
    Ok(())
}

async fn handle_update(args: PackageUpdateArgs) -> anyhow::Result<()> {
    // v1.x: update == re-install at the new path (server re-reads manifest +
    // re-activates). No-op fast path is server-side.
    let server = server_url(args.server.as_deref());
    let abs_path = absolutize(&args.path)?;
    let body = InstallBody { path: abs_path };
    let resp: InstallResponse = post_json(&server, "/api/v2/packages", &body)
        .await
        .map_err(|e| anyhow::anyhow!("❌ update failed: {}", e))?;
    if resp.id != args.package {
        anyhow::bail!(
            "manifest at path declares id={}, expected id={} — refusing to update",
            resp.id,
            args.package
        );
    }
    println!("Updated package: {} -> v{}", resp.id, resp.version);
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn find_kind_for_id(server: &str, pkg_id: &str) -> anyhow::Result<String> {
    let resp: PackagesResponse = get_json(server, "/api/v2/packages").await?;
    let matches: Vec<&PackageView> = resp.packages.iter().filter(|p| p.id == pkg_id).collect();
    match matches.as_slice() {
        [] => anyhow::bail!("package not found by id: {}", pkg_id),
        [hit] => Ok(hit.kind.clone()),
        multiple => anyhow::bail!(
            "id `{}` matches {} packages across kinds ({}); pass `--kind` to disambiguate",
            pkg_id,
            multiple.len(),
            multiple
                .iter()
                .map(|p| p.kind.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn absolutize(raw: &str) -> anyhow::Result<String> {
    let p = std::path::PathBuf::from(raw);
    let abs = if p.is_absolute() {
        p
    } else {
        std::env::current_dir()?.join(p)
    };
    Ok(abs.to_string_lossy().into_owned())
}

/// Minimal percent-encode for path segments (only encodes `/`, `?`, `#`, space).
fn urlencode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => out.push(c),
            _ => {
                let mut buf = [0u8; 4];
                for b in c.encode_utf8(&mut buf).as_bytes() {
                    out.push_str(&format!("%{:02X}", b));
                }
            }
        }
    }
    out
}
