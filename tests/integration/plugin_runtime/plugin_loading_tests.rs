// tests/integration/plugin_runtime/plugin_loading_tests.rs
// Plugin 加载和卸载测试

use anyhow::Result;
use cyberclaw_core::plugin::{PluginId, PluginState};
use cyberclaw_plugin_runtime::{PluginLoader, PluginRegistry};
use std::path::PathBuf;
use tempfile::TempDir;

mod helpers {
    pub use crate::helpers::plugin::*;
}

/// 测试 Plugin 基本加载
#[tokio::test]
async fn test_plugin_basic_loading() -> Result<()> {
    // 创建测试 Plugin
    let (_temp_dir, plugin_dir) = helpers::create_test_plugin("test_plugin").await?;

    // 创建 PluginLoader
    let loader = PluginLoader::new();

    // 加载 Plugin
    let plugin_id = loader.load_plugin(plugin_dir.join("cyberclaw-plugin.toml")).await?;

    // 验证 Plugin 已加载
    assert!(!plugin_id.to_string().is_empty());

    // 获取 Plugin 状态
    let state = loader.get_plugin_state(&plugin_id).await?;
    assert_eq!(state, PluginState::Loaded);

    Ok(())
}

/// 测试 Plugin 卸载
#[tokio::test]
async fn test_plugin_unloading() -> Result<()> {
    let (_temp_dir, plugin_dir) = helpers::create_test_plugin("test_plugin").await?;
    let loader = PluginLoader::new();

    // 加载 Plugin
    let plugin_id = loader.load_plugin(plugin_dir.join("cyberclaw-plugin.toml")).await?;

    // 启用 Plugin
    loader.enable_plugin(&plugin_id).await?;

    // 卸载 Plugin
    loader.unload_plugin(&plugin_id).await?;

    // 验证 Plugin 已卸载
    let state = loader.get_plugin_state(&plugin_id).await;
    assert!(state.is_err() || state.unwrap() == PluginState::Unloaded);

    Ok(())
}

/// 测试加载多个 Plugin
#[tokio::test]
async fn test_multiple_plugin_loading() -> Result<()> {
    let loader = PluginLoader::new();
    let mut plugin_ids = Vec::new();

    // 加载多个 Plugin
    for i in 0..5 {
        let (_temp_dir, plugin_dir) = helpers::create_test_plugin(&format!("plugin_{}", i)).await?;
        let plugin_id = loader.load_plugin(plugin_dir.join("cyberclaw-plugin.toml")).await?;
        plugin_ids.push(plugin_id);
    }

    // 验证所有 Plugin 都已加载
    assert_eq!(plugin_ids.len(), 5);

    // 获取已加载的 Plugin 列表
    let loaded_plugins = loader.list_plugins().await?;
    assert_eq!(loaded_plugins.len(), 5);

    Ok(())
}

/// 测试加载损坏的 Plugin
#[tokio::test]
async fn test_loading_broken_plugin() -> Result<()> {
    let (_temp_dir, plugin_dir) = helpers::create_broken_plugin("broken_plugin").await?;
    let loader = PluginLoader::new();

    // 尝试加载损坏的 Plugin
    let result = loader.load_plugin(plugin_dir.join("cyberclaw-plugin.toml")).await;

    // 验证加载失败
    assert!(result.is_err());

    Ok(())
}

/// 测试 Plugin 重复加载
#[tokio::test]
async fn test_duplicate_plugin_loading() -> Result<()> {
    let (_temp_dir, plugin_dir) = helpers::create_test_plugin("test_plugin").await?;
    let loader = PluginLoader::new();

    // 第一次加载
    let plugin_id1 = loader.load_plugin(plugin_dir.join("cyberclaw-plugin.toml")).await?;

    // 尝试重复加载同一个 Plugin
    let result = loader.load_plugin(plugin_dir.join("cyberclaw-plugin.toml")).await;

    // 验证重复加载被拒绝或返回相同ID
    match result {
        Ok(plugin_id2) => assert_eq!(plugin_id1, plugin_id2),
        Err(_) => {
            // 如果不允许重复加载也是正确的行为
            assert!(true);
        }
    }

    Ok(())
}

/// 测试 Plugin 依赖解析
#[tokio::test]
async fn test_plugin_dependency_resolution() -> Result<()> {
    // 创建有依赖关系的 Plugin
    let temp_dir = TempDir::new()?;

    // 创建 Plugin A（被依赖）
    let plugin_a_dir = temp_dir.path().join("plugin_a");
    tokio::fs::create_dir_all(&plugin_a_dir).await?;
    let manifest_a = r#"
[plugin]
id = "plugin_a"
name = "Plugin A"
version = "1.0.0"

[plugin.library]
path = "lib.so"
"#;
    tokio::fs::write(plugin_a_dir.join("cyberclaw-plugin.toml"), manifest_a).await?;

    // 创建 Plugin B（依赖 Plugin A）
    let plugin_b_dir = temp_dir.path().join("plugin_b");
    tokio::fs::create_dir_all(&plugin_b_dir).await?;
    let manifest_b = r#"
[plugin]
id = "plugin_b"
name = "Plugin B"
version = "1.0.0"

[plugin.dependencies]
plugin_a = "1.0.0"

[plugin.library]
path = "lib.so"
"#;
    tokio::fs::write(plugin_b_dir.join("cyberclaw-plugin.toml"), manifest_b).await?;

    let loader = PluginLoader::new();

    // 加载 Plugin A
    let _plugin_a_id = loader.load_plugin(plugin_a_dir.join("cyberclaw-plugin.toml")).await?;

    // 加载 Plugin B（应该成功，因为依赖已满足）
    let _plugin_b_id = loader.load_plugin(plugin_b_dir.join("cyberclaw-plugin.toml")).await?;

    Ok(())
}

