use crate::types::{
    LoadedAgent, LoadedConnector, LoadedObject, LoadedPackage, LoadedPlatformPlugin, LoadedSkill,
    PackageSource,
};
use async_trait::async_trait;
use cyberclaw_core::manifests::{PackageKind, PackageManifest};
use std::path::{Path, PathBuf};

#[async_trait]
pub trait Loader: Send + Sync {
    async fn load(&self, source: PackageSource) -> anyhow::Result<LoadedPackage>;
}

#[derive(Debug, Clone, Default)]
pub struct ManifestLoader {
    base_path: Option<PathBuf>,
}

impl ManifestLoader {
    pub fn new() -> Self {
        Self { base_path: None }
    }

    pub fn with_base_path<P: AsRef<Path>>(base_path: P) -> Self {
        Self {
            base_path: Some(base_path.as_ref().to_path_buf()),
        }
    }

    async fn resolve_path(&self, relative: &Path) -> anyhow::Result<PathBuf> {
        // 构造完整路径
        let resolved = match &self.base_path {
            Some(base) => base.join(relative),
            None => relative.to_path_buf(),
        };

        // 检查是否为符号链接（防止符号链接攻击）
        // Use spawn_blocking for synchronous is_symlink() to avoid blocking tokio runtime
        let resolved_clone = resolved.clone();
        let is_symlink = tokio::task::spawn_blocking(move || resolved_clone.is_symlink()).await?;
        if is_symlink {
            anyhow::bail!("symbolic links are not allowed: '{}'", resolved.display());
        }

        // 规范化路径并验证边界（防止路径遍历攻击）
        // Use spawn_blocking for synchronous canonicalize() to avoid blocking tokio runtime
        let resolved_clone = resolved.clone();
        let canonical = tokio::task::spawn_blocking(move || {
            resolved_clone
                .canonicalize()
                .map_err(|e| anyhow::anyhow!("invalid path '{}': {}", resolved_clone.display(), e))
        })
        .await??;

        // 如果有 base_path，验证规范化后的路径仍在 base 目录内
        if let Some(base) = &self.base_path {
            let base_clone = base.clone();
            let relative_display = relative.display().to_string();
            let canonical_base = tokio::task::spawn_blocking(move || {
                base_clone.canonicalize().map_err(|e| {
                    anyhow::anyhow!("invalid base path '{}': {}", base_clone.display(), e)
                })
            })
            .await??;

            if !canonical.starts_with(&canonical_base) {
                anyhow::bail!(
                    "path traversal attempt detected: '{}' resolves outside base directory",
                    relative_display
                );
            }
        }

        Ok(canonical)
    }

    /// 安全地 join 相对路径，防止路径遍历攻击
    async fn safe_join(&self, base: &Path, relative: &str) -> anyhow::Result<PathBuf> {
        // 禁止绝对路径
        let relative_path = Path::new(relative);
        if relative_path.is_absolute() {
            anyhow::bail!("absolute paths are not allowed: {}", relative);
        }

        // Join 路径
        let joined = base.join(relative_path);

        // 检查是否为符号链接（防止符号链接攻击）
        // Use spawn_blocking for synchronous is_symlink() to avoid blocking tokio runtime
        let joined_clone = joined.clone();
        let is_symlink = tokio::task::spawn_blocking(move || joined_clone.is_symlink()).await?;
        if is_symlink {
            anyhow::bail!("symbolic links are not allowed: '{}'", joined.display());
        }

        // 规范化并验证仍在 base 目录内
        // Use spawn_blocking for synchronous canonicalize() to avoid blocking tokio runtime
        let joined_clone = joined.clone();
        let relative_str = relative.to_string();
        let canonical = tokio::task::spawn_blocking(move || {
            joined_clone
                .canonicalize()
                .map_err(|e| anyhow::anyhow!("invalid artifact path '{}': {}", relative_str, e))
        })
        .await??;

        let base_clone = base.to_path_buf();
        let canonical_base = tokio::task::spawn_blocking(move || {
            base_clone
                .canonicalize()
                .map_err(|e| anyhow::anyhow!("invalid base path: {}", e))
        })
        .await??;

        if !canonical.starts_with(&canonical_base) {
            anyhow::bail!(
                "path traversal attempt detected: '{}' resolves outside package directory",
                relative
            );
        }

        Ok(canonical)
    }

