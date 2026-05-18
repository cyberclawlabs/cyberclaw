# CyberClaw Plugin Runtime

Dynamic plugin loading system for the CyberClaw platform, supporting Agent, Skill, Connector, and Platform plugins with sandboxed execution.

## Overview

The Plugin Runtime provides:
- Dynamic plugin discovery and loading
- Manifest-based plugin configuration
- Permission-based sandboxing
- Dependency management
- Plugin lifecycle management

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│                   Plugin Registry                        │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐     │
│  │   Agent     │  │    Skill    │  │  Connector  │     │
│  │  Plugins    │  │   Plugins   │  │   Plugins   │     │
│  └─────────────┘  └─────────────┘  └─────────────┘     │
└─────────────────────────┬───────────────────────────────┘
                          │
                 ┌────────▼────────┐
                 │  Plugin Loader   │
                 │   (libloading)   │
                 └────────┬────────┘
                          │
                 ┌────────▼────────┐
                 │  Sandbox Manager │
                 │  (Permissions)   │
                 └─────────────────┘
```

## Usage

### Creating a Plugin Registry

```rust
use cyberclaw_plugin_runtime::PluginRegistry;
use std::path::PathBuf;

// Create registry with plugin directory
let registry = PluginRegistry::new(PathBuf::from("./plugins"));

// Discover and register plugins
let plugins = registry.discover_and_register(&PathBuf::from("./plugins")).await?;

// List registered plugins
let all_plugins = registry.list().await;
```

### Plugin Manifest Format

```json
{
  "id": "my-agent-plugin",
  "version": "1.0.0",
  "kind": "agent",
  "name": "My Agent Plugin",
  "description": "An example agent plugin",
  "entry_point": "my_agent.so",
  "dependencies": [
    {
      "name": "base-plugin",
      "version": "1.0.0",
      "optional": false
    }
  ],
  "permissions": [
    {
      "type": "FileSystem",
      "config": {
        "read_paths": ["/data/*"],
        "write_paths": ["/tmp/*"],
        "allow_temp": true
      }
    },
    {
      "type": "Network",
      "config": {
        "allowed_hosts": ["api.example.com"],
        "allowed_protocols": ["https"],
        "max_connections": 10
      }
    }
  ]
}
```

### Executing Plugins

```rust
// Execute a plugin
let result = registry.execute(
    "my-agent-plugin",
    serde_json::json!({
        "command": "process",
        "data": "example"
    })
).await?;
```

## Plugin Development Guide

### Creating a Plugin

1. Implement the `Plugin` trait:

```rust
use cyberclaw_plugin_runtime::Plugin;

pub struct MyPlugin {
    manifest: PluginManifest,
}

impl Plugin for MyPlugin {
    fn init(&mut self) -> Result<()> {
        // Initialize plugin
        Ok(())
    }

    fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        // Execute plugin logic
        Ok(serde_json::json!({
            "result": "success"
        }))
    }

    fn shutdown(&mut self) -> Result<()> {
        // Cleanup
        Ok(())
    }

    fn metadata(&self) -> &PluginManifest {
        &self.manifest
    }
}
```

2. Export the constructor function:

```rust
#[no_mangle]
pub extern "C" fn create_plugin() -> *mut dyn Plugin {
    Box::into_raw(Box::new(MyPlugin::new()))
}
```

3. Create a plugin manifest file (`plugin.json`)

4. Build as a dynamic library

## Permission System

The runtime enforces permissions through sandboxing:

### File System Permissions
- `read_paths`: Paths the plugin can read from
- `write_paths`: Paths the plugin can write to
- `allow_temp`: Allow temporary file creation

### Network Permissions
- `allowed_hosts`: Hosts the plugin can connect to
- `allowed_protocols`: Allowed network protocols
- `max_connections`: Maximum concurrent connections

### Execution Permissions
- `allowed_commands`: Commands the plugin can execute
- `allow_shell`: Allow shell command execution
- `max_processes`: Maximum concurrent processes

### Environment Permissions
- `read_vars`: Environment variables the plugin can read
- `write_vars`: Environment variables the plugin can write

## API Documentation

For detailed API documentation, run:

```bash
cargo doc --open
```

## Testing

Run tests with:

```bash
cargo test --workspace --package cyberclaw-plugin-runtime
```

## License

Apache-2.0