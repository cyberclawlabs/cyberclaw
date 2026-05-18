//! Skill Loader 系统 - 支持多格式 Skill 加载
//!
//! 提供统一的 Skill 加载接口，支持：
//! - Claude Code Skill 格式
//! - Codex Skill 格式
//! - OpenClaw Skill 格式
//!
//! ## 核心组件
//!
//! - `FormatLoader`: Skill 格式加载器 trait
//! - `UnifiedSkillLoader`: 统一加载器，自动检测格式
//! - `HotReloadWatcher`: 热重载监控器

use async_trait::async_trait;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

use serde::{Deserialize, Serialize};

use crate::error::SkillRuntimeError;
use crate::handler::SkillHandler;
use cyberclaw_core::ids::SkillId;

pub mod claude_code;
pub mod codex;
pub mod hot_reload;
pub mod openclaw;

pub use claude_code::ClaudeCodeSkillLoader;
pub use codex::CodexSkillLoader;
pub use hot_reload::HotReloadWatcher;
pub use openclaw::OpenClawSkillLoader;

/// Skill 工具声明
///
/// 描述 Skill 提供的工具及其参数和能力映射
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SkillToolDeclaration {
    /// 工具名称
    pub name: String,
    /// 工具描述
    #[serde(default)]
    pub description: String,
    /// 参数 JSON Schema
    #[serde(default)]
    pub parameters_schema: serde_json::Value,
    /// 映射到的 Capability 名称
    #[serde(default)]
    pub capability_mapping: Option<String>,
}

/// Skill 元数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMetadata {
    /// Skill 名称
    pub name: String,
    /// 版本号
    pub version: String,
    /// 描述信息
    pub description: String,
    /// 作者
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// 主页链接
    #[serde(skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    /// 标签
    #[serde(default)]
    pub tags: Vec<String>,
    /// 工具声明列表
    #[serde(default)]
    pub tools: Vec<SkillToolDeclaration>,
    /// 脚本列表
    #[serde(default)]
    pub scripts: Vec<String>,
    /// 资产文件列表
    #[serde(default)]
    pub assets: Vec<String>,
    /// 平台列表 (Hermes 兼容)
    #[serde(default)]
    pub platforms: Vec<String>,
    /// 触发器列表 (Hermes 兼容)
    #[serde(default)]
    pub triggers: Vec<String>,
    /// 必需的工具集 (Hermes 兼容)
    #[serde(default)]
    pub required_toolsets: Vec<String>,
    /// Skill 来源生态（运行时由 `compat::detect_source_ecosystem` 推断）。
    ///
    /// `None` 表示尚未检测；序列化时省略以保兼容现有 SKILL.md。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_ecosystem: Option<crate::compat::SourceEcosystem>,
    /// 由 SkillCompat 推断的"该 skill 在 SKILL.md 中引用过的 cyberclaw facade"集合。
    ///
    /// 用于后续 PolicyEngine 自动授权（本批次仅产出推断结果，未接入 policy）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_capabilities: Vec<String>,
}

/// 已加载的 Skill 信息
#[derive(Clone)]
pub struct LoadedSkill {
    /// Skill ID
    pub id: SkillId,
    /// 元数据
    pub metadata: SkillMetadata,
    /// Skill 所在路径
    pub path: PathBuf,
    /// Skill 处理器
    pub handler: Arc<dyn SkillHandler>,
}

impl std::fmt::Debug for LoadedSkill {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadedSkill")
            .field("id", &self.id)
            .field("metadata", &self.metadata)
            .field("path", &self.path)
            .field("handler", &"<SkillHandler>")
            .finish()
    }
}

/// Skill 格式加载器 trait
#[async_trait]
pub trait FormatLoader: Send + Sync {
    /// 检测是否可以加载指定路径的 Skill
    fn can_load(&self, path: &Path) -> bool;

