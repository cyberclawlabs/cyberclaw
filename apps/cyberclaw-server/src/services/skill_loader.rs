//! Skill 加载服务
//!
//! 提供 Skill 的动态加载、验证和管理功能。
//!
//! ## 支持的 Skill 格式
//!
//! - **Claude Code Skill**: 使用 `SKILL.md` 含 YAML frontmatter
//! - **OpenClaw Skill**: 使用 `skill.toml`
//! - **Codex Skill**: 使用 `manifest.yaml` / `manifest.yml`
//!
//! ## 验证规则
//!
//! 加载时对每个 Skill 执行以下验证：
//! - `name` 非空，仅包含合法字符（字母、数字、`-`、`_`、空格）
//! - `version` 非空，符合语义化版本格式（`major.minor.patch`）
//! - `description` 非空
//! - `tags` 中的每个标签非空
//!
//! ## 卸载
//!
//! 通过 [`SkillLoaderService::unload_skill`] 可动态卸载已加载的 Skill，
//! 从运行时注册表中删除对应处理器并更新内部追踪状态。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use cyberclaw_core::ids::SkillId;
use cyberclaw_skill_runtime::{
    handler::SkillHandler,
    loaders::{
        ClaudeCodeSkillLoader, CodexSkillLoader, OpenClawSkillLoader, SkillMetadata,
        UnifiedSkillLoader,
    },
    runtime::SkillRuntime,
};
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// Skill 加载服务配置
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SkillLoaderConfig {
    /// Skills 目录路径
    pub skills_dir: PathBuf,

    /// 是否启用热重载
    pub enable_hot_reload: bool,

    /// 最大并行加载数量
    pub max_concurrent_loads: usize,

    /// 是否验证 Skill 元数据
    pub verify_signatures: bool,

    /// 允许的 Skill 格式
    pub allowed_formats: Vec<SkillFormat>,
}

impl Default for SkillLoaderConfig {
    fn default() -> Self {
        Self {
            skills_dir: PathBuf::from("./ecosystem/skills"),
            enable_hot_reload: false,
            max_concurrent_loads: 4,
            verify_signatures: true,
            allowed_formats: vec![
                SkillFormat::ClaudeCode,
                SkillFormat::OpenClaw,
                SkillFormat::Codex,
            ],
        }
    }
}

/// 支持的 Skill 格式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum SkillFormat {
    ClaudeCode,
    OpenClaw,
    Codex,
}

/// Skill 加载服务
pub struct SkillLoaderService {
    config: SkillLoaderConfig,
    skill_runtime: Arc<dyn SkillRuntime>,
    /// Arc 包装以便在并行任务中共享
    unified_loader: Arc<UnifiedSkillLoader>,
    /// 已加载的 Skill ID 集合（用于追踪和卸载）
    loaded_skill_ids: RwLock<HashSet<String>>,
}

impl SkillLoaderService {
    /// 创建新的 Skill 加载服务
    pub fn new(config: SkillLoaderConfig, skill_runtime: Arc<dyn SkillRuntime>) -> Self {
        // 创建统一加载器（含全部三种格式）
        let unified_loader = UnifiedSkillLoader::with_loaders(vec![
            Box::new(ClaudeCodeSkillLoader::new()),
            Box::new(OpenClawSkillLoader::new()),
            Box::new(CodexSkillLoader::new()),
        ]);

        Self {
            config,
            skill_runtime,
            unified_loader: Arc::new(unified_loader),
            loaded_skill_ids: RwLock::new(HashSet::new()),
        }
    }

    /// 从指定目录加载所有 Skills
    pub async fn load_all_skills(&self) -> Result<Vec<LoadedSkillInfo>> {
        info!(
            "Loading skills from directory: {:?}",
            self.config.skills_dir
        );

        // 确保目录存在
        if !self.config.skills_dir.exists() {
            warn!(
                "Skills directory does not exist: {:?}",
                self.config.skills_dir
            );
            return Ok(Vec::new());
        }

        // 扫描目录获取所有 Skill 子目录
        let mut skill_dirs = Vec::new();
        let mut entries = fs::read_dir(&self.config.skills_dir)
            .await
            .context("Failed to read skills directory")?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.is_dir() {
                skill_dirs.push(path);
            }
        }

        info!("Found {} potential skill directories", skill_dirs.len());

