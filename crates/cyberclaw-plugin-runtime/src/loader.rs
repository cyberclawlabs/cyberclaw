//! Plugin loader implementation

use crate::error::{PluginError, Result};
use crate::manifest::PluginManifest;
use crate::sandbox::SandboxManager;
use libloading::Library;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, error, info, warn};

/// Plugin loader for dynamic library loading
pub struct PluginLoader {
    /// Loaded plugins
    plugins: Arc<RwLock<HashMap<String, LoadedPlugin>>>,

    /// Plugin directory
    plugin_dir: PathBuf,

    /// Sandbox manager
    sandbox_manager: Arc<RwLock<SandboxManager>>,
}

/// A loaded plugin instance
struct LoadedPlugin {
    /// Plugin manifest
    manifest: PluginManifest,

    /// Plugin instance created from dynamic library constructor
    instance: Arc<Mutex<Box<dyn Plugin>>>,

    /// Loaded library
    #[allow(dead_code)]
    library: Arc<Library>,

    /// Plugin state
    state: PluginState,
}

/// Plugin state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
enum PluginState {
    /// Plugin is loaded but not initialized
    Loaded,

    /// Plugin is initialized and ready
    Ready,

    /// Plugin is paused
    Paused,

    /// Plugin has errors
    Error,
}

/// Plugin interface that all plugins must implement
pub trait Plugin: Send + Sync {
    /// Initialize the plugin
    fn init(&mut self) -> Result<()>;

    /// Execute plugin functionality
    fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value>;

    /// Shutdown the plugin
    fn shutdown(&mut self) -> Result<()>;

    /// Get plugin metadata
    fn metadata(&self) -> &PluginManifest;
}

/// Type alias for plugin constructor function
#[allow(dead_code, improper_ctypes_definitions)]
type PluginConstructor = unsafe extern "C" fn() -> *mut dyn Plugin;

impl PluginLoader {
    /// Create a new plugin loader
    pub fn new(plugin_dir: PathBuf) -> Self {
        Self {
            plugins: Arc::new(RwLock::new(HashMap::new())),
            plugin_dir,
            sandbox_manager: Arc::new(RwLock::new(SandboxManager::new())),
        }
    }

    /// Discover plugins in the configured directory
    pub async fn discover(&self, path: &Path) -> Result<Vec<PluginManifest>> {
        let search_path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.plugin_dir.join(path)
        };

        debug!("Discovering plugins in: {:?}", search_path);

        let mut manifests = Vec::new();

        if !search_path.exists() {
            warn!("Plugin directory does not exist: {:?}", search_path);
            return Ok(manifests);
        }

