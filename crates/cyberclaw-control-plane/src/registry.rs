use crate::types::PackageRecord;
use async_trait::async_trait;
use cyberclaw_core::manifests::{CapabilityContract, PackageKind};
use cyberclaw_core::prelude::{CapabilityId, ConnectorId};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[async_trait]
pub trait Registry: Send + Sync {
    async fn upsert(&self, package: PackageRecord) -> anyhow::Result<()>;
    async fn get(&self, kind: PackageKind, id: &str) -> anyhow::Result<Option<PackageRecord>>;
    async fn list(&self, kind: Option<PackageKind>) -> anyhow::Result<Vec<PackageRecord>>;
    async fn activate(&self, kind: PackageKind, id: &str, version: &str) -> anyhow::Result<()>;

    // Capability-level queries
    async fn get_capability(
        &self,
        capability_id: &CapabilityId,
    ) -> anyhow::Result<Option<(ConnectorId, CapabilityContract)>>;
    async fn find_connector_for_capability(
        &self,
        capability_id: &CapabilityId,
    ) -> anyhow::Result<Option<ConnectorId>>;
    async fn list_capabilities(&self) -> anyhow::Result<Vec<(ConnectorId, CapabilityId)>>;
}

#[derive(Clone, Default)]
pub struct InMemoryRegistry {
    records: Arc<RwLock<BTreeMap<(PackageKind, String), PackageRecord>>>,
}

impl InMemoryRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 批量加载 LoadedPackage 到 Registry
    pub async fn load_packages(
        &self,
        packages: Vec<crate::types::LoadedPackage>,
    ) -> anyhow::Result<usize> {
        let mut count = 0;
        for package in packages {
            let record = PackageRecord {
                kind: package.manifest.kind.clone(),
                id: package.manifest.id.clone(),
                latest_version: package.manifest.version.clone(),
                installed_versions: vec![package.manifest.version.clone()],
                active_version: Some(package.manifest.version.clone()),
                source: package.source.clone(),
                state: crate::types::RegistryState::Active,
                available_nodes: Vec::new(),
                runtime_requirements: package.manifest.compatibility.runtime.clone(),
                manifest: package.manifest.clone(),
            };
            self.upsert(record).await?;
            count += 1;
        }
        Ok(count)
    }
}

#[async_trait]
impl Registry for InMemoryRegistry {
    async fn upsert(&self, package: PackageRecord) -> anyhow::Result<()> {
        let key = (package.kind.clone(), package.id.clone());
        let mut entries = self.records.write().await;
        entries.insert(key, package);
        Ok(())
    }

    async fn get(&self, kind: PackageKind, id: &str) -> anyhow::Result<Option<PackageRecord>> {
        let entries = self.records.read().await;
        Ok(entries.get(&(kind, id.to_string())).cloned())
    }

    async fn list(&self, kind: Option<PackageKind>) -> anyhow::Result<Vec<PackageRecord>> {
        let entries = self.records.read().await;

        let packages = entries
            .values()
            .filter(|record| {
                kind.as_ref()
                    .map(|expected| expected == &record.kind)
                    .unwrap_or(true)
            })
            .cloned()
            .collect();

        Ok(packages)
    }

    async fn activate(&self, kind: PackageKind, id: &str, version: &str) -> anyhow::Result<()> {
        let mut entries = self.records.write().await;

        let Some(record) = entries.get_mut(&(kind, id.to_string())) else {
            anyhow::bail!("package not found: {}", id);
        };

        if !record.installed_versions.iter().any(|item| item == version) {
            record.installed_versions.push(version.to_string());
        }

        record.active_version = Some(version.to_string());
        record.state = crate::types::RegistryState::Active;
        Ok(())
    }

