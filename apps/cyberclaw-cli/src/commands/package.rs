//! Package 管理命令
//!
//! 支持列出、安装、卸载和更新 Agent/Skill/Connector/Plugin 包。

use crate::cli_state::CliState;
use crate::output::OutputFormat;
use anyhow::Context;
use clap::{Args, Subcommand, ValueEnum};
use cyberclaw_control_plane::{
    EcosystemScanner, LoadedPackage, Loader, ManifestLoader, PackageRecord, PackageSource,
    RegistryState,
};
use cyberclaw_core::manifests::PackageKind;
use std::path::PathBuf;

/// 包类型过滤参数（用于 --kind 标志）
#[derive(Debug, Clone, ValueEnum)]
pub enum PackageKindFilter {
    Agent,
    Skill,
    Connector,
    Plugin,
}

impl From<PackageKindFilter> for PackageKind {
    fn from(f: PackageKindFilter) -> Self {
        match f {
            PackageKindFilter::Agent => PackageKind::Agent,
            PackageKindFilter::Skill => PackageKind::Skill,
            PackageKindFilter::Connector => PackageKind::Connector,
            PackageKindFilter::Plugin => PackageKind::PlatformPlugin,
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
    /// 更新包到最新版本
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
}

#[derive(Debug, Args)]
pub struct PackageInstallArgs {
    /// 包 ID 或路径
    pub package: String,
}

#[derive(Debug, Args)]
pub struct PackageUninstallArgs {
    /// 要卸载的包 ID
    pub package: String,
}

#[derive(Debug, Args)]
pub struct PackageUpdateArgs {
    /// 要更新的包 ID（不指定则更新所有包）
    pub package: Option<String>,
}

pub async fn handle_package_command(cmd: PackageCommand, state: &CliState) -> anyhow::Result<()> {
    match cmd {
        PackageCommand::List(args) => handle_list(args, state).await,
        PackageCommand::Install(args) => handle_install(args, state).await,
        PackageCommand::Uninstall(args) => handle_uninstall(args, state).await,
        PackageCommand::Update(args) => handle_update(args, state).await,
    }
}

async fn handle_list(args: PackageListArgs, state: &CliState) -> anyhow::Result<()> {
    let kind_filter = args.kind.map(PackageKind::from);
    let packages = state.package_registry.list(kind_filter).await?;

    match args.format {
        OutputFormat::Json => {
            println!("{:#?}", packages);
        }
        OutputFormat::Text => {
            if packages.is_empty() {
                println!("No packages installed.");
            } else {
                println!("Total packages: {}\n", packages.len());
                for pkg in packages {
                    println!("ID:      {}", pkg.id);
                    println!("Version: {}", pkg.latest_version);
                    println!("State:   {:?}", pkg.state);
                    println!("Source:  {:?}", pkg.source);
                    if let Some(active) = &pkg.active_version {
                        println!("Active:  {}", active);
                    }
                    println!("---");
                }
            }
        }
    }

    Ok(())
}

async fn handle_install(args: PackageInstallArgs, state: &CliState) -> anyhow::Result<()> {
    let loaded = resolve_package_reference(&args.package).await?;
    let record = install_loaded_package(loaded, state).await?;
    println!(
        "Installed package: {} v{} ({:?})",
        record.id, record.latest_version, record.kind
    );
    Ok(())
}

async fn handle_uninstall(args: PackageUninstallArgs, state: &CliState) -> anyhow::Result<()> {
    let mut record = find_package_by_id(&args.package, state).await?;
    record.state = RegistryState::Disabled;
    record.active_version = None;
    state.package_registry.upsert(record.clone()).await?;
    println!("Uninstalled package: {} ({:?})", record.id, record.kind);
    Ok(())
}

async fn handle_update(args: PackageUpdateArgs, state: &CliState) -> anyhow::Result<()> {
    let mut updated = 0usize;
    if let Some(package_id) = args.package {
        let record = find_package_by_id(&package_id, state).await?;
        let refreshed = refresh_package_from_source(record).await?;
        state.package_registry.upsert(refreshed.clone()).await?;
        state
            .package_registry
            .activate(
                refreshed.kind.clone(),
                &refreshed.id,
                &refreshed.latest_version,
            )
            .await?;
        println!(
            "Updated package: {} -> v{}",
            refreshed.id, refreshed.latest_version
        );
        updated = 1;
    } else {
        let records = state.package_registry.list(None).await?;
        for record in records {
            let refreshed = refresh_package_from_source(record).await?;
            state.package_registry.upsert(refreshed.clone()).await?;
            state
                .package_registry
                .activate(
                    refreshed.kind.clone(),
                    &refreshed.id,
                    &refreshed.latest_version,
                )
                .await?;
            updated += 1;
        }
        println!("Updated all packages: {}", updated);
    }
    if updated == 0 {
        println!("No packages were updated.");
    }
    Ok(())
}

fn default_ecosystem_dir() -> PathBuf {
    std::env::var("CYBERCLAW_ECOSYSTEM_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../ecosystem"))
}

pub(crate) async fn resolve_package_reference(package_ref: &str) -> anyhow::Result<LoadedPackage> {
    let candidate_path = PathBuf::from(package_ref);
    if candidate_path.exists() {
        let loader = ManifestLoader::new();
        let source = PackageSource::LocalPath(candidate_path.to_string_lossy().into_owned());
        return loader.load(source).await;
    }

    let scanner = EcosystemScanner::new(default_ecosystem_dir());
    let packages = scanner
        .scan_all()
        .await
        .context("failed to scan ecosystem for package install")?;
    packages
        .into_iter()
        .find(|p| p.manifest.id == package_ref)
        .ok_or_else(|| anyhow::anyhow!("package not found by id or path: {}", package_ref))
}

pub(crate) async fn install_loaded_package(
    loaded: LoadedPackage,
    state: &CliState,
) -> anyhow::Result<PackageRecord> {
    let record = PackageRecord {
        kind: loaded.manifest.kind.clone(),
        id: loaded.manifest.id.clone(),
        latest_version: loaded.manifest.version.clone(),
        installed_versions: vec![loaded.manifest.version.clone()],
        active_version: Some(loaded.manifest.version.clone()),
        source: loaded.source.clone(),
        state: RegistryState::Active,
        available_nodes: Vec::new(),
        runtime_requirements: loaded.manifest.compatibility.runtime.clone(),
        manifest: loaded.manifest.clone(),
    };

    state.package_registry.upsert(record.clone()).await?;
    state
        .package_registry
        .activate(record.kind.clone(), &record.id, &record.latest_version)
        .await?;
    Ok(record)
}

async fn find_package_by_id(id: &str, state: &CliState) -> anyhow::Result<PackageRecord> {
    let records = state.package_registry.list(None).await?;
    records
        .into_iter()
        .find(|record| record.id == id)
        .ok_or_else(|| anyhow::anyhow!("package not found: {}", id))
}

async fn refresh_package_from_source(mut record: PackageRecord) -> anyhow::Result<PackageRecord> {
    if let PackageSource::LocalPath(path) = &record.source {
        let local_path = PathBuf::from(path);
        if local_path.exists() {
            let loader = ManifestLoader::new();
            let loaded = loader
                .load(PackageSource::LocalPath(path.clone()))
                .await
                .with_context(|| format!("failed to reload package from source {}", path))?;
            record.latest_version = loaded.manifest.version.clone();
            if !record
                .installed_versions
                .iter()
                .any(|v| v == &record.latest_version)
            {
                record
                    .installed_versions
                    .push(record.latest_version.clone());
            }
            record.runtime_requirements = loaded.manifest.compatibility.runtime.clone();
            record.manifest = loaded.manifest;
        }
    }
    record.state = RegistryState::Active;
    if record.active_version.as_deref() != Some(record.latest_version.as_str()) {
        record.active_version = Some(record.latest_version.clone());
    }
    Ok(record)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli_state::CliState;

    fn repo_ecosystem_path(segment: &str) -> String {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../ecosystem")
            .join(segment)
            .to_string_lossy()
            .into_owned()
    }

    #[tokio::test]
    async fn test_install_uninstall_update_package_lifecycle() {
        let state = CliState::new().expect("state");
        let package_path = repo_ecosystem_path("agents/master-agent");

        handle_install(
            PackageInstallArgs {
                package: package_path.clone(),
            },
            &state,
        )
        .await
        .expect("install should succeed");

        let loaded = resolve_package_reference(&package_path)
            .await
            .expect("resolve");
        let package_id = loaded.manifest.id.clone();
        let installed = find_package_by_id(&package_id, &state)
            .await
            .expect("installed package should exist");
        assert_eq!(installed.state, RegistryState::Active);
        assert!(installed.active_version.is_some());

        handle_uninstall(
            PackageUninstallArgs {
                package: package_id.clone(),
            },
            &state,
        )
        .await
        .expect("uninstall should succeed");
        let uninstalled = find_package_by_id(&package_id, &state)
            .await
            .expect("uninstalled package should still exist");
        assert_eq!(uninstalled.state, RegistryState::Disabled);
        assert!(uninstalled.active_version.is_none());

        handle_update(
            PackageUpdateArgs {
                package: Some(package_id.clone()),
            },
            &state,
        )
        .await
        .expect("update should succeed");
        let updated = find_package_by_id(&package_id, &state)
            .await
            .expect("updated package should exist");
        assert_eq!(updated.state, RegistryState::Active);
        assert!(updated.active_version.is_some());
    }
}