    async fn read_manifest_file(&self, manifest_path: &Path) -> anyhow::Result<PackageManifest> {
        // 检查文件大小，防止 DoS 攻击（限制 1MB）
        const MAX_MANIFEST_SIZE: u64 = 1024 * 1024; // 1MB
        let metadata = tokio::fs::metadata(manifest_path)
            .await
            .map_err(|e| anyhow::anyhow!("failed to read manifest metadata: {}", e))?;

        if metadata.len() > MAX_MANIFEST_SIZE {
            anyhow::bail!(
                "manifest file too large: {} bytes (max: {} bytes)",
                metadata.len(),
                MAX_MANIFEST_SIZE
            );
        }

        let content = tokio::fs::read_to_string(manifest_path)
            .await
            .map_err(|e| anyhow::anyhow!("failed to read manifest file: {}", e))?;

        let manifest: PackageManifest = serde_yaml::from_str(&content)
            .map_err(|e| anyhow::anyhow!("failed to parse manifest YAML: {}", e))?;

        Ok(manifest)
    }

    fn validate_manifest(&self, manifest: &PackageManifest) -> anyhow::Result<()> {
        // 验证 apiVersion
        if manifest.api_version != "cyberclaw.io/v2" {
            anyhow::bail!(
                "unsupported apiVersion: {}, expected cyberclaw.io/v2",
                manifest.api_version
            );
        }

        // 验证 kind 和 spec 一致性
        manifest.validate_spec_kind()?;

        Ok(())
    }

    async fn validate_artifacts(
        &self,
        manifest: &PackageManifest,
        package_dir: &Path,
    ) -> anyhow::Result<()> {
        // 验证 entry 文件存在
        if let Some(entry) = &manifest.artifacts.entry {
            let entry_path = self.safe_join(package_dir, entry).await?;
            if !tokio::fs::try_exists(&entry_path).await.unwrap_or(false) {
                anyhow::bail!("entry file not found: {}", entry);
            }
        }

        // 验证 files 列表中的文件存在
        for file in &manifest.artifacts.files {
            let file_path = self.safe_join(package_dir, file).await?;
            if !tokio::fs::try_exists(&file_path).await.unwrap_or(false) {
                anyhow::bail!("artifact file not found: {}", file);
            }
        }

        Ok(())
    }

    async fn validate_agent_spec(
        &self,
        manifest: &PackageManifest,
        package_dir: &Path,
    ) -> anyhow::Result<()> {
        let Some(agent_spec) = manifest.agent_spec() else {
            anyhow::bail!("not an agent manifest");
        };

        // 验证 system_prompt_file 存在
        let system_prompt_path = self
            .safe_join(package_dir, &agent_spec.system_prompt_file)
            .await?;
        if !tokio::fs::try_exists(&system_prompt_path)
            .await
            .unwrap_or(false)
        {
            anyhow::bail!(
                "system prompt file not found: {}",
                agent_spec.system_prompt_file
            );
        }

        // 验证可选文件
        if let Some(persona_file) = &agent_spec.persona_file {
            let persona_path = self.safe_join(package_dir, persona_file).await?;
            if !tokio::fs::try_exists(&persona_path).await.unwrap_or(false) {
                anyhow::bail!("persona file not found: {}", persona_file);
            }
        }

        if let Some(policy_file) = &agent_spec.policy_file {
            let policy_path = self.safe_join(package_dir, policy_file).await?;
            if !tokio::fs::try_exists(&policy_path).await.unwrap_or(false) {
                anyhow::bail!("policy file not found: {}", policy_file);
            }
        }

        if let Some(memory_file) = &agent_spec.memory_file {
            let memory_path = self.safe_join(package_dir, memory_file).await?;
            if !tokio::fs::try_exists(&memory_path).await.unwrap_or(false) {
                anyhow::bail!("memory file not found: {}", memory_file);
            }
        }

        Ok(())
    }

