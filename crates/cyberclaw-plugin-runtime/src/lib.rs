//! CyberClaw Plugin Runtime
//!
//! This crate provides a dynamic plugin loading system for the CyberClaw platform,
//! supporting Agent, Skill, Connector, and Platform plugins with sandboxed execution.

pub mod error;
pub mod loader;
pub mod manifest;
pub mod registry;
pub mod sandbox;

pub use error::{PluginError, Result};
pub use loader::{Plugin, PluginLoader};
pub use manifest::{
    Dependency, EnvironmentPermission, ExecutionPermission, FileSystemPermission,
    NetworkPermission, Permission, PluginKind, PluginManifest,
};
pub use registry::{PluginRegistry, RegistryStats};
pub use sandbox::{PluginSandbox, SandboxManager};

/// Plugin runtime version
pub const RUNTIME_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Re-export commonly used types
pub mod prelude {
    pub use crate::error::{PluginError, Result};
    pub use crate::manifest::{Permission, PluginKind, PluginManifest};
    pub use crate::registry::PluginRegistry;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_runtime_version() {
        assert!(!RUNTIME_VERSION.is_empty());
    }
}
