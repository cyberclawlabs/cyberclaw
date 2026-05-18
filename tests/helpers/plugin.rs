// tests/helpers/plugin.rs
// Plugin 测试辅助工具

use anyhow::Result;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use tokio::fs;

/// 创建测试 Plugin
pub async fn create_test_plugin(name: &str) -> Result<(TempDir, PathBuf)> {
    let temp_dir = TempDir::new()?;
    let plugin_dir = temp_dir.path().join(name);
    fs::create_dir_all(&plugin_dir).await?;

    // 创建 Plugin manifest
    let manifest = format!(
        r#"[plugin]
id = "{}"
name = "Test Plugin - {}"
version = "0.1.0"
description = "Test plugin for integration testing"
authors = ["Test Author <test@example.com>"]

[plugin.library]
path = "target/release/lib{}.so"
entry_point = "cyberclaw_plugin_init"

[plugin.hooks]
before_execution = {{ handler = "before_exec_hook", priority = 100, timeout_ms = 5000 }}
after_execution = {{ handler = "after_exec_hook", priority = 100, timeout_ms = 5000 }}

[plugin.capabilities]
required = ["fs.read", "network.http"]

[plugin.resources]
memory = 104857600
cpu_ms = 10000
file_handles = 100
network_connections = 10

[plugin.metadata]
homepage = "https://example.com"
repository = "https://github.com/example/plugin"
license = "Apache-2.0"
"#,
        name, name, name
    );

    fs::write(plugin_dir.join("cyberclaw-plugin.toml"), manifest).await?;

    // 创建示例代码文件
    let lib_code = format!(
        r#"// lib.rs for {} plugin
use cyberclaw_core::plugin::{{Plugin, PluginResult, HookContext}};

pub struct TestPlugin {{
    name: String,
}}

impl Plugin for TestPlugin {{
    fn name(&self) -> &str {{
        &self.name
    }}

    fn before_execution(&self, context: &HookContext) -> PluginResult<()> {{
        println!("Before execution hook for {{}}", self.name);
        Ok(())
    }}

    fn after_execution(&self, context: &HookContext) -> PluginResult<()> {{
        println!("After execution hook for {{}}", self.name);
        Ok(())
    }}
}}

#[no_mangle]
pub extern "C" fn cyberclaw_plugin_init() -> *mut dyn Plugin {{
    Box::into_raw(Box::new(TestPlugin {{
        name: "{}".to_string(),
    }}))
}}
"#,
        name, name
    );

    fs::write(plugin_dir.join("lib.rs"), lib_code).await?;

    Ok((temp_dir, plugin_dir))
}

/// 创建损坏的 Plugin (用于错误处理测试)
pub async fn create_broken_plugin(name: &str) -> Result<(TempDir, PathBuf)> {
    let temp_dir = TempDir::new()?;
    let plugin_dir = temp_dir.path().join(name);
    fs::create_dir_all(&plugin_dir).await?;

    // 创建无效的 manifest
    let manifest = r#"[invalid]
this_is = "not valid"
"#;

    fs::write(plugin_dir.join("cyberclaw-plugin.toml"), manifest).await?;

    Ok((temp_dir, plugin_dir))
}

/// 创建 Plugin 沙箱环境
pub struct PluginSandbox {
    work_dir: TempDir,
    plugins: Vec<PathBuf>,
}

impl PluginSandbox {
    pub async fn new() -> Result<Self> {
        let work_dir = TempDir::new()?;

        // 创建插件目录结构
        let plugins_dir = work_dir.path().join("plugins");
        fs::create_dir_all(&plugins_dir).await?;

        Ok(Self {
            work_dir,
            plugins: Vec::new(),
        })
    }

    pub async fn add_plugin(&mut self, name: &str) -> Result<PathBuf> {
        let plugin_dir = self.work_dir.path().join("plugins").join(name);
        fs::create_dir_all(&plugin_dir).await?;

        // 创建简单的 manifest
        let manifest = format!(
            r#"[plugin]
id = "{}"
name = "{}"
version = "0.1.0"
"#,
            name, name
        );

        fs::write(plugin_dir.join("cyberclaw-plugin.toml"), manifest).await?;
        self.plugins.push(plugin_dir.clone());

        Ok(plugin_dir)
    }

    pub fn plugins_dir(&self) -> PathBuf {
        self.work_dir.path().join("plugins")
    }

    pub fn plugin_paths(&self) -> &[PathBuf] {
        &self.plugins
    }
}

/// Mock Plugin Hook Handler
pub struct MockHookHandler {
    pub calls: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl MockHookHandler {
    pub fn new() -> Self {
        Self {
            calls: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    pub fn record_call(&self, hook_name: &str) {
        self.calls.lock().unwrap().push(hook_name.to_string());
    }

    pub fn get_calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    pub fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }

    pub fn was_called(&self, hook_name: &str) -> bool {
        self.calls.lock().unwrap().contains(&hook_name.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_plugin() {
        let (_temp, plugin_dir) = create_test_plugin("test").await.unwrap();

        let manifest_path = plugin_dir.join("cyberclaw-plugin.toml");
        assert!(manifest_path.exists());

        let lib_path = plugin_dir.join("lib.rs");
        assert!(lib_path.exists());
    }

    #[tokio::test]
    async fn test_plugin_sandbox() {
        let mut sandbox = PluginSandbox::new().await.unwrap();
        let plugin_path = sandbox.add_plugin("test-plugin").await.unwrap();

        assert!(plugin_path.exists());
        assert_eq!(sandbox.plugin_paths().len(), 1);
    }
}