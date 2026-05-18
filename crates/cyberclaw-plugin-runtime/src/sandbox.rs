//! Plugin sandboxing and permission enforcement

use crate::error::{PluginError, Result};
use crate::manifest::Permission;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Plugin sandbox for enforcing permissions
#[derive(Debug)]
pub struct PluginSandbox {
    /// Plugin ID
    #[allow(dead_code)]
    plugin_id: String,

    /// Granted permissions
    permissions: Vec<Permission>,

    /// Active file handles
    file_handles: HashSet<PathBuf>,

    /// Active network connections
    network_connections: HashSet<String>,

    /// Active processes
    processes: HashSet<u32>,
}

impl PluginSandbox {
    /// Create a new sandbox for a plugin
    pub fn new(plugin_id: String, permissions: Vec<Permission>) -> Self {
        Self {
            plugin_id,
            permissions,
            file_handles: HashSet::new(),
            network_connections: HashSet::new(),
            processes: HashSet::new(),
        }
    }

    /// Check if a file system operation is allowed
    pub fn check_file_access(&self, path: &Path, write: bool) -> Result<()> {
        let path_str = path.to_string_lossy().to_string();

        for perm in &self.permissions {
            if let Permission::FileSystem(fs_perm) = perm {
                if write {
                    if Self::matches_any_pattern(&path_str, &fs_perm.write_paths) {
                        return Ok(());
                    }
                } else {
                    if Self::matches_any_pattern(&path_str, &fs_perm.read_paths) {
                        return Ok(());
                    }
                }
            }
        }

        Err(PluginError::PermissionDenied {
            operation: format!(
                "File {} access to {}",
                if write { "write" } else { "read" },
                path_str
            ),
        })
    }

    /// Check if a network operation is allowed
    pub fn check_network_access(&self, host: &str, protocol: &str) -> Result<()> {
        for perm in &self.permissions {
            if let Permission::Network(net_perm) = perm {
                if net_perm.allowed_hosts.contains(&host.to_string())
                    && net_perm.allowed_protocols.contains(&protocol.to_string())
                {
                    // Check connection limit
                    if self.network_connections.len() >= net_perm.max_connections {
                        return Err(PluginError::PermissionDenied {
                            operation: format!(
                                "Maximum network connections ({}) exceeded",
                                net_perm.max_connections
                            ),
                        });
                    }

                    return Ok(());
                }
            }
        }

        Err(PluginError::PermissionDenied {
            operation: format!("Network access to {} via {}", host, protocol),
        })
    }

    /// Check if command execution is allowed
    pub fn check_execution(&self, command: &str) -> Result<()> {
        for perm in &self.permissions {
            if let Permission::Execution(exec_perm) = perm {
                if exec_perm.allowed_commands.contains(&command.to_string()) {
                    // Check process limit
                    if self.processes.len() >= exec_perm.max_processes {
                        return Err(PluginError::PermissionDenied {
                            operation: format!(
                                "Maximum processes ({}) exceeded",
                                exec_perm.max_processes
                            ),
                        });
                    }

                    return Ok(());
                }

                if exec_perm.allow_shell && command == "sh" {
                    return Ok(());
                }
            }
        }

        Err(PluginError::PermissionDenied {
            operation: format!("Execute command: {}", command),
        })
    }

    /// Check if environment variable access is allowed
    pub fn check_env_access(&self, var: &str, write: bool) -> Result<()> {
        for perm in &self.permissions {
            if let Permission::Environment(env_perm) = perm {
                if write {
                    if env_perm.write_vars.contains(&var.to_string()) {
                        return Ok(());
                    }
                } else {
                    if env_perm.read_vars.contains(&var.to_string()) {
                        return Ok(());
                    }
                }
            }
        }

        Err(PluginError::PermissionDenied {
            operation: format!(
                "Environment {} access to {}",
                if write { "write" } else { "read" },
                var
            ),
        })
    }

    /// Register a file handle
    pub fn register_file(&mut self, path: PathBuf) {
        self.file_handles.insert(path);
    }

