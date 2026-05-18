//! Plugin registry for managing registered plugins

use crate::error::{PluginError, Result};
use crate::loader::PluginLoader;
use crate::manifest::{PluginKind, PluginManifest};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

/// Plugin registry for managing all plugins
#[derive(Clone)]
pub struct PluginRegistry {
    /// Registered plugin manifests
    manifests: Arc<RwLock<HashMap<String, PluginManifest>>>,

    /// Plugin loader
    loader: Arc<PluginLoader>,

    /// Plugin directory
    #[allow(dead_code)]
    plugin_dir: PathBuf,
}

impl PluginRegistry {
    /// Create a new plugin registry
    pub fn new(plugin_dir: PathBuf) -> Self {
        let loader = Arc::new(PluginLoader::new(plugin_dir.clone()));

        Self {
            manifests: Arc::new(RwLock::new(HashMap::new())),
            loader,
            plugin_dir,
        }
    }

    /// Register a plugin manifest
    pub async fn register(&self, manifest: PluginManifest) -> Result<()> {
        let plugin_id = manifest.id.clone();

        // Validate manifest
        manifest.validate()?;

        // Check if already registered
        {
            let manifests = self.manifests.read().await;
            if manifests.contains_key(&plugin_id) {
                return Err(PluginError::AlreadyRegistered { id: plugin_id });
            }
        }

        info!("Registering plugin: {} v{}", plugin_id, manifest.version);

        // Store manifest
        {
            let mut manifests = self.manifests.write().await;
            manifests.insert(plugin_id.clone(), manifest.clone());
        }

        // Try to load the plugin
        if let Err(e) = self.loader.load(&manifest).await {
            // Remove from registry if loading fails
            let mut manifests = self.manifests.write().await;
            manifests.remove(&plugin_id);
            return Err(e);
        }

        info!("Successfully registered plugin: {}", plugin_id);
        Ok(())
    }

    /// Unregister a plugin
    pub async fn unregister(&self, id: &str) -> Result<()> {
        info!("Unregistering plugin: {}", id);

        // Remove manifest
        let removed = {
            let mut manifests = self.manifests.write().await;
            manifests.remove(id)
        };

        if removed.is_none() {
            return Err(PluginError::NotFound { id: id.to_string() });
        }

        // Unload the plugin
        self.loader.unload(id).await?;

        info!("Successfully unregistered plugin: {}", id);
        Ok(())
    }

    /// List all registered plugins
    pub async fn list(&self) -> Vec<PluginManifest> {
        let manifests = self.manifests.read().await;
        manifests.values().cloned().collect()
    }

    /// List plugins by kind
    pub async fn list_by_kind(&self, kind: PluginKind) -> Vec<PluginManifest> {
        let manifests = self.manifests.read().await;
        manifests
            .values()
            .filter(|m| m.kind == kind)
            .cloned()
            .collect()
    }

    /// Get a specific plugin manifest
    pub async fn get(&self, id: &str) -> Option<PluginManifest> {
        let manifests = self.manifests.read().await;
        manifests.get(id).cloned()
    }

    /// Execute a plugin
    pub async fn execute(&self, id: &str, input: serde_json::Value) -> Result<serde_json::Value> {
        // Check if registered
        {
            let manifests = self.manifests.read().await;
            if !manifests.contains_key(id) {
                return Err(PluginError::NotFound { id: id.to_string() });
            }
        }

        // Execute through loader
        self.loader.execute(id, input).await
    }

    /// Discover and register plugins from directory
    pub async fn discover_and_register(&self, path: &PathBuf) -> Result<Vec<String>> {
        info!("Discovering plugins in: {:?}", path);

        let manifests = self.loader.discover(path).await?;
        let mut registered = Vec::new();

        for manifest in manifests {
            match self.register(manifest.clone()).await {
                Ok(_) => {
                    registered.push(manifest.id);
                }
                Err(e) => {
                    warn!("Failed to register plugin {}: {}", manifest.id, e);
                }
            }
        }

        info!("Registered {} plugins", registered.len());
        Ok(registered)
    }

