use crate::loader::{Loader, ManifestLoader};
use crate::types::{LoadedPackage, PackageSource};
use cyberclaw_core::manifests::PackageKind;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

pub struct EcosystemScanner {
    base_path: PathBuf,
    loader: ManifestLoader,
}

impl EcosystemScanner {
    pub fn new<P: AsRef<Path>>(base_path: P) -> Self {
        let base_path = base_path.as_ref().to_path_buf();
        let loader = ManifestLoader::new();
        Self { base_path, loader }
    }

    /// 根据 PackageKind 获取对应的子目录名
    fn kind_to_subdir(kind: &PackageKind) -> &'static str {
        match kind {
            PackageKind::Agent => "agents",
            PackageKind::Skill => "skills",
            PackageKind::Connector => "connectors",
            PackageKind::PlatformPlugin => "platform-plugins",
        }
    }

    pub async fn scan_all(&self) -> anyhow::Result<Vec<LoadedPackage>> {
        let mut packages = Vec::new();

        // 扫描四类生态对象
        packages.extend(self.scan_kind(PackageKind::Agent).await?);
        packages.extend(self.scan_kind(PackageKind::Skill).await?);
        packages.extend(self.scan_kind(PackageKind::Connector).await?);
        packages.extend(self.scan_kind(PackageKind::PlatformPlugin).await?);

        info!(
            "ecosystem scan completed: {} packages loaded",
            packages.len()
        );

        Ok(packages)
    }

    pub async fn scan_kind(&self, kind: PackageKind) -> anyhow::Result<Vec<LoadedPackage>> {
        let subdir = Self::kind_to_subdir(&kind);
        let kind_path = self.base_path.join(subdir);

        if !tokio::fs::try_exists(&kind_path).await.unwrap_or(false) {
            warn!(
                "ecosystem subdirectory not found: {}, skipping {:?}",
                kind_path.display(),
                kind
            );
            return Ok(Vec::new());
        }

        let metadata = tokio::fs::metadata(&kind_path).await.map_err(|e| {
            anyhow::anyhow!("failed to read metadata for {}: {}", kind_path.display(), e)
        })?;

        if !metadata.is_dir() {
            warn!(
                "ecosystem path is not a directory: {}, skipping {:?}",
                kind_path.display(),
                kind
            );
            return Ok(Vec::new());
        }

        let mut packages = Vec::new();

        // 读取子目录下的所有包目录
        let mut entries = tokio::fs::read_dir(&kind_path).await.map_err(|e| {
            anyhow::anyhow!("failed to read directory {}: {}", kind_path.display(), e)
        })?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| anyhow::anyhow!("failed to read directory entry: {}", e))?
        {
            let path = entry.path();

            // 跳过非目录
            let metadata = match tokio::fs::metadata(&path).await {
                Ok(m) => m,
                Err(e) => {
                    warn!("failed to read metadata for {}: {}", path.display(), e);
                    continue;
                }
            };

            if !metadata.is_dir() {
                debug!("skipping non-directory: {}", path.display());
                continue;
            }

            // 跳过隐藏目录
            if let Some(name) = path.file_name() {
                if name.to_string_lossy().starts_with('.') {
                    debug!("skipping hidden directory: {}", path.display());
                    continue;
                }
            }

            // 尝试加载该包
            let package_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown");

            debug!("attempting to load {:?} package: {}", kind, package_name);

            match self
                .loader
                .load(PackageSource::LocalPath(path.display().to_string()))
                .await
            {
                Ok(package) => {
                    // 验证 kind 匹配
                    if package.manifest.kind != kind {
                        warn!(
                            "package kind mismatch: expected {:?}, got {:?} in {}",
                            kind,
                            package.manifest.kind,
                            path.display()
                        );
                        continue;
                    }

                    info!("loaded {:?} package: {}", kind, package.manifest.id);
                    packages.push(package);
                }
                Err(e) => {
                    warn!("failed to load package from {}: {}", path.display(), e);
                }
            }
        }

        info!(
            "loaded {} {:?} packages from {}",
            packages.len(),
            kind,
            kind_path.display()
        );

        Ok(packages)
    }

    pub async fn scan_agents(&self) -> anyhow::Result<Vec<LoadedPackage>> {
        self.scan_kind(PackageKind::Agent).await
    }

    pub async fn scan_skills(&self) -> anyhow::Result<Vec<LoadedPackage>> {
        self.scan_kind(PackageKind::Skill).await
    }

    pub async fn scan_connectors(&self) -> anyhow::Result<Vec<LoadedPackage>> {
        self.scan_kind(PackageKind::Connector).await
    }

    pub async fn scan_platform_plugins(&self) -> anyhow::Result<Vec<LoadedPackage>> {
        self.scan_kind(PackageKind::PlatformPlugin).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_scan_ecosystem_agents() {
        let scanner = EcosystemScanner::new("../../ecosystem");
        let packages = scanner.scan_agents().await;

        assert!(
            packages.is_ok(),
            "failed to scan agents: {:?}",
            packages.err()
        );

        let packages = packages.unwrap();
        assert!(!packages.is_empty(), "expected at least one agent package");

        // 至少应该有 master-agent 和 report-agent
        let master_agent = packages
            .iter()
            .find(|p| p.manifest.id == "cyberclaw/master-agent");
        assert!(master_agent.is_some(), "master-agent not found");
    }

    #[tokio::test]
    async fn test_scan_ecosystem_skills() {
        let scanner = EcosystemScanner::new("../../ecosystem");
        let packages = scanner.scan_skills().await;

        assert!(
            packages.is_ok(),
            "failed to scan skills: {:?}",
            packages.err()
        );

        let packages = packages.unwrap();
        assert!(!packages.is_empty(), "expected at least one skill package");

        let report_summary = packages.iter().find(|p| p.manifest.id == "report-summary");
        assert!(report_summary.is_some(), "report-summary skill not found");
    }

    #[tokio::test]
    async fn test_scan_ecosystem_connectors() {
        let scanner = EcosystemScanner::new("../../ecosystem");
        let packages = scanner.scan_connectors().await;

        assert!(
            packages.is_ok(),
            "failed to scan connectors: {:?}",
            packages.err()
        );

        let packages = packages.unwrap();
        assert!(
            !packages.is_empty(),
            "expected at least one connector package"
        );

        let github = packages.iter().find(|p| p.manifest.id == "github");
        assert!(github.is_some(), "github connector not found");
    }

    #[tokio::test]
    async fn test_scan_all() {
        let scanner = EcosystemScanner::new("../../ecosystem");
        let packages = scanner.scan_all().await;

        assert!(
            packages.is_ok(),
            "failed to scan ecosystem: {:?}",
            packages.err()
        );

        let packages = packages.unwrap();
        assert!(
            packages.len() >= 3,
            "expected at least 3 packages (agent, skill, connector)"
        );

        // 验证包含不同 kind 的包
        let has_agent = packages
            .iter()
            .any(|p| p.manifest.kind == PackageKind::Agent);
        let has_skill = packages
            .iter()
            .any(|p| p.manifest.kind == PackageKind::Skill);
        let has_connector = packages
            .iter()
            .any(|p| p.manifest.kind == PackageKind::Connector);

        assert!(has_agent, "no agent packages found");
        assert!(has_skill, "no skill packages found");
        assert!(has_connector, "no connector packages found");
    }
}