    /// Unregister a file handle
    pub fn unregister_file(&mut self, path: &Path) {
        self.file_handles.remove(path);
    }

    /// Register a network connection
    pub fn register_connection(&mut self, host: String) {
        self.network_connections.insert(host);
    }

    /// Unregister a network connection
    pub fn unregister_connection(&mut self, host: &str) {
        self.network_connections.remove(host);
    }

    /// Register a process
    pub fn register_process(&mut self, pid: u32) {
        self.processes.insert(pid);
    }

    /// Unregister a process
    pub fn unregister_process(&mut self, pid: u32) {
        self.processes.remove(&pid);
    }

    /// Helper to match glob patterns
    fn matches_any_pattern(path: &str, patterns: &[String]) -> bool {
        patterns.iter().any(|pattern| {
            // Simple glob pattern matching (can be enhanced with glob crate)
            if pattern == "*" {
                return true;
            }
            if pattern.ends_with("/*") {
                let prefix = &pattern[..pattern.len() - 2];
                return path.starts_with(prefix);
            }
            path == pattern || path.starts_with(&format!("{}/", pattern))
        })
    }

    /// Clean up all resources
    pub fn cleanup(&mut self) {
        self.file_handles.clear();
        self.network_connections.clear();
        self.processes.clear();
    }
}

/// Sandbox manager for all plugins
#[derive(Debug, Default)]
pub struct SandboxManager {
    /// Active sandboxes by plugin ID
    sandboxes: std::collections::HashMap<String, PluginSandbox>,
}

impl SandboxManager {
    /// Create a new sandbox manager
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a sandbox for a plugin
    pub fn create_sandbox(
        &mut self,
        plugin_id: String,
        permissions: Vec<Permission>,
    ) -> Result<()> {
        if self.sandboxes.contains_key(&plugin_id) {
            return Err(PluginError::AlreadyRegistered { id: plugin_id });
        }

        let sandbox = PluginSandbox::new(plugin_id.clone(), permissions);
        self.sandboxes.insert(plugin_id, sandbox);
        Ok(())
    }

    /// Get a sandbox for a plugin
    pub fn get_sandbox(&self, plugin_id: &str) -> Option<&PluginSandbox> {
        self.sandboxes.get(plugin_id)
    }

    /// Get a mutable sandbox for a plugin
    pub fn get_sandbox_mut(&mut self, plugin_id: &str) -> Option<&mut PluginSandbox> {
        self.sandboxes.get_mut(plugin_id)
    }

    /// Remove a sandbox
    pub fn remove_sandbox(&mut self, plugin_id: &str) -> Option<PluginSandbox> {
        self.sandboxes.remove(plugin_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{FileSystemPermission, NetworkPermission};

    #[test]
    fn test_file_permission_check() {
        let permissions = vec![Permission::FileSystem(FileSystemPermission {
            read_paths: vec!["/tmp/*".to_string()],
            write_paths: vec!["/tmp/write/*".to_string()],
            allow_temp: true,
        })];

        let sandbox = PluginSandbox::new("test".to_string(), permissions);

        // Read permission check
        assert!(sandbox
            .check_file_access(Path::new("/tmp/test.txt"), false)
            .is_ok());
        assert!(sandbox
            .check_file_access(Path::new("/home/test.txt"), false)
            .is_err());

        // Write permission check
        assert!(sandbox
            .check_file_access(Path::new("/tmp/write/test.txt"), true)
            .is_ok());
        assert!(sandbox
            .check_file_access(Path::new("/tmp/test.txt"), true)
            .is_err());
    }

    #[test]
    fn test_network_permission_check() {
        let permissions = vec![Permission::Network(NetworkPermission {
            allowed_hosts: vec!["api.example.com".to_string()],
            allowed_protocols: vec!["https".to_string()],
            max_connections: 5,
        })];

        let sandbox = PluginSandbox::new("test".to_string(), permissions);

        assert!(sandbox
            .check_network_access("api.example.com", "https")
            .is_ok());
        assert!(sandbox.check_network_access("evil.com", "https").is_err());
        assert!(sandbox
            .check_network_access("api.example.com", "http")
            .is_err());
    }
}