    async fn validate_skill_spec(
        &self,
        manifest: &PackageManifest,
        package_dir: &Path,
    ) -> anyhow::Result<()> {
        let Some(skill_spec) = manifest.skill_spec() else {
            anyhow::bail!("not a skill manifest");
        };

        // 验证 entry_file 存在
        let entry_path = self.safe_join(package_dir, &skill_spec.entry_file).await?;
        if !tokio::fs::try_exists(&entry_path).await.unwrap_or(false) {
            anyhow::bail!("skill entry file not found: {}", skill_spec.entry_file);
        }

        Ok(())
    }

    fn validate_connector_spec(&self, manifest: &PackageManifest) -> anyhow::Result<()> {
        let Some(connector_spec) = manifest.connector_spec() else {
            anyhow::bail!("not a connector manifest");
        };

        // 验证至少有一个 capability
        if connector_spec.capabilities.is_empty() {
            anyhow::bail!("connector must have at least one capability");
        }

        // 验证每个 capability 的 schema 字段
        for cap in &connector_spec.capabilities {
            if cap.input_schema.is_empty() {
                anyhow::bail!("capability {} missing input_schema", cap.id);
            }
            if cap.output_schema.is_empty() {
                anyhow::bail!("capability {} missing output_schema", cap.id);
            }
            if cap.effects.is_empty() {
                anyhow::bail!("capability {} must have at least one effect", cap.id);
            }
        }

        Ok(())
    }

    fn validate_plugin_spec(&self, manifest: &PackageManifest) -> anyhow::Result<()> {
        let Some(plugin_spec) = manifest.platform_plugin_spec() else {
            anyhow::bail!("not a platform plugin manifest");
        };

        // 验证至少有一个 hook
        if plugin_spec.hooks.is_empty() {
            anyhow::bail!("platform plugin must have at least one hook");
        }

        Ok(())
    }

    fn assemble_package(
        &self,
        manifest: PackageManifest,
        source: PackageSource,
    ) -> anyhow::Result<LoadedPackage> {
        let object = match manifest.kind {
            PackageKind::Agent => LoadedObject::Agent(LoadedAgent),
            PackageKind::Skill => LoadedObject::Skill(LoadedSkill),
            PackageKind::Connector => LoadedObject::Connector(LoadedConnector),
            PackageKind::PlatformPlugin => LoadedObject::PlatformPlugin(LoadedPlatformPlugin),
        };

        Ok(LoadedPackage {
            manifest,
            object,
            source,
        })
    }
}