        // Look for plugin.json files
        let entries = std::fs::read_dir(&search_path)?;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                // Check for plugin.json in subdirectory
                let manifest_path = path.join("plugin.json");
                if manifest_path.exists() {
                    match PluginManifest::from_file(&manifest_path) {
                        Ok(manifest) => {
                            info!("Discovered plugin: {} v{}", manifest.id, manifest.version);
                            manifests.push(manifest);
                        }
                        Err(e) => {
                            error!("Failed to load manifest from {:?}: {}", manifest_path, e);
                        }
                    }
                }
            } else if path.file_name() == Some("plugin.json".as_ref()) {
                // Direct plugin.json file
                match PluginManifest::from_file(&path) {
                    Ok(manifest) => {
                        info!("Discovered plugin: {} v{}", manifest.id, manifest.version);
                        manifests.push(manifest);
                    }
                    Err(e) => {
                        error!("Failed to load manifest from {:?}: {}", path, e);
                    }
                }
            }
        }

        Ok(manifests)
    }

    /// Load a plugin from its manifest
    pub async fn load(&self, manifest: &PluginManifest) -> Result<()> {
        let plugin_id = manifest.id.clone();

        // Check if already loaded
        {
            let plugins = self.plugins.read().await;
            if plugins.contains_key(&plugin_id) {
                return Err(PluginError::AlreadyRegistered { id: plugin_id });
            }
        }

        info!("Loading plugin: {} v{}", plugin_id, manifest.version);

        // Resolve library path with path traversal prevention
        let lib_path = self.resolve_library_path(&manifest.entry_point)?;

        // Security: reject paths that escape the plugin directory via traversal.
        // We canonicalize the parent directory (which must exist) and check that
        // the resolved path stays within the plugin directory boundary.
        if let Ok(canonical_plugin_dir) = self.plugin_dir.canonicalize() {
            let parent = lib_path.parent().unwrap_or(&lib_path);
            if let Ok(canonical_parent) = parent.canonicalize() {
                if !canonical_parent.starts_with(&canonical_plugin_dir) {
                    return Err(PluginError::LoadFailed {
                        message: format!(
                            "plugin entry_point escapes plugin directory: {:?}",
                            manifest.entry_point
                        ),
                    });
                }
            }
            // If parent can't be canonicalized, the load will fail naturally below
        }

        // Create sandbox BEFORE loading native code — ensures isolation is
        // established before any untrusted code can execute.
        {
            let mut sandbox_manager = self.sandbox_manager.write().await;
            sandbox_manager.create_sandbox(plugin_id.clone(), manifest.permissions.clone())?;
        }

        // Load the library (after sandbox is in place)
        let library = unsafe {
            Library::new(&lib_path).map_err(|e| PluginError::LibraryLoading(e.to_string()))?
        };

        let plugin_instance = unsafe {
            let constructor: libloading::Symbol<PluginConstructor> =
                library.get(b"create_plugin\0").map_err(|e| {
                    PluginError::LibraryLoading(format!("missing create_plugin: {}", e))
                })?;
            let raw = constructor();
            if raw.is_null() {
                return Err(PluginError::LoadFailed {
                    message: format!("plugin constructor returned null pointer: {}", plugin_id),
                });
            }
            Box::from_raw(raw)
        };

        // Store loaded plugin
        let loaded_plugin = LoadedPlugin {
            manifest: manifest.clone(),
            instance: Arc::new(Mutex::new(plugin_instance)),
            library: Arc::new(library),
            state: PluginState::Loaded,
        };

        {
            let mut plugins = self.plugins.write().await;
            plugins.insert(plugin_id.clone(), loaded_plugin);
        }

        info!("Successfully loaded plugin: {}", plugin_id);
        Ok(())
    }

    /// Unload a plugin
    pub async fn unload(&self, id: &str) -> Result<()> {
        info!("Unloading plugin: {}", id);

        let removed = {
            let mut plugins = self.plugins.write().await;
            plugins.remove(id)
        };

        let Some(loaded) = removed else {
            return Err(PluginError::NotFound { id: id.to_string() });
        };

        {
            let mut instance = loaded.instance.lock().await;
            instance
                .shutdown()
                .map_err(|e| PluginError::ExecutionFailed {
                    message: format!("failed to shutdown plugin {}: {}", id, e),
                })?;
        }

        // Clean up sandbox
        {
            let mut sandbox_manager = self.sandbox_manager.write().await;
            sandbox_manager.remove_sandbox(id);
        }

        info!("Successfully unloaded plugin: {}", id);
        Ok(())
    }

    /// Execute a plugin
    pub async fn execute(&self, id: &str, input: serde_json::Value) -> Result<serde_json::Value> {
        self.ensure_initialized(id).await?;

        let instance = {
            let plugins = self.plugins.read().await;
            let plugin = plugins
                .get(id)
                .ok_or_else(|| PluginError::NotFound { id: id.to_string() })?;
            if plugin.state != PluginState::Ready && plugin.state != PluginState::Loaded {
                return Err(PluginError::ExecutionFailed {
                    message: format!("Plugin {} is not ready (state: {:?})", id, plugin.state),
                });
            }
            plugin.instance.clone()
        };

        debug!("Executing plugin: {} with input: {:?}", id, input);
        let plugin = instance.lock().await;
        plugin
            .execute(input)
            .map_err(|e| PluginError::ExecutionFailed {
                message: format!("plugin {} execute failed: {}", id, e),
            })
    }

    async fn ensure_initialized(&self, id: &str) -> Result<()> {
        let (needs_init, instance) = {
            let plugins = self.plugins.read().await;
            let plugin = plugins
                .get(id)
                .ok_or_else(|| PluginError::NotFound { id: id.to_string() })?;
            match plugin.state {
                PluginState::Ready => return Ok(()),
                PluginState::Loaded => (true, plugin.instance.clone()),
                _ => {
                    return Err(PluginError::ExecutionFailed {
                        message: format!(
                            "Plugin {} cannot initialize from state: {:?}",
                            id, plugin.state
                        ),
                    })
                }
            }
        };

        if needs_init {
            {
                let mut plugin = instance.lock().await;
                plugin.init().map_err(|e| PluginError::ExecutionFailed {
                    message: format!("plugin {} init failed: {}", id, e),
                })?;
            }
            let mut plugins = self.plugins.write().await;
            let plugin = plugins
                .get_mut(id)
                .ok_or_else(|| PluginError::NotFound { id: id.to_string() })?;
            if plugin.state == PluginState::Loaded {
                plugin.state = PluginState::Ready;
            }
        }

        Ok(())
    }

    /// Initialize a loaded plugin
    pub async fn initialize(&self, id: &str) -> Result<()> {
        self.ensure_initialized(id).await?;
        info!("Initialized plugin: {}", id);
        Ok(())
    }

    /// Get loaded plugin manifests
    pub async fn list_loaded(&self) -> Vec<PluginManifest> {
        let plugins = self.plugins.read().await;
        plugins.values().map(|p| p.manifest.clone()).collect()
    }

    /// Check if a plugin is loaded
    pub async fn is_loaded(&self, id: &str) -> bool {
        let plugins = self.plugins.read().await;
        plugins.contains_key(id)
    }

    /// Resolve library path from entry point
    fn resolve_library_path(&self, entry_point: &str) -> Result<PathBuf> {
        let path = PathBuf::from(entry_point);

        // Security: reject absolute paths — plugins must be relative to plugin_dir
        if path.is_absolute() {
            return Err(PluginError::LoadFailed {
                message: format!(
                    "absolute entry_point rejected for security: {}",
                    entry_point
                ),
            });
        }

        Ok(self.plugin_dir.join(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_plugin_loader_creation() {
        let temp_dir = TempDir::new().unwrap();
        let loader = PluginLoader::new(temp_dir.path().to_path_buf());

        let loaded = loader.list_loaded().await;
        assert_eq!(loaded.len(), 0);
    }

    #[tokio::test]
    async fn test_plugin_discovery() {
        let temp_dir = TempDir::new().unwrap();
        let plugin_dir = temp_dir.path().join("plugins");
        std::fs::create_dir(&plugin_dir).unwrap();

        // Create a sample plugin manifest
        let manifest = PluginManifest {
            id: "test-plugin".to_string(),
            version: semver::Version::new(1, 0, 0),
            kind: crate::manifest::PluginKind::Agent,
            name: "Test Plugin".to_string(),
            description: "A test plugin".to_string(),
            entry_point: "test_plugin.so".to_string(),
            dependencies: vec![],
            permissions: vec![],
            metadata: HashMap::new(),
            author: None,
            license: None,
        };

        let manifest_path = plugin_dir.join("plugin.json");
        let manifest_json = serde_json::to_string_pretty(&manifest).unwrap();
        std::fs::write(&manifest_path, manifest_json).unwrap();

        let loader = PluginLoader::new(temp_dir.path().to_path_buf());
        let discovered = loader.discover(&PathBuf::from("plugins")).await.unwrap();

        assert_eq!(discovered.len(), 1);
        assert_eq!(discovered[0].id, "test-plugin");
    }

    #[tokio::test]
    async fn test_execute_not_found_returns_error() {
        let temp_dir = TempDir::new().unwrap();
        let loader = PluginLoader::new(temp_dir.path().to_path_buf());
        let err = loader
            .execute("missing-plugin", serde_json::json!({"foo": "bar"}))
            .await
            .expect_err("missing plugin should fail");
        assert!(matches!(err, PluginError::NotFound { .. }));
    }
}