        // 并行加载 Skills（受最大并发数限制）
        let mut loaded_skills = Vec::new();
        let semaphore = Arc::new(tokio::sync::Semaphore::new(
            self.config.max_concurrent_loads,
        ));

        let mut tasks = Vec::new();
        for dir in skill_dirs {
            let sem = semaphore.clone();
            let loader = Arc::clone(&self.unified_loader);

            tasks.push(tokio::spawn(async move {
                let _permit = sem.acquire().await.unwrap();
                Self::load_skill_from_dir(&dir, &loader).await
            }));
        }

        // 收集结果
        for task in tasks {
            match task.await {
                Ok(Ok(skill_info)) => {
                    // 验证（如果启用）
                    if self.config.verify_signatures {
                        if let Err(e) = self.verify_skill(&skill_info) {
                            warn!("Skill validation failed for {}: {}", skill_info.id, e);
                            continue;
                        }
                    }
                    // 注册到运行时
                    if let Err(e) = self.register_skill(&skill_info).await {
                        error!("Failed to register skill {}: {}", skill_info.id, e);
                    } else {
                        loaded_skills.push(skill_info);
                    }
                }
                Ok(Err(e)) => {
                    warn!("Failed to load skill: {}", e);
                }
                Err(e) => {
                    error!("Task failed: {}", e);
                }
            }
        }

