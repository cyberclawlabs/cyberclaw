//! Declarative plugin registry for skill-runtime plugins.
//!
//! Replaces dynamic library loading with a manifest-driven registration model.
//! Plugins declare their name, version, hooks, and configuration in a YAML
//! manifest file, which the registry loads and indexes.

use std::collections::HashMap;
use std::path::Path;

/// A declarative plugin registration entry.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PluginDeclaration {
    /// Plugin name (unique identifier within the registry).
    pub name: String,
    /// Semantic version string.
    pub version: String,
    /// Lifecycle hooks this plugin participates in (e.g. "on_skill_load", "on_skill_unload").
    #[serde(default)]
    pub hooks: Vec<String>,
    /// Arbitrary plugin configuration.
    #[serde(default)]
    pub config: serde_json::Value,
}

/// Declarative plugin registry that replaces dynamic library loading.
///
/// Plugins are registered by declaration (name, version, hooks, config) rather
/// than by loading shared libraries at runtime. This keeps the platform
/// deterministic and auditable.
#[derive(Debug, Default)]
pub struct PluginRegistry {
    /// Registered plugin declarations keyed by plugin name.
    plugins: HashMap<String, PluginDeclaration>,
}

impl PluginRegistry {
    /// Create an empty plugin registry.
    pub fn new() -> Self {
        Self {
            plugins: HashMap::new(),
        }
    }

    /// Register a plugin declaration.
    ///
    /// If a plugin with the same name already exists, it is replaced and the
    /// previous declaration is returned.
    pub fn register(&mut self, declaration: PluginDeclaration) -> Option<PluginDeclaration> {
        tracing::info!(
            plugin = %declaration.name,
            version = %declaration.version,
            hooks = ?declaration.hooks,
            "registering plugin"
        );
        self.plugins.insert(declaration.name.clone(), declaration)
    }

    /// Unregister a plugin by name.
    ///
    /// Returns the removed declaration, or `None` if not found.
    pub fn unregister(&mut self, name: &str) -> Option<PluginDeclaration> {
        let removed = self.plugins.remove(name);
        if let Some(ref decl) = removed {
            tracing::info!(plugin = %decl.name, "unregistered plugin");
        }
        removed
    }

    /// List all registered plugin declarations.
    pub fn list(&self) -> Vec<&PluginDeclaration> {
        self.plugins.values().collect()
    }

    /// Get a plugin declaration by name.
    pub fn get(&self, name: &str) -> Option<&PluginDeclaration> {
        self.plugins.get(name)
    }

    /// Return the number of registered plugins.
    pub fn len(&self) -> usize {
        self.plugins.len()
    }

    /// Return `true` if no plugins are registered.
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// Load plugin declarations from a YAML manifest file.
    ///
    /// The manifest must be a YAML file containing a top-level `plugins` array,
    /// where each element is a `PluginDeclaration`.
    ///
    /// ```yaml
    /// plugins:
    ///   - name: my-plugin
    ///     version: "1.0.0"
    ///     hooks:
    ///       - on_skill_load
    ///     config:
    ///       key: value
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or parsed.
    pub fn load_from_manifest(&mut self, path: &Path) -> anyhow::Result<usize> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            anyhow::anyhow!("failed to read plugin manifest '{}': {}", path.display(), e)
        })?;

        let manifest: PluginManifest = serde_yaml::from_str(&content).map_err(|e| {
            anyhow::anyhow!(
                "failed to parse plugin manifest '{}': {}",
                path.display(),
                e
            )
        })?;

        let count = manifest.plugins.len();
        for decl in manifest.plugins {
            self.register(decl);
        }

        tracing::info!(
            path = %path.display(),
            count,
            "loaded plugins from manifest"
        );
        Ok(count)
    }
}

/// Internal manifest structure for YAML deserialization.
#[derive(Debug, serde::Deserialize)]
struct PluginManifest {
    /// List of plugin declarations.
    #[serde(default)]
    plugins: Vec<PluginDeclaration>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as IoWrite;