    /// 加载 Skill
    async fn load(&self, path: &Path) -> Result<LoadedSkill, SkillRuntimeError>;

    /// 获取加载器名称
    fn name(&self) -> &str;
}

/// 统一 Skill 加载器
///
/// 支持多种格式的自动检测和加载，并提供缓存功能
pub struct UnifiedSkillLoader {
    /// 格式加载器列表
    loaders: Vec<Box<dyn FormatLoader>>,
    /// Skill 缓存 (路径 -> LoadedSkill)
    cache: Arc<RwLock<HashMap<PathBuf, LoadedSkill>>>,
}

impl UnifiedSkillLoader {
    /// 创建新的统一加载器
    pub fn new() -> Self {
        Self {
            loaders: vec![
                Box::new(ClaudeCodeSkillLoader::new()),
                Box::new(CodexSkillLoader::new()),
                Box::new(OpenClawSkillLoader::new()),
            ],
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 使用自定义加载器列表创建
    pub fn with_loaders(loaders: Vec<Box<dyn FormatLoader>>) -> Self {
        Self {
            loaders,
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 自动检测并加载 Skill
    ///
    /// # 参数
    ///
    /// * `path` - Skill 目录路径
    ///
    /// # 返回
    ///
    /// 加载成功返回 `LoadedSkill`，失败返回错误
    pub async fn load_skill(&self, path: &Path) -> Result<LoadedSkill, SkillRuntimeError> {
        // 标准化路径
        let canonical_path = path
            .canonicalize()
            .map_err(|e| SkillRuntimeError::LoadFailed {
                path: path.to_path_buf(),
                source: anyhow::Error::from(e),
            })?;

        // 检查缓存
        {
            let cache = self.cache.read().await;
            if let Some(cached) = cache.get(&canonical_path) {
                tracing::debug!(
                    path = %canonical_path.display(),
                    skill_id = %cached.id,
                    "使用缓存的 Skill"
                );
                return Ok(cached.clone());
            }
        }

        // 尝试所有加载器
        for loader in &self.loaders {
            if loader.can_load(&canonical_path) {
                tracing::info!(
                    path = %canonical_path.display(),
                    loader = loader.name(),
                    "检测到 Skill 格式，开始加载"
                );

                let skill = loader.load(&canonical_path).await?;

                // 缓存加载结果
                {
                    let mut cache = self.cache.write().await;
                    cache.insert(canonical_path.clone(), skill.clone());
                }

                tracing::info!(
                    skill_id = %skill.id,
                    skill_name = %skill.metadata.name,
                    "Skill 加载成功"
                );

                return Ok(skill);
            }
        }

        // 所有加载器都无法识别
        Err(SkillRuntimeError::UnsupportedFormat {
            path: canonical_path,
        })
    }

    /// 重新加载 Skill（清除缓存后重新加载）
    pub async fn reload_skill(&self, path: &Path) -> Result<LoadedSkill, SkillRuntimeError> {
        // 标准化路径
        let canonical_path = path
            .canonicalize()
            .map_err(|e| SkillRuntimeError::LoadFailed {
                path: path.to_path_buf(),
                source: anyhow::Error::from(e),
            })?;

        // 清除缓存
        {
            let mut cache = self.cache.write().await;
            cache.remove(&canonical_path);
        }

        tracing::info!(
            path = %canonical_path.display(),
            "重新加载 Skill"
        );

        // 重新加载
        self.load_skill(&canonical_path).await
    }

    /// 驱逐指定路径的缓存条目
    pub async fn evict_cache(&self, path: &Path) {
        let canonical_path = match path.canonicalize() {
            Ok(p) => p,
            // 文件已删除时 canonicalize 会失败，直接用原始路径尝试移除
            Err(_) => path.to_path_buf(),
        };
        let removed = {
            let mut cache = self.cache.write().await;
            cache.remove(&canonical_path).is_some()
        };
        if removed {
            tracing::debug!(path = %canonical_path.display(), "Skill 缓存条目已驱逐");
        }
    }

    /// 清除所有缓存
    pub async fn clear_cache(&self) {
        {
            let mut cache = self.cache.write().await;
            cache.clear();
        }
        tracing::debug!("Skill 缓存已清除");
    }

    /// 获取缓存的 Skill 数量
    pub async fn cached_count(&self) -> usize {
        self.cache.read().await.len()
    }

    /// 检查指定路径的 Skill 是否已缓存
    pub async fn is_cached(&self, path: &Path) -> bool {
        if let Ok(canonical_path) = path.canonicalize() {
            self.cache.read().await.contains_key(&canonical_path)
        } else {
            false
        }
    }
}

impl Default for UnifiedSkillLoader {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tokio::fs;

    #[tokio::test]
    async fn test_unsupported_format_returns_error() {
        let loader = UnifiedSkillLoader::new();
        let temp_dir = TempDir::new().unwrap();

        // 创建一个空目录，没有任何格式标识文件
        let result = loader.load_skill(temp_dir.path()).await;

        assert!(result.is_err());
        match result.unwrap_err() {
            SkillRuntimeError::UnsupportedFormat { .. } => {}
            other => panic!("Expected UnsupportedFormat error, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_cache_hit() {
        let loader = UnifiedSkillLoader::new();
        let temp_dir = TempDir::new().unwrap();
        let skill_path = temp_dir.path().join("test-skill");
        fs::create_dir(&skill_path).await.unwrap();

        // 创建一个 Claude Code Skill 标识文件
        fs::write(
            skill_path.join("SKILL.md"),
            r#"---
name: Test Skill
version: 1.0.0
description: Test skill
---
# Test Skill
"#,
        )
        .await
        .unwrap();

        // 首次加载
        let first_load = loader.load_skill(&skill_path).await.unwrap();

        // 验证缓存
        assert_eq!(loader.cached_count().await, 1);
        assert!(loader.is_cached(&skill_path).await);

        // 第二次加载应该命中缓存
        let second_load = loader.load_skill(&skill_path).await.unwrap();

        // 验证返回相同的 Skill ID
        assert_eq!(first_load.id, second_load.id);
    }

    #[tokio::test]
    async fn test_clear_cache() {
        let loader = UnifiedSkillLoader::new();
        let temp_dir = TempDir::new().unwrap();
        let skill_path = temp_dir.path().join("test-skill");
        fs::create_dir(&skill_path).await.unwrap();

        fs::write(
            skill_path.join("SKILL.md"),
            r#"---
name: Test Skill
version: 1.0.0
description: Test skill
---
# Test Skill
"#,
        )
        .await
        .unwrap();

        // 加载 Skill
        loader.load_skill(&skill_path).await.unwrap();
        assert_eq!(loader.cached_count().await, 1);

        // 清除缓存
        loader.clear_cache().await;
        assert_eq!(loader.cached_count().await, 0);
    }

    #[tokio::test]
    async fn test_reload_skill_clears_cache_and_reloads() {
        let loader = UnifiedSkillLoader::new();
        let temp_dir = TempDir::new().unwrap();
        let skill_path = temp_dir.path().join("test-skill");
        fs::create_dir(&skill_path).await.unwrap();

        fs::write(
            skill_path.join("SKILL.md"),
            r#"---
name: Test Skill
version: 1.0.0
description: Test skill
---
# Test Skill
"#,
        )
        .await
        .unwrap();

        // 首次加载
        loader.load_skill(&skill_path).await.unwrap();
        assert_eq!(loader.cached_count().await, 1);

        // 重新加载
        let reloaded = loader.reload_skill(&skill_path).await.unwrap();
        assert_eq!(reloaded.metadata.name, "Test Skill");

        // 缓存应该仍然存在（重新加载后会重新缓存）
        assert_eq!(loader.cached_count().await, 1);
    }
}