        info!("Successfully loaded {} skills", loaded_skills.len());
        Ok(loaded_skills)
    }

    /// 加载单个 Skill
    pub async fn load_skill(&self, skill_path: &Path) -> Result<LoadedSkillInfo> {
        debug!("Loading skill from path: {:?}", skill_path);

        let skill_info = Self::load_skill_from_dir(skill_path, &self.unified_loader).await?;

        // 验证 Skill
        if self.config.verify_signatures {
            self.verify_skill(&skill_info)?;
        }

        // 注册到运行时
        self.register_skill(&skill_info).await?;

        Ok(skill_info)
    }

    /// 从目录加载 Skill
    async fn load_skill_from_dir(
        dir: &Path,
        loader: &Arc<UnifiedSkillLoader>,
    ) -> Result<LoadedSkillInfo> {
        debug!("Attempting to load skill from directory: {:?}", dir);

        // 使用统一加载器加载
        let loaded_skill = loader
            .load_skill(dir)
            .await
            .context("Failed to load skill with unified loader")?;

        // 创建 Skill ID
        let skill_id = SkillId::from_string(loaded_skill.metadata.name.clone())
            .context("Invalid skill name")?;

        Ok(LoadedSkillInfo {
            id: skill_id,
            metadata: loaded_skill.metadata,
            handler: loaded_skill.handler,
            source_path: dir.to_path_buf(),
        })
    }

    /// 验证 Skill 元数据合法性
    ///
    /// 检查以下规则：
    /// - `name` 非空，仅包含合法字符（字母、数字、连字符、下划线、空格）
    /// - `version` 非空，符合语义化版本格式（`major.minor.patch[.prerelease]`）
    /// - `description` 非空
    /// - 每个 `tag` 非空
    fn verify_skill(&self, skill_info: &LoadedSkillInfo) -> Result<()> {
        debug!("Verifying skill: {}", skill_info.id);

        let meta = &skill_info.metadata;

        // 验证 name 非空
        if meta.name.is_empty() {
            return Err(anyhow::anyhow!("Skill name cannot be empty"));
        }

        // 验证 name 仅包含合法字符
        // 允许：字母、数字、连字符、下划线、空格
        if !meta
            .name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | ' '))
        {
            return Err(anyhow::anyhow!(
                "Skill name '{}' contains invalid characters; only alphanumeric, '-', '_', and spaces are allowed",
                meta.name
            ));
        }

        // 验证 version 非空
        if meta.version.is_empty() {
            return Err(anyhow::anyhow!("Skill version cannot be empty"));
        }

        // 验证 version 符合语义化版本格式（major.minor.patch 或含预发布标签）
        Self::validate_semver(&meta.version)?;

        // 验证 description 非空
        if meta.description.is_empty() {
            return Err(anyhow::anyhow!("Skill description cannot be empty"));
        }

        // 验证每个 tag 非空且仅含合法字符
        for tag in &meta.tags {
            if tag.is_empty() {
                return Err(anyhow::anyhow!("Skill tags must not contain empty strings"));
            }
            if !tag
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_'))
            {
                return Err(anyhow::anyhow!(
                    "Skill tag '{}' contains invalid characters; only alphanumeric, '-', and '_' are allowed",
                    tag
                ));
            }
        }

        debug!("Skill '{}' v{} passed validation", meta.name, meta.version);
        Ok(())
    }

    /// 验证版本字符串符合语义化版本格式
    ///
    /// 接受 `major.minor.patch` 和 `major.minor.patch-prerelease` 格式。
    fn validate_semver(version: &str) -> Result<()> {
        // 分离预发布标签（如 "1.0.0-alpha"）
        let base = version.split('-').next().unwrap_or(version);
        let parts: Vec<&str> = base.split('.').collect();

        if parts.len() < 3 {
            return Err(anyhow::anyhow!(
                "Skill version '{}' does not follow semantic versioning (expected major.minor.patch)",
                version
            ));
        }

        for part in &parts[..3] {
            if part.parse::<u64>().is_err() {
                return Err(anyhow::anyhow!(
                    "Skill version '{}' has non-numeric component '{}'; expected major.minor.patch",
                    version,
                    part
                ));
            }
        }

        Ok(())
    }

    /// 注册 Skill 到运行时，并记录到内部追踪集合
    async fn register_skill(&self, skill_info: &LoadedSkillInfo) -> Result<()> {
        info!(
            "Registering skill: {} v{}",
            skill_info.metadata.name, skill_info.metadata.version
        );

        self.skill_runtime
            .register_skill(skill_info.id.clone(), skill_info.handler.clone())
            .await;

        self.loaded_skill_ids
            .write()
            .await
            .insert(skill_info.id.as_str().to_string());

        Ok(())
    }

    /// 卸载指定 Skill
    ///
    /// 从运行时注册表中移除处理器，并更新内部追踪状态。
    ///
    /// # 错误
    ///
    /// 当运行时不支持卸载时返回错误。
    pub async fn unload_skill(&self, skill_id: &SkillId) -> Result<()> {
        info!("Unloading skill: {}", skill_id);

        let was_tracked = self
            .loaded_skill_ids
            .write()
            .await
            .remove(skill_id.as_str());

        if !was_tracked {
            warn!(
                "unload_skill: skill '{}' was not tracked as loaded by this service",
                skill_id
            );
        }

        let removed = self
            .skill_runtime
            .unregister_skill(skill_id)
            .await
            .with_context(|| format!("Failed to unregister skill '{}'", skill_id))?;

        if removed {
            info!("Skill '{}' successfully unloaded", skill_id);
        } else {
            warn!(
                "Skill '{}' was not found in runtime registry during unload",
                skill_id
            );
        }

        Ok(())
    }

    /// 重新加载指定 Skill
    pub async fn reload_skill(&self, skill_path: &Path) -> Result<LoadedSkillInfo> {
        debug!("Reloading skill from: {:?}", skill_path);

        // 提取 Skill ID
        let skill_name = skill_path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow::anyhow!("Invalid skill path"))?;

        let skill_id = SkillId::from_string(skill_name.to_string())?;

        // 先卸载（如果存在）
        self.unload_skill(&skill_id).await?;

        // 然后重新加载
        self.load_skill(skill_path).await
    }

    /// 返回当前由本服务实例追踪的已加载 Skill ID 集合快照
    pub async fn loaded_skill_ids(&self) -> HashSet<String> {
        self.loaded_skill_ids.read().await.clone()
    }
}

/// 已加载的 Skill 信息
#[derive(Clone)]
pub struct LoadedSkillInfo {
    /// Skill ID
    pub id: SkillId,

    /// Skill 元数据
    pub metadata: SkillMetadata,

    /// Skill 处理器
    pub handler: Arc<dyn SkillHandler>,

    /// Skill 源文件路径
    pub source_path: PathBuf,
}