    fn sample_declaration(name: &str) -> PluginDeclaration {
        PluginDeclaration {
            name: name.to_string(),
            version: "1.0.0".to_string(),
            hooks: vec!["on_skill_load".to_string()],
            config: serde_json::json!({ "enabled": true }),
        }
    }

    #[test]
    fn test_register_and_get() {
        let mut registry = PluginRegistry::new();
        assert!(registry.is_empty());

        registry.register(sample_declaration("alpha"));
        assert_eq!(registry.len(), 1);
        assert!(!registry.is_empty());

        let got = registry.get("alpha").expect("plugin should exist");
        assert_eq!(got.name, "alpha");
        assert_eq!(got.version, "1.0.0");
        assert_eq!(got.hooks, vec!["on_skill_load"]);
    }

    #[test]
    fn test_register_overwrites() {
        let mut registry = PluginRegistry::new();

        let first = sample_declaration("beta");
        assert!(registry.register(first).is_none());

        let mut second = sample_declaration("beta");
        second.version = "2.0.0".to_string();
        let prev = registry.register(second);

        assert!(prev.is_some());
        assert_eq!(prev.unwrap().version, "1.0.0");
        assert_eq!(registry.get("beta").unwrap().version, "2.0.0");
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn test_unregister() {
        let mut registry = PluginRegistry::new();
        registry.register(sample_declaration("gamma"));
        assert_eq!(registry.len(), 1);

        let removed = registry.unregister("gamma");
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().name, "gamma");
        assert!(registry.is_empty());

        // Unregister non-existent returns None
        assert!(registry.unregister("gamma").is_none());
    }

    #[test]
    fn test_list() {
        let mut registry = PluginRegistry::new();
        registry.register(sample_declaration("a"));
        registry.register(sample_declaration("b"));
        registry.register(sample_declaration("c"));

        let list = registry.list();
        assert_eq!(list.len(), 3);

        let names: Vec<&str> = list.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"a"));
        assert!(names.contains(&"b"));
        assert!(names.contains(&"c"));
    }

    #[test]
    fn test_load_from_manifest() {
        let manifest_content = r#"
plugins:
  - name: file-reader
    version: "1.2.0"
    hooks:
      - on_skill_load
      - on_skill_unload
    config:
      max_size: 1048576
  - name: logger
    version: "0.5.0"
    hooks: []
    config: {}
"#;
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        tmpfile.write_all(manifest_content.as_bytes()).unwrap();
        tmpfile.flush().unwrap();

        let mut registry = PluginRegistry::new();
        let count = registry.load_from_manifest(tmpfile.path()).unwrap();
        assert_eq!(count, 2);
        assert_eq!(registry.len(), 2);

        let fr = registry.get("file-reader").unwrap();
        assert_eq!(fr.version, "1.2.0");
        assert_eq!(fr.hooks.len(), 2);
        assert_eq!(fr.config["max_size"], 1048576);

        let lg = registry.get("logger").unwrap();
        assert_eq!(lg.version, "0.5.0");
        assert!(lg.hooks.is_empty());
    }

    #[test]
    fn test_load_from_manifest_empty() {
        let manifest_content = "plugins: []\n";
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        tmpfile.write_all(manifest_content.as_bytes()).unwrap();
        tmpfile.flush().unwrap();

        let mut registry = PluginRegistry::new();
        let count = registry.load_from_manifest(tmpfile.path()).unwrap();
        assert_eq!(count, 0);
        assert!(registry.is_empty());
    }

    #[test]
    fn test_load_from_manifest_file_not_found() {
        let mut registry = PluginRegistry::new();
        let result = registry.load_from_manifest(Path::new("/nonexistent/manifest.yaml"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("failed to read"));
    }

    #[test]
    fn test_load_from_manifest_invalid_yaml() {
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        tmpfile.write_all(b"not: [valid: yaml: {{").unwrap();
        tmpfile.flush().unwrap();

        let mut registry = PluginRegistry::new();
        let result = registry.load_from_manifest(tmpfile.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("failed to parse"));
    }

    #[test]
    fn test_default_trait() {
        let registry = PluginRegistry::default();
        assert!(registry.is_empty());
    }
}