#[async_trait]
impl Loader for ManifestLoader {
    async fn load(&self, source: PackageSource) -> anyhow::Result<LoadedPackage> {
        let package_dir = match &source {
            PackageSource::LocalPath(path) => self.resolve_path(Path::new(path)).await?,
            _ => anyhow::bail!("only LocalPath source is supported for now"),
        };

        if !tokio::fs::try_exists(&package_dir).await.unwrap_or(false) {
            anyhow::bail!("package directory not found: {}", package_dir.display());
        }

        // 读取 manifest.yaml
        let manifest_path = package_dir.join("manifest.yaml");
        if !tokio::fs::try_exists(&manifest_path).await.unwrap_or(false) {
            anyhow::bail!("manifest.yaml not found in {}", package_dir.display());
        }

        let manifest = self.read_manifest_file(&manifest_path).await?;

        // 验证 manifest 顶层结构
        self.validate_manifest(&manifest)?;

        // 验证 artifacts 文件存在性
        self.validate_artifacts(&manifest, &package_dir).await?;

        // 根据 kind 验证 spec
        match manifest.kind {
            PackageKind::Agent => self.validate_agent_spec(&manifest, &package_dir).await?,
            PackageKind::Skill => self.validate_skill_spec(&manifest, &package_dir).await?,
            PackageKind::Connector => self.validate_connector_spec(&manifest)?,
            PackageKind::PlatformPlugin => self.validate_plugin_spec(&manifest)?,
        }

        // 组装 LoadedPackage
        self.assemble_package(manifest, source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========== 正常路径测试 ==========

    #[tokio::test]
    async fn test_load_master_agent() {
        let loader = ManifestLoader::with_base_path("../../ecosystem/agents/master-agent");
        let result = loader.load(PackageSource::LocalPath(".".to_string())).await;

        assert!(
            result.is_ok(),
            "failed to load master-agent: {:?}",
            result.err()
        );

        let package = result.unwrap();
        assert_eq!(package.manifest.kind, PackageKind::Agent);
        assert_eq!(package.manifest.id, "cyberclaw/master-agent");
    }

    #[tokio::test]
    async fn test_load_report_summary_skill() {
        let loader = ManifestLoader::with_base_path("../../ecosystem/skills/report-summary");
        let result = loader.load(PackageSource::LocalPath(".".to_string())).await;

        assert!(
            result.is_ok(),
            "failed to load report-summary: {:?}",
            result.err()
        );

        let package = result.unwrap();
        assert_eq!(package.manifest.kind, PackageKind::Skill);
        assert_eq!(package.manifest.id, "report-summary");
    }

    #[tokio::test]
    async fn test_load_github_connector() {
        let loader = ManifestLoader::with_base_path("../../ecosystem/connectors/github");
        let result = loader.load(PackageSource::LocalPath(".".to_string())).await;

        assert!(
            result.is_ok(),
            "failed to load github connector: {:?}",
            result.err()
        );

        let package = result.unwrap();
        assert_eq!(package.manifest.kind, PackageKind::Connector);
        assert_eq!(package.manifest.id, "github");
    }

    // ========== 错误路径测试 ==========

    #[tokio::test]
    async fn test_path_traversal_attack() {
        // 使用存在的路径测试边界检查
        // 从 ecosystem/agents/master-agent 尝试访问 ../../../（应该超出 base_path）
        let loader = ManifestLoader::with_base_path("../../ecosystem/agents");

        // 尝试通过相对路径访问父目录外的内容
        // 这应该被 resolve_path 的边界检查拒绝
        let result = loader
            .load(PackageSource::LocalPath("../../crates".to_string()))
            .await;

        assert!(result.is_err(), "path traversal should be rejected");
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("path traversal")
                || err.to_string().contains("outside base")
                || err.to_string().contains("outside"),
            "error should mention path traversal or boundary violation, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_missing_manifest_file() {
        let loader = ManifestLoader::new();

        // 使用当前目录（crates/cyberclaw-control-plane/src）
        // 这个目录存在但没有 manifest.yaml
        let result = loader
            .load(PackageSource::LocalPath("./src".to_string()))
            .await;

        assert!(result.is_err(), "missing manifest.yaml should fail");
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("manifest.yaml not found"),
            "error should mention missing manifest.yaml, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_missing_package_directory() {
        let loader = ManifestLoader::new();

        // 使用一个不存在的目录
        let result = loader
            .load(PackageSource::LocalPath(
                "/nonexistent/path/to/package".to_string(),
            ))
            .await;

        assert!(result.is_err(), "nonexistent directory should fail");
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("not found") || err.to_string().contains("invalid path"),
            "error should mention path not found, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_absolute_path_in_artifacts() {
        // 这个测试需要一个带有绝对路径的无效 manifest
        // 由于我们无法创建临时测试文件，我们将测试 safe_join 方法的行为
        let loader = ManifestLoader::with_base_path("../../ecosystem/agents/master-agent");

        // 测试 safe_join 拒绝绝对路径
        let base = std::path::Path::new("/tmp");
        let result = loader.safe_join(base, "/etc/passwd").await;

        assert!(result.is_err(), "absolute path should be rejected");
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("absolute paths are not allowed"),
            "error should mention absolute paths, got: {}",
            err
        );
    }
}
