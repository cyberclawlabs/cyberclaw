//! Plugin manifest types and utilities

use semver::Version;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Plugin manifest containing metadata and configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    /// Unique plugin identifier
    pub id: String,

    /// Plugin version
    pub version: Version,

    /// Plugin type
    pub kind: PluginKind,

    /// Human-readable name
    pub name: String,

    /// Plugin description
    pub description: String,

    /// Entry point (library path or module name)
    pub entry_point: String,

    /// Plugin dependencies
    #[serde(default)]
    pub dependencies: Vec<Dependency>,

    /// Required permissions
    #[serde(default)]
    pub permissions: Vec<Permission>,

    /// Plugin metadata
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,

    /// Author information
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,

    /// License information
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
}

/// Plugin types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginKind {
    /// Agent plugin
    Agent,

    /// Skill plugin
    Skill,

    /// Connector plugin
    Connector,

    /// Platform plugin
    Platform,
}

/// Plugin dependency specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dependency {
    /// Dependency name
    pub name: String,

    /// Required version
    pub version: String,

    /// Whether the dependency is optional
    #[serde(default)]
    pub optional: bool,
}

/// Permission types for plugin sandboxing
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum Permission {
    /// File system access permission
    FileSystem(FileSystemPermission),

    /// Network access permission
    Network(NetworkPermission),

    /// Process execution permission
    Execution(ExecutionPermission),

    /// Environment access permission
    Environment(EnvironmentPermission),
}

/// File system permission configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSystemPermission {
    /// Allowed paths (glob patterns)
    pub read_paths: Vec<String>,

    /// Write-allowed paths (glob patterns)
    pub write_paths: Vec<String>,

    /// Whether to allow temporary file creation
    #[serde(default)]
    pub allow_temp: bool,
}

/// Network permission configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkPermission {
    /// Allowed hosts (with optional port)
    pub allowed_hosts: Vec<String>,

    /// Allowed protocols
    pub allowed_protocols: Vec<String>,

    /// Maximum concurrent connections
    #[serde(default = "default_max_connections")]
    pub max_connections: usize,
}

fn default_max_connections() -> usize {
    10
}

/// Process execution permission configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionPermission {
    /// Allowed commands
    pub allowed_commands: Vec<String>,

    /// Whether to allow shell execution
    #[serde(default)]
    pub allow_shell: bool,

    /// Maximum concurrent processes
    #[serde(default = "default_max_processes")]
    pub max_processes: usize,
}

fn default_max_processes() -> usize {
    5
}

/// Environment variable access permission
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentPermission {
    /// Allowed environment variables to read
    pub read_vars: Vec<String>,

    /// Allowed environment variables to write
    pub write_vars: Vec<String>,
}

impl PluginManifest {
    /// Load manifest from a file
    pub fn from_file(path: &PathBuf) -> crate::error::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let manifest: Self = serde_json::from_str(&content)?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Validate the manifest
    pub fn validate(&self) -> crate::error::Result<()> {
        if self.id.is_empty() {
            return Err(crate::error::PluginError::InvalidManifest {
                message: "Plugin ID cannot be empty".to_string(),
            });
        }

        if self.name.is_empty() {
            return Err(crate::error::PluginError::InvalidManifest {
                message: "Plugin name cannot be empty".to_string(),
            });
        }

        if self.entry_point.is_empty() {
            return Err(crate::error::PluginError::InvalidManifest {
                message: "Entry point cannot be empty".to_string(),
            });
        }

        Ok(())
    }

    /// Check if the plugin has a specific permission
    pub fn has_permission(&self, permission: &Permission) -> bool {
        self.permissions
            .iter()
            .any(|p| std::mem::discriminant(p) == std::mem::discriminant(permission))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manifest_serialization() {
        let manifest = PluginManifest {
            id: "test-plugin".to_string(),
            version: Version::new(1, 0, 0),
            kind: PluginKind::Agent,
            name: "Test Plugin".to_string(),
            description: "A test plugin".to_string(),
            entry_point: "test_plugin.so".to_string(),
            dependencies: vec![],
            permissions: vec![],
            metadata: HashMap::new(),
            author: Some("Test Author".to_string()),
            license: Some("Apache-2.0".to_string()),
        };

        let json = serde_json::to_string(&manifest).unwrap();
        let deserialized: PluginManifest = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.id, manifest.id);
        assert_eq!(deserialized.version, manifest.version);
        assert_eq!(deserialized.kind, manifest.kind);
    }

    #[test]
    fn test_manifest_validation() {
        let mut manifest = PluginManifest {
            id: "".to_string(),
            version: Version::new(1, 0, 0),
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

        assert!(manifest.validate().is_err());

        manifest.id = "test-plugin".to_string();
        assert!(manifest.validate().is_ok());
    }
}