impl std::fmt::Debug for LoadedSkillInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadedSkillInfo")
            .field("id", &self.id)
            .field("metadata", &self.metadata)
            .field("source_path", &self.source_path)
            .field("handler", &"<SkillHandler>")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyberclaw_skill_runtime::runtime::MinimalSkillRuntime;
    use tempfile::TempDir;
    use tokio::fs;

    // ──────────────────────────────────────────────────────────────────────────
    // 辅助函数
    // ──────────────────────────────────────────────────────────────────────────

    async fn create_test_service() -> (SkillLoaderService, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let config = SkillLoaderConfig {
            skills_dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        };
        let skill_runtime = Arc::new(MinimalSkillRuntime::new());
        let service = SkillLoaderService::new(config, skill_runtime);
        (service, temp_dir)
    }

    /// 创建一个合法的 Claude Code Skill 目录
    async fn create_valid_claude_code_skill(base_dir: &Path, skill_name: &str) -> PathBuf {
        let skill_path = base_dir.join(skill_name);
        fs::create_dir_all(&skill_path).await.unwrap();
        fs::write(
            skill_path.join("SKILL.md"),
            format!(
                "---\nname: {}\nversion: 1.0.0\ndescription: A test skill\n---\n# {}\n",
                skill_name, skill_name
            ),
        )
        .await
        .unwrap();
        skill_path
    }

    fn make_valid_skill_info(name: &str) -> LoadedSkillInfo {
        LoadedSkillInfo {
            id: SkillId::from_string(name.to_string()).unwrap(),
            metadata: SkillMetadata {
                name: name.to_string(),
                version: "1.0.0".to_string(),
                description: "A valid test skill".to_string(),
                author: None,
                homepage: None,
                tags: vec![],
                tools: vec![],
                scripts: vec![],
                assets: vec![],
                platforms: vec![],
                triggers: vec![],
                required_toolsets: vec![],
                source_ecosystem: None,
                allowed_capabilities: vec![],
                prompt_body: None,
            },
            handler: Arc::new(cyberclaw_skill_runtime::skills::EchoSkill),
            source_path: PathBuf::from("/test"),
        }
    }

    // ──────────────────────────────────────────────────────────────────────────
    // load_all_skills
    // ──────────────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_load_empty_directory() {
        let (service, _temp_dir) = create_test_service().await;
        let result = service.load_all_skills().await.unwrap();
        assert_eq!(result.len(), 0);
    }

    #[tokio::test]
    async fn test_load_all_skills_nonexistent_directory() {
        let config = SkillLoaderConfig {
            skills_dir: PathBuf::from("/nonexistent/skills/dir"),
            ..Default::default()
        };
        let skill_runtime = Arc::new(MinimalSkillRuntime::new());
        let service = SkillLoaderService::new(config, skill_runtime);

        let result = service.load_all_skills().await.unwrap();
        assert_eq!(result.len(), 0);
    }

    // ──────────────────────────────────────────────────────────────────────────
    // verify_skill - name 验证
    // ──────────────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_verify_skill_empty_name_rejected() {
        let (service, _temp_dir) = create_test_service().await;

        let skill = LoadedSkillInfo {
            metadata: SkillMetadata {
                name: String::new(),
                version: "1.0.0".to_string(),
                description: "desc".to_string(),
                author: None,
                homepage: None,
                tags: vec![],
                tools: vec![],
                scripts: vec![],
                assets: vec![],
                platforms: vec![],
                triggers: vec![],
                required_toolsets: vec![],
                source_ecosystem: None,
                allowed_capabilities: vec![],
                prompt_body: None,
            },
            ..make_valid_skill_info("test")
        };

        let err = service.verify_skill(&skill).unwrap_err();
        assert!(err.to_string().contains("name cannot be empty"));
    }

    #[tokio::test]
    async fn test_verify_skill_invalid_name_characters_rejected() {
        let (service, _temp_dir) = create_test_service().await;

        let skill = LoadedSkillInfo {
            metadata: SkillMetadata {
                name: "bad/name!".to_string(),
                version: "1.0.0".to_string(),
                description: "desc".to_string(),
                author: None,
                homepage: None,
                tags: vec![],
                tools: vec![],
                scripts: vec![],
                assets: vec![],
                platforms: vec![],
                triggers: vec![],
                required_toolsets: vec![],
                source_ecosystem: None,
                allowed_capabilities: vec![],
                prompt_body: None,
            },
            ..make_valid_skill_info("test")
        };

        let err = service.verify_skill(&skill).unwrap_err();
        assert!(err.to_string().contains("invalid characters"));
    }

    #[tokio::test]
    async fn test_verify_skill_name_with_allowed_special_chars_accepted() {
        let (service, _temp_dir) = create_test_service().await;

        // name 允许：字母、数字、-、_、空格
        let skill = LoadedSkillInfo {
            metadata: SkillMetadata {
                name: "My Skill-v2_alpha".to_string(),
                version: "2.0.0".to_string(),
                description: "desc".to_string(),
                author: None,
                homepage: None,
                tags: vec![],
                tools: vec![],
                scripts: vec![],
                assets: vec![],
                platforms: vec![],
                triggers: vec![],
                required_toolsets: vec![],
                source_ecosystem: None,
                allowed_capabilities: vec![],
                prompt_body: None,
            },
            ..make_valid_skill_info("test")
        };

        assert!(service.verify_skill(&skill).is_ok());
    }

    // ──────────────────────────────────────────────────────────────────────────
    // verify_skill - version 验证
    // ──────────────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_verify_skill_empty_version_rejected() {
        let (service, _temp_dir) = create_test_service().await;

        let skill = LoadedSkillInfo {
            metadata: SkillMetadata {
                name: "valid-name".to_string(),
                version: String::new(),
                description: "desc".to_string(),
                author: None,
                homepage: None,
                tags: vec![],
                tools: vec![],
                scripts: vec![],
                assets: vec![],
                platforms: vec![],
                triggers: vec![],
                required_toolsets: vec![],
                source_ecosystem: None,
                allowed_capabilities: vec![],
                prompt_body: None,
            },
            ..make_valid_skill_info("test")
        };

        let err = service.verify_skill(&skill).unwrap_err();
        assert!(err.to_string().contains("version cannot be empty"));
    }

    #[tokio::test]
    async fn test_verify_skill_invalid_semver_rejected() {
        let (service, _temp_dir) = create_test_service().await;

        for bad_version in &["1.0", "v1.0.0", "latest", "1.0.x"] {
            let skill = LoadedSkillInfo {
                metadata: SkillMetadata {
                    name: "valid-name".to_string(),
                    version: bad_version.to_string(),
                    description: "desc".to_string(),
                    author: None,
                    homepage: None,
                    tags: vec![],
                    tools: vec![],
                    scripts: vec![],
                    assets: vec![],
                    platforms: vec![],
                    triggers: vec![],
                    required_toolsets: vec![],
                    source_ecosystem: None,
                    allowed_capabilities: vec![],
                    prompt_body: None,
                },
                ..make_valid_skill_info("test")
            };
            let result = service.verify_skill(&skill);
            assert!(
                result.is_err(),
                "Expected version '{}' to be rejected",
                bad_version
            );
        }
    }

    #[tokio::test]
    async fn test_verify_skill_valid_semver_accepted() {
        let (service, _temp_dir) = create_test_service().await;

        for good_version in &["1.0.0", "0.1.0", "2.10.3", "1.0.0-alpha", "3.2.1-rc2"] {
            let skill = LoadedSkillInfo {
                metadata: SkillMetadata {
                    name: "valid-name".to_string(),
                    version: good_version.to_string(),
                    description: "desc".to_string(),
                    author: None,
                    homepage: None,
                    tags: vec![],
                    tools: vec![],
                    scripts: vec![],
                    assets: vec![],
                    platforms: vec![],
                    triggers: vec![],
                    required_toolsets: vec![],
                    source_ecosystem: None,
                    allowed_capabilities: vec![],
                    prompt_body: None,
                },
                ..make_valid_skill_info("test")
            };
            assert!(
                service.verify_skill(&skill).is_ok(),
                "Expected version '{}' to be accepted",
                good_version
            );
        }
    }

    // ──────────────────────────────────────────────────────────────────────────
    // verify_skill - description 验证
    // ──────────────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_verify_skill_empty_description_rejected() {
        let (service, _temp_dir) = create_test_service().await;

        let skill = LoadedSkillInfo {
            metadata: SkillMetadata {
                name: "valid-name".to_string(),
                version: "1.0.0".to_string(),
                description: String::new(),
                author: None,
                homepage: None,
                tags: vec![],
                tools: vec![],
                scripts: vec![],
                assets: vec![],
                platforms: vec![],
                triggers: vec![],
                required_toolsets: vec![],
                source_ecosystem: None,
                allowed_capabilities: vec![],
                prompt_body: None,
            },
            ..make_valid_skill_info("test")
        };

        let err = service.verify_skill(&skill).unwrap_err();
        assert!(err.to_string().contains("description cannot be empty"));
    }

    // ──────────────────────────────────────────────────────────────────────────
    // verify_skill - tags 验证
    // ──────────────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_verify_skill_empty_tag_rejected() {
        let (service, _temp_dir) = create_test_service().await;

        let skill = LoadedSkillInfo {
            metadata: SkillMetadata {
                name: "valid-name".to_string(),
                version: "1.0.0".to_string(),
                description: "desc".to_string(),
                author: None,
                homepage: None,
                tags: vec!["good-tag".to_string(), String::new()],
                tools: vec![],
                scripts: vec![],
                assets: vec![],
                platforms: vec![],
                triggers: vec![],
                required_toolsets: vec![],
                source_ecosystem: None,
                allowed_capabilities: vec![],
                prompt_body: None,
            },
            ..make_valid_skill_info("test")
        };

        let err = service.verify_skill(&skill).unwrap_err();
        assert!(err.to_string().contains("empty strings"));
    }

    #[tokio::test]
    async fn test_verify_skill_invalid_tag_characters_rejected() {
        let (service, _temp_dir) = create_test_service().await;

        let skill = LoadedSkillInfo {
            metadata: SkillMetadata {
                name: "valid-name".to_string(),
                version: "1.0.0".to_string(),
                description: "desc".to_string(),
                author: None,
                homepage: None,
                tags: vec!["valid_tag".to_string(), "bad tag!".to_string()],
                tools: vec![],
                scripts: vec![],
                assets: vec![],
                platforms: vec![],
                triggers: vec![],
                required_toolsets: vec![],
                source_ecosystem: None,
                allowed_capabilities: vec![],
                prompt_body: None,
            },
            ..make_valid_skill_info("test")
        };

        let err = service.verify_skill(&skill).unwrap_err();
        assert!(err.to_string().contains("invalid characters"));
    }

    #[tokio::test]
    async fn test_verify_skill_valid_full_metadata_accepted() {
        let (service, _temp_dir) = create_test_service().await;
        let skill = make_valid_skill_info("my-skill");
        assert!(service.verify_skill(&skill).is_ok());
    }

    // ──────────────────────────────────────────────────────────────────────────
    // unload_skill
    // ──────────────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_unload_skill_removes_from_runtime() {
        let (service, temp_dir) = create_test_service().await;

        // 先加载一个 Skill
        let skill_path = create_valid_claude_code_skill(temp_dir.path(), "my-skill").await;
        let loaded = service.load_skill(&skill_path).await.unwrap();
        let skill_id = loaded.id.clone();

        // 确认已被追踪
        assert!(service.loaded_skill_ids().await.contains(skill_id.as_str()));

        // 执行卸载
        service.unload_skill(&skill_id).await.unwrap();

        // 确认已从追踪集合移除
        assert!(!service.loaded_skill_ids().await.contains(skill_id.as_str()));
    }

    #[tokio::test]
    async fn test_unload_skill_not_loaded_succeeds_with_warning() {
        let (service, _temp_dir) = create_test_service().await;
        let unknown_id = SkillId::from_string("nonexistent-skill".to_string()).unwrap();

        // 卸载未加载的 Skill 不应返回错误
        let result = service.unload_skill(&unknown_id).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_unload_then_reload_skill() {
        let (service, temp_dir) = create_test_service().await;

        let skill_path = create_valid_claude_code_skill(temp_dir.path(), "reload-skill").await;

        // 第一次加载
        let loaded = service.load_skill(&skill_path).await.unwrap();
        let skill_id = loaded.id.clone();
        assert!(service.loaded_skill_ids().await.contains(skill_id.as_str()));

        // 卸载
        service.unload_skill(&skill_id).await.unwrap();
        assert!(!service.loaded_skill_ids().await.contains(skill_id.as_str()));

        // 重新加载
        service.load_skill(&skill_path).await.unwrap();
        assert!(service.loaded_skill_ids().await.contains(skill_id.as_str()));
    }

    // ──────────────────────────────────────────────────────────────────────────
    // validate_semver 单元测试
    // ──────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_validate_semver_valid_versions() {
        for v in &["1.0.0", "0.0.1", "10.20.30", "1.0.0-alpha", "2.1.0-rc.1"] {
            assert!(
                SkillLoaderService::validate_semver(v).is_ok(),
                "Expected '{}' to be valid semver",
                v
            );
        }
    }

    #[test]
    fn test_validate_semver_invalid_versions() {
        for v in &["1.0", "1", "v1.0.0", "1.0.x", "latest", ""] {
            assert!(
                SkillLoaderService::validate_semver(v).is_err(),
                "Expected '{}' to be invalid semver",
                v
            );
        }
    }
}
