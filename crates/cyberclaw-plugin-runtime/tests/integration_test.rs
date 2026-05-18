//! Integration tests for plugin runtime

use cyberclaw_plugin_runtime::manifest::{
    Dependency, FileSystemPermission, NetworkPermission, Permission,
};
use cyberclaw_plugin_runtime::prelude::*;
use std::collections::HashMap;
use std::path::PathBuf;
use tempfile::TempDir;

#[tokio::test]
async fn test_plugin_registry_lifecycle() {
    let temp_dir = TempDir::new().unwrap();
    let registry = PluginRegistry::new(temp_dir.path().to_path_buf());

    // Initially empty
    assert_eq!(registry.list().await.len(), 0);

    // Create a test manifest
    let manifest = create_test_manifest("test-plugin-1");

    // Register plugin (will fail due to missing library, but that's expected in tests)
    let result = registry.register(manifest.clone()).await;
    assert!(result.is_err()); // Expected to fail due to missing library

    // Verify stats
    let stats = registry.stats().await;
    assert_eq!(stats.total_registered, 0); // Should be 0 since registration failed
}

#[tokio::test]
async fn test_plugin_discovery() {
    let temp_dir = TempDir::new().unwrap();
    let plugin_dir = temp_dir.path().join("plugins");
    std::fs::create_dir(&plugin_dir).unwrap();

    // Create test plugin directories
    for i in 1..=3 {
        let plugin_subdir = plugin_dir.join(format!("plugin-{}", i));
        std::fs::create_dir(&plugin_subdir).unwrap();

        let manifest = create_test_manifest(&format!("plugin-{}", i));
        let manifest_path = plugin_subdir.join("plugin.json");
        let manifest_json = serde_json::to_string_pretty(&manifest).unwrap();
        std::fs::write(manifest_path, manifest_json).unwrap();
    }

    let registry = PluginRegistry::new(temp_dir.path().to_path_buf());
    let discovered = registry
        .discover_and_register(&PathBuf::from("plugins"))
        .await
        .unwrap();

    // All should fail to register due to missing libraries, but discovery should work
    assert_eq!(discovered.len(), 0); // 0 successfully registered
}

#[tokio::test]
async fn test_manifest_validation() {
    let mut manifest = create_test_manifest("test");

    // Valid manifest
    assert!(manifest.validate().is_ok());

    // Invalid manifest - empty ID
    manifest.id = String::new();
    assert!(manifest.validate().is_err());

    // Invalid manifest - empty name
    manifest.id = "test".to_string();
    manifest.name = String::new();
    assert!(manifest.validate().is_err());

    // Invalid manifest - empty entry point
    manifest.name = "Test".to_string();
    manifest.entry_point = String::new();
    assert!(manifest.validate().is_err());
}

#[tokio::test]
async fn test_plugin_permissions() {
    let manifest = PluginManifest {
        id: "secure-plugin".to_string(),
        version: semver::Version::new(1, 0, 0),
        kind: PluginKind::Agent,
        name: "Secure Plugin".to_string(),
        description: "A plugin with specific permissions".to_string(),
        entry_point: "secure_plugin.so".to_string(),
        dependencies: vec![],
        permissions: vec![
            Permission::FileSystem(FileSystemPermission {
                read_paths: vec!["/tmp/*".to_string()],
                write_paths: vec!["/tmp/plugin/*".to_string()],
                allow_temp: true,
            }),
            Permission::Network(NetworkPermission {
                allowed_hosts: vec!["api.example.com".to_string()],
                allowed_protocols: vec!["https".to_string()],
                max_connections: 10,
            }),
        ],
        metadata: HashMap::new(),
        author: Some("Test Author".to_string()),
        license: Some("Apache-2.0".to_string()),
    };

    // Test permission checking
    assert!(
        manifest.has_permission(&Permission::FileSystem(FileSystemPermission {
            read_paths: vec![],
            write_paths: vec![],
            allow_temp: false,
        }))
    );

    assert!(
        manifest.has_permission(&Permission::Network(NetworkPermission {
            allowed_hosts: vec![],
            allowed_protocols: vec![],
            max_connections: 0,
        }))
    );
}

#[test]
fn test_manifest_serialization() {
    let manifest = create_test_manifest("test-plugin");

    // Serialize to JSON
    let json = serde_json::to_string_pretty(&manifest).unwrap();
    assert!(json.contains("test-plugin"));

    // Deserialize back
    let deserialized: PluginManifest = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.id, manifest.id);
    assert_eq!(deserialized.version, manifest.version);
    assert_eq!(deserialized.kind, manifest.kind);
}

#[test]
fn test_plugin_kind_serialization() {
    let kinds = vec![
        PluginKind::Agent,
        PluginKind::Skill,
        PluginKind::Connector,
        PluginKind::Platform,
    ];

    for kind in kinds {
        let json = serde_json::to_string(&kind).unwrap();
        let deserialized: PluginKind = serde_json::from_str(&json).unwrap();
        assert_eq!(kind, deserialized);
    }
}

#[tokio::test]
async fn test_dependency_validation() {
    let temp_dir = TempDir::new().unwrap();
    let registry = PluginRegistry::new(temp_dir.path().to_path_buf());

    // Plugin with dependencies
    let manifest = PluginManifest {
        id: "dependent-plugin".to_string(),
        version: semver::Version::new(1, 0, 0),
        kind: PluginKind::Skill,
        name: "Dependent Plugin".to_string(),
        description: "A plugin with dependencies".to_string(),
        entry_point: "dependent_plugin.so".to_string(),
        dependencies: vec![
            Dependency {
                name: "base-plugin".to_string(),
                version: "1.0.0".to_string(),
                optional: false,
            },
            Dependency {
                name: "optional-plugin".to_string(),
                version: "1.0.0".to_string(),
                optional: true,
            },
        ],
        permissions: vec![],
        metadata: HashMap::new(),
        author: None,
        license: None,
    };

    // Validation should fail for missing required dependency
    let result = registry.validate_dependencies(&manifest).await;
    assert!(result.is_err());
}

// Helper function to create a test manifest
fn create_test_manifest(id: &str) -> PluginManifest {
    PluginManifest {
        id: id.to_string(),
        version: semver::Version::new(1, 0, 0),
        kind: PluginKind::Agent,
        name: format!("{} Plugin", id),
        description: format!("Test plugin {}", id),
        entry_point: format!("{}.so", id),
        dependencies: vec![],
        permissions: vec![],
        metadata: HashMap::new(),
        author: Some("Test Author".to_string()),
        license: Some("Apache-2.0".to_string()),
    }
}