    async fn get_capability(
        &self,
        capability_id: &CapabilityId,
    ) -> anyhow::Result<Option<(ConnectorId, CapabilityContract)>> {
        let entries = self.records.read().await;

        for (key, record) in entries.iter() {
            if key.0 == PackageKind::Connector {
                if let cyberclaw_core::manifests::PackageSpec::Connector(spec) =
                    &record.manifest.spec
                {
                    for cap in &spec.capabilities {
                        if cap.id == capability_id.as_str() {
                            return Ok(Some((
                                ConnectorId::from_string(record.id.clone()).unwrap(),
                                cap.clone(),
                            )));
                        }
                    }
                }
            }
        }

        Ok(None)
    }

    async fn find_connector_for_capability(
        &self,
        capability_id: &CapabilityId,
    ) -> anyhow::Result<Option<ConnectorId>> {
        let entries = self.records.read().await;

        for (key, record) in entries.iter() {
            if key.0 == PackageKind::Connector
                && record.state == crate::types::RegistryState::Active
            {
                if let cyberclaw_core::manifests::PackageSpec::Connector(spec) =
                    &record.manifest.spec
                {
                    for cap in &spec.capabilities {
                        if cap.id == capability_id.as_str() {
                            return Ok(Some(ConnectorId::from_string(record.id.clone()).unwrap()));
                        }
                    }
                }
            }
        }

        Ok(None)
    }

    async fn list_capabilities(&self) -> anyhow::Result<Vec<(ConnectorId, CapabilityId)>> {
        let entries = self.records.read().await;
        let mut result = Vec::new();

        for (key, record) in entries.iter() {
            if key.0 == PackageKind::Connector
                && record.state == crate::types::RegistryState::Active
            {
                if let cyberclaw_core::manifests::PackageSpec::Connector(spec) =
                    &record.manifest.spec
                {
                    for cap in &spec.capabilities {
                        result.push((
                            ConnectorId::from_string(record.id.clone()).unwrap(),
                            CapabilityId::from_string(cap.id.clone()).unwrap(),
                        ));
                    }
                }
            }
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecosystem_scanner::EcosystemScanner;

    #[tokio::test]
    async fn test_registry_load_packages() {
        let scanner = EcosystemScanner::new("../../ecosystem");
        let packages = scanner.scan_all().await;
        assert!(
            packages.is_ok(),
            "failed to scan ecosystem: {:?}",
            packages.err()
        );

        let packages = packages.unwrap();
        let registry = InMemoryRegistry::new();
        let result = registry.load_packages(packages).await;

        assert!(
            result.is_ok(),
            "failed to load packages: {:?}",
            result.err()
        );
        let count = result.unwrap();
        assert!(
            count >= 3,
            "expected at least 3 packages loaded, got {}",
            count
        );

        // 验证可以从 registry 检索包
        let agents = registry.list(Some(PackageKind::Agent)).await.unwrap();
        assert!(!agents.is_empty(), "no agents found in registry");

        let skills = registry.list(Some(PackageKind::Skill)).await.unwrap();
        assert!(!skills.is_empty(), "no skills found in registry");

        let connectors = registry.list(Some(PackageKind::Connector)).await.unwrap();
        assert!(!connectors.is_empty(), "no connectors found in registry");
    }

    #[tokio::test]
    async fn test_registry_get_specific_package() {
        let scanner = EcosystemScanner::new("../../ecosystem");
        let packages = scanner.scan_all().await.unwrap();

        let registry = InMemoryRegistry::new();
        registry.load_packages(packages).await.unwrap();

        // 测试获取特定包
        let master_agent = registry
            .get(PackageKind::Agent, "cyberclaw/master-agent")
            .await
            .unwrap();

        assert!(master_agent.is_some(), "master-agent not found in registry");
        let master_agent = master_agent.unwrap();
        assert_eq!(master_agent.id, "cyberclaw/master-agent");
        assert_eq!(master_agent.kind, PackageKind::Agent);
        assert_eq!(master_agent.state, crate::types::RegistryState::Active);
        assert!(master_agent.active_version.is_some());
    }
}