/// 测试 Plugin 资源限制
#[tokio::test]
async fn test_plugin_resource_limits() -> Result<()> {
    // 创建请求过多资源的 Plugin
    let temp_dir = TempDir::new()?;
    let plugin_dir = temp_dir.path().join("resource_plugin");
    tokio::fs::create_dir_all(&plugin_dir).await?;

    let manifest = r#"
[plugin]
id = "resource_plugin"
name = "Resource Plugin"
version = "1.0.0"

[plugin.resources]
memory = 10737418240  # 10 GB - 过大的内存请求
cpu_ms = 3600000      # 1 hour - 过长的CPU时间

[plugin.library]
path = "lib.so"
"#;
    tokio::fs::write(plugin_dir.join("cyberclaw-plugin.toml"), manifest).await?;

    let loader = PluginLoader::with_resource_limits(
        1073741824,  // 1 GB max memory
        60000,       // 1 minute max CPU
    );

    // 尝试加载请求过多资源的 Plugin
    let result = loader.load_plugin(plugin_dir.join("cyberclaw-plugin.toml")).await;

    // 验证加载被拒绝
    assert!(result.is_err());

    Ok(())
}

/// 测试 Plugin 热重载
#[tokio::test]
async fn test_plugin_hot_reload() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let plugin_dir = temp_dir.path().join("reload_plugin");
    tokio::fs::create_dir_all(&plugin_dir).await?;

    // 创建初始版本
    let manifest_v1 = r#"
[plugin]
id = "reload_plugin"
name = "Reload Plugin"
version = "1.0.0"

[plugin.library]
path = "lib.so"
"#;
    let manifest_path = plugin_dir.join("cyberclaw-plugin.toml");
    tokio::fs::write(&manifest_path, manifest_v1).await?;

    let loader = PluginLoader::new();

    // 加载初始版本
    let plugin_id = loader.load_plugin(manifest_path.clone()).await?;

    // 获取初始版本信息
    let info_v1 = loader.get_plugin_info(&plugin_id).await?;
    assert_eq!(info_v1.version, "1.0.0");

    // 更新 manifest 到新版本
    let manifest_v2 = r#"
[plugin]
id = "reload_plugin"
name = "Reload Plugin"
version = "2.0.0"

[plugin.library]
path = "lib.so"
"#;
    tokio::fs::write(&manifest_path, manifest_v2).await?;

    // 热重载 Plugin
    loader.reload_plugin(&plugin_id).await?;

    // 验证版本已更新
    let info_v2 = loader.get_plugin_info(&plugin_id).await?;
    assert_eq!(info_v2.version, "2.0.0");

    Ok(())
}

/// 测试 Plugin 沙箱隔离
#[tokio::test]
async fn test_plugin_sandbox_isolation() -> Result<()> {
    let sandbox = helpers::PluginSandbox::new().await?;

    // 在沙箱中添加 Plugin
    let plugin_path = sandbox.add_plugin("sandboxed_plugin").await?;

    // 创建带沙箱的 PluginLoader
    let loader = PluginLoader::with_sandbox(sandbox.plugins_dir());

    // 加载 Plugin
    let plugin_id = loader.load_plugin(plugin_path.join("cyberclaw-plugin.toml")).await?;

    // 验证 Plugin 在沙箱中运行
    let info = loader.get_plugin_info(&plugin_id).await?;
    assert!(info.sandboxed);

    Ok(())
}

/// 测试 Plugin 生命周期
#[tokio::test]
async fn test_plugin_lifecycle() -> Result<()> {
    let (_temp_dir, plugin_dir) = helpers::create_test_plugin("lifecycle_plugin").await?;
    let loader = PluginLoader::new();

    // 加载
    let plugin_id = loader.load_plugin(plugin_dir.join("cyberclaw-plugin.toml")).await?;
    assert_eq!(loader.get_plugin_state(&plugin_id).await?, PluginState::Loaded);

    // 初始化
    loader.initialize_plugin(&plugin_id).await?;
    assert_eq!(loader.get_plugin_state(&plugin_id).await?, PluginState::Initialized);

    // 启用
    loader.enable_plugin(&plugin_id).await?;
    assert_eq!(loader.get_plugin_state(&plugin_id).await?, PluginState::Enabled);

    // 禁用
    loader.disable_plugin(&plugin_id).await?;
    assert_eq!(loader.get_plugin_state(&plugin_id).await?, PluginState::Disabled);

    // 卸载
    loader.unload_plugin(&plugin_id).await?;

    Ok(())
}

#[cfg(test)]
mod integration_tests {
    use super::*;

    /// 测试完整的 Plugin 系统集成
    #[tokio::test]
    async fn test_plugin_system_integration() -> Result<()> {
        // 创建 Registry
        let registry = PluginRegistry::new(PathBuf::from("/tmp/test_plugins"));

        // 创建多个 Plugin
        let mut plugin_ids = Vec::new();

        for i in 0..3 {
            let (_temp_dir, plugin_dir) = helpers::create_test_plugin(&format!("plugin_{}", i)).await?;
            let plugin_id = registry.register_from_path(plugin_dir.join("cyberclaw-plugin.toml")).await?;
            plugin_ids.push(plugin_id);
        }

        // 验证所有 Plugin 都已注册
        let all_plugins = registry.list_all().await?;
        assert_eq!(all_plugins.len(), 3);

        // 启用所有 Plugin
        for plugin_id in &plugin_ids {
            registry.enable(plugin_id).await?;
        }

        // 验证所有 Plugin 都已启用
        for plugin_id in &plugin_ids {
            let state = registry.get_state(plugin_id).await?;
            assert_eq!(state, PluginState::Enabled);
        }

        Ok(())
    }
}