    /// Initialize a plugin
    pub async fn initialize(&self, id: &str) -> Result<()> {
        // Check if registered
        {
            let manifests = self.manifests.read().await;
            if !manifests.contains_key(id) {
                return Err(PluginError::NotFound { id: id.to_string() });
            }
        }

        self.loader.initialize(id).await
    }

    /// Check if a plugin is registered
    pub async fn is_registered(&self, id: &str) -> bool {
        let manifests = self.manifests.read().await;
        manifests.contains_key(id)
    }

    /// Get registry statistics
    pub async fn stats(&self) -> RegistryStats {
        let manifests = self.manifests.read().await;

        let mut stats = RegistryStats {
            total_registered: manifests.len(),
            ..Default::default()
        };

        for manifest in manifests.values() {
            match manifest.kind {
                PluginKind::Agent => stats.agents += 1,
                PluginKind::Skill => stats.skills += 1,
                PluginKind::Connector => stats.connectors += 1,
                PluginKind::Platform => stats.platform += 1,
            }
        }

        stats.total_loaded = self.loader.list_loaded().await.len();
        stats
    }

    /// Validate plugin dependencies
    pub async fn validate_dependencies(&self, manifest: &PluginManifest) -> Result<()> {
        for dep in &manifest.dependencies {
            if !dep.optional && !self.is_registered(&dep.name).await {
                return Err(PluginError::DependencyNotSatisfied {
                    dependency: dep.name.clone(),
                });
            }
        }
        Ok(())
    }

    /// Get plugin dependency graph
    pub async fn dependency_graph(&self) -> HashMap<String, Vec<String>> {
        let manifests = self.manifests.read().await;
        let mut graph = HashMap::new();

        for (id, manifest) in manifests.iter() {
            let deps: Vec<String> = manifest
                .dependencies
                .iter()
                .map(|d| d.name.clone())
                .collect();
            graph.insert(id.clone(), deps);
        }

        graph
    }
}

/// Registry statistics
#[derive(Debug, Default, Clone)]
pub struct RegistryStats {
    /// Total registered plugins
    pub total_registered: usize,

    /// Total loaded plugins
    pub total_loaded: usize,

    /// Number of agent plugins
    pub agents: usize,

    /// Number of skill plugins
    pub skills: usize,

    /// Number of connector plugins
    pub connectors: usize,

    /// Number of platform plugins
    pub platform: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_registry_creation() {
        let temp_dir = TempDir::new().unwrap();
        let registry = PluginRegistry::new(temp_dir.path().to_path_buf());

        let plugins = registry.list().await;
        assert_eq!(plugins.len(), 0);
    }

    #[tokio::test]
    async fn test_registry_register() {
        let temp_dir = TempDir::new().unwrap();
        let registry = PluginRegistry::new(temp_dir.path().to_path_buf());

        let manifest = PluginManifest {
            id: "test-plugin".to_string(),
            version: semver::Version::new(1, 0, 0),
            kind: PluginKind::Agent,
            name: "Test Plugin".to_string(),
            description: "A test plugin".to_string(),
            entry_point: "test_plugin.so".to_string(),
            dependencies: vec![],
            permissions: vec![],
            metadata: HashMap::new(),
            author: None,
            license: None,
        };

        // Note: This will fail in tests because the library doesn't exist
        // In production, the library would be present
        let result = registry.register(manifest.clone()).await;
        assert!(result.is_err()); // Expected to fail due to missing library

        // But we can verify the error is correct
        match result {
            Err(PluginError::LibraryLoading(_)) => {
                // Expected error
            }
            _ => panic!("Unexpected error type"),
        }
    }

    #[tokio::test]
    async fn test_registry_stats() {
        let temp_dir = TempDir::new().unwrap();
        let registry = PluginRegistry::new(temp_dir.path().to_path_buf());

        let stats = registry.stats().await;
        assert_eq!(stats.total_registered, 0);
        assert_eq!(stats.agents, 0);
        assert_eq!(stats.skills, 0);
        assert_eq!(stats.connectors, 0);
    }
}
