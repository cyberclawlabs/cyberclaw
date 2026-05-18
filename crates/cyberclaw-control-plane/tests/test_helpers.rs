//! Test helpers for control plane tests

use cyberclaw_plugin_runtime::PluginRegistry;
use std::sync::Arc;

/// Create a test plugin registry
#[cfg(test)]
pub fn create_test_plugin_registry() -> Arc<PluginRegistry> {
    let temp_dir = tempfile::TempDir::new().unwrap();
    Arc::new(PluginRegistry::new(temp_dir.path().to_path_buf()))
}